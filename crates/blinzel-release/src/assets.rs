//! The files of a release, checked before they are uploaded
//! (docs/specs/release.md, AC-04, AC-05, AC-07): every expected file and
//! nothing else, each executable built for the architecture its name says,
//! and `SHA256SUMS` listing every `amigo-blinzel-*` file with its hash, read
//! with the updater's own parser.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use blinzel_core::update::{
    SUMS, SUMS_SIG, asset_name, linux_asset_name, macos_asset_name, sha256, sum_for,
};

use crate::notices::PLATFORMS;

/// The architectures of the Windows and Linux builds.
const ARCHES: [&str; 2] = ["x64", "arm64"];

/// What the release carries, sorted.
pub fn release_files() -> Vec<String> {
    let mut files: Vec<String> = ARCHES
        .iter()
        .flat_map(|a| {
            [
                asset_name(a),
                linux_asset_name(a),
                format!("{}.tar.gz", linux_asset_name(a)),
            ]
        })
        .collect();
    files.push(macos_asset_name());
    files.push("amigo-blinzel-macos.dmg".into());
    files.push(SUMS.into());
    files.push(SUMS_SIG.into());
    files.extend(PLATFORMS.iter().map(|p| p.file.to_string()));
    files.sort();
    files
}

/// Checks the release folder `dir` (`check-release <dir>`).
pub fn check_dir(dir: &Path) -> Result<(), String> {
    let mut files = BTreeMap::new();
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let data = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        files.insert(name.into_owned(), data);
    }
    check(&files)?;
    println!(
        "{}: the {} release files are complete",
        dir.display(),
        files.len()
    );
    Ok(())
}

