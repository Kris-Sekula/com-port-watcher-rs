//! Windows AppUserModelID registration (Windows only).
//!
//! Windows refuses to brand toast notifications for an app that has
//! no registered AppUserModelID, so notify-rust falls back to
//! PowerShell's identity and our toasts show up headed "Windows
//! PowerShell". To post as ourselves we register APP_ID once at
//! startup, three independent ways — each best-effort, failures only
//! logged:
//!
//! 1. SetCurrentProcessExplicitAppUserModelID for this process.
//! 2. HKCU\Software\Classes\AppUserModelId\<APP_ID> with
//!    DisplayName + IconUri (the shell's name/icon lookup).
//! 3. A Start Menu shortcut whose property store carries
//!    PKEY_AppUserModel_ID — the documented registration route for
//!    unpackaged desktop apps.
//!
//! The toast icon is the .ico embedded in this binary, extracted
//! next to the executable on first run (IconUri must be a file the
//! shell can read; registry values pointing into the exe's resources
//! are not honoured for toasts).

/// On non-Windows builds this is only referenced from cfg'd-out code.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub const APP_ID: &str = "Kris.ComPortWatcher";
#[cfg(target_os = "windows")]
pub const APP_NAME: &str = "COM Port Watcher";

#[cfg(target_os = "windows")]
const ICON_BYTES: &[u8] = include_bytes!("../assets/com_port_watcher.ico");
#[cfg(target_os = "windows")]
const ICON_FILE_NAME: &str = "com_port_watcher.ico";

#[cfg(not(target_os = "windows"))]
pub fn register() {}

#[cfg(target_os = "windows")]
pub fn register() {
    // Cheap and unconditional: pin this process to our identity.
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
        if let Err(e) = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID)) {
            crate::lwarn!("SetCurrentProcessExplicitAppUserModelID failed: {e}");
        }
    }
    let icon_path = ensure_icon_file();
    register_in_registry(icon_path.as_deref());
    register_start_menu_shortcut();
    crate::linfo!("AppUserModelID registered: {APP_ID}");
}

#[cfg(target_os = "windows")]
fn ensure_icon_file() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let path = dir.join(ICON_FILE_NAME);
    let stale = match std::fs::read(&path) {
        Ok(existing) => existing != ICON_BYTES,
        Err(_) => true,
    };
    if stale {
        if let Err(e) = std::fs::write(&path, ICON_BYTES) {
            crate::lwarn!("Could not write icon file {}: {e}", path.display());
            return None;
        }
    }
    Some(path)
}

#[cfg(target_os = "windows")]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn register_in_registry(icon_path: Option<&std::path::Path>) {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_SZ,
    };

    let key_path = wide(&format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
    let mut hkey = HKEY::default();
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            None,
            PCWSTR::null(),
            windows::Win32::System::Registry::REG_OPEN_CREATE_OPTIONS(0),
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
    };
    if rc != ERROR_SUCCESS {
        crate::lwarn!("AppUserModelId registry key failed: {rc:?}");
        return;
    }
    let set_sz = |name: &str, value: &str| {
        let name_w = wide(name);
        let value_w = wide(value);
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(value_w.as_ptr() as *const u8, value_w.len() * 2)
        };
        let rc = unsafe { RegSetValueExW(hkey, PCWSTR(name_w.as_ptr()), None, REG_SZ, Some(bytes)) };
        if rc != ERROR_SUCCESS {
            crate::lwarn!("AppUserModelId registry value {name} failed: {rc:?}");
        }
    };
    set_sz("DisplayName", APP_NAME);
    if let Some(p) = icon_path {
        set_sz("IconUri", &p.to_string_lossy());
    }
    unsafe {
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
}

#[cfg(target_os = "windows")]
fn register_start_menu_shortcut() {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Interface, GUID, PCWSTR};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    // PKEY_AppUserModel_ID = {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, pid 5.
    // Hardcoded instead of the windows-rs constant: that constant's
    // module moves between crate versions.
    const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3),
        pid: 5,
    };

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            crate::lwarn!("Start Menu shortcut skipped (no exe path): {e}");
            return;
        }
    };
    let appdata = match std::env::var_os("APPDATA") {
        Some(a) => std::path::PathBuf::from(a),
        None => {
            crate::lwarn!("Start Menu shortcut skipped (APPDATA unset)");
            return;
        }
    };
    let lnk_path = appdata
        .join(r"Microsoft\Windows\Start Menu\Programs")
        .join(format!("{APP_NAME}.lnk"));
    let lnk_w: Vec<u16> = lnk_path.as_os_str().encode_wide().chain([0]).collect();
    let exe_w: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();

    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            crate::lwarn!("Start Menu shortcut skipped (COM init failed)");
            return;
        }
        let result: windows::core::Result<()> = (|| {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(PCWSTR(exe_w.as_ptr()))?;
            let store: IPropertyStore = link.cast()?;
            store.SetValue(&PKEY_APP_USER_MODEL_ID, &PROPVARIANT::from(APP_ID))?;
            store.Commit()?;
            let persist: IPersistFile = link.cast()?;
            persist.Save(PCWSTR(lnk_w.as_ptr()), true)?;
            Ok(())
        })();
        if let Err(e) = result {
            crate::lwarn!("Start Menu shortcut failed: {e}");
        }
        CoUninitialize();
    }
}
