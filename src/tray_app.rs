//! Tray / menu-bar application: tao event loop + tray-icon.
//! A worker thread owns the scanner and the debounced watcher and
//! forwards confirmed events + port snapshots to the UI thread over
//! an mpsc channel; the UI thread owns the tray icon, its menu, and
//! notification dispatch (the arrangement every OS's tray API wants).

use std::sync::mpsc;
use std::time::{Duration, Instant};

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::core::{scan_ports, DebouncedWatcher, PortEventKind, PortInfo};
use crate::popup::PopupManager;
use crate::settings::{Corner, Settings, DURATION_CHOICES};
use crate::{fake_ports, linfo, lwarn, notify};

const ID_TEST: &str = "test";
const ID_PAUSE: &str = "pause";
const ID_POPUPS: &str = "popups";
const ID_AUTOSTART: &str = "autostart";
const ID_LOG: &str = "log";
const ID_QUIT: &str = "quit";

const CORNER_CHOICES: [(Corner, &str); 4] = [
    (Corner::BottomRight, "Bottom right"),
    (Corner::BottomLeft, "Bottom left"),
    (Corner::TopRight, "Top right"),
    (Corner::TopLeft, "Top left"),
];

fn duration_id(seconds: u64) -> String {
    format!("dur_{seconds}")
}

fn corner_id(corner: Corner) -> String {
    format!("corner_{}", corner.as_str())
}

/// Route a confirmed event to the custom popup when enabled and
/// available, else to a native notification. Popup queueing is
/// fallible at enqueue time (fonts gone after a fatal failure);
/// windows that fail later re-send themselves natively in pump().
fn dispatch(
    popups: &mut PopupManager,
    use_popups: bool,
    kind: PortEventKind,
    info: &PortInfo,
) {
    if !(use_popups && popups.enqueue(kind, info)) {
        notify::send(kind, info);
    }
}

pub enum UiMsg {
    Event(PortEventKind, PortInfo),
    PortsChanged(Vec<PortInfo>),
}

fn make_icon() -> Icon {
    // 64x64 RGBA drawn in code (same look as the Python version):
    // green rounded square, white serial plug.
    let (w, h) = (64u32, 64u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let set = |rgba: &mut Vec<u8>, x: u32, y: u32, c: [u8; 4]| {
        if x < w && y < h {
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&c);
        }
    };
    let green = [31, 122, 58, 255];
    let white = [255, 255, 255, 255];
    for y in 6..58 {
        for x in 6..58 {
            set(&mut rgba, x, y, green);
        }
    }
    for y in 18..34 {
        for x in 22..42 {
            set(&mut rgba, x, y, white);
        }
    }
    for px in [27u32, 32, 37] {
        for y in 24..28 {
            for x in px - 2..px + 2 {
                set(&mut rgba, x, y, green);
            }
        }
    }
    for y in 34..48 {
        for x in 29..35 {
            set(&mut rgba, x, y, white);
        }
    }
    Icon::from_rgba(rgba, w, h).expect("static icon is valid")
}

fn build_menu(ports: &[PortInfo], paused: bool, settings: &Settings) -> Menu {
    let menu = Menu::new();
    if ports.is_empty() {
        let _ = menu.append(&MenuItem::new("(no serial ports)", false, None));
    } else {
        for p in ports {
            let _ = menu.append(&MenuItem::new(p.label(), false, None));
        }
    }
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id(ID_TEST, "Test notification", true, None));
    let _ = menu.append(&CheckMenuItem::with_id(
        ID_PAUSE,
        "Pause notifications",
        true,
        paused,
        None,
    ));
    // Custom popups are a Windows/Linux surface; macOS stays on
    // Notification Center banners, like the Python version — so
    // the popup toggle and the Settings submenu are omitted there.
    #[cfg(not(target_os = "macos"))]
    {
        let _ = menu.append(&CheckMenuItem::with_id(
            ID_POPUPS,
            "Custom popups",
            true,
            settings.custom_popups,
            None,
        ));
        let duration_menu = Submenu::new("Popup time on screen", true);
        for n in DURATION_CHOICES {
            let _ = duration_menu.append(&CheckMenuItem::with_id(
                duration_id(n),
                format!("{n} seconds"),
                true,
                settings.popup_seconds == n,
                None,
            ));
        }
        let corner_menu = Submenu::new("Popup corner", true);
        for (corner, label) in CORNER_CHOICES {
            let _ = corner_menu.append(&CheckMenuItem::with_id(
                corner_id(corner),
                label,
                true,
                settings.corner == corner,
                None,
            ));
        }
        let settings_menu = Submenu::new("Settings", true);
        let _ = settings_menu.append(&duration_menu);
        let _ = settings_menu.append(&corner_menu);
        let _ = menu.append(&settings_menu);
    }
    // Start-at-login exists on all three OSes (Run key / XDG
    // autostart / LaunchAgent), so this item is not cfg-gated.
    // The checked state is read live from the OS each time the
    // menu is built, so it can never drift from reality.
    let _ = menu.append(&CheckMenuItem::with_id(
        ID_AUTOSTART,
        "Start at login",
        true,
        crate::autostart::is_enabled(),
        None,
    ));
    let _ = menu.append(&MenuItem::with_id(ID_LOG, "Open log file", true, None));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id(ID_QUIT, "Quit", true, None));
    menu
}

