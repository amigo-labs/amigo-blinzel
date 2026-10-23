//! Minimal registry access for the per-user installation (HKCU only).

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_NONE, REG_OPTION_NON_VOLATILE,
    REG_SZ, REG_VALUE_TYPE, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegDeleteValueW, RegGetValueW,
};
use windows::core::PCWSTR;

use crate::platform::wide;

fn check(e: WIN32_ERROR, what: &str) -> Result<(), String> {
    if e == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!(
            "{what}: {}",
            windows::core::Error::from(e.to_hresult()).message()
        ))
    }
}

/// An open key under HKEY_CURRENT_USER, closed on drop.
pub struct Key(HKEY);

impl Key {
    /// Opens `path` below HKCU for writing, creating it if needed.
    pub fn create(path: &str) -> Result<Key, String> {
        let p = wide(path);
        let mut h = HKEY::default();
        let e = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(p.as_ptr()),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                None,
                &mut h,
                None,
            )
        };
        check(e, path)?;
        Ok(Key(h))
    }

    fn set(&self, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), String> {
        let n = wide(name);
        let e = unsafe {
            windows::Win32::System::Registry::RegSetValueExW(
                self.0,
                if name.is_empty() {
                    PCWSTR::null()
                } else {
                    PCWSTR(n.as_ptr())
                },
                None,
                kind,
                Some(data),
            )
        };
        check(e, name)
    }

    /// Sets a string value; `""` is the key's default value.
    pub fn set_str(&self, name: &str, value: &str) -> Result<(), String> {
        let w = wide(value);
        let bytes: Vec<u8> = w.iter().flat_map(|c| c.to_le_bytes()).collect();
        self.set(name, REG_SZ, &bytes)
    }

    pub fn set_dword(&self, name: &str, value: u32) -> Result<(), String> {
        self.set(name, REG_DWORD, &value.to_le_bytes())
    }

    /// An empty value, as `OpenWithProgids` entries are.
    pub fn set_none(&self, name: &str) -> Result<(), String> {
        self.set(name, REG_NONE, &[])
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// Deletes `path` below HKCU with all subkeys; a missing key is fine.
pub fn delete_tree(path: &str) -> Result<(), String> {
    let p = wide(path);
    let e = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr())) };
    if e == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(e, path)
}

/// Deletes one value of `path` below HKCU; a missing value is fine.
pub fn delete_value(path: &str, name: &str) -> Result<(), String> {
    let Ok(k) = Key::create(path) else {
        return Ok(());
    };
    let n = wide(name);
    let e = unsafe { RegDeleteValueW(k.0, PCWSTR(n.as_ptr())) };
    if e == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(e, name)
}

/// Reads a string value below HKCU.
pub fn get_str(path: &str, name: &str) -> Option<String> {
    let (p, n) = (wide(path), wide(name));
    let mut buf = vec![0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let e = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(p.as_ptr()),
            if name.is_empty() {
                PCWSTR::null()
            } else {
                PCWSTR(n.as_ptr())
            },
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    (e == ERROR_SUCCESS).then(|| crate::platform::from_wide(&buf))
}

/// Whether `path` below HKCU has a value `name` (of any type).
pub fn value_exists(path: &str, name: &str) -> bool {
    use windows::Win32::System::Registry::RRF_RT_ANY;
    let (p, n) = (wide(path), wide(name));
    let e = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(p.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_ANY,
            None,
            None,
            None,
        )
    };
    e == ERROR_SUCCESS
}
