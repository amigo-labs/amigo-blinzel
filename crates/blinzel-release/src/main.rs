//! Release tooling for the self-updater's own signature (M7).
//!
//! ```text
//! blinzel-release keygen <secret-key-file>   new key pair; prints the public key
//! blinzel-release sign <file> <out.sig>      signs with $RELEASE_SIGNING_KEY (hex seed)
//! blinzel-release verify <file> <sig> <public-key-hex>
//! ```
//!
//! The secret key is the 32-byte Ed25519 seed as hex. It belongs in the
//! repository secret `RELEASE_SIGNING_KEY` and nowhere in the repository;
//! the public key goes into `blinzel-app/src/update.rs` (`RELEASE_KEY`).

use std::fs;
use std::io::Write;

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
        _ => Err(usage()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn usage() -> String {
    "usage:\n  blinzel-release keygen <secret-key-file>\n  blinzel-release sign <file> <out.sig>   \
     (secret from $RELEASE_SIGNING_KEY)\n  blinzel-release verify <file> <sig> <public-key-hex>"
        .into()
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
        "for update.rs: pub const RELEASE_KEY: Option<[u8; 32]> = Some({});",
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
    let seed = parse_hex::<32>(&hex).ok_or("RELEASE_SIGNING_KEY is not a 64-digit hex seed")?;
    let kp = KeyPair::from_seed(Seed::new(seed));
    let msg = fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    let sig = kp.sk.sign(&msg, None);
    // Check before publishing anything.
    if !verify_signature(&msg, sig.as_ref(), &kp.pk) {
        return Err("signature does not verify".into());
    }
    fs::write(out, format!("{}\n", to_hex(sig.as_ref()))).map_err(|e| format!("{out}: {e}"))?;
    println!(
        "signed {file} -> {out} (public key {})",
        to_hex(kp.pk.as_ref())
    );
    Ok(())
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