fn open_log_file() {
    let path = crate::logfile::log_path();
    linfo!("Open log requested: {}", path.display());
    let result = if cfg!(target_os = "windows") {
        std::process::Command::new("explorer").arg(&path).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(&path).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(&path).spawn()
    };
    if let Err(e) = result {
        lwarn!("Could not open log file: {e}");
    }
}

pub fn run(test_notification: bool) {
    let event_loop = EventLoopBuilder::with_user_event().build();
    // The worker pokes the loop awake through this proxy after every
    // queued message. Relying on the loop's WaitUntil timer alone
    // proved unreliable on the Linux (GTK) backend: confirmed events
    // sat in the channel for 5–31 s before dispatch (Mint bench,
    // 2026-11-07). A user event wakes the loop immediately.
    let proxy = event_loop.create_proxy();
    let (tx, rx) = mpsc::channel::<UiMsg>();

    // Scan worker: owns watcher + baseline, forwards confirmed changes.
    std::thread::spawn(move || {
        let mut watcher = DebouncedWatcher::new();
        let baseline = scan_ports();
        watcher.update(&baseline);
        linfo!(
            "Baseline ({} port(s)): {}",
            baseline.len(),
            baseline
                .iter()
                .map(|p| p.label())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut last_key = String::new();
        loop {
            std::thread::sleep(Duration::from_millis(750));
            let current = scan_ports();
            for (kind, info) in watcher.update(&current) {
                linfo!("Event confirmed: {kind:?} {}", info.label());
                let _ = tx.send(UiMsg::Event(kind, info));
                let _ = proxy.send_event(());
            }
            let confirmed = watcher.confirmed_ports();
            let key = confirmed
                .iter()
                .map(|p| p.device.clone())
                .collect::<Vec<_>>()
                .join(",");
            if key != last_key {
                last_key = key;
                let _ = tx.send(UiMsg::PortsChanged(confirmed));
                let _ = proxy.send_event(());
            }
        }
    });

    let mut tray: Option<TrayIcon> = None;
    let mut ports: Vec<PortInfo> = Vec::new();
    let mut paused = false;
    let mut test_sent = false;
    let mut settings = Settings::load();
    let mut popups = PopupManager::new(&settings);
    if settings.custom_popups && !popups.available() {
        linfo!("Custom popups unavailable on this system — native notifications will be used");
    }

    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(120));

        // Poll / ResumeTimeReached mark WaitUntil-timer wakes (as
        // opposed to event-driven ones); popup lifetimes are
        // measured in this timer's time because the GTK backend
        // can starve it.
        if let Event::NewEvents(
            StartCause::Poll | StartCause::ResumeTimeReached { .. },
        ) = event
        {
            popups.note_timer_wake(Instant::now());
        }

        if let Event::NewEvents(StartCause::Init) = event {
            let icon = TrayIconBuilder::new()
                .with_menu(Box::new(build_menu(&ports, paused, &settings)))
                .with_tooltip("COM Port Watcher")
                .with_icon(make_icon())
                .build();
            match icon {
                Ok(t) => {
                    tray = Some(t);
                    linfo!("Tray icon running");
                }
                Err(e) => lwarn!("Could not create tray icon: {e}"),
            }
            if test_notification && !test_sent {
                test_sent = true;
                let (added, _removed) = fake_ports();
                dispatch(&mut popups, settings.custom_popups, PortEventKind::Added, &added);
            }
        }

        // Popup window events (clicks / close requests)
        if let Event::WindowEvent {
            window_id,
            event: ref win_event,
            ..
        } = event
        {
            popups.handle_window_event(window_id, win_event);
        }
        if let Event::RedrawRequested(window_id) = event {
            popups.repaint(window_id);
        }

        // Port events / menu updates from the worker
        while let Ok(msg) = rx.try_recv() {
            match msg {
                UiMsg::Event(kind, info) => {
                    if paused {
                        linfo!("(paused — notification suppressed for {})", info.device);
                    } else {
                        dispatch(&mut popups, settings.custom_popups, kind, &info);
                    }
                }
                UiMsg::PortsChanged(new_ports) => {
                    ports = new_ports;
                    if let Some(t) = &tray {
                        t.set_menu(Some(Box::new(build_menu(&ports, paused, &settings))));
                    }
                }
            }
        }

        // Tray menu clicks
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            match ev.id {
                id if id == MenuId::new(ID_QUIT) => {
                    linfo!("Quit requested from tray menu");
                    std::process::exit(0);
                }
                id if id == MenuId::new(ID_TEST) => {
                    let (added, _r) = fake_ports();
                    dispatch(&mut popups, settings.custom_popups, PortEventKind::Added, &added);
                }
                id if id == MenuId::new(ID_PAUSE) => {
                    paused = !paused;
                    linfo!("Notifications {}", if paused { "paused" } else { "resumed" });
                    if let Some(t) = &tray {
                        t.set_menu(Some(Box::new(build_menu(&ports, paused, &settings))));
                    }
                }
                id if id == MenuId::new(ID_POPUPS) => {
                    settings.custom_popups = !settings.custom_popups;
                    settings.save();
                    linfo!(
                        "Custom popups {}",
                        if settings.custom_popups { "on" } else { "off" }
                    );
                    if let Some(t) = &tray {
                        t.set_menu(Some(Box::new(build_menu(&ports, paused, &settings))));
                    }
                }
                id if DURATION_CHOICES.iter().any(|n| id == MenuId::new(duration_id(*n))) => {
                    if let Some(n) = DURATION_CHOICES
                        .iter()
                        .find(|n| id == MenuId::new(duration_id(**n)))
                    {
                        settings.popup_seconds = *n;
                        settings.save();
                        popups.set_settings(&settings);
                        linfo!("Popup time on screen: {} seconds", settings.popup_seconds);
                        if let Some(t) = &tray {
                            t.set_menu(Some(Box::new(build_menu(&ports, paused, &settings))));
                        }
                    }
                }
                id if CORNER_CHOICES.iter().any(|(c, _)| id == MenuId::new(corner_id(*c))) => {
                    if let Some((c, label)) = CORNER_CHOICES
                        .iter()
                        .find(|(c, _)| id == MenuId::new(corner_id(*c)))
                    {
                        settings.corner = *c;
                        settings.save();
                        popups.set_settings(&settings);
                        linfo!("Popup corner: {label}");
                        if let Some(t) = &tray {
                            t.set_menu(Some(Box::new(build_menu(&ports, paused, &settings))));
                        }
                    }
                }
                id if id == MenuId::new(ID_AUTOSTART) => {
                    let target = !crate::autostart::is_enabled();
                    match crate::autostart::set_enabled(target) {
                        Ok(()) => linfo!(
                            "Start at login {}",
                            if target { "enabled" } else { "disabled" }
                        ),
                        Err(e) => lwarn!("Start at login change failed: {e}"),
                    }
                    if let Some(t) = &tray {
                        t.set_menu(Some(Box::new(build_menu(&ports, paused, &settings))));
                    }
                }
                id if id == MenuId::new(ID_LOG) => open_log_file(),
                _ => {}
            }
        }

        // Popup lifecycle: expire old ones, show queued ones, keep
        // the stack anchored in the chosen corner.
        popups.tick(target);
        popups.pump(target);
        popups.restack(target);
    });
}
