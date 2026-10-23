//! Start menu shortcut with an AppUserModelID, so the taskbar groups the
//! running viewer with its pinned shortcut.

use std::path::Path;

use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{IShellLinkW, SHStrDupW, ShellLink};
use windows::core::{Interface, PCWSTR};

use crate::platform::wide;

/// The AppUserModelID of the viewer process and its shortcut.
pub const APP_ID: &str = "amigo-labs.amigo-blinzel";

fn err(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |e| format!("{what}: {}", e.message())
}

/// Creates or replaces the shortcut `lnk` to `target`.
pub fn create(lnk: &Path, target: &Path, description: &str) -> Result<(), String> {
    unsafe {
        // Balanced only if this call initialized COM on the thread.
        let inited = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let r = create_inner(lnk, target, description);
        if inited {
            CoUninitialize();
        }
        r
    }
}

unsafe fn create_inner(lnk: &Path, target: &Path, description: &str) -> Result<(), String> {
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(err("ShellLink"))?;
        let t = wide(target);
        link.SetPath(PCWSTR(t.as_ptr())).map_err(err("SetPath"))?;
        link.SetIconLocation(PCWSTR(t.as_ptr()), 0)
            .map_err(err("SetIconLocation"))?;
        if let Some(dir) = target.parent() {
            let d = wide(dir);
            link.SetWorkingDirectory(PCWSTR(d.as_ptr()))
                .map_err(err("SetWorkingDirectory"))?;
        }
        let desc = wide(description);
        link.SetDescription(PCWSTR(desc.as_ptr()))
            .map_err(err("SetDescription"))?;

        let store: IPropertyStore = link.cast().map_err(err("IPropertyStore"))?;
        let id = wide(APP_ID);
        let mut pv = PROPVARIANT::default();
        (*pv.Anonymous.Anonymous).vt = VT_LPWSTR;
        (*pv.Anonymous.Anonymous).Anonymous.pwszVal =
            SHStrDupW(PCWSTR(id.as_ptr())).map_err(err("SHStrDupW"))?;
        let set = store
            .SetValue(&PKEY_AppUserModel_ID, &pv)
            .and_then(|()| store.Commit());
        let _ = PropVariantClear(&mut pv);
        set.map_err(err("AppUserModelID"))?;

        let file: IPersistFile = link.cast().map_err(err("IPersistFile"))?;
        let l = wide(lnk);
        file.Save(PCWSTR(l.as_ptr()), true).map_err(err("Save"))
    }
}
