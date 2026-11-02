//! Self-update decisions, independent of Windows (M7): versions, the GitHub
//! release JSON, `SHA256SUMS` with its Ed25519 signature, the daily
//! schedule and the start counter that triggers a rollback. The platform
//! side (download, file swap) lives in the app.
//!
//! Releases carry no code signing certificate, so the Ed25519 signature of
//! `SHA256SUMS` with our own release key is what proves an executable is
//! ours.

use std::fmt;
use std::time::Duration;

use crate::json::Json;

/// Public key of the release signing key (docs/specs/blinzel.md, section 10), the
/// same on every platform. Until it is set, the updaters check nothing and
/// install nothing.
pub const RELEASE_KEY: Option<[u8; 32]> = Some([
    0x27, 0x6b, 0x0f, 0xbd, 0x4a, 0x1f, 0x9e, 0xb6, 0x84, 0x76, 0xf0, 0xb8, 0xa1, 0x4c, 0x5c, 0xc0,
    0xba, 0xb4, 0x03, 0xcb, 0xca, 0x24, 0xbc, 0xf0, 0x7d, 0x8c, 0xff, 0x33, 0x3d, 0x3f, 0xa9, 0xb7,
]);

pub const REPO: &str = "amigo-labs/amigo-blinzel";
pub const SUMS: &str = "SHA256SUMS";
pub const SUMS_SIG: &str = "SHA256SUMS.sig";
/// Seconds between automatic checks.
pub const CHECK_INTERVAL_S: u64 = 24 * 3600;
/// Starts of a new version that may fail before the first frame; the next
/// start rolls back.
pub const MAX_FAILED_STARTS: u32 = 3;

/// The Windows release asset for an architecture (`x64`, `arm64`).
pub fn asset_name(arch: &str) -> String {
    format!("amigo-blinzel-setup-{arch}.exe")
}

/// The bare Linux executable for an architecture (`x64`, `arm64`); the
/// updater fetches it instead of the `.tar.gz` people download.
pub fn linux_asset_name(arch: &str) -> String {
    format!("amigo-blinzel-linux-{arch}")
}

/// The zipped macOS app bundle (universal: Apple silicon and Intel).
pub fn macos_asset_name() -> String {
    "amigo-blinzel-macos.zip".into()
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

/// Reads `GET /repos/{REPO}/releases/latest` for the executable `asset`
/// ([`asset_name`], [`linux_asset_name`]); `None` for drafts, pre-releases,
/// unparsable tags or missing assets.
pub fn parse_release(json: &str, asset: &str) -> Option<Release> {
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
        exe_url: url(asset)?,
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

/// SHA-256 (FIPS 180-4) of `data`, for platforms without one at hand.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut block = |chunk: &[u8; 64]| {
        let mut w = [0u32; 64];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    };
    let (blocks, rest) = data.as_chunks::<64>();
    for b in blocks {
        block(b);
    }
    // Padding: 0x80, zeros, the bit length; one or two final blocks.
    let mut tail = [0u8; 128];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;
    let n = if rest.len() < 56 { 64 } else { 128 };
    tail[n - 8..n].copy_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for b in tail[..n].as_chunks::<64>().0 {
        block(b);
    }
    let mut out = [0u8; 32];
    for (o, x) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *o = x.to_be_bytes();
    }
    out
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

/// The GitHub API address of the latest release.
pub fn latest_release_url() -> String {
    format!("https://api.github.com/repos/{REPO}/releases/latest")
}

/// What [`check`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// The latest release is not newer than the running version (given).
    Current(Version),
    /// A newer release, downloaded and verified: the shell puts it in place.
    New { version: Version, download: Vec<u8> },
}

