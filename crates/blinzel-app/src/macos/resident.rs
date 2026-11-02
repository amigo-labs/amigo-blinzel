//! Resident mode on macOS: an icon in the menu bar (`NSStatusItem`) with
//! the tray menu, no Dock icon while the window is hidden (activation
//! policy "accessory"), and a login item, a LaunchAgent that starts the
//! bundle's program with `--resident` at sign-in.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use dispatch2::{DispatchQueue, DispatchTime};
use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSImage, NSMenu, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSString, ns_string};

use blinzel_i18n::{Msg, tr};
use blinzel_ui::menu::MenuItem;

/// The LaunchAgent's label and file name.
const AGENT: &str = "io.github.amigo-labs.blinzel";

thread_local! {
    static ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
    static MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
    /// Counts [`flash`] messages.
    static FLASH: Cell<u32> = const { Cell::new(0) };
}

/// What turning resident mode on or off does to the menu bar icon.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TrayChange {
    /// Put the icon into the menu bar.
    Add,
    /// Take it out of the menu bar.
    Remove,
    /// It already is as resident mode wants it.
    Keep,
}

/// The change from an icon that is `shown` to resident mode `on`.
fn tray_change(shown: bool, on: bool) -> TrayChange {
    match (shown, on) {
        (false, true) => TrayChange::Add,
        (true, false) => TrayChange::Remove,
        _ => TrayChange::Keep,
    }
}

/// Resident mode on or off (`docs/specs/resident-mode.md` AC-02): the
/// sign-in entry follows at once through `sign_in` (the app passes
/// [`set_autostart`]), the menu bar icon through `tray` ([`set_tray`]).
pub fn set_resident(on: bool, sign_in: impl FnOnce(bool), tray: impl FnOnce(TrayChange)) {
    sign_in(on);
    tray(tray_change(ITEM.with(|i| i.borrow().is_some()), on));
}

/// How the menu bar icon looks (`docs/specs/resident-mode.md` AC-14).
#[derive(Debug, PartialEq)]
struct TrayLook {
    /// The SF Symbol it shows.
    symbol: &'static str,
    /// Drawn as a template image, in the menu bar's own colours.
    template: bool,
    /// Its tooltip and accessibility description (also the title where
    /// the symbol is missing).
    tooltip: &'static str,
}

fn tray_look() -> TrayLook {
    TrayLook {
        symbol: "eye",
        template: true,
        tooltip: tr(Msg::AppName),
    }
}

/// The menu bar icon's menu: the tray menu, built anew each time it opens
/// (`menuNeedsUpdate:`).
pub fn tray_menu() -> Vec<MenuItem> {
    blinzel_ui::menu::tray_items()
}

/// Adds or removes the menu bar icon. Its menu is filled each time it
/// opens ([`is_tray_menu`], [`tray_menu`]), by the delegate set on it.
pub fn set_tray(
    mtm: MainThreadMarker,
    change: TrayChange,
    delegate: Option<&super::shell::Delegate>,
) {
    match change {
        TrayChange::Keep => {}
        TrayChange::Remove => {
            if let Some(item) = ITEM.with(|i| i.borrow_mut().take()) {
                NSStatusBar::systemStatusBar().removeStatusItem(&item);
            }
            MENU.with(|m| m.borrow_mut().take());
        }
        TrayChange::Add => add_tray(mtm, delegate),
    }
}

fn add_tray(mtm: MainThreadMarker, delegate: Option<&super::shell::Delegate>) {
    let look = tray_look();
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        let name = NSString::from_str(look.tooltip);
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(look.symbol),
            Some(&name),
        ) {
            image.setTemplate(look.template);
            button.setImage(Some(&image));
        } else {
            button.setTitle(&name);
        }
        button.setToolTip(Some(&name));
    }
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!(""));
    menu.setAutoenablesItems(false);
    if let Some(d) = delegate {
        menu.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(d)));
    }
    item.setMenu(Some(&menu));
    ITEM.with(|i| *i.borrow_mut() = Some(item));
    MENU.with(|m| *m.borrow_mut() = Some(menu));
}

/// A short message next to the menu bar icon (the window is hidden).
pub fn flash(text: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(button) = ITEM.with(|i| i.borrow().as_ref().and_then(|i| i.button(mtm))) else {
        return;
    };
    button.setTitle(&NSString::from_str(text));
    let generation = FLASH.with(|f| {
        f.set(f.get() + 1);
        f.get()
    });
    let ns = std::time::Duration::from_secs(4).as_nanos() as i64;
    let _ = DispatchQueue::main().after(DispatchTime::NOW.time(ns), move || {
        // Only the latest message clears the title.
        if FLASH.with(|f| f.get()) != generation {
            return;
        }
        if let Some(mtm) = MainThreadMarker::new()
            && let Some(b) = ITEM.with(|i| i.borrow().as_ref().and_then(|i| i.button(mtm)))
        {
            b.setTitle(ns_string!(""));
        }
    });
}

/// Whether `menu` is the menu bar icon's menu.
pub fn is_tray_menu(menu: &NSMenu) -> bool {
    MENU.with(|m| {
        m.borrow()
            .as_ref()
            .is_some_and(|t| std::ptr::eq(&**t, menu))
    })
}

/// With the window hidden, the app leaves the Dock and the app switcher.
pub fn set_in_dock(mtm: MainThreadMarker, on: bool) {
    NSApplication::sharedApplication(mtm).setActivationPolicy(dock_policy(on));
}

