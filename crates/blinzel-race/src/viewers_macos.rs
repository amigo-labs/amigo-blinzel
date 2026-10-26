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
    let known: &[(&str, &[&str])] = &[
        ("amigo-blinzel", &["amigo-blinzel.app"]),
        ("Preview", &["Preview.app"]),
        ("XnView MP", &["XnViewMP.app", "XnView MP.app"]),
        ("qView", &["qView.app"]),
        ("ApolloOne", &["ApolloOne.app"]),
        ("Phoenix Slides", &["Phoenix Slides.app"]),
        ("Pixea", &["Pixea.app"]),
    ];
    let roots = roots();
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

/// `name=/path/App.app` from the command line.
pub fn parse_override(s: &str) -> Option<Viewer> {
    let (name, path) = s.split_once('=')?;
    Some(Viewer {
        name: name.trim().into(),
        launch: PathBuf::from(path.trim()),
    })
}
