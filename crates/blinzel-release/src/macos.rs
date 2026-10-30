//! `blinzel-release macos-bundle <out.app> <blinzel>...`: the macOS app
//! bundle around one executable, or several (one per architecture) joined
//! with `lipo` into a universal binary: `Info.plist` (document types for
//! Finder's "Open With"), the icon, and a signature: with the identity in
//! `$BLINZEL_MACOS_SIGN_IDENTITY` (our own self-signed certificate, so
//! macOS keeps the screen recording permission across updates), otherwise
//! ad hoc (`codesign -s -`). Releases are not notarized (docs/PLAN.md
//! section 10). `lipo` and `codesign` run on macOS only; elsewhere the
//! bundle is written without them (tests).
//!
//! `blinzel-release macos-dmg <app> <out.dmg>`: the disk image people
//! download, the bundle next to a link to /Applications (`hdiutil`, macOS
//! only).

use std::fs;
use std::path::Path;
use std::process::Command;

#[path = "../../blinzel-app/build_icon.rs"]
#[allow(dead_code)]
mod icon;

pub const BUNDLE_ID: &str = "io.github.amigo-labs.blinzel";
const EXECUTABLE: &str = "blinzel";
const ICON: &str = "amigo-blinzel";
/// The oldest macOS the viewer supports.
const MIN_MACOS: &str = "14.0";
/// The signing identity (certificate name or SHA-1 hash); ad hoc without.
pub const SIGN_IDENTITY_ENV: &str = "BLINZEL_MACOS_SIGN_IDENTITY";

/// Uniform type identifiers the viewer opens: everything natively decoded
/// plus what ImageIO adds.
const TYPES: &[&str] = &[
    "public.jpeg",
    "public.png",
    "com.compuserve.gif",
    "com.microsoft.bmp",
    "public.tiff",
    "org.webmproject.webp",
    "public.heic",
    "public.heif",
    "public.avif",
    "public.jpeg-xl",
    "public.camera-raw-image",
    "com.adobe.photoshop-image",
    "com.microsoft.ico",
];

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The bundle's `Info.plist`.
pub fn info_plist(version: &str) -> String {
    let types: String = TYPES
        .iter()
        .map(|t| format!("\t\t\t\t<string>{}</string>\n", escape(t)))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>amigo-blinzel</string>
	<key>CFBundleExecutable</key>
	<string>{EXECUTABLE}</string>
	<key>CFBundleIconFile</key>
	<string>{ICON}</string>
	<key>CFBundleIdentifier</key>
	<string>{BUNDLE_ID}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>amigo-blinzel</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.photography</string>
	<key>LSMinimumSystemVersion</key>
	<string>{MIN_MACOS}</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key>
	<true/>
	<key>NSScreenCaptureUsageDescription</key>
	<string>amigo-blinzel captures the part of the screen you pick for a screenshot.</string>
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleTypeName</key>
			<string>Image</string>
			<key>CFBundleTypeRole</key>
			<string>Viewer</string>
			<key>LSHandlerRank</key>
			<string>Alternate</string>
			<key>LSItemContentTypes</key>
			<array>
{types}			</array>
		</dict>
	</array>
</dict>
</plist>
"#,
        version = escape(version),
    )
}

