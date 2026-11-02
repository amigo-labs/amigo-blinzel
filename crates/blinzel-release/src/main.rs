//! Release tooling for the self-updater's own signature (M7).
//!
//! ```text
//! blinzel-release keygen <secret-key-file>   new key pair; prints the public key
//! blinzel-release sign <file> <out.sig>      signs with $RELEASE_SIGNING_KEY (hex seed)
//! blinzel-release verify <file> <sig> <public-key-hex>
//! blinzel-release notices [--check]          writes (or checks) THIRD-PARTY-NOTICES*.txt
//! blinzel-release check-release <dir>        the release files are complete, their
//!                                            architectures and SHA256SUMS right
//! blinzel-release macos-bundle <out.app> <blinzel>...
//!                                            the macOS app bundle (universal with
//!                                            one executable per architecture)
//! blinzel-release macos-dmg <app> <out.dmg>  the disk image for downloads
//! ```
//!
//! The secret key is the 32-byte Ed25519 seed as hex. It belongs in the
//! repository secret `RELEASE_SIGNING_KEY` and nowhere in the repository;
//! the public key goes into `blinzel-core/src/update.rs` (`RELEASE_KEY`).

mod assets;
mod macos;
mod notices;

use std::fs;
use std::io::Write;
use std::path::Path;

use blinzel_core::update::{parse_hex, to_hex, verify_signature};
use ed25519_compact::{KeyPair, Seed};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["keygen", path] => keygen(path),
        ["sign", file, out] => sign(file, out),
        ["verify", file, sig, key] => verify(file, sig, key),
        ["notices"] => notices(false),
        ["notices", "--check"] => notices(true),
        ["check-release", dir] => assets::check_dir(Path::new(dir)),
        ["macos-bundle", out, exes @ ..] => {
            let exes: Vec<&Path> = exes.iter().map(Path::new).collect();
            macos::bundle(Path::new(out), &exes, env!("CARGO_PKG_VERSION"))
        }
        ["macos-dmg", app, out] => macos::dmg(Path::new(app), Path::new(out)),
        _ => Err(usage()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn usage() -> String {
    "usage:\n  blinzel-release keygen <secret-key-file>\n  blinzel-release sign <file> <out.sig>   \
     (secret from $RELEASE_SIGNING_KEY)\n  blinzel-release verify <file> <sig> <public-key-hex>\n  \
     blinzel-release notices [--check]\n  \
     blinzel-release check-release <dir>\n  \
     blinzel-release macos-bundle <out.app> <blinzel>...\n  \
     blinzel-release macos-dmg <app> <out.dmg>"
        .into()
}

/// The notices files live in the workspace root, next to `LICENSE`.
fn notices(check: bool) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = fs::canonicalize(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    notices::run_all(&root, check)
}

fn keygen(path: &str) -> Result<(), String> {
    let kp = KeyPair::from_seed(Seed::generate());
    let seed = kp.sk.seed();
    // Never overwrite an existing key.
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{path}: {e}"))?;
    writeln!(f, "{}", to_hex(seed.as_ref())).map_err(|e| e.to_string())?;
    let pk = to_hex(kp.pk.as_ref());
    println!(
        "secret key written to {path} (store it as the RELEASE_SIGNING_KEY secret, then delete the file)"
    );
    println!("public key: {pk}");
    println!(
        "for blinzel-core/src/update.rs: pub const RELEASE_KEY: Option<[u8; 32]> = Some({});",
        rust_bytes(kp.pk.as_ref())
    );
    Ok(())
}

fn rust_bytes(b: &[u8]) -> String {
    let items: Vec<String> = b.iter().map(|x| format!("0x{x:02x}")).collect();
    format!("[{}]", items.join(", "))
}

fn sign(file: &str, out: &str) -> Result<(), String> {
    let hex = std::env::var("RELEASE_SIGNING_KEY").map_err(|_| "RELEASE_SIGNING_KEY is not set")?;
    let msg = fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    let (sig, pk) = signature_file(&msg, &hex, verify_signature)?;
    fs::write(out, sig).map_err(|e| format!("{out}: {e}"))?;
    println!("signed {file} -> {out} (public key {pk})");
    Ok(())
}

/// The `.sig` file for `msg` (the Ed25519 signature with the hex seed
/// `seed_hex`, as hex plus a newline) and the public key as hex. Only a
/// signature that `verifies` accepts is returned, so nothing unverified is
/// published.
fn signature_file(
    msg: &[u8],
    seed_hex: &str,
    verifies: fn(&[u8], &[u8], &[u8; 32]) -> bool,
) -> Result<(String, String), String> {
    let seed = parse_hex::<32>(seed_hex).ok_or("RELEASE_SIGNING_KEY is not a 64-digit hex seed")?;
    let kp = KeyPair::try_from_seed(Seed::new(seed))
        .map_err(|_| "RELEASE_SIGNING_KEY is not a valid seed (all zeros)")?;
    let sig = format!("{}\n", to_hex(kp.sk.sign(msg, None).as_ref()));
    if !verifies(msg, sig.as_bytes(), &kp.pk) {
        return Err("signature does not verify".into());
    }
    Ok((sig, to_hex(kp.pk.as_ref())))
}

fn verify(file: &str, sig: &str, key: &str) -> Result<(), String> {
    let pk = parse_hex::<32>(key).ok_or("public key must be 64 hex digits")?;
    let msg = fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    let sig = fs::read(sig).map_err(|e| format!("{sig}: {e}"))?;
    let sig = String::from_utf8_lossy(&sig).trim().as_bytes().to_vec();
    if verify_signature(&msg, &sig, &pk) {
        println!("valid");
        Ok(())
    } else {
        Err("INVALID signature".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn signature_file_is_hex_and_checked_before_it_is_returned() {
        let (sig, pk) = signature_file(b"sums", SEED, verify_signature).unwrap();
        assert_eq!(sig.len(), 129);
        assert!(sig.ends_with('\n'));
        let pk = parse_hex::<32>(&pk).unwrap();
        assert!(verify_signature(b"sums", sig.as_bytes(), &pk));
        // A check that fails stops the signing.
        assert_eq!(
            signature_file(b"sums", SEED, |_, _, _| false),
            Err("signature does not verify".to_string())
        );
    }
}
