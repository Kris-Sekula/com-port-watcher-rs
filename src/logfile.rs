//! Minimal file logger — no external crates.
//! Writes to com_port_watcher_rs.log next to the executable (or the
//! current directory if that fails), and echoes to stderr when present.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn log_path() -> std::path::PathBuf {
    let base = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("com_port_watcher_rs.log")
}

pub fn init() {
    let path = log_path();
    if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
        *LOG_FILE.lock().unwrap() = Some(f);
    }
}

/// UTC timestamp "2026-11-06 08:44:24" from epoch seconds (civil-date algorithm).
fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days algorithm
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year,
        m,
        d,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

pub fn log(level: &str, msg: &str) {
    let line = format!("{} {:5} {}", utc_now(), level, msg);
    eprintln!("{line}");
    if let Ok(mut guard) = LOG_FILE.lock() {
        if let Some(f) = guard.as_mut() {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

#[macro_export]
macro_rules! linfo {
    ($($arg:tt)*) => { $crate::logfile::log("INFO", &format!($($arg)*)) };
}
#[macro_export]
macro_rules! lwarn {
    ($($arg:tt)*) => { $crate::logfile::log("WARN", &format!($($arg)*)) };
}
#[macro_export]
macro_rules! lerror {
    ($($arg:tt)*) => { $crate::logfile::log("ERROR", &format!($($arg)*)) };
}