/// One check against the latest release, the same on every platform
/// (`docs/specs/updates.md` AC-01..AC-08). `fetch(url, limit)` is the
/// shell's HTTPS GET of at most `limit` bytes, `hash` its SHA-256. Only a
/// release newer than `running` is downloaded: first `SHA256SUMS` and its
/// signature, which must verify with `key`, then `asset` (at most
/// `max_download` bytes), whose hash must be the one listed for it.
pub fn check(
    asset: &str,
    running: Version,
    key: Option<[u8; 32]>,
    max_download: usize,
    mut fetch: impl FnMut(&str, usize) -> Result<Vec<u8>, String>,
    hash: impl FnOnce(&[u8]) -> Result<[u8; 32], String>,
) -> Result<Found, String> {
    let key = key.ok_or("no release signing key in this build")?;
    let json = fetch(&latest_release_url(), 1 << 20)?;
    let json = String::from_utf8(json).map_err(|_| "release info is not UTF-8")?;
    let Release {
        version,
        exe_url,
        sums_url,
        sig_url,
    } = parse_release(&json, asset).ok_or("no usable release")?;
    if version <= running {
        return Ok(Found::Current(running));
    }
    let sums = fetch(&sums_url, 64 << 10)?;
    let sig = fetch(&sig_url, 4 << 10)?;
    if !verify_signature(&sums, &sig, &key) {
        return Err("checksums are not signed with the release key".into());
    }
    let sums = String::from_utf8(sums).map_err(|_| "checksums are not UTF-8")?;
    let want = sum_for(&sums, asset).ok_or("no checksum for us")?;
    let download = fetch(&exe_url, max_download)?;
    if hash(&download)? != want {
        return Err("download does not match its checksum".into());
    }
    Ok(Found::New { version, download })
}

/// Whether an automatic check is due. A last check in the future (clock
/// changed) counts as due.
pub fn check_due(last_check: Option<u64>, now: u64) -> bool {
    match last_check {
        None => true,
        Some(t) => t > now || now - t >= CHECK_INTERVAL_S,
    }
}

/// Seconds after start before the first automatic check: the startup path
/// stays free of it.
pub const FIRST_CHECK_DELAY_S: u64 = 10;
/// Seconds between two looks of a long-running (resident) instance at
/// whether a check is due.
pub const RECHECK_S: u64 = 3600;

/// What the automatic check needs from a shell ([`auto_checks`]).
pub trait AutoCheck {
    /// Waits `d`; `false` ends the checks.
    fn sleep(&mut self, d: Duration) -> bool;
    /// The `auto_update` setting, read anew each time.
    fn auto_update(&mut self) -> bool;
    /// Unix time now.
    fn now(&mut self) -> u64;
    fn load(&mut self) -> State;
    fn save(&mut self, state: &State);
    /// Checks and puts an update in place; the shell logs a failure.
    fn check(&mut self);
}

