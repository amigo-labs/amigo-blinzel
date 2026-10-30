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

/// The LaunchAgent's label and file name.
const AGENT: &str = "io.github.amigo-labs.blinzel";

thread_local! {
    static ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
    static MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
    /// Counts [`flash`] messages.
    static FLASH: Cell<u32> = const { Cell::new(0) };
}

/// Shows or removes the menu bar icon. Its menu is filled each time it
/// opens ([`is_tray_menu`]), by the delegate set on it.
pub fn set_tray(mtm: MainThreadMarker, on: bool, delegate: Option<&super::shell::Delegate>) {
    let bar = NSStatusBar::systemStatusBar();
    if !on {
        if let Some(item) = ITEM.with(|i| i.borrow_mut().take()) {
            bar.removeStatusItem(&item);
        }
        MENU.with(|m| m.borrow_mut().take());
        return;
    }
    if ITEM.with(|i| i.borrow().is_some()) {
        return;
    }
    let item = bar.statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        let name = NSString::from_str(tr(Msg::AppName));
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("eye"),
            Some(&name),
        ) {
            image.setTemplate(true);
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
    let policy = if on {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    };
    NSApplication::sharedApplication(mtm).setActivationPolicy(policy);
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
    match super::update::bundle_program().filter(|_| on) {
        Some(program) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&path, launch_agent(&program))
                .map_err(|e| format!("{}: {e}", path.display()))
        }
        None => match std::fs::remove_file(&path) {
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

    #[test]
    fn launch_agent_starts_the_program_hidden() {
        let p = launch_agent(Path::new("/Applications/A & B.app/Contents/MacOS/blinzel"));
        assert!(p.contains("<string>/Applications/A &amp; B.app/Contents/MacOS/blinzel</string>"));
        assert!(p.contains("<string>--resident</string>"));
        assert!(p.contains(&format!("<string>{AGENT}</string>")));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
    }
}
