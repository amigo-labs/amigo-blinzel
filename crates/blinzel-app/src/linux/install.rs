//! Per-user installation on Linux, no root needed: `blinzel --install`
//! copies the executable to `~/.local/share/amigo-blinzel/`, links it as
//! `~/.local/bin/blinzel` and adds a desktop entry with the image types
//! (app menu, "Open with") and an icon. `blinzel --uninstall` removes all
//! of it; settings in `~/.config/amigo-blinzel` stay.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use blinzel_i18n::{Msg, tr, trf};

pub const PRODUCT: &str = "amigo-blinzel";
pub const EXE_NAME: &str = "blinzel";
/// The desktop entry's id; also the window's app id / WM_CLASS, so the
/// desktop matches windows to the entry (icon, name).
pub const APP_ID: &str = "amigo-blinzel";

/// Image types the native decoders open.
pub const MIME_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/bmp",
    "image/webp",
    "image/avif",
    "image/jxl",
];

const ICON: &[u8] = include_bytes!("../../../../packaging/linux/amigo-blinzel.svg");

/// Where an installation lives. [`Layout::user`] is the real one; tests use
/// [`Layout::under`].
#[derive(Clone, Debug)]
pub struct Layout {
    pub dir: PathBuf,
    /// On the PATH of most desktops: holds the `blinzel` link.
    pub bin: PathBuf,
    pub applications: PathBuf,
    pub icons: PathBuf,
    /// Cache that belongs to the installation (downloaded updates,
    /// thumbnails).
    pub cache: PathBuf,
    /// Tell the desktop about the new entry (off in tests).
    pub notify_desktop: bool,
}

impl Layout {
    pub fn user() -> Option<Layout> {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
        let home = PathBuf::from(home);
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map_or_else(|| home.join(".local/share"), PathBuf::from);
        Some(Layout {
            dir: data.join(PRODUCT),
            bin: home.join(".local/bin"),
            applications: data.join("applications"),
            icons: data.join("icons/hicolor/scalable/apps"),
            cache: blinzel_ui::dirs::cache()?,
            notify_desktop: true,
        })
    }

    pub fn under(root: &Path) -> Layout {
        Layout {
            dir: root.join("share").join(PRODUCT),
            bin: root.join("bin"),
            applications: root.join("share/applications"),
            icons: root.join("share/icons"),
            cache: root.join("cache"),
            notify_desktop: false,
        }
    }

    pub fn exe(&self) -> PathBuf {
        self.dir.join(EXE_NAME)
    }

    pub fn link(&self) -> PathBuf {
        self.bin.join(EXE_NAME)
    }

    pub fn desktop_file(&self) -> PathBuf {
        self.applications.join(format!("{APP_ID}.desktop"))
    }

    pub fn icon(&self) -> PathBuf {
        self.icons.join(format!("{APP_ID}.svg"))
    }

    /// Whether `exe` is (or would be) the installed executable.
    pub fn is_installed_exe(&self, exe: &Path) -> bool {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(exe) == canon(&self.exe())
    }
}

/// Installs (or updates in place) `source`, the running executable.
pub fn install(layout: &Layout, source: &Path) -> Result<(), String> {
    let io = |what: &str, p: &Path| {
        let what = what.to_string();
        let p = p.display().to_string();
        move |e: std::io::Error| format!("{what} {p}: {e}")
    };
    std::fs::create_dir_all(&layout.dir).map_err(io("create", &layout.dir))?;
    let exe = layout.exe();
    if !layout.is_installed_exe(source) {
        // Copy next to the target, then rename over it: a running
        // installed copy keeps its (now unlinked) file.
        let tmp = exe.with_extension("new");
        std::fs::copy(source, &tmp).map_err(io("copy to", &tmp))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(io("chmod", &tmp))?;
        std::fs::rename(&tmp, &exe).map_err(io("rename", &exe))?;
    }
    link(layout)?;
    std::fs::create_dir_all(&layout.icons).map_err(io("create", &layout.icons))?;
    write_atomic(&layout.icon(), ICON).map_err(io("write", &layout.icon()))?;
    std::fs::create_dir_all(&layout.applications).map_err(io("create", &layout.applications))?;
    let entry = desktop_entry(&exe, tr(Msg::AppDescription));
    write_atomic(&layout.desktop_file(), entry.as_bytes())
        .map_err(io("write", &layout.desktop_file()))?;
    if layout.notify_desktop {
        notify_desktop(layout);
    }
    Ok(())
}

/// `~/.local/bin/blinzel` → the installed executable. Leaves a file there
/// that is not our link alone.
fn link(layout: &Layout) -> Result<(), String> {
    let link = layout.link();
    match std::fs::symlink_metadata(&link) {
        Ok(m) if !m.file_type().is_symlink() => return Ok(()),
        Ok(_) => {
            let _ = std::fs::remove_file(&link);
        }
        Err(_) => {}
    }
    std::fs::create_dir_all(&layout.bin).map_err(|e| format!("{}: {e}", layout.bin.display()))?;
    std::os::unix::fs::symlink(layout.exe(), &link)
        .map_err(|e| format!("link {}: {e}", link.display()))
}

