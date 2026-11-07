//! Native OS notifications via notify-rust:
//! Windows toast · Linux org.freedesktop.Notifications (D-Bus) ·
//! macOS Notification Center. Errors are logged, never fatal —
//! a headless session or a missing notification server must not
//! kill the watcher.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::core::{PortEventKind, PortInfo};

/// How long a sent notification's handle is kept alive. Some
/// notification servers tie a notification's lifetime to the
/// sending handle/connection, and dropping the handle the instant
/// `show()` returns can make the banner vanish before it is seen
/// (observed risk on Xfce (xfce4-notifyd), 2026-11-07). Keeping handles for a
/// couple of minutes costs a few bytes and removes that variable.
const HANDLE_KEEP: Duration = Duration::from_secs(120);
static LIVE_HANDLES: Mutex<Vec<(Instant, notify_rust::NotificationHandle)>> =
    Mutex::new(Vec::new());

fn retain_handle(handle: notify_rust::NotificationHandle) {
    if let Ok(mut live) = LIVE_HANDLES.lock() {
        let now = Instant::now();
        live.retain(|(t, _)| now.duration_since(*t) < HANDLE_KEEP);
        live.push((now, handle));
    }
}

pub fn port_noun() -> &'static str {
    if cfg!(target_os = "windows") {
        "COM port"
    } else {
        "Serial port"
    }
}

pub fn send(kind: PortEventKind, info: &PortInfo) {
    let verb = match kind {
        PortEventKind::Added => "added",
        PortEventKind::Removed => "removed",
    };
    let title = format!("{} {}", port_noun(), verb);
    let mut body = info.label();
    let detail = info.detail();
    if !detail.is_empty() {
        body += &format!("\n{detail}");
    }
    send_raw(&title, &body);
}

pub fn send_raw(title: &str, body: &str) {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname("COM Port Watcher")
        .summary(title)
        .body(body)
        // Ask for 10 s on screen instead of the server default,
        // which is short enough to miss while looking
        // at the hardware. (Windows maps this to a short toast;
        // macOS ignores it.)
        .timeout(notify_rust::Timeout::Milliseconds(10_000));
    // On Windows, app_id() is the AppUserModelID the toast posts
    // under (registered at startup by crate::appid). Without it,
    // notify-rust falls back to PowerShell's identity. The method
    // only exists on Windows builds, hence the cfg.
    #[cfg(target_os = "windows")]
    notification.app_id(crate::appid::APP_ID);
    let started = Instant::now();
    match notification.show() {
        Ok(handle) => {
            // The duration separates "the desktop was slow to answer"
            // from "our event loop was slow to dispatch" in the log.
            crate::linfo!(
                "Notification sent: {title} — {body} (show call took {:.2?})",
                started.elapsed()
            );
            retain_handle(handle);
        }
        Err(e) => crate::lwarn!("Notification failed ({title}): {e}"),
    }
}
