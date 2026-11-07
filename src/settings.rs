//! Persisted user settings (v0.3.0), stored as simple key=value
//! lines in `com_port_watcher_rs.cfg` next to the executable —
//! the same place as the log file. Written by the tray's Settings
//! menu; read once at startup. Parsing is deliberately lenient:
//! unknown keys are ignored and bad values fall back to defaults,
//! so a hand-edited or partially written file can never break
//! startup.

use std::path::PathBuf;

use crate::{linfo, lwarn};

/// Which screen corner popups anchor to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
}

impl Corner {
    pub fn as_str(&self) -> &'static str {
        match self {
            Corner::BottomRight => "bottom-right",
            Corner::BottomLeft => "bottom-left",
            Corner::TopRight => "top-right",
            Corner::TopLeft => "top-left",
        }
    }

    pub fn from_str(s: &str) -> Option<Corner> {
        match s.trim() {
            "bottom-right" => Some(Corner::BottomRight),
            "bottom-left" => Some(Corner::BottomLeft),
            "top-right" => Some(Corner::TopRight),
            "top-left" => Some(Corner::TopLeft),
            _ => None,
        }
    }

    pub fn is_bottom(&self) -> bool {
        matches!(self, Corner::BottomRight | Corner::BottomLeft)
    }

    pub fn is_right(&self) -> bool {
        matches!(self, Corner::BottomRight | Corner::TopRight)
    }
}

/// Durations offered in the Settings menu (seconds).
pub const DURATION_CHOICES: [u64; 6] = [3, 5, 7, 10, 15, 30];

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// How long a popup stays on screen, in seconds.
    pub popup_seconds: u64,
    /// Which corner popups anchor to.
    pub corner: Corner,
    /// Whether the custom popup surface is used (vs native
    /// notifications). Persisted since v0.3.0.
    pub custom_popups: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            popup_seconds: 7,
            corner: Corner::BottomRight,
            custom_popups: true,
        }
    }
}

pub fn settings_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("com_port_watcher_rs.cfg");
        }
    }
    PathBuf::from("com_port_watcher_rs.cfg")
}

impl Settings {
    pub fn load() -> Settings {
        let path = settings_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let settings = Settings::parse(&text);
                linfo!(
                    "Settings loaded from {}: popup_seconds={}, corner={}, custom_popups={}",
                    path.display(),
                    settings.popup_seconds,
                    settings.corner.as_str(),
                    settings.custom_popups
                );
                settings
            }
            Err(_) => Settings::default(),
        }
    }

    pub fn parse(text: &str) -> Settings {
        let mut settings = Settings::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match (key.trim(), value.trim()) {
                ("popup_seconds", v) => {
                    if let Ok(n) = v.parse::<u64>() {
                        if (1..=120).contains(&n) {
                            settings.popup_seconds = n;
                        }
                    }
                }
                ("popup_corner", v) => {
                    if let Some(c) = Corner::from_str(v) {
                        settings.corner = c;
                    }
                }
                ("custom_popups", v) => {
                    settings.custom_popups = v == "true";
                }
                _ => {}
            }
        }
        settings
    }

    pub fn serialize(&self) -> String {
        format!(
            "# COM Port Watcher (Rust) settings — written by the tray Settings menu\n\
             popup_seconds={}\n\
             popup_corner={}\n\
             custom_popups={}\n",
            self.popup_seconds,
            self.corner.as_str(),
            self.custom_popups
        )
    }

    pub fn save(&self) {
        let path = settings_path();
        match std::fs::write(&path, self.serialize()) {
            Ok(()) => linfo!("Settings saved to {}", path.display()),
            Err(e) => lwarn!("Could not save settings to {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Corner, Settings};

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.popup_seconds, 7);
        assert_eq!(s.corner, Corner::BottomRight);
        assert!(s.custom_popups);
    }

    #[test]
    fn parse_reads_all_keys() {
        let s = Settings::parse(
            "popup_seconds=15\npopup_corner=top-left\ncustom_popups=false\n",
        );
        assert_eq!(s.popup_seconds, 15);
        assert_eq!(s.corner, Corner::TopLeft);
        assert!(!s.custom_popups);
    }

    #[test]
    fn parse_ignores_junk_and_bad_values() {
        let s = Settings::parse(
            "# comment\nnonsense line\npopup_seconds=999\npopup_corner=diagonal\nunknown=1\n",
        );
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn serialize_round_trips() {
        let s = Settings {
            popup_seconds: 30,
            corner: Corner::TopRight,
            custom_popups: false,
        };
        assert_eq!(Settings::parse(&s.serialize()), s);
    }
}