/// Every problem of a release's `files` (name and content), one per line.
fn check(files: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let expected = release_files();
    let mut problems = Vec::new();
    for name in &expected {
        if !files.contains_key(name) {
            problems.push(format!("missing {name}"));
        }
    }
    for name in files.keys().filter(|n| !expected.contains(n)) {
        problems.push(format!("unexpected file {name}"));
    }
    for arch in ARCHES {
        for (name, machine) in [
            (asset_name(arch), pe_machine as fn(&[u8]) -> Option<u16>),
            (linux_asset_name(arch), elf_machine),
        ] {
            let want = machine_of(&name, arch);
            if let Some(data) = files.get(&name)
                && machine(data) != Some(want)
            {
                problems.push(format!("{name} is not built for {arch}"));
            }
        }
    }
    let sums = files
        .get(SUMS)
        .map(|s| String::from_utf8_lossy(s).into_owned());
    if let Some(sums) = sums {
        problems.extend(sums_problems(files, &sums));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// What is wrong with `sums`: a line that is not `<sha256>  <name>`, a name
/// listed twice or that is no `amigo-blinzel-*` file, and every such file
/// that is missing or listed with another hash.
fn sums_problems(files: &BTreeMap<String, Vec<u8>>, sums: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let mut listed = Vec::new();
    for line in sums.lines() {
        let name = line.split_once(char::is_whitespace).map(|(_, n)| {
            let n = n.trim_start();
            n.strip_prefix('*').unwrap_or(n)
        });
        match name {
            Some(name) if sum_for(line, name).is_some() => {
                if listed.contains(&name) {
                    problems.push(format!("{SUMS} lists {name} twice"));
                }
                if !(name.starts_with("amigo-blinzel-") && files.contains_key(name)) {
                    problems.push(format!("{SUMS} lists {name}, which is no release file"));
                }
                listed.push(name);
            }
            _ => problems.push(format!("{SUMS}: not a sha256sum line: {line:?}")),
        }
    }
    for (name, data) in files
        .iter()
        .filter(|(n, _)| n.starts_with("amigo-blinzel-"))
    {
        match sum_for(sums, name) {
            None => problems.push(format!("{SUMS} does not list {name}")),
            Some(sum) if sum != sha256(data) => {
                problems.push(format!("{SUMS} has the wrong hash for {name}"))
            }
            Some(_) => {}
        }
    }
    problems
}

/// The machine type a PE or ELF executable for `arch` carries.
fn machine_of(name: &str, arch: &str) -> u16 {
    match (name.ends_with(".exe"), arch) {
        // IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64
        (true, "x64") => 0x8664,
        (true, _) => 0xaa64,
        // EM_X86_64, EM_AARCH64
        (false, "x64") => 62,
        (false, _) => 183,
    }
}

/// The `Machine` field of a PE executable's COFF header.
fn pe_machine(data: &[u8]) -> Option<u16> {
    let u16_at = |i: usize| Some(u16::from_le_bytes(data.get(i..i + 2)?.try_into().ok()?));
    if data.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(data.get(0x3c..0x40)?.try_into().ok()?) as usize;
    (data.get(pe..pe + 4)? == b"PE\0\0").then(|| u16_at(pe + 4))?
}

/// `e_machine` of a little-endian ELF executable.
fn elf_machine(data: &[u8]) -> Option<u16> {
    (data.get(..6)? == b"\x7fELF\x02\x01")
        .then(|| Some(u16::from_le_bytes(data.get(18..20)?.try_into().ok()?)))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use blinzel_core::update::to_hex;

    /// A minimal PE executable with `machine` in its COFF header.
    fn pe(machine: u16) -> Vec<u8> {
        let mut d = vec![0u8; 0x48];
        d[..2].copy_from_slice(b"MZ");
        d[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        d[0x40..0x44].copy_from_slice(b"PE\0\0");
        d[0x44..0x46].copy_from_slice(&machine.to_le_bytes());
        d
    }

    /// A minimal 64-bit little-endian ELF header with `machine`.
    fn elf(machine: u16) -> Vec<u8> {
        let mut d = vec![0u8; 64];
        d[..6].copy_from_slice(b"\x7fELF\x02\x01");
        d[18..20].copy_from_slice(&machine.to_le_bytes());
        d
    }

    /// A complete release, its `SHA256SUMS` as `sha256sum` writes it.
    fn release() -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        for name in release_files() {
            let data = match name.as_str() {
                "amigo-blinzel-setup-x64.exe" => pe(0x8664),
                "amigo-blinzel-setup-arm64.exe" => pe(0xaa64),
                "amigo-blinzel-linux-x64" => elf(62),
                "amigo-blinzel-linux-arm64" => elf(183),
                n => n.as_bytes().to_vec(),
            };
            files.insert(name, data);
        }
        let sums = sums_of(&files);
        files.insert(SUMS.into(), sums.into_bytes());
        files
    }

    fn sums_of(files: &BTreeMap<String, Vec<u8>>) -> String {
        files
            .iter()
            .filter(|(n, _)| n.starts_with("amigo-blinzel-"))
            .map(|(n, d)| format!("{}  {n}\n", to_hex(&sha256(d))))
            .collect()
    }

    fn problems(files: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
        match check(files) {
            Ok(()) => vec![],
            Err(e) => e.lines().map(String::from).collect(),
        }
    }

    /// AC-04: the release carries exactly these files.
    #[test]
    fn release_files_are_the_thirteen_of_the_spec() {
        let mut spec = vec![
            "amigo-blinzel-setup-x64.exe",
            "amigo-blinzel-setup-arm64.exe",
            "amigo-blinzel-linux-x64",
            "amigo-blinzel-linux-arm64",
            "amigo-blinzel-linux-x64.tar.gz",
            "amigo-blinzel-linux-arm64.tar.gz",
            "amigo-blinzel-macos.dmg",
            "amigo-blinzel-macos.zip",
            "SHA256SUMS",
            "SHA256SUMS.sig",
            "THIRD-PARTY-NOTICES.txt",
            "THIRD-PARTY-NOTICES-linux.txt",
            "THIRD-PARTY-NOTICES-macos.txt",
        ];
        spec.sort();
        assert_eq!(release_files(), spec);
    }

    /// AC-04: a missing file and an extra one both stop the release.
    #[test]
    fn a_missing_or_extra_file_fails_the_check() {
        assert_eq!(problems(&release()), Vec::<String>::new());

        let mut files = release();
        files.remove("THIRD-PARTY-NOTICES-macos.txt");
        files.remove("SHA256SUMS.sig");
        assert_eq!(
            problems(&files),
            [
                "missing SHA256SUMS.sig",
                "missing THIRD-PARTY-NOTICES-macos.txt"
            ]
        );

        let mut files = release();
        files.insert("notes.txt".into(), vec![]);
        assert_eq!(problems(&files), ["unexpected file notes.txt"]);
    }

    /// AC-05: each executable is built for the architecture in its name.
    #[test]
    fn an_executable_for_the_wrong_architecture_fails_the_check() {
        let mut files = release();
        files.insert("amigo-blinzel-setup-arm64.exe".into(), pe(0x8664));
        files.insert("amigo-blinzel-linux-x64".into(), elf(183));
        files.insert("amigo-blinzel-linux-arm64".into(), b"#!/bin/sh".to_vec());
        let sums = sums_of(&files);
        files.insert(SUMS.into(), sums.into_bytes());
        assert_eq!(
            problems(&files),
            [
                "amigo-blinzel-linux-x64 is not built for x64",
                "amigo-blinzel-setup-arm64.exe is not built for arm64",
                "amigo-blinzel-linux-arm64 is not built for arm64",
            ]
        );
    }

    /// AC-07: `SHA256SUMS` lists every `amigo-blinzel-*` file with its
    /// hash, in `sha256sum` format (text or binary mode), and nothing else.
    #[test]
    fn sha256sums_must_list_every_release_file_with_its_hash() {
        // Git Bash's sha256sum marks binary mode with `*`.
        let mut files = release();
        let binary = sums_of(&files).replace("  ", " *");
        files.insert(SUMS.into(), binary.into_bytes());
        assert_eq!(problems(&files), Vec::<String>::new());

        let mut files = release();
        let sums = sums_of(&files);
        let without_dmg: String = sums
            .lines()
            .filter(|l| !l.ends_with("macos.dmg"))
            .map(|l| format!("{l}\n"))
            .collect();
        files.insert(SUMS.into(), without_dmg.into_bytes());
        assert_eq!(
            problems(&files),
            ["SHA256SUMS does not list amigo-blinzel-macos.dmg"]
        );

        let mut files = release();
        files.insert("amigo-blinzel-macos.zip".into(), b"changed".to_vec());
        assert_eq!(
            problems(&files),
            ["SHA256SUMS has the wrong hash for amigo-blinzel-macos.zip"]
        );

        let mut files = release();
        let first = sums.lines().next().unwrap().to_string();
        let extra = format!(
            "{sums}{first}\n{}  LICENSE\nnot a line\n",
            to_hex(&sha256(b""))
        );
        files.insert(SUMS.into(), extra.into_bytes());
        assert_eq!(
            problems(&files),
            [
                format!("SHA256SUMS lists {} twice", &first[66..]),
                "SHA256SUMS lists LICENSE, which is no release file".into(),
                "SHA256SUMS: not a sha256sum line: \"not a line\"".to_string(),
            ]
        );
    }
}