/// Writes `out` (an `.app` folder) around `executables`.
pub fn bundle(out: &Path, executables: &[&Path], version: &str) -> Result<(), String> {
    let io = |p: &Path| {
        let p = p.display().to_string();
        move |e: std::io::Error| format!("{p}: {e}")
    };
    if executables.is_empty() {
        return Err("no executable".into());
    }
    if out.exists() {
        fs::remove_dir_all(out).map_err(io(out))?;
    }
    let contents = out.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos).map_err(io(&macos))?;
    fs::create_dir_all(&resources).map_err(io(&resources))?;
    let exe = macos.join(EXECUTABLE);
    if executables.len() > 1 && cfg!(target_os = "macos") {
        let ok = Command::new("lipo")
            .arg("-create")
            .arg("-output")
            .arg(&exe)
            .args(executables)
            .status()
            .map_err(|e| format!("lipo: {e}"))?
            .success();
        if !ok {
            return Err("lipo failed".into());
        }
    } else {
        fs::copy(executables[0], &exe).map_err(io(&exe))?;
    }
    let plist = contents.join("Info.plist");
    fs::write(&plist, info_plist(version)).map_err(io(&plist))?;
    let pkginfo = contents.join("PkgInfo");
    fs::write(&pkginfo, "APPL????").map_err(io(&pkginfo))?;
    let icns = resources.join(format!("{ICON}.icns"));
    fs::write(&icns, icon::icns()).map_err(io(&icns))?;
    if cfg!(target_os = "macos") {
        // Apple silicon runs only signed code; ad hoc needs no identity.
        let identity = sign_identity(std::env::var(SIGN_IDENTITY_ENV).ok());
        let ok = Command::new("codesign")
            .args(["--force", "--deep", "--sign", &identity])
            .arg(out)
            .status()
            .map_err(|e| format!("codesign: {e}"))?
            .success();
        if !ok {
            return Err("codesign failed".into());
        }
    }
    Ok(())
}

/// The identity for `codesign --sign`: the configured one, or `-` (ad hoc).
pub fn sign_identity(configured: Option<String>) -> String {
    configured
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "-".into())
}

/// Writes `out`, a compressed disk image holding `app` and a link to
/// /Applications to drag it onto.
#[cfg(target_os = "macos")]
pub fn dmg(app: &Path, out: &Path) -> Result<(), String> {
    let name = app.file_name().ok_or("no app bundle name")?;
    let staging = out.with_file_name(".dmg-staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let result = (|| {
        // ditto keeps the signature's extended attributes.
        let ok = Command::new("ditto")
            .arg(app)
            .arg(staging.join(name))
            .status()
            .map_err(|e| format!("ditto: {e}"))?
            .success();
        if !ok {
            return Err("ditto failed".to_string());
        }
        std::os::unix::fs::symlink("/Applications", staging.join("Applications"))
            .map_err(|e| format!("Applications link: {e}"))?;
        let ok = Command::new("hdiutil")
            .args(["create", "-volname", "amigo-blinzel", "-fs", "HFS+"])
            .args(["-format", "UDZO", "-ov", "-srcfolder"])
            .arg(&staging)
            .arg(out)
            .status()
            .map_err(|e| format!("hdiutil: {e}"))?
            .success();
        if ok {
            Ok(())
        } else {
            Err("hdiutil failed".to_string())
        }
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

#[cfg(not(target_os = "macos"))]
pub fn dmg(_app: &Path, _out: &Path) -> Result<(), String> {
    Err("macos-dmg needs hdiutil (macOS)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_bundle() {
        let dir = std::env::temp_dir().join(format!("blinzel-bundle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("blinzel");
        fs::write(&exe, b"\xcf\xfa\xed\xfe").unwrap();
        let app = dir.join("amigo-blinzel.app");
        bundle(&app, &[&exe], "1.2.3").unwrap();
        let plist = fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>1.2.3</string>"));
        assert!(plist.contains("<string>public.heic</string>"));
        assert!(plist.contains(BUNDLE_ID));
        assert!(plist.contains("<key>NSScreenCaptureUsageDescription</key>"));
        let icns = fs::read(app.join("Contents/Resources/amigo-blinzel.icns")).unwrap();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize,
            icns.len()
        );
        assert!(app.join("Contents/MacOS/blinzel").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn signs_ad_hoc_without_an_identity() {
        assert_eq!(sign_identity(None), "-");
        assert_eq!(sign_identity(Some(String::new())), "-");
        assert_eq!(sign_identity(Some("  ".into())), "-");
        assert_eq!(sign_identity(Some(" 0123ABCD\n".into())), "0123ABCD");
    }

    #[test]
    fn icns_entries_are_pngs() {
        let icns = icon::icns();
        let mut at = 8;
        let mut n = 0;
        while at < icns.len() {
            let len = u32::from_be_bytes(icns[at + 4..at + 8].try_into().unwrap()) as usize;
            assert_eq!(&icns[at + 8..at + 12], b"\x89PNG");
            at += len;
            n += 1;
        }
        assert_eq!(at, icns.len());
        assert_eq!(n, 10);
    }
}
