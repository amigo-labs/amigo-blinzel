//! The order of the cold start, seen from outside: the viewer binary is
//! started as a child process (`docs/specs/startup.md` AC-01, AC-02).

#[cfg(all(unix, not(target_os = "macos")))]
mod linux {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Output, Stdio};
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    /// A fresh folder for one test: settings, caches and the runtime
    /// folder with the single-instance socket.
    fn folder(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blinzel-start-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The viewer with `args`, its settings, caches and socket in `dir`, on
    /// `display` (none: no X11 and no Wayland display at all).
    fn viewer(dir: &Path, display: Option<&str>, args: &[&Path]) -> Child {
        let mut c = Command::new(env!("CARGO_BIN_EXE_blinzel"));
        c.args(args)
            .current_dir(dir)
            .env("HOME", dir)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_CACHE_HOME", dir.join("cache"))
            .env("XDG_RUNTIME_DIR", dir)
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match display {
            Some(d) => c.env("DISPLAY", d),
            None => c.env_remove("DISPLAY"),
        };
        c.spawn().expect("start the viewer")
    }

    /// Waits up to `limit` for the child; kills it after that.
    fn finish(mut child: Child, limit: Duration) -> (ExitStatus, String) {
        let until = Instant::now() + limit;
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > until {
                let _ = child.kill();
                let out = child.wait_with_output().unwrap();
                panic!(
                    "still running after {limit:?}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let Output { status, stderr, .. } = child.wait_with_output().unwrap();
        (status, String::from_utf8_lossy(&stderr).into_owned())
    }

    /// The test image of the CI smoke test.
    fn image() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../blinzel-core/testdata/rgb420-64x48.avif")
    }

    /// AC-01: a launch with a file that finds a running viewer hands the
    /// file over and exits with code 0, before it reads the image or creates
    /// a window or a GPU device. The child has no display at all: creating
    /// its window would fail with exit code 1, as the second launch, with
    /// nobody listening, shows.
    #[test]
    fn a_launch_hands_its_file_over_and_exits_before_the_window() {
        let dir = folder("handoff");
        let file = image().canonicalize().unwrap();
        let listener = UnixListener::bind(dir.join("amigo-blinzel.sock")).unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            if let Some(Ok(s)) = listener.incoming().next() {
                let mut line = String::new();
                let _ = BufReader::new(s).read_line(&mut line);
                let _ = tx.send(line);
            }
        });
        let (status, stderr) = finish(viewer(&dir, None, &[&file]), Duration::from_secs(30));
        assert_eq!(status.code(), Some(0), "handed over: {stderr}");
        let line = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("no hand-over");
        assert_eq!(line.trim_end(), file.to_str().unwrap());

        std::fs::remove_file(dir.join("amigo-blinzel.sock")).unwrap();
        let (status, stderr) = finish(viewer(&dir, None, &[&file]), Duration::from_secs(30));
        assert_eq!(status.code(), Some(1), "nobody to hand over to");
        assert!(stderr.contains("no display"), "{stderr}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The value of the mark `name` in a startup report.
    fn mark(report: &str, name: &str) -> Option<f64> {
        let marks = &report[report.find("\"marks\"")?..];
        let key = format!("\"{name}\":");
        let rest = marks[marks.find(&key)? + key.len()..].trim_start();
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        rest[..end].trim().parse().ok()
    }

    #[test]
    fn mark_reads_a_value_from_the_report() {
        let r = r#"{"adapter": "x", "marks": {"main": 1.500, "first_present": 20.000}}"#;
        assert_eq!(mark(r, "main"), Some(1.5));
        assert_eq!(mark(r, "first_present"), Some(20.0));
        assert_eq!(mark(r, "window_created"), None);
    }

    /// AC-02: the decode of the file is queued before the window and the
    /// GPU device are created. Needs an X server (Xvfb, see AGENTS.md
    /// "Display-bound Linux tests run only on Xvfb").
    #[test]
    fn startup_order_queues_the_decode_before_the_window_and_the_device() {
        let Some(display) = std::env::var_os("BLINZEL_X11_TEST") else {
            return;
        };
        let dir = folder("order");
        let json = dir.join("startup.json");
        let img = image();
        let args = [
            img.as_path(),
            Path::new("--exit-after-first-frame"),
            Path::new("--json"),
            json.as_path(),
        ];
        let child = viewer(&dir, display.to_str(), &args);
        let (status, stderr) = finish(child, Duration::from_secs(120));
        assert_eq!(status.code(), Some(0), "{stderr}");
        let report = std::fs::read_to_string(&json).unwrap();
        let at = |name| mark(&report, name).unwrap_or_else(|| panic!("no {name}: {report}"));
        let queued = at("first_job_queued");
        assert!(queued < at("window_created"), "{report}");
        assert!(queued < at("device_ready"), "{report}");
        assert!(at("first_present") > queued, "{report}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(windows)]
mod win {
    use std::path::Path;
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use blinzel_app::instance::{self, Request, WINDOW_CLASS};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW, MSG,
        PM_REMOVE, PeekMessageW, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WM_COPYDATA,
        WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::{PCWSTR, w};

    /// What the stand-in for the running viewer received, with the number
    /// of viewer windows that existed at that moment.
    static RECEIVED: Mutex<Vec<(Option<Request>, usize)>> = Mutex::new(Vec::new());

    /// Top-level windows of the viewer's class, hidden ones included.
    fn viewer_windows() -> usize {
        let mut n = 0;
        let mut prev: Option<HWND> = None;
        while let Ok(h) = unsafe { FindWindowExW(None, prev, WINDOW_CLASS, PCWSTR::null()) } {
            if h.is_invalid() {
                break;
            }
            n += 1;
            prev = Some(h);
        }
        n
    }

    extern "system" fn proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if m == WM_COPYDATA {
            // SAFETY: a WM_COPYDATA's LPARAM points to the sender's
            // COPYDATASTRUCT while the message is handled.
            let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
            let request = instance::received(cds);
            RECEIVED.lock().unwrap().push((request, viewer_windows()));
            return LRESULT(1);
        }
        unsafe { DefWindowProcW(h, m, wp, lp) }
    }

    /// A hidden window of the viewer's class that takes every hand-over.
    fn running_viewer() -> HWND {
        unsafe {
            let instance = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance,
                lpszClassName: WINDOW_CLASS,
                ..Default::default()
            };
            RegisterClassW(&wc);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                WINDOW_CLASS,
                w!("test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                300,
                200,
                None,
                None,
                Some(instance),
                None,
            )
            .unwrap()
        }
    }

    /// Handles the messages sent to this thread's windows.
    fn pump() {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// AC-01: a launch with a file that finds a running viewer hands the
    /// file over and exits with code 0; while the running viewer handles
    /// the hand-over, the launch has no window of its own yet.
    #[test]
    fn a_launch_hands_its_file_over_and_exits_before_the_window() {
        let dir = std::env::temp_dir().join(format!("blinzel-start-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../blinzel-core/testdata/rgb420-64x48.avif")
            .canonicalize()
            .unwrap();
        let hwnd = running_viewer();
        assert_eq!(viewer_windows(), 1);
        let mut child = Command::new(env!("CARGO_BIN_EXE_blinzel"))
            .arg(&file)
            .current_dir(&dir)
            .env("APPDATA", dir.join("roaming"))
            .env("LOCALAPPDATA", dir.join("local"))
            .spawn()
            .expect("start the viewer");
        let until = Instant::now() + Duration::from_secs(60);
        let status = loop {
            pump();
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            if Instant::now() > until {
                let _ = child.kill();
                panic!("the launch did not exit");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        assert_eq!(status.code(), Some(0));
        let received = std::mem::take(&mut *RECEIVED.lock().unwrap());
        let Some((Some(Request::Open { path, .. }), count)) = received.into_iter().next() else {
            panic!("nothing handed over");
        };
        // `canonicalize` gives a verbatim path (\\?\C:\…); compare the files.
        assert_eq!(path.canonicalize().unwrap(), file);
        assert_eq!(count, 1, "the launch created a window before handing over");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
