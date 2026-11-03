//! The application manifest embedded in `blinzel.exe`
//! (`docs/specs/platform-shells.md` AC-02). The linker stores it as an
//! `RT_MANIFEST` resource, the XML as it is in `res/blinzel.manifest`.

#![cfg(windows)]

#[test]
fn the_executable_embeds_its_application_manifest() {
    let exe = std::fs::read(env!("CARGO_BIN_EXE_blinzel")).unwrap();
    let has = |s: &str| exe.windows(s.len()).any(|w| w == s.as_bytes());
    for declared in [
        // supportedOS: Windows 10 / 11.
        "8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a",
        ">PerMonitorV2<",
        "longPathAware",
        ">UTF-8<",
        "asInvoker",
    ] {
        assert!(has(declared), "{declared} is not in the manifest");
    }
}