/// Removes everything [`install`] created (also from the installed
/// executable itself: a running program's file can be deleted on Linux).
pub fn uninstall(layout: &Layout) -> Result<(), String> {
    let mut errors = Vec::new();
    let link = layout.link();
    if std::fs::read_link(&link).is_ok_and(|t| t == layout.exe()) {
        let _ = std::fs::remove_file(&link);
    }
    for f in [layout.desktop_file(), layout.icon()] {
        if let Err(e) = std::fs::remove_file(&f)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            errors.push(format!("{}: {e}", f.display()));
        }
    }
    for dir in [&layout.dir, &layout.cache] {
        if dir.exists()
            && let Err(e) = std::fs::remove_dir_all(dir)
        {
            errors.push(format!("{}: {e}", dir.display()));
        }
    }
    if layout.notify_desktop {
        notify_desktop(layout);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// The desktop entry (freedesktop Desktop Entry Specification 1.5).
pub fn desktop_entry(exe: &Path, comment: &str) -> String {
    let mut mime = MIME_TYPES.join(";");
    mime.push(';');
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={PRODUCT}\n\
         Comment={comment}\n\
         Exec={} %f\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Graphics;Viewer;2DGraphics;RasterGraphics;\n\
         MimeType={mime}\n\
         StartupWMClass={APP_ID}\n",
        exec_quote(&exe.to_string_lossy())
    )
}

/// Quotes an `Exec` argument: inside double quotes `"`, `` ` ``, `$` and
/// `\` are escaped, and the escape is written escaped again because the
/// value is a string (spec, "The Exec key").
fn exec_quote(arg: &str) -> String {
    let plain = arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c));
    if plain {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '\\' => out.push_str("\\\\\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The command that makes the installed entry the default for images.
pub fn default_command() -> String {
    format!("xdg-mime default {APP_ID}.desktop {}", MIME_TYPES.join(" "))
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// Refreshes the desktop's MIME cache for "Open with"; desktops without
/// the tool rescan the folder themselves.
fn notify_desktop(layout: &Layout) {
    let _ = std::process::Command::new("update-desktop-database")
        .arg("-q")
        .arg(&layout.applications)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// `--install` / `--uninstall` from the command line; returns the exit
/// code.
pub fn run(uninstall_mode: bool) -> i32 {
    let Some(layout) = Layout::user() else {
        eprintln!("amigo-blinzel: HOME is not set");
        return 1;
    };
    if uninstall_mode {
        return match uninstall(&layout) {
            Ok(()) => {
                let settings = blinzel_ui::dirs::config().unwrap_or_default();
                println!(
                    "{}",
                    trf(Msg::UninstallDone, &[("dir", &settings.display())])
                );
                0
            }
            Err(e) => {
                eprintln!("{}", trf(Msg::UninstallFailed, &[("reason", &e)]));
                1
            }
        };
    }
    let source = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}: {e}", tr(Msg::SetupFailedTitle));
            return 1;
        }
    };
    match install(&layout, &source) {
        Ok(()) => {
            println!(
                "{}",
                trf(
                    Msg::SetupLinuxDone,
                    &[
                        ("version", &env!("CARGO_PKG_VERSION")),
                        ("exe", &layout.link().display()),
                        ("command", &default_command()),
                    ],
                )
            );
            0
        }
        Err(e) => {
            eprintln!("{}: {e}", tr(Msg::SetupFailedTitle));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blinzel-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn installs_and_uninstalls() {
        let root = temp("roundtrip");
        let layout = Layout::under(&root);
        let source = root.join("download/blinzel");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"#!/bin/sh\n").unwrap();
        std::fs::create_dir_all(&layout.cache).unwrap();

        install(&layout, &source).unwrap();
        assert_eq!(std::fs::read(layout.exe()).unwrap(), b"#!/bin/sh\n");
        let mode = std::fs::metadata(layout.exe())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(std::fs::read_link(layout.link()).unwrap(), layout.exe());
        let entry = std::fs::read_to_string(layout.desktop_file()).unwrap();
        assert!(entry.contains(&format!("Exec={} %f", layout.exe().display())));
        assert!(entry.contains("MimeType=image/jpeg;"));
        assert!(layout.icon().exists());
        // Again (an update in place): still fine.
        install(&layout, &source).unwrap();
        // From the installed copy itself: nothing to copy.
        install(&layout, &layout.exe()).unwrap();

        uninstall(&layout).unwrap();
        for p in [
            layout.exe(),
            layout.link(),
            layout.desktop_file(),
            layout.icon(),
            layout.cache.clone(),
        ] {
            assert!(std::fs::symlink_metadata(&p).is_err(), "{}", p.display());
        }
        assert!(source.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn leaves_foreign_files_alone() {
        let root = temp("foreign");
        let layout = Layout::under(&root);
        std::fs::create_dir_all(&layout.bin).unwrap();
        std::fs::write(layout.link(), b"someone else's").unwrap();
        let source = root.join("blinzel");
        std::fs::write(&source, b"x").unwrap();
        install(&layout, &source).unwrap();
        uninstall(&layout).unwrap();
        assert_eq!(std::fs::read(layout.link()).unwrap(), b"someone else's");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn quotes_exec_paths() {
        assert_eq!(exec_quote("/home/a/b-c_d.e"), "/home/a/b-c_d.e");
        assert_eq!(exec_quote("/home/a b/x"), "\"/home/a b/x\"");
        assert_eq!(exec_quote("/a$b"), "\"/a\\\\$b\"");
        assert_eq!(exec_quote("/a\\b"), "\"/a\\\\\\\\b\"");
    }
}
