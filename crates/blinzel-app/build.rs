//! Compiles the HLSL shaders to DXBC with fxc from the Windows SDK, so no
//! shader is ever compiled at runtime (d3dcompiler would cost startup time).
//!
//! Off Windows (cross type-checks from Linux) fxc is unavailable; empty
//! placeholders are written and the renderer reports missing shaders if such
//! a build were ever run.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const SHADERS: &[(&str, &str, &str)] = &[
    ("vs_main", "vs_5_0", "vs.cso"),
    ("ps_image", "ps_5_0", "ps_image.cso"),
    ("ps_overlay", "ps_5_0", "ps_overlay.cso"),
];

fn main() {
    let src = PathBuf::from("src/gfx/shaders/blinzel.hlsl");
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-env-changed=FXC");
    println!("cargo:rerun-if-env-changed=RC");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let target_windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");

    let fxc = if cfg!(windows) {
        find_sdk_tool("fxc.exe", "FXC")
    } else {
        None
    };
    match (target_windows, fxc) {
        (true, Some(fxc)) => {
            for (entry, profile, file) in SHADERS {
                let dst = out.join(file);
                let status = Command::new(&fxc)
                    .args([
                        "/nologo", "/O3", "/WX", "/Ges", "/T", profile, "/E", entry, "/Fo",
                    ])
                    .arg(&dst)
                    .arg(&src)
                    .status()
                    .unwrap_or_else(|e| panic!("cannot run {}: {e}", fxc.display()));
                assert!(status.success(), "fxc failed for {entry}");
            }
        }
        (true, None) if cfg!(windows) => {
            panic!("fxc.exe not found: install the Windows SDK or set FXC to its path");
        }
        _ => {
            for (_, _, file) in SHADERS {
                std::fs::write(out.join(file), []).unwrap();
            }
            println!("cargo:rustc-cfg=blinzel_no_shaders");
        }
    }
    println!("cargo:rustc-check-cfg=cfg(blinzel_no_shaders)");

    // Application manifest (supported OS, PerMonitorV2, long paths, UTF-8).
    // Embedded by MSVC's link.exe; lld-link without libxml2 (Linux cross
    // checks) cannot merge manifests, so it is skipped there.
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("res/blinzel.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    if target_windows && cfg!(windows) && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
        println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");
        version_resource(&out);
    }
}

/// VERSIONINFO from the crate version: file properties, Authenticode
/// metadata and benchmark tools read it.
fn version_resource(out: &Path) {
    let Some(rc) = find_sdk_tool("rc.exe", "RC") else {
        println!("cargo:warning=rc.exe not found: no version resource");
        return;
    };
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let nums: Vec<&str> = version.split('.').collect();
    let comma = format!("{},{},{},0", nums[0], nums[1], nums[2]);
    let script = format!(
        r#"1 VERSIONINFO
FILEVERSION {comma}
PRODUCTVERSION {comma}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "amigo-labs"
      VALUE "FileDescription", "amigo-blinzel"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "blinzel"
      VALUE "OriginalFilename", "blinzel.exe"
      VALUE "ProductName", "amigo-blinzel"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let src = out.join("version.rc");
    let res = out.join("version.res");
    std::fs::write(&src, script).unwrap();
    let status = Command::new(&rc)
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg(&src)
        .status()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", rc.display()));
    assert!(status.success(), "rc.exe failed");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// The `var` environment variable, else the newest
/// `Windows Kits\10\bin\<ver>\x64\<name>`.
fn find_sdk_tool(name: &str, var: &str) -> Option<PathBuf> {
    if let Some(p) = env::var_os(var) {
        return Some(PathBuf::from(p));
    }
    let roots = [
        env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        env::var_os("ProgramFiles").map(PathBuf::from),
    ];
    let arch = if env::var("HOST")
        .map(|h| h.starts_with("aarch64"))
        .unwrap_or(false)
    {
        "arm64"
    } else {
        "x64"
    };
    for root in roots.into_iter().flatten() {
        let bin = root.join("Windows Kits").join("10").join("bin");
        let mut versions: Vec<PathBuf> = std::fs::read_dir(&bin)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join(arch).join(name).is_file())
            .collect();
        versions.sort_by_key(|p| version_key(p));
        if let Some(v) = versions.pop() {
            return Some(v.join(arch).join(name));
        }
        let flat = bin.join(arch).join(name);
        if flat.is_file() {
            return Some(flat);
        }
    }
    None
}

fn version_key(p: &Path) -> Vec<u32> {
    p.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.split('.').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_default()
}
