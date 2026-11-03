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

    /// The `WM_CLASS` of every top-level window on `display`.
    fn window_classes(display: &str) -> Vec<Vec<u8>> {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
        let (conn, screen) = x11rb::connect(Some(display)).expect("X server");
        let root = conn.setup().roots[screen].root;
        let tree = conn.query_tree(root).unwrap().reply().unwrap();
        tree.children
            .iter()
            .filter_map(|&w| {
                let p = conn
                    .get_property(false, w, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
                    .ok()?
                    .reply()
                    .ok()?;
                (!p.value.is_empty()).then_some(p.value)
            })
            .collect()
    }

    /// `docs/specs/installation.md` AC-16: the viewer window's X11
    /// `WM_CLASS` (instance and class) is `amigo-blinzel`, the id and the
    /// `StartupWMClass` of the desktop entry `--install` writes. Needs an X
    /// server (Xvfb).
    #[test]
    fn window_class_is_the_desktop_entrys() {
        use blinzel_app::linux::install::{APP_ID, Layout, desktop_entry};
        let Some(display) = std::env::var_os("BLINZEL_X11_TEST") else {
            return;
        };
        let display = display.to_str().unwrap();
        let dir = folder("class");
        let mut child = viewer(&dir, Some(display), &[&image()]);
        let want = format!("{APP_ID}\0{APP_ID}\0").into_bytes();
        let until = Instant::now() + Duration::from_secs(60);
        let mut seen = vec![];
        while Instant::now() < until && !seen.contains(&want) {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            seen = window_classes(display);
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let out = child.wait_with_output().unwrap();
        let classes: Vec<_> = seen.iter().map(|c| String::from_utf8_lossy(c)).collect();
        assert!(
            seen.contains(&want),
            "WM_CLASS {classes:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let layout = Layout::under(&dir);
        let file = layout.desktop_file();
        assert_eq!(file.file_name().unwrap(), "amigo-blinzel.desktop");
        let entry = desktop_entry(&layout.exe(), "");
        assert!(
            entry.contains("\nStartupWMClass=amigo-blinzel\n"),
            "{entry}"
        );
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

/// AC-08: what the viewer loads before its first frame, seen by the test
/// as the viewer's debugger (module loads with the thread that loaded
/// them) and through traps in the viewer's own COM imports.
#[cfg(windows)]
mod win_modules {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{
        CloseHandle, DBG_CONTINUE, DBG_EXCEPTION_NOT_HANDLED, EXCEPTION_ACCESS_VIOLATION,
        EXCEPTION_BREAKPOINT, HANDLE, HLOCAL, LocalFree,
    };
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_DOS};
    use windows::Win32::System::Diagnostics::Debug::{
        CREATE_PROCESS_DEBUG_EVENT, ContinueDebugEvent, DEBUG_EVENT, EXCEPTION_DEBUG_EVENT,
        EXIT_PROCESS_DEBUG_EVENT, LOAD_DLL_DEBUG_EVENT, ReadProcessMemory, WaitForDebugEvent,
        WriteProcessMemory,
    };
    use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtectEx};
    use windows::Win32::System::Threading::{
        DEBUG_ONLY_THIS_PROCESS, GetThreadDescription, OpenThread,
        THREAD_QUERY_LIMITED_INFORMATION, TerminateProcess,
    };

    /// The COM entry points the viewer may call, trapped in its imports.
    const COM: [&str; 3] = ["CoInitializeEx", "CoInitialize", "OleInitialize"];
    /// Import `COM[i]` points at `TRAP + 16 * i`, in the never-mapped
    /// first 64 KiB: a call from the viewer's code faults there.
    const TRAP: usize = 0x100;

    /// One module load: file name (lowercase), loading thread, its name.
    #[derive(Debug)]
    struct Load {
        dll: String,
        tid: u32,
        thread: String,
    }

    /// `len` bytes of the debuggee at `addr` (fewer where it stops being
    /// readable).
    fn read(process: HANDLE, addr: usize, len: usize) -> Vec<u8> {
        let mut buf = vec![0u8; len];
        let mut got = 0;
        // SAFETY: `buf` holds `len` writable bytes; the debuggee's memory
        // is only read.
        let _ = unsafe {
            ReadProcessMemory(
                process,
                addr as _,
                buf.as_mut_ptr().cast(),
                len,
                Some(&mut got),
            )
        };
        buf.truncate(got);
        buf
    }

    fn u32_at(b: &[u8], at: usize) -> usize {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
    }

    /// Points the import slots of [`COM`] in the image at `base` (the
    /// viewer's executable, imports already bound) at their traps;
    /// returns how many slots it changed.
    fn trap_com_imports(process: HANDLE, base: usize) -> usize {
        let dos = read(process, base, 0x40);
        let nt = base + u32_at(&dos, 0x3c);
        // Signature, file header (20 bytes), optional header.
        let hdr = read(process, nt, 24 + 240);
        assert_eq!(
            u16::from_le_bytes([hdr[24], hdr[25]]),
            0x20b,
            "a PE32+ image"
        );
        // The import directory: data directory 1 of PE32+, at 112.
        let imports = u32_at(&hdr, 24 + 112 + 8);
        let mut changed = 0;
        for d in 0.. {
            let desc = read(process, base + imports + 20 * d, 20);
            let (names, dll, slots) = (u32_at(&desc, 0), u32_at(&desc, 12), u32_at(&desc, 16));
            if dll == 0 {
                break;
            }
            if names == 0 {
                continue;
            }
            for i in 0.. {
                let entry = read(process, base + names + 8 * i, 8);
                let thunk = u64::from_le_bytes(entry.try_into().unwrap());
                if thunk == 0 {
                    break;
                }
                if thunk >> 63 == 1 {
                    continue; // by ordinal
                }
                // IMAGE_IMPORT_BY_NAME: a 2-byte hint, then the name.
                let name = read(process, base + thunk as usize + 2, 16);
                let Some(k) = COM
                    .iter()
                    .position(|f| name.starts_with(f.as_bytes()) && name.get(f.len()) == Some(&0))
                else {
                    continue;
                };
                let slot = base + slots + 8 * i;
                let trap = (TRAP + 16 * k) as u64;
                let mut old = PAGE_PROTECTION_FLAGS::default();
                // SAFETY: `slot` is an import slot of the stopped debuggee;
                // 8 bytes are made writable, written from `trap` and
                // protected again.
                unsafe {
                    VirtualProtectEx(process, slot as _, 8, PAGE_READWRITE, &mut old).unwrap();
                    WriteProcessMemory(process, slot as _, (&raw const trap).cast(), 8, None)
                        .unwrap();
                    VirtualProtectEx(process, slot as _, 8, old, &mut old).unwrap();
                }
                changed += 1;
            }
        }
        changed
    }

    /// The name of the debuggee's thread `tid` ("" if it has none).
    fn thread_name(tid: u32) -> String {
        // SAFETY: the handle is closed here; the description is freed
        // with LocalFree as GetThreadDescription requires.
        unsafe {
            let Ok(h) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid) else {
                return "?".into();
            };
            let name = match GetThreadDescription(h) {
                Ok(p) => {
                    let s = p.to_string().unwrap_or_default();
                    let _ = LocalFree(Some(HLOCAL(p.0.cast())));
                    s
                }
                Err(_) => "?".into(),
            };
            let _ = CloseHandle(h);
            name
        }
    }

    /// The file name (lowercase) of a module load's file handle.
    fn dll_name(file: HANDLE) -> String {
        let mut buf = [0u16; 1024];
        // SAFETY: `buf` is the output buffer; `file` is the debug event's
        // handle, valid until closed by the caller.
        let n = unsafe { GetFinalPathNameByHandleW(file, &mut buf, VOLUME_NAME_DOS) } as usize;
        let path = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        path.rsplit('\\').next().unwrap_or("?").to_lowercase()
    }

    /// Until the first image is presented the viewer loads neither
    /// `d2d1.dll` nor `dwrite.dll` (only the text warm-up thread, started
    /// after `first_present`, does), `d3d11.dll` is loaded at run time by
    /// the device thread (not by the loader on the main thread), and the
    /// viewer's code initializes COM nowhere (a native PNG needs no WIC,
    /// dialog or shortcut).
    #[test]
    fn the_first_frame_needs_no_text_dll_d3d11_from_the_device_thread_and_no_com() {
        let dir = std::env::temp_dir().join(format!("blinzel-modules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("a.png");
        let bgra = [40u8, 90, 200, 255].repeat(64 * 48);
        std::fs::write(
            &image,
            blinzel_core::export::encode_png_bgra(&bgra, 64, 48).unwrap(),
        )
        .unwrap();
        let report = dir.join("startup.json");
        let child = Command::new(env!("CARGO_BIN_EXE_blinzel"))
            .arg(&image)
            .arg("--exit-after-first-frame")
            .arg("--json")
            .arg(&report)
            .current_dir(&dir)
            .env("APPDATA", dir.join("roaming"))
            .env("LOCALAPPDATA", dir.join("local"))
            .creation_flags(DEBUG_ONLY_THIS_PROCESS.0)
            .spawn()
            .expect("start the viewer under the test as its debugger");

        let until = Instant::now() + Duration::from_secs(120);
        let mut process = HANDLE::default();
        let mut main_tid = 0;
        let mut trapped = None;
        let mut loads: Vec<Load> = Vec::new();
        let mut com: Option<(&str, String)> = None;
        let exit_code = loop {
            let mut ev = DEBUG_EVENT::default();
            // SAFETY: `ev` is written by the call; this thread started the
            // debuggee, so it receives its events.
            if unsafe { WaitForDebugEvent(&mut ev, 1000) }.is_err() {
                assert!(Instant::now() < until, "no exit after 120 s: {loads:#?}");
                continue;
            }
            let mut status = DBG_CONTINUE;
            // SAFETY: each arm reads the union member its event code names.
            unsafe {
                match ev.dwDebugEventCode {
                    CREATE_PROCESS_DEBUG_EVENT => {
                        let info = ev.u.CreateProcessInfo;
                        process = info.hProcess;
                        main_tid = ev.dwThreadId;
                        trapped = Some(info.lpBaseOfImage as usize);
                        let _ = CloseHandle(info.hFile);
                    }
                    LOAD_DLL_DEBUG_EVENT => {
                        let file = ev.u.LoadDll.hFile;
                        loads.push(Load {
                            dll: dll_name(file),
                            tid: ev.dwThreadId,
                            thread: thread_name(ev.dwThreadId),
                        });
                        let _ = CloseHandle(file);
                    }
                    EXCEPTION_DEBUG_EVENT => {
                        let rec = ev.u.Exception.ExceptionRecord;
                        let at = rec.ExceptionAddress as usize;
                        if rec.ExceptionCode == EXCEPTION_BREAKPOINT
                            && let Some(base) = trapped.take()
                        {
                            // The loader's breakpoint: imports are bound,
                            // `main` has not run.
                            assert!(
                                trap_com_imports(process, base) >= 1,
                                "CoInitializeEx imported"
                            );
                        } else if rec.ExceptionCode == EXCEPTION_ACCESS_VIOLATION
                            && (TRAP..TRAP + 16 * COM.len()).contains(&at)
                        {
                            if com.is_none() {
                                com = Some((COM[(at - TRAP) / 16], thread_name(ev.dwThreadId)));
                            }
                            let _ = TerminateProcess(process, 3);
                        } else {
                            // The viewer's own exceptions go to its handlers.
                            status = DBG_EXCEPTION_NOT_HANDLED;
                        }
                    }
                    EXIT_PROCESS_DEBUG_EVENT => {
                        let code = ev.u.ExitProcess.dwExitCode;
                        let _ = ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, status);
                        break code;
                    }
                    _ => {}
                }
                let _ = ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, status);
            }
        };
        drop(child);
        let trace: Vec<String> = loads
            .iter()
            .map(|l| format!("{} on {} {:?}", l.dll, l.tid, l.thread))
            .collect();
        eprintln!(
            "main thread {main_tid}; module loads:\n{}",
            trace.join("\n")
        );

        assert_eq!(
            com, None,
            "the viewer initialized COM before its first frame"
        );
        assert_eq!(exit_code, 0, "the first frame was presented");
        let json = std::fs::read_to_string(&report).unwrap();
        assert!(json.contains("\"first_present\": "), "{json}");
        let first = |dll: &str| loads.iter().find(|l| l.dll == dll);
        let d3d = first("d3d11.dll").expect("d3d11.dll is loaded");
        assert_ne!(
            d3d.tid, main_tid,
            "d3d11.dll comes from the main thread: {d3d:?}"
        );
        assert_eq!(d3d.thread, "blinzel-gfx-init", "{d3d:?}");
        for dll in ["d2d1.dll", "dwrite.dll"] {
            if let Some(l) = first(dll) {
                assert_eq!(
                    l.thread, "blinzel-text-prewarm",
                    "{dll} before the first frame: {l:?}"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
