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

/// Public key of the release signing key (docs/PLAN.md, section 10), the
/// same on every platform. Until it is set, the updaters check nothing and
/// install nothing.
pub const RELEASE_KEY: Option<[u8; 32]> = None;

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
