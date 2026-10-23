//! The candidates: where they usually live, how to start them with a file,
//! which version is installed.

use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::core::PCWSTR;

use crate::sys::wide;

#[derive(Clone, Debug)]
pub enum Launch {
    /// An executable started with the image path as its argument.
    Exe(PathBuf),
    /// A URI activation (packaged apps such as Windows Photos); `{file}` is
    /// replaced by the path. `process` is the image name to close.
    Uri { template: String, process: String },
}

#[derive(Clone, Debug)]
pub struct Viewer {
    pub name: String,
    pub launch: Launch,
}

impl Viewer {
    /// The executable's file name, used to close every instance.
    pub fn process_name(&self) -> String {
        match &self.launch {
            Launch::Exe(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Launch::Uri { process, .. } => process.clone(),
        }
    }

    pub fn version(&self) -> String {
        match &self.launch {
            Launch::Exe(p) => file_version(p).unwrap_or_else(|| "?".into()),
            Launch::Uri { .. } => "store app".into(),
        }
    }
}

fn program_files() -> Vec<PathBuf> {
    [
        "ProgramW6432",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
    ]
    .iter()
    .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
    .flat_map(|p| [p.clone(), p.join("Programs")])
    .collect()
}

/// Installed candidates at their default locations, amigo-blinzel first.
pub fn detect() -> Vec<Viewer> {
    let known: &[(&str, &[&str])] = &[
        ("amigo-blinzel", &[r"amigo-blinzel\blinzel.exe"]),
        ("IrfanView 64", &[r"IrfanView\i_view64.exe"]),
        ("ImageGlass", &[r"ImageGlass\ImageGlass.exe"]),
        ("XnView MP", &[r"XnViewMP\xnviewmp.exe"]),
        (
            "FastStone Image Viewer",
            &[r"FastStone Image Viewer\FSViewer.exe"],
        ),
        (
            "JPEGView",
            &[r"JPEGView64\JPEGView.exe", r"JPEGView\JPEGView.exe"],
        ),
        ("qimgv", &[r"qimgv\qimgv.exe"]),
    ];
    let roots = program_files();
    let mut out: Vec<Viewer> = known
        .iter()
        .filter_map(|(name, rels)| {
            let exe = roots
                .iter()
                .flat_map(|r| rels.iter().map(move |rel| r.join(rel)))
                .find(|p| p.is_file())?;
            Some(Viewer {
                name: (*name).into(),
                launch: Launch::Exe(exe),
            })
        })
        .collect();
    out.push(Viewer {
        name: "Windows Photos".into(),
        launch: Launch::Uri {
            template: "ms-photos:viewer?fileName={file}".into(),
            process: "Photos.exe".into(),
        },
    });
    out
}

/// `name=path` from the command line.
pub fn parse_override(s: &str) -> Option<Viewer> {
    let (name, path) = s.split_once('=')?;
    Some(Viewer {
        name: name.trim().into(),
        launch: Launch::Exe(PathBuf::from(path.trim())),
    })
}

/// ProductVersion from the version resource.
fn file_version(path: &Path) -> Option<String> {
    let p = wide(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(p.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(p.as_ptr()), None, size, buf.as_mut_ptr().cast()).ok()?;
        // The first language/codepage pair names the string table.
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        let q = wide(r"\VarFileInfo\Translation");
        if !VerQueryValueW(buf.as_ptr().cast(), PCWSTR(q.as_ptr()), &mut ptr, &mut len).as_bool()
            || len < 4
        {
            return None;
        }
        let pair = std::slice::from_raw_parts(ptr.cast::<u16>(), 2);
        let key = wide(format!(
            r"\StringFileInfo\{:04x}{:04x}\ProductVersion",
            pair[0], pair[1]
        ));
        if !VerQueryValueW(
            buf.as_ptr().cast(),
            PCWSTR(key.as_ptr()),
            &mut ptr,
            &mut len,
        )
        .as_bool()
            || len == 0
        {
            return None;
        }
        let s = std::slice::from_raw_parts(ptr.cast::<u16>(), len as usize);
        Some(crate::sys::from_wide(s).trim().to_string())
    }
}
