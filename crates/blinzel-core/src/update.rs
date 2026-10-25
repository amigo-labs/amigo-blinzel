//! Self-update decisions, independent of Windows (M7): versions, the GitHub
//! release JSON, `SHA256SUMS` with its Ed25519 signature, the daily
//! schedule and the start counter that triggers a rollback. The platform
//! side (download, file swap) lives in the app.
//!
//! Releases carry no code signing certificate, so the Ed25519 signature of
//! `SHA256SUMS` with our own release key is what proves an executable is
//! ours.

use std::fmt;

use crate::json::Json;

pub const REPO: &str = "amigo-labs/amigo-blinzel";
pub const SUMS: &str = "SHA256SUMS";
pub const SUMS_SIG: &str = "SHA256SUMS.sig";
/// Seconds between automatic checks.
pub const CHECK_INTERVAL_S: u64 = 24 * 3600;
/// Starts of a new version that may fail before the first frame; the next
/// start rolls back.
pub const MAX_FAILED_STARTS: u32 = 3;

/// The release asset for an architecture (`x64`, `arm64`).
pub fn asset_name(arch: &str) -> String {
    format!("amigo-blinzel-setup-{arch}.exe")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// `1.2.3` or `v1.2.3`; pre-releases (`1.2.3-rc.1`) are not offered.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let mut it = s.split('.');
        let mut part = || it.next()?.parse::<u32>().ok();
        let v = Version {
            major: part()?,
            minor: part()?,
            patch: part()?,
        };
        it.next().is_none().then_some(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// What the updater downloads for one release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub exe_url: String,
    pub sums_url: String,
    pub sig_url: String,
}

/// Reads `GET /repos/{REPO}/releases/latest`; `None` for drafts,
/// pre-releases, unparsable tags or missing assets.
pub fn parse_release(json: &str, arch: &str) -> Option<Release> {
    let j = Json::parse(json)?;
    if j.get("draft").and_then(Json::as_bool) == Some(true)
        || j.get("prerelease").and_then(Json::as_bool) == Some(true)
    {
        return None;
    }
    let version = Version::parse(j.get("tag_name")?.as_str()?)?;
    let assets = j.get("assets")?.as_array()?;
    let url = |name: &str| {
        assets
            .iter()
            .find(|a| a.get("name").and_then(Json::as_str) == Some(name))?
            .get("browser_download_url")?
            .as_str()
            .map(str::to_owned)
    };
    Some(Release {
        version,
        exe_url: url(&asset_name(arch))?,
        sums_url: url(SUMS)?,
        sig_url: url(SUMS_SIG)?,
    })
}

/// The SHA-256 listed for `name` in a `sha256sum`-style file
/// (`<hex>  <name>` or `<hex> *<name>`).
pub fn sum_for(sums: &str, name: &str) -> Option<[u8; 32]> {
    sums.lines().find_map(|line| {
        let (hex, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start();
        let file = file.strip_prefix('*').unwrap_or(file);
        (file == name).then(|| parse_hex::<32>(hex)).flatten()
    })
}

/// Fixed-size bytes from hex (either case).
pub fn parse_hex<const N: usize>(hex: &str) -> Option<[u8; N]> {
    let hex = hex.trim();
    if hex.len() != 2 * N || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks the Ed25519 signature of `message`. `sig` is the `.sig` file: the
/// 64-byte signature, raw or as hex text.
pub fn verify_signature(message: &[u8], sig: &[u8], public_key: &[u8; 32]) -> bool {
    let sig: [u8; 64] = match sig.len() {
        64 => sig.try_into().unwrap(),
        _ => match std::str::from_utf8(sig).ok().and_then(parse_hex::<64>) {
            Some(s) => s,
            None => return false,
        },
    };
    let pk = ed25519_compact::PublicKey::new(*public_key);
    pk.verify(message, &ed25519_compact::Signature::new(sig))
        .is_ok()
}

/// Whether an automatic check is due. A last check in the future (clock
/// changed) counts as due.
pub fn check_due(last_check: Option<u64>, now: u64) -> bool {
    match last_check {
        None => true,
        Some(t) => t > now || now - t >= CHECK_INTERVAL_S,
    }
}

/// Persistent updater state (a small `key = value` file).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Unix time of the last automatic check.
    pub last_check: Option<u64>,
    /// A freshly installed version that has not shown a frame yet.
    pub pending: Option<Version>,
    /// Starts of `pending` that did not reach the first frame.
    pub failed_starts: u32,
}

impl State {
    pub fn parse(text: &str) -> State {
        let mut s = State::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim().trim_matches('"');
            match k.trim() {
                "last_check" => s.last_check = v.parse().ok(),
                "pending" => s.pending = Version::parse(v),
                "failed_starts" => s.failed_starts = v.parse().unwrap_or(0),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if let Some(t) = self.last_check {
            out += &format!("last_check = {t}\n");
        }
        if let Some(p) = self.pending {
            out += &format!(
                "pending = \"{p}\"\nfailed_starts = {}\n",
                self.failed_starts
            );
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartAction {
    Continue,
    /// The new version failed too often: restore the previous executable.
    Rollback,
}

/// Called at every start, before anything that could crash; the caller
/// saves the state when it changed.
pub fn on_start(state: &mut State, running: Version) -> StartAction {
    match state.pending {
        Some(p) if p == running => {
            if state.failed_starts >= MAX_FAILED_STARTS {
                return StartAction::Rollback;
            }
            state.failed_starts += 1;
        }
        // Rolled back, or replaced by hand: nothing pending for us.
        Some(_) => {
            state.pending = None;
            state.failed_starts = 0;
        }
        None => {}
    }
    StartAction::Continue
}

/// Called after the first frame: the running version works.
pub fn on_first_frame(state: &mut State) {
    state.pending = None;
    state.failed_starts = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        let v = |s| Version::parse(s).unwrap();
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("2.0.1").to_string(), "2.0.1");
        for bad in ["1.2", "1.2.3.4", "1.2.3-rc.1", "x.y.z", ""] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    const RELEASE: &str = r#"{
        "tag_name": "v0.2.0", "draft": false, "prerelease": false,
        "assets": [
            {"name": "amigo-blinzel-setup-x64.exe", "browser_download_url": "https://x/exe64"},
            {"name": "amigo-blinzel-setup-arm64.exe", "browser_download_url": "https://x/exearm"},
            {"name": "SHA256SUMS", "browser_download_url": "https://x/sums"},
            {"name": "SHA256SUMS.sig", "browser_download_url": "https://x/sig"}
        ]}"#;

    #[test]
    fn picks_the_assets_for_the_architecture() {
        let r = parse_release(RELEASE, "arm64").unwrap();
        assert_eq!(r.version, Version::parse("0.2.0").unwrap());
        assert_eq!(r.exe_url, "https://x/exearm");
        assert_eq!(r.sums_url, "https://x/sums");
        assert_eq!(r.sig_url, "https://x/sig");
        assert!(parse_release(RELEASE, "riscv").is_none());
        let pre = RELEASE.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(parse_release(&pre, "x64").is_none());
    }

    #[test]
    fn reads_checksum_files() {
        let h = "ab".repeat(32);
        let sums = format!(
            "{h}  amigo-blinzel-setup-x64.exe\n{} *other.exe\n",
            "cd".repeat(32)
        );
        assert_eq!(
            sum_for(&sums, "amigo-blinzel-setup-x64.exe"),
            Some([0xab; 32])
        );
        assert_eq!(sum_for(&sums, "other.exe"), Some([0xcd; 32]));
        assert_eq!(sum_for(&sums, "missing.exe"), None);
        assert_eq!(sum_for("zz  x", "x"), None);
    }

    #[test]
    fn verifies_ed25519_signatures() {
        let kp = ed25519_compact::KeyPair::from_seed(ed25519_compact::Seed::new([7; 32]));
        let msg = b"hash  amigo-blinzel-setup-x64.exe\n";
        let sig = kp.sk.sign(msg, None);
        let pk: [u8; 32] = *kp.pk;
        assert!(verify_signature(msg, sig.as_ref(), &pk));
        assert!(verify_signature(msg, to_hex(sig.as_ref()).as_bytes(), &pk));
        assert!(!verify_signature(b"tampered", sig.as_ref(), &pk));
        assert!(!verify_signature(msg, b"garbage", &pk));
        let other: [u8; 32] =
            *ed25519_compact::KeyPair::from_seed(ed25519_compact::Seed::new([8; 32])).pk;
        assert!(!verify_signature(msg, sig.as_ref(), &other));
    }

    #[test]
    fn checks_at_most_daily() {
        assert!(check_due(None, 1000));
        assert!(!check_due(Some(1000), 1000 + CHECK_INTERVAL_S - 1));
        assert!(check_due(Some(1000), 1000 + CHECK_INTERVAL_S));
        assert!(check_due(Some(5000), 1000));
    }

    #[test]
    fn rolls_back_after_three_failed_starts() {
        let new = Version::parse("0.2.0").unwrap();
        let mut s = State {
            last_check: Some(1),
            pending: Some(new),
            failed_starts: 0,
        };
        for _ in 0..MAX_FAILED_STARTS {
            assert_eq!(on_start(&mut s, new), StartAction::Continue);
        }
        assert_eq!(on_start(&mut s, new), StartAction::Rollback);
        // The restored old version clears the pending update.
        let old = Version::parse("0.1.0").unwrap();
        assert_eq!(on_start(&mut s, old), StartAction::Continue);
        assert_eq!(s.pending, None);
        // A start that reaches the first frame ends the trial.
        let mut s = State {
            pending: Some(new),
            ..State::default()
        };
        on_start(&mut s, new);
        on_first_frame(&mut s);
        assert_eq!(s, State::default());
    }

    #[test]
    fn state_round_trips() {
        let s = State {
            last_check: Some(1_700_000_000),
            pending: Version::parse("1.0.1"),
            failed_starts: 2,
        };
        assert_eq!(State::parse(&s.to_text()), s);
        assert_eq!(State::parse("garbage\n= =\n"), State::default());
    }
}
