//! One window for all opened files: the first instance listens on a Unix
//! socket in `$XDG_RUNTIME_DIR`; later ones send it their file and exit.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

fn socket() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("amigo-blinzel.sock"))
}

/// Hands `file` (or just "show yourself") to a running instance. True if
/// one took it.
pub fn hand_off(file: Option<&Path>) -> bool {
    let Some(sock) = socket() else { return false };
    let Ok(mut s) = UnixStream::connect(&sock) else {
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
    let Some(sock) = socket() else { return };
    let _ = std::fs::remove_file(&sock);
    let Ok(listener) = UnixListener::bind(&sock) else {
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