/// The activation policy for an app that is `on` the Dock: "accessory"
/// keeps it out of the Dock and the app switcher.
fn dock_policy(on: bool) -> NSApplicationActivationPolicy {
    if on {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    }
}

fn agent_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        Path::new(&home)
            .join("Library/LaunchAgents")
            .join(format!("{AGENT}.plist")),
    )
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The LaunchAgent that starts `program` hidden at sign-in.
pub fn launch_agent(program: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{AGENT}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{}</string>
		<string>--resident</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#,
        escape(&program.to_string_lossy())
    )
}

/// Writes or removes the login item. Only a copy running from an app
/// bundle gets one: a bare executable may move.
pub fn set_autostart(on: bool) -> Result<(), String> {
    let path = agent_path().ok_or("no home folder")?;
    sign_in(&path, on, super::update::bundle_program())
}

/// The LaunchAgent at `path`: written for `bundle`'s program while `on`,
/// removed while off or outside an app bundle (`bundle` is `None`).
fn sign_in(path: &Path, on: bool, bundle: Option<PathBuf>) -> Result<(), String> {
    match bundle.filter(|_| on) {
        Some(program) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(path, launch_agent(&program))
                .map_err(|e| format!("{}: {e}", path.display()))
        }
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("{}: {e}", path.display()))
            }
            _ => Ok(()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blinzel_ui::menu::Cmd;

    /// `docs/specs/resident-mode.md` AC-18: only a copy in an app bundle
    /// writes the LaunchAgent; outside one, or with resident mode off, an
    /// existing one is removed (in a temporary folder, never the real one).
    #[test]
    fn only_a_copy_in_an_app_bundle_writes_the_launch_agent() {
        let dir = std::env::temp_dir().join(format!("blinzel-agent-{}", std::process::id()));
        let path = dir.join("LaunchAgents").join(format!("{AGENT}.plist"));
        let program = PathBuf::from("/Applications/amigo-blinzel.app/Contents/MacOS/blinzel");
        sign_in(&path, true, Some(program.clone())).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            launch_agent(&program)
        );
        sign_in(&path, true, None).unwrap();
        assert!(!path.exists(), "outside an app bundle: removed");
        sign_in(&path, true, None).unwrap();
        sign_in(&path, true, Some(program.clone())).unwrap();
        sign_in(&path, false, Some(program)).unwrap();
        assert!(!path.exists(), "resident mode off: removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `docs/specs/resident-mode.md` AC-02 (the macOS shell): turning
    /// resident mode on adds the menu bar icon and writes the LaunchAgent
    /// at once, off removes both (in a temporary folder, never the real
    /// one; the icon itself needs the main thread, so `tray` records it).
    #[test]
    fn toggling_resident_mode_adds_or_removes_the_icon_and_the_launch_agent() {
        assert_eq!(tray_change(false, true), TrayChange::Add);
        assert_eq!(tray_change(true, false), TrayChange::Remove);
        assert_eq!(tray_change(true, true), TrayChange::Keep);
        assert_eq!(tray_change(false, false), TrayChange::Keep);

        let dir = std::env::temp_dir().join(format!("blinzel-toggle-{}", std::process::id()));
        let path = dir.join("LaunchAgents").join(format!("{AGENT}.plist"));
        let program = PathBuf::from("/Applications/amigo-blinzel.app/Contents/MacOS/blinzel");
        let agent = |on| sign_in(&path, on, Some(program.clone())).unwrap();
        let mut tray = vec![];
        set_resident(true, agent, |c| tray.push(c));
        assert!(path.exists(), "LaunchAgent written");
        set_resident(false, agent, |c| tray.push(c));
        assert!(!path.exists(), "LaunchAgent removed");
        // No icon was really added on this thread, so off keeps it absent.
        assert_eq!(tray, [TrayChange::Add, TrayChange::Keep]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `docs/specs/resident-mode.md` AC-14: the menu bar item shows the SF
    /// Symbol "eye" as a template image with the app name as tooltip; its
    /// menu is the tray menu, screenshots included; while the window is
    /// hidden the app is an accessory (not in the Dock or app switcher).
    #[test]
    fn the_menu_bar_item_is_an_eye_with_the_tray_menu() {
        assert_eq!(
            tray_look(),
            TrayLook {
                symbol: "eye",
                template: true,
                tooltip: tr(Msg::AppName),
            }
        );
        let entries: Vec<Option<(Cmd, String)>> = tray_menu()
            .into_iter()
            .map(|i| match i {
                MenuItem::Item { cmd, label, .. } => Some((cmd, label)),
                _ => None,
            })
            .collect();
        let item = |c, m| Some((c, tr(m).to_string()));
        assert_eq!(
            entries,
            [
                item(Cmd::ShowWindow, Msg::TrayShow),
                item(Cmd::Open, Msg::MenuOpen),
                None,
                item(Cmd::Capture, Msg::TrayCapture),
                item(Cmd::CaptureScreen, Msg::TrayCaptureScreen),
                None,
                item(Cmd::Quit, Msg::Exit),
            ]
        );
        assert_eq!(dock_policy(false), NSApplicationActivationPolicy::Accessory);
        assert_eq!(dock_policy(true), NSApplicationActivationPolicy::Regular);
    }

    #[test]
    fn launch_agent_starts_the_program_hidden() {
        let p = launch_agent(Path::new("/Applications/A & B.app/Contents/MacOS/blinzel"));
        assert!(p.contains("<string>/Applications/A &amp; B.app/Contents/MacOS/blinzel</string>"));
        assert!(p.contains("<string>--resident</string>"));
        assert!(p.contains(&format!("<string>{AGENT}</string>")));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
    }
}
