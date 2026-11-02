//! One window for all opened files: the first instance listens on a Unix
//! socket in `$XDG_RUNTIME_DIR`; later ones send it their file and exit.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

fn socket() -> Option<PathBuf> {
    socket_in(std::env::var_os("XDG_RUNTIME_DIR"))
}

/// The socket in the runtime folder; none without one.
fn socket_in(runtime_dir: Option<OsString>) -> Option<PathBuf> {
    let dir = runtime_dir.filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("amigo-blinzel.sock"))
}

/// Hands `file` (or just "show yourself") to a running instance. True if
/// one took it.
pub fn hand_off(file: Option<&Path>) -> bool {
    socket().is_some_and(|sock| hand_off_to(&sock, file))
}

fn hand_off_to(sock: &Path, file: Option<&Path>) -> bool {
    let Ok(mut s) = UnixStream::connect(sock) else {
        return false;
    };
    let line = file
        .and_then(|f| std::path::absolute(f).ok())
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    s.write_all(line.as_bytes())
        .and_then(|()| s.write_all(b"\n"))
        .is_ok()
}

/// Listens for later instances; `received` gets each file (`None`: show
/// the window). The socket of an instance that died is replaced.
pub fn listen(received: impl Fn(Option<PathBuf>) + Send + 'static) {
    if let Some(sock) = socket() {
        listen_on(&sock, received);
    }
}

fn listen_on(sock: &Path, received: impl Fn(Option<PathBuf>) + Send + 'static) {
    let _ = std::fs::remove_file(sock);
    let Ok(listener) = UnixListener::bind(sock) else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("blinzel-instance".into())
        .spawn(move || {
            for s in listener.incoming().flatten() {
                let mut line = String::new();
                if BufReader::new(s).read_line(&mut line).is_ok() {
                    let line = line.trim_end_matches('\n');
                    received((!line.is_empty()).then(|| PathBuf::from(line)));
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    /// A fresh folder for one test's socket.
    fn folder(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blinzel-inst-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Listens on `sock`; what arrives goes into the receiver.
    fn listening(sock: &Path) -> Receiver<Option<PathBuf>> {
        let (tx, rx) = channel();
        listen_on(sock, move |f| {
            let _ = tx.send(f);
        });
        rx
    }

    fn next(rx: &Receiver<Option<PathBuf>>) -> Option<PathBuf> {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("nothing arrived")
    }

    /// `docs/specs/browsing.md` AC-15.
    #[test]
    fn a_later_launch_hands_its_file_to_the_running_window() {
        let dir = folder("handoff");
        let sock = dir.join("s.sock");
        assert!(
            !hand_off_to(&sock, Some(Path::new("/a.png"))),
            "no window yet"
        );
        let rx = listening(&sock);
        assert!(hand_off_to(&sock, Some(Path::new("/pics/a b.png"))));
        assert_eq!(next(&rx), Some(PathBuf::from("/pics/a b.png")));
        assert!(hand_off_to(&sock, Some(Path::new("b.png"))));
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(next(&rx), Some(cwd.join("b.png")), "made absolute");
        assert!(hand_off_to(&sock, None));
        assert_eq!(next(&rx), None, "show the window");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `docs/specs/browsing.md` AC-15, edge case: the socket of a crashed
    /// instance.
    #[test]
    fn the_socket_of_a_crashed_instance_is_replaced() {
        let dir = folder("stale");
        let sock = dir.join("s.sock");
        drop(UnixListener::bind(&sock).unwrap());
        assert!(sock.exists());
        assert!(!hand_off_to(&sock, None), "nobody listens");
        let rx = listening(&sock);
        assert!(hand_off_to(&sock, Some(Path::new("/c.png"))));
        assert_eq!(next(&rx), Some(PathBuf::from("/c.png")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `docs/specs/browsing.md` AC-15, edge case: no `XDG_RUNTIME_DIR`.
    #[test]
    fn without_a_runtime_folder_there_is_no_socket() {
        assert_eq!(socket_in(None), None);
        assert_eq!(socket_in(Some(OsString::new())), None);
        assert_eq!(
            socket_in(Some("/run/user/1000".into())),
            Some(PathBuf::from("/run/user/1000/amigo-blinzel.sock"))
        );
    }
}
