//! The macOS candidates: app bundles in the usual places, started through
//! Launch Services (`open -a`), the way Finder opens a file with them.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct Viewer {
    pub name: String,
    /// The app bundle.
    pub launch: PathBuf,
}

impl Viewer {
    /// A viewer given on the command line (`--viewer name=/path/App.app`).
    pub fn custom(name: String, app: PathBuf) -> Viewer {
        Viewer { name, launch: app }
    }

    /// The bundle path, which identifies the viewer's processes.
    pub fn process_name(&self) -> String {
        self.launch.display().to_string()
    }

    pub fn version(&self) -> String {
        Command::new("/usr/bin/plutil")
            .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
            .arg(self.launch.join("Contents/Info.plist"))
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "?".into())
    }
}

fn roots() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        out.push(Path::new(&home).join("Applications"));
    }
    out.push("/System/Applications".into());
    out
}

/// Installed candidates at their default locations, amigo-blinzel first.
pub fn detect() -> Vec<Viewer> {
    detect_in(&roots())
}

/// The known app bundles found under `roots` (the first root holding one
/// wins).
fn detect_in(roots: &[PathBuf]) -> Vec<Viewer> {
    let known: &[(&str, &[&str])] = &[
        ("amigo-blinzel", &["amigo-blinzel.app"]),
        ("Preview", &["Preview.app"]),
        ("XnView MP", &["XnViewMP.app", "XnView MP.app"]),
        ("qView", &["qView.app"]),
        ("ApolloOne", &["ApolloOne.app"]),
        ("Phoenix Slides", &["Phoenix Slides.app"]),
        ("Pixea", &["Pixea.app"]),
    ];
    known
        .iter()
        .filter_map(|(name, rels)| {
            let app = roots
                .iter()
                .flat_map(|r| rels.iter().map(move |rel| r.join(rel)))
                .find(|p| p.join("Contents/Info.plist").is_file())?;
            Some(Viewer {
                name: (*name).into(),
                launch: app,
            })
        })
        .collect()
}

/// Opens `file` with the viewer.
pub fn launch(v: &Viewer, file: &Path) -> Result<(), String> {
    let ok = Command::new("/usr/bin/open")
        .arg("-a")
        .arg(&v.launch)
        .arg(file)
        .status()
        .map_err(|e| format!("open: {e}"))?
        .success();
    if ok {
        Ok(())
    } else {
        Err(format!("open -a {} failed", v.launch.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(app: &Path, version: Option<&str>) {
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        let key = version.map_or(String::new(), |v| {
            format!("<key>CFBundleShortVersionString</key><string>{v}</string>")
        });
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <plist version=\"1.0\"><dict>{key}</dict></plist>\n"
        );
        std::fs::write(app.join("Contents/Info.plist"), plist).unwrap();
    }

    #[test]
    fn detects_app_bundles_with_their_versions() {
        let tmp = std::env::temp_dir().join(format!("blinzel-race-viewers-{}", std::process::id()));
        let (a, b) = (tmp.join("a"), tmp.join("b"));
        bundle(&b.join("Pixea.app"), Some("6.1"));
        bundle(&a.join("XnView MP.app"), Some("1.8.2"));
        bundle(&b.join("amigo-blinzel.app"), Some("1.3.0"));
        bundle(&a.join("qView.app"), None);
        // Not a bundle: no Info.plist.
        std::fs::create_dir_all(a.join("Preview.app")).unwrap();
        let found = detect_in(&[a.clone(), b.clone()]);
        let names: Vec<&str> = found.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["amigo-blinzel", "XnView MP", "qView", "Pixea"]);
        let versions: Vec<String> = found.iter().map(Viewer::version).collect();
        assert_eq!(versions, ["1.3.0", "1.8.2", "?", "6.1"]);
        assert_eq!(found[0].launch, b.join("amigo-blinzel.app"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// What `blinzel-race list` shows on every Mac: Preview from
    /// `/System/Applications`, with its version.
    #[test]
    fn detects_preview_on_this_mac() {
        let preview = detect().into_iter().find(|v| v.name == "Preview");
        let preview = preview.expect("Preview.app not detected");
        let version = preview.version();
        assert!(
            version.starts_with(|c: char| c.is_ascii_digit()),
            "{version}"
        );
    }
}
