//! `blinzel-release keygen | sign | verify` as the release workflow and an
//! operator run them (docs/specs/release.md, AC-09 to AC-12).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use blinzel_core::update::{parse_hex, to_hex, verify_signature};
use ed25519_compact::{KeyPair, Seed};

/// A made-up seed for the tests (not a key that signs anything real).
const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

/// A fresh, empty folder for one test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("blinzel-release-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&Path], key: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blinzel-release"));
    cmd.args(args).env_remove("RELEASE_SIGNING_KEY");
    if let Some(key) = key {
        cmd.env("RELEASE_SIGNING_KEY", key);
    }
    cmd.output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn public_key(seed_hex: &str) -> String {
    let seed = parse_hex::<32>(seed_hex).unwrap();
    to_hex(KeyPair::from_seed(Seed::new(seed)).pk.as_ref())
}

/// A file to sign, the path of its signature, and the folder.
fn sums(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = temp_dir(name);
    let file = dir.join("SHA256SUMS");
    std::fs::write(&file, "00  amigo-blinzel-linux-x64\n").unwrap();
    (file, dir.join("SHA256SUMS.sig"), dir)
}

#[test]
fn sign_writes_a_verified_hex_signature_and_prints_the_public_key() {
    let (file, sig, dir) = sums("sign");
    let o = run(&[Path::new("sign"), &file, &sig], Some(SEED));
    assert!(o.status.success(), "{}", stderr(&o));
    let text = std::fs::read_to_string(&sig).unwrap();
    let hex = text.strip_suffix('\n').expect("ends with a newline");
    assert_eq!(hex.len(), 128, "{text:?}");
    assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()), "{text:?}");
    let pk = parse_hex::<32>(&public_key(SEED)).unwrap();
    let msg = std::fs::read(&file).unwrap();
    assert!(verify_signature(&msg, text.as_bytes(), &pk));
    assert!(stdout(&o).contains(&public_key(SEED)), "{}", stdout(&o));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sign_without_a_valid_seed_fails_and_writes_nothing() {
    let (file, sig, dir) = sums("sign-bad");
    let short = &SEED[..63];
    let long = format!("{SEED}00");
    // `u8::from_str_radix` takes a sign; Ed25519 rejects an all-zero seed.
    let signed = "+1".repeat(32);
    let zero = "0".repeat(64);
    let not_hex = "g".repeat(64);
    for key in [
        None,
        Some(""),
        Some(short),
        Some(&long),
        Some(&signed),
        Some(&zero),
        Some(&not_hex),
    ] {
        let o = run(&[Path::new("sign"), &file, &sig], key);
        assert_eq!(o.status.code(), Some(1), "{key:?}");
        assert!(
            stderr(&o).contains("RELEASE_SIGNING_KEY"),
            "{key:?}: {}",
            stderr(&o)
        );
        assert!(!sig.exists(), "{key:?} wrote a signature");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn verify_accepts_the_matching_signature_only() {
    let (file, sig, dir) = sums("verify");
    assert!(
        run(&[Path::new("sign"), &file, &sig], Some(SEED))
            .status
            .success()
    );
    let pk = public_key(SEED);
    let o = run(&[Path::new("verify"), &file, &sig, Path::new(&pk)], None);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stdout(&o).trim(), "valid");

    // Another file, and another key.
    let other = dir.join("other");
    std::fs::write(&other, "00  amigo-blinzel-linux-arm64\n").unwrap();
    let other_pk = public_key(&SEED.replace("00", "ff"));
    for (f, key) in [(&other, &pk), (&file, &other_pk)] {
        let o = run(&[Path::new("verify"), f, &sig, Path::new(key)], None);
        assert_eq!(o.status.code(), Some(1));
        assert!(stderr(&o).contains("INVALID signature"), "{}", stderr(&o));
        assert!(!stdout(&o).contains("valid"), "{}", stdout(&o));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn keygen_writes_a_seed_prints_its_key_and_never_overwrites() {
    let dir = temp_dir("keygen");
    let key = dir.join("release.key");
    let o = run(&[Path::new("keygen"), &key], None);
    assert!(o.status.success(), "{}", stderr(&o));
    let seed = std::fs::read_to_string(&key).unwrap();
    let hex = seed.strip_suffix('\n').expect("ends with a newline");
    assert_eq!(hex.len(), 64);
    let pk = public_key(hex);
    let out = stdout(&o);
    assert!(out.contains(&format!("public key: {pk}")), "{out}");
    let bytes: Vec<String> = parse_hex::<32>(&pk)
        .unwrap()
        .iter()
        .map(|b| format!("0x{b:02x}"))
        .collect();
    let line = format!(
        "pub const RELEASE_KEY: Option<[u8; 32]> = Some([{}]);",
        bytes.join(", ")
    );
    assert!(out.contains(&line), "{out}");

    // A second run against the same file keeps the first key.
    let o = run(&[Path::new("keygen"), &key], None);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(std::fs::read_to_string(&key).unwrap(), seed);

    // Two runs make two different keys.
    let second = dir.join("second.key");
    assert!(run(&[Path::new("keygen"), &second], None).status.success());
    assert_ne!(std::fs::read_to_string(&second).unwrap(), seed);
    std::fs::remove_dir_all(dir).unwrap();
}