/// The automatic check's thread body (`docs/specs/updates.md` AC-10..AC-13):
/// [`FIRST_CHECK_DELAY_S`] after start, and with `repeat` (a Windows
/// instance that keeps running) every [`RECHECK_S`] after that, it reads
/// the setting and, when it is on and a check is due, saves the time of the
/// check before it runs, so a failed check is not repeated within a day.
pub fn auto_checks(shell: &mut impl AutoCheck, repeat: bool) {
    if !shell.sleep(Duration::from_secs(FIRST_CHECK_DELAY_S)) {
        return;
    }
    loop {
        if shell.auto_update() {
            let mut state = shell.load();
            let now = shell.now();
            if check_due(state.last_check, now) {
                state.last_check = Some(now);
                shell.save(&state);
                shell.check();
            }
        }
        if !repeat || !shell.sleep(Duration::from_secs(RECHECK_S)) {
            return;
        }
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

    /// An update was installed while `running` kept running (resident
    /// mode): a hiding window hands over to it
    /// (`docs/specs/resident-mode.md` AC-21).
    pub fn restart_pending(&self, running: Version) -> bool {
        self.pending.is_some_and(|p| p != running)
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

    /// `docs/specs/resident-mode.md` AC-21: only an installed version other
    /// than the running one takes over.
    #[test]
    fn restart_pending_only_for_another_installed_version() {
        let v = |s| Version::parse(s).unwrap();
        let state = |pending| State {
            pending,
            ..State::default()
        };
        assert!(
            !state(None).restart_pending(v("1.3.0")),
            "nothing installed"
        );
        assert!(
            !state(Some(v("1.3.0"))).restart_pending(v("1.3.0")),
            "running"
        );
        assert!(state(Some(v("1.4.0"))).restart_pending(v("1.3.0")));
        assert!(
            state(Some(v("1.2.0"))).restart_pending(v("1.3.0")),
            "a rollback"
        );
    }

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
        let r = parse_release(RELEASE, &asset_name("arm64")).unwrap();
        assert_eq!(r.version, Version::parse("0.2.0").unwrap());
        assert_eq!(r.exe_url, "https://x/exearm");
        assert_eq!(r.sums_url, "https://x/sums");
        assert_eq!(r.sig_url, "https://x/sig");
        assert!(parse_release(RELEASE, &asset_name("riscv")).is_none());
        assert!(parse_release(RELEASE, &linux_asset_name("x64")).is_none());
        let pre = RELEASE.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(parse_release(&pre, &asset_name("x64")).is_none());
    }

    /// AC-01: drafts, pre-releases, tags other than `x.y.z` / `vx.y.z` and
    /// releases without the asset, `SHA256SUMS` or `SHA256SUMS.sig` offer
    /// nothing.
    #[test]
    fn parse_release_offers_only_complete_published_releases() {
        let x64 = asset_name("x64");
        assert!(parse_release(RELEASE, &x64).is_some());
        let tag = |t: &str| RELEASE.replace("\"v0.2.0\"", &format!("\"{t}\""));
        assert!(parse_release(&tag("0.2.0"), &x64).is_some());
        for bad in ["v0.2", "0.2.0.1", "v0.2.0-rc.1", "latest"] {
            assert!(parse_release(&tag(bad), &x64).is_none(), "{bad}");
        }
        let draft = RELEASE.replace("\"draft\": false", "\"draft\": true");
        assert!(parse_release(&draft, &x64).is_none(), "draft");
        let pre = RELEASE.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(parse_release(&pre, &x64).is_none(), "pre-release");
        let without =
            |name: &str| RELEASE.replace(&format!("\"name\": \"{name}\""), "\"name\": \"x\"");
        assert!(
            parse_release(&without(SUMS), &x64).is_none(),
            "no SHA256SUMS"
        );
        assert!(parse_release(&without(SUMS_SIG), &x64).is_none(), "no .sig");
        assert!(parse_release(&without(&x64), &x64).is_none(), "no asset");
    }

    /// AC-04: Linux fetches the bare executable for its architecture, macOS
    /// the zipped universal bundle, out of a release that carries all
    /// platforms.
    #[test]
    fn picks_the_linux_and_macos_assets() {
        assert_eq!(linux_asset_name("x64"), "amigo-blinzel-linux-x64");
        assert_eq!(linux_asset_name("arm64"), "amigo-blinzel-linux-arm64");
        assert_eq!(macos_asset_name(), "amigo-blinzel-macos.zip");
        let all = RELEASE.replace(
            "\"assets\": [",
            r#""assets": [
            {"name": "amigo-blinzel-linux-x64.tar.gz", "browser_download_url": "https://x/tgz"},
            {"name": "amigo-blinzel-linux-x64", "browser_download_url": "https://x/linux64"},
            {"name": "amigo-blinzel-linux-arm64", "browser_download_url": "https://x/linuxarm"},
            {"name": "amigo-blinzel-macos.dmg", "browser_download_url": "https://x/dmg"},
            {"name": "amigo-blinzel-macos.zip", "browser_download_url": "https://x/zip"},"#,
        );
        let url = |asset: String| parse_release(&all, &asset).unwrap().exe_url;
        assert_eq!(url(linux_asset_name("x64")), "https://x/linux64");
        assert_eq!(url(linux_asset_name("arm64")), "https://x/linuxarm");
        assert_eq!(url(macos_asset_name()), "https://x/zip");
    }

    /// A release `tag` whose `SHA256SUMS` lists `download` for the x64
    /// setup, signed with the key from `seed`; the fetcher serves it and
    /// records every URL asked for.
    struct Server {
        files: Vec<(String, Vec<u8>)>,
        asked: Vec<String>,
    }

    impl Server {
        fn new(tag: &str, sums: &str, seed: u8) -> Server {
            let kp = ed25519_compact::KeyPair::from_seed(ed25519_compact::Seed::new([seed; 32]));
            let sig = kp.sk.sign(sums.as_bytes(), None);
            Server {
                files: vec![
                    (
                        latest_release_url(),
                        RELEASE.replace("v0.2.0", tag).into_bytes(),
                    ),
                    ("https://x/sums".into(), sums.as_bytes().to_vec()),
                    ("https://x/sig".into(), sig.as_ref().to_vec()),
                    ("https://x/exe64".into(), b"new exe".to_vec()),
                ],
                asked: Vec::new(),
            }
        }

        fn check(&mut self, running: &str) -> Result<Found, String> {
            let key = *ed25519_compact::KeyPair::from_seed(ed25519_compact::Seed::new([7; 32])).pk;
            let (files, asked) = (&self.files, &mut self.asked);
            check(
                &asset_name("x64"),
                Version::parse(running).unwrap(),
                Some(key),
                1 << 20,
                |url, _| {
                    asked.push(url.to_string());
                    let f = files.iter().find(|(u, _)| u == url).ok_or("404")?;
                    Ok(f.1.clone())
                },
                |d| Ok(sha256(d)),
            )
        }
    }

    fn good_sums() -> String {
        format!(
            "{}  amigo-blinzel-setup-x64.exe\n",
            to_hex(&sha256(b"new exe"))
        )
    }

    /// AC-05: a latest release that is not newer downloads nothing beyond
    /// the release info and reports the running version as current.
    #[test]
    fn check_downloads_nothing_when_not_newer() {
        for (latest, running) in [("v0.2.0", "0.2.0"), ("v0.2.0", "0.10.0")] {
            let mut s = Server::new(latest, &good_sums(), 7);
            assert_eq!(
                s.check(running),
                Ok(Found::Current(Version::parse(running).unwrap()))
            );
            assert_eq!(s.asked, [latest_release_url()], "{latest} vs {running}");
        }
    }

    /// AC-06..AC-08: a newer release is handed over only with a valid
    /// signature, a line for the asset and a matching hash; otherwise the
    /// check fails with its reason and hands nothing over.
    #[test]
    fn check_hands_over_only_a_verified_download() {
        let mut s = Server::new("v0.3.0", &good_sums(), 7);
        assert_eq!(
            s.check("0.2.0"),
            Ok(Found::New {
                version: Version::parse("0.3.0").unwrap(),
                download: b"new exe".to_vec()
            })
        );
        let fails = |sums: &str, seed: u8| Server::new("v0.3.0", sums, seed).check("0.2.0");
        assert_eq!(
            fails(&good_sums(), 8),
            Err("checksums are not signed with the release key".into())
        );
        let other = good_sums().replace("setup-x64", "setup-arm64");
        assert_eq!(fails(&other, 7), Err("no checksum for us".into()));
        let wrong = format!("{}  amigo-blinzel-setup-x64.exe\n", "00".repeat(32));
        assert_eq!(
            fails(&wrong, 7),
            Err("download does not match its checksum".into())
        );
        // A bad signature stops before the download.
        let mut s = Server::new("v0.3.0", &good_sums(), 8);
        let _ = s.check("0.2.0");
        assert!(!s.asked.iter().any(|u| u == "https://x/exe64"));
        // No key in the build: nothing is asked for.
        let r = check(
            "a",
            Version::parse("0.1.0").unwrap(),
            None,
            1,
            |_, _| unreachable!(),
            |_| unreachable!(),
        );
        assert_eq!(r, Err("no release signing key in this build".into()));
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
    fn sha256_matches_known_digests() {
        let h = |d: &[u8]| to_hex(&sha256(d));
        assert_eq!(
            h(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            h(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 56 bytes: the length no longer fits the first padding block.
        assert_eq!(
            h(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            h(&[b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
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

    /// A shell whose clock moves only while it sleeps, which ends the
    /// checks after `sleeps` waits and logs what they do.
    struct Clock {
        t: u64,
        sleeps: u32,
        /// The setting per look; the last one stays.
        setting: Vec<bool>,
        state: State,
        log: Vec<String>,
    }

    const T0: u64 = 1_700_000_000;

    impl Clock {
        fn new(sleeps: u32, setting: &[bool]) -> Clock {
            Clock {
                t: T0,
                sleeps,
                setting: setting.to_vec(),
                state: State::default(),
                log: Vec::new(),
            }
        }

        fn count(&self, what: &str) -> usize {
            self.log.iter().filter(|l| l.starts_with(what)).count()
        }
    }

    impl AutoCheck for Clock {
        fn sleep(&mut self, d: Duration) -> bool {
            if self.sleeps == 0 {
                return false;
            }
            self.sleeps -= 1;
            self.t += d.as_secs();
            self.log.push(format!("sleep {}", d.as_secs()));
            true
        }
        fn auto_update(&mut self) -> bool {
            let on = if self.setting.len() > 1 {
                self.setting.remove(0)
            } else {
                self.setting[0]
            };
            self.log.push(format!("setting {on}"));
            on
        }
        fn now(&mut self) -> u64 {
            self.t
        }
        fn load(&mut self) -> State {
            self.state.clone()
        }
        fn save(&mut self, state: &State) {
            self.state = state.clone();
            self.log.push(format!("save {:?}", state.last_check));
        }
        fn check(&mut self) {
            self.log.push(format!("check at {}", self.t - T0));
        }
    }

    /// AC-11, AC-12: 10 s after start, the time of the check is saved before
    /// the check runs; Linux and macOS (no repeat) look once.
    #[test]
    fn the_automatic_check_waits_and_saves_its_time_first() {
        let mut c = Clock::new(100, &[true]);
        auto_checks(&mut c, false);
        assert_eq!(
            c.log,
            [
                "sleep 10",
                "setting true",
                &format!("save Some({})", T0 + 10),
                "check at 10"
            ]
        );
    }

    /// AC-10, AC-12: a running Windows instance looks every hour and reads
    /// the setting each time, but checks again only a day after the last
    /// check (failed or not).
    #[test]
    fn a_running_instance_looks_every_hour_and_checks_daily() {
        let mut c = Clock::new(1 + 48, &[true]);
        auto_checks(&mut c, true);
        assert_eq!(c.count("sleep 10"), 1);
        assert_eq!(c.count("sleep 3600"), 48);
        assert_eq!(c.count("setting"), 49, "the setting is read at every look");
        let checks: Vec<_> = c.log.iter().filter(|l| l.starts_with("check")).collect();
        assert_eq!(
            checks,
            ["check at 10", "check at 86410", "check at 172810"],
            "{:?}",
            c.log
        );
        // A recent check (another process) is not repeated.
        let mut c = Clock::new(1, &[true]);
        c.state.last_check = Some(T0 - 3600);
        auto_checks(&mut c, false);
        assert_eq!(c.count("check"), 0);
        assert_eq!(c.count("save"), 0);
    }

    /// AC-13: with `auto_update` off nothing is checked or saved; turned on
    /// while the instance runs, the next look checks.
    #[test]
    fn auto_update_off_checks_nothing() {
        let mut c = Clock::new(1 + 30, &[false]);
        auto_checks(&mut c, true);
        assert_eq!((c.count("check"), c.count("save")), (0, 0), "{:?}", c.log);
        let mut c = Clock::new(1 + 20, &[false, false, true]);
        auto_checks(&mut c, true);
        let checks: Vec<_> = c.log.iter().filter(|l| l.starts_with("check")).collect();
        assert_eq!(checks, ["check at 7210"]);
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
