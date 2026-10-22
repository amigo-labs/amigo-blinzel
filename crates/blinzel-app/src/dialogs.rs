//! System dialogs: open file, move to recycle bin. Both are localized by
//! Windows itself.

use std::path::{Path, PathBuf};

use blinzel_core::format::IMAGE_EXTENSIONS;
use blinzel_i18n::{Msg, tr};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FO_DELETE, FOF_ALLOWUNDO, FOF_WANTNUKEWARNING, FileOpenDialog, IFileOpenDialog,
    SHFILEOPSTRUCTW, SHFileOperationW, SIGDN_FILESYSPATH,
};
use windows::core::PCWSTR;

use crate::platform::wide;

fn ensure_sta() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    // The UI thread joins an STA lazily, only when a dialog is needed.
    ONCE.call_once(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    });
}

pub fn open_file(hwnd: HWND) -> Option<PathBuf> {
    ensure_sta();
    unsafe {
        let dlg: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let spec: String = IMAGE_EXTENSIONS
            .iter()
            .map(|e| format!("*.{e}"))
            .collect::<Vec<_>>()
            .join(";");
        let (n1, s1) = (wide(tr(Msg::OpenFilterImages)), wide(&spec));
        let (n2, s2) = (wide(tr(Msg::OpenFilterAll)), wide("*.*"));
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: PCWSTR(n1.as_ptr()),
                pszSpec: PCWSTR(s1.as_ptr()),
            },
            COMDLG_FILTERSPEC {
                pszName: PCWSTR(n2.as_ptr()),
                pszSpec: PCWSTR(s2.as_ptr()),
            },
        ];
        dlg.SetFileTypes(&filters).ok()?;
        dlg.Show(Some(hwnd)).ok()?;
        let item = dlg.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.0 as *const _));
        path
    }
}

/// Moves `path` to the recycle bin (Windows asks for confirmation).
pub fn recycle(hwnd: HWND, path: &Path) -> bool {
    // pFrom is a list terminated by two NULs.
    let mut from = wide(path);
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        hwnd,
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO.0 | FOF_WANTNUKEWARNING.0) as u16,
        ..Default::default()
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    r == 0 && !op.fAnyOperationsAborted.as_bool() && !path.exists()
}
