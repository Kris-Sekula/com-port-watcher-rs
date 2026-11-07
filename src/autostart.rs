//! "Start at login" (v0.4.0) — the per-OS autostart registration:
//!
//! - Windows: a value under HKCU\...\CurrentVersion\Run naming
//!   the quoted exe path (no admin rights needed, per-user).
//! - Linux: `~/.config/autostart/com-port-watcher-rs.desktop`
//!   (XDG_CONFIG_HOME honoured).
//! - macOS: `~/Library/LaunchAgents/com.kris.comportwatcher.plist`.
//!
//! The pure path/content builders are unit-tested; the public
//! entry points wire them to the real user profile and the
//! current executable.

use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
const RUN_VALUE_NAME: &str = "COM Port Watcher";
// The file-based constants/helpers below serve Linux and macOS;
// on Windows they're compiled out with the rest of that path, and
// the RUN_VALUE_NAME pattern above only covers the Windows-only
// items, so silence the dead-code lint for the shared file items.
#[cfg_attr(target_os = "windows", allow(dead_code))]
const DESKTOP_FILE_NAME: &str = "com-port-watcher-rs.desktop";
#[cfg_attr(target_os = "windows", allow(dead_code))]
const PLIST_LABEL: &str = "com.kris.comportwatcher";
#[cfg_attr(target_os = "windows", allow(dead_code))]
const PLIST_FILE_NAME: &str = "com.kris.comportwatcher.plist";

pub fn is_enabled() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_impl::is_enabled()
    }
    #[cfg(not(target_os = "windows"))]
    {
        autostart_file().map(|f| f.exists()).unwrap_or(false)
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("no exe path: {e}"))?;
    #[cfg(target_os = "windows")]
    {
        let _ = &exe;
        windows_impl::set_enabled(enabled, &exe)
    }
    #[cfg(not(target_os = "windows"))]
    {
        set_enabled_at(&autostart_file().ok_or("no home directory")?, &exe, enabled)
    }
}

/// The autostart entry file for this OS, resolved from the user
/// profile environment. None when HOME is unset.
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn autostart_file() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("LaunchAgents")
                .join(PLIST_FILE_NAME),
        )
    } else {
        // Linux / other Unix: XDG autostart directory.
        let base = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(x) if !x.is_empty() => PathBuf::from(x),
            _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
        };
        Some(base.join("autostart").join(DESKTOP_FILE_NAME))
    }
}

/// File-based implementation shared by Linux and macOS: write or
/// remove the entry file for `exe`.
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn set_enabled_at(file: &Path, exe: &Path, enabled: bool) -> Result<(), String> {
    if enabled {
        let content = if cfg!(target_os = "macos") {
            plist_content(exe)
        } else {
            desktop_content(exe)
        };
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        std::fs::write(file, content)
            .map_err(|e| format!("write {}: {e}", file.display()))
    } else if file.exists() {
        std::fs::remove_file(file)
            .map_err(|e| format!("remove {}: {e}", file.display()))
    } else {
        Ok(())
    }
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
fn desktop_content(exe: &Path) -> String {
    let path = exe.to_string_lossy();
    let exec = if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path.into_owned()
    };
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=COM Port Watcher\n\
         Exec={exec}\n\
         X-GNOME-Autostart-enabled=true\n\
         NoDisplay=false\n"
    )
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
fn plist_content(exe: &Path) -> String {
    let path = xml_escape(&exe.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{PLIST_LABEL}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{path}</string>\n\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         </dict>\n\
         </plist>\n"
    )
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::path::Path;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SAM_FLAGS,
        REG_VALUE_TYPE,
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn open_run_key(access: REG_SAM_FLAGS) -> Result<HKEY, String> {
        let key_path = wide(RUN_KEY);
        let mut hkey = HKEY::default();
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key_path.as_ptr()),
                None,
                PCWSTR::null(),
                windows::Win32::System::Registry::REG_OPEN_CREATE_OPTIONS(0),
                access,
                None,
                &mut hkey,
                None,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(format!("opening the Run key failed: {rc:?}"));
        }
        Ok(hkey)
    }

    pub fn is_enabled() -> bool {
        let Ok(hkey) = open_run_key(KEY_QUERY_VALUE) else {
            return false;
        };
        let name = wide(super::RUN_VALUE_NAME);
        let mut ty = REG_VALUE_TYPE::default();
        let mut size: u32 = 0;
        let rc = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(name.as_ptr()),
                None,
                Some(&mut ty),
                None,
                Some(&mut size),
            )
        };
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        rc == ERROR_SUCCESS && size > 0
    }

    pub fn set_enabled(enabled: bool, exe: &Path) -> Result<(), String> {
        let hkey = open_run_key(KEY_SET_VALUE | KEY_QUERY_VALUE)?;
        let name = wide(super::RUN_VALUE_NAME);
        let rc = if enabled {
            let value = wide(&format!("\"{}\"", exe.to_string_lossy()));
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    value.as_ptr() as *const u8,
                    value.len() * 2,
                )
            };
            unsafe {
                windows::Win32::System::Registry::RegSetValueExW(
                    hkey,
                    PCWSTR(name.as_ptr()),
                    None,
                    windows::Win32::System::Registry::REG_SZ,
                    Some(bytes),
                )
            }
        } else {
            unsafe { RegDeleteValueW(hkey, PCWSTR(name.as_ptr())) }
        };
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        // Deleting a value that was never there is not an error
        // for the toggle's purposes.
        if rc != ERROR_SUCCESS && enabled {
            return Err(format!("setting the Run value failed: {rc:?}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{desktop_content, plist_content, set_enabled_at};
    use std::path::{Path, PathBuf};

    fn temp_file(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("cpw-autostart-test-{}-{tag}", std::process::id()))
            .join("entry.file")
    }

    #[test]
    fn desktop_entry_contains_exe_and_autostart_flag() {
        let content = desktop_content(Path::new("/opt/watcher/com-port-watcher-rs"));
        assert!(content.contains("Exec=/opt/watcher/com-port-watcher-rs"));
        assert!(content.contains("X-GNOME-Autostart-enabled=true"));
        assert!(content.starts_with("[Desktop Entry]"));
    }

    #[test]
    fn desktop_entry_quotes_paths_with_spaces() {
        let content = desktop_content(Path::new("/home/kris/my apps/watcher"));
        assert!(content.contains("Exec=\"/home/kris/my apps/watcher\""));
    }

    #[test]
    fn plist_contains_label_and_escaped_path() {
        let content = plist_content(Path::new("/Users/kris/Apps & Tools/watcher"));
        assert!(content.contains("com.kris.comportwatcher"));
        assert!(content.contains("/Users/kris/Apps &amp; Tools/watcher"));
        assert!(content.contains("<key>RunAtLoad</key>"));
    }

    #[test]
    fn file_round_trip() {
        let file = temp_file("roundtrip");
        let exe = Path::new("/usr/local/bin/com-port-watcher-rs");
        set_enabled_at(&file, exe, true).expect("enable writes the file");
        assert!(file.exists());
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(!written.is_empty());
        set_enabled_at(&file, exe, false).expect("disable removes the file");
        assert!(!file.exists());
        // Disabling twice is fine.
        set_enabled_at(&file, exe, false).expect("disable is idempotent");
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }
}
