//! HTTPS downloads through the system's `curl` (on every Linux desktop
//! and every Mac): the updaters' Ed25519 signature, not the transport,
//! proves where an update comes from, so the viewer carries no TLS stack
//! of its own.

use std::process::{Command, Stdio};

/// GET `url` (HTTPS only, redirects allowed) with at most `limit` bytes.
pub fn get(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let out = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--proto-redir", "=https"])
        .args(["--max-time", "300", "--max-filesize", &limit.to_string()])
        .arg("--user-agent")
        .arg(format!("amigo-blinzel/{}", env!("CARGO_PKG_VERSION")))
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("curl: {}", err.trim()));
    }
    if out.stdout.len() > limit {
        return Err("download too large".into());
    }
    Ok(out.stdout)
}
