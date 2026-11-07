//! COM Port Watcher — native Rust prototype (Windows / Linux / macOS)
//!
//! Same behaviour as the Python version it prototypes an alternative
//! to: silent baseline, debounced add/remove events (2 scans), tray
//! menu with current ports / test / pause / open log / quit, native
//! OS notifications instead of a custom popup, rotating-free single
//! log file next to the executable.
//!
//! Usage:
//!   com-port-watcher-rs                 tray app (default; on Unix it
//!                                       detaches into the background)
//!   com-port-watcher-rs --foreground    tray app, stay attached to the terminal
//!   com-port-watcher-rs --headless      no tray: log + notifications in foreground
//!   com-port-watcher-rs --list          print current serial ports and exit
//!   com-port-watcher-rs --self-check    enumerate once, verify the stack, exit
//!   com-port-watcher-rs --test-notification   send a fake notification (headless)
//!   com-port-watcher-rs --version

// Release builds are GUI-subsystem apps on Windows: no console window
// is allocated, so the tray app runs window-free. Debug builds keep
// the console (and its log output). Log lines still go to
// com_port_watcher_rs.log next to the executable either way.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appid;
mod core;
mod logfile;
mod notify;
#[cfg(feature = "tray")]
mod autostart;
#[cfg(feature = "tray")]
mod popup;
#[cfg(feature = "tray")]
mod settings;
#[cfg(feature = "tray")]
mod tray_app;

use core::{scan_ports, DebouncedWatcher, PortEventKind, PortInfo};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn fake_ports() -> (PortInfo, PortInfo) {
    let mk = |device: &str, desc: &str, vid: u16, pid: u16| {
        let mut p = PortInfo::new(device);
        p.description = desc.to_string();
        p.vid = Some(vid);
        p.pid = Some(pid);
        p
    };
    if cfg!(target_os = "macos") {
        let mut a = mk("/dev/cu.usbmodem999", "Test USB Serial Adapter", 0x303A, 0x1001);
        a.manufacturer = "Espressif".into();
        a.serial_number = "TEST123".into();
        (a, mk("/dev/cu.usbserial-0001", "USB-SERIAL CH340", 0x1A86, 0x7523))
    } else if cfg!(target_os = "linux") {
        let mut a = mk("/dev/ttyUSB99", "Test USB Serial Adapter", 0x303A, 0x1001);
        a.manufacturer = "Espressif".into();
        a.serial_number = "TEST123".into();
        (a, mk("/dev/ttyACM0", "USB-SERIAL CH340", 0x1A86, 0x7523))
    } else {
        let mut a = mk("COM99", "Test USB Serial Adapter", 0x303A, 0x1001);
        a.manufacturer = "Espressif".into();
        a.serial_number = "TEST123".into();
        (a, mk("COM4", "USB-SERIAL CH340", 0x1A86, 0x7523))
    }
}

/// Unix tray mode: detach from the launching terminal so no console
/// stays open behind the tray icon (Linux/macOS have no equivalent
/// of the Windows GUI-subsystem flag used in v0.1.1). Classic
/// double-fork: the parent prints a line and exits, the intermediate
/// child starts a new session and forks again so the daemon can
/// never reacquire a terminal, and the final child's stdin/stdout/
/// stderr go to /dev/null. Everything the app reports after this
/// point is in the log file anyway (crate::logfile). Returns only in
/// the daemon child; on any failure it warns and stays foreground.
#[cfg(unix)]
fn daemonize() {
    unsafe {
        match libc::fork() {
            -1 => eprintln!("warning: fork failed — staying in the foreground"),
            0 => {
                // First child: new session, then fork once more.
                let _ = libc::setsid();
                match libc::fork() {
                    -1 | 0 => {
                        // Daemon (or fallback): detach the std streams.
                        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
                        if devnull >= 0 {
                            let _ = libc::dup2(devnull, 0);
                            let _ = libc::dup2(devnull, 1);
                            let _ = libc::dup2(devnull, 2);
                            if devnull > 2 {
                                libc::close(devnull);
                            }
                        }
                    }
                    _ => std::process::exit(0), // intermediate parent
                }
            }
            pid => {
                println!("COM Port Watcher is running in the background (pid {pid})");
                std::process::exit(0);
            }
        }
    }
}

fn print_ports(ports: &[PortInfo]) {
    if ports.is_empty() {
        println!("No serial ports found.");
    }
    for p in ports {
        let detail = p.detail();
        if detail.is_empty() {
            println!("{}", p.label());
        } else {
            println!("{}  \u{b7}  {}", p.label(), detail);
        }
    }
}

fn run_headless(test_notification: bool) {
    if test_notification {
        let (added, removed) = fake_ports();
        notify::send(PortEventKind::Added, &added);
        notify::send(PortEventKind::Removed, &removed);
    }
    let mut watcher = DebouncedWatcher::new();
    let baseline = scan_ports();
    watcher.update(&baseline);
    linfo!(
        "Headless mode. Baseline ({} port(s)): {}",
        baseline.len(),
        baseline.iter().map(|p| p.label()).collect::<Vec<_>>().join(", ")
    );
    loop {
        std::thread::sleep(std::time::Duration::from_millis(750));
        let current = scan_ports();
        for (kind, info) in watcher.update(&current) {
            linfo!("Event confirmed: {kind:?} {}", info.label());
            notify::send(kind, &info);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    if has("--version") {
        println!("COM Port Watcher (Rust prototype) v{VERSION}");
        return;
    }
    if has("--help") || has("-h") {
        println!("Usage: com-port-watcher-rs [--headless] [--foreground] [--list] [--self-check] [--test-notification] [--version]");
        return;
    }
    if has("--list") {
        print_ports(&scan_ports());
        return;
    }

    // Unix tray mode detaches into the background by default so the
    // launching terminal is free again; --foreground opts out (the
    // log then also echoes to the console, as before). Headless and
    // --self-check always stay in the foreground.
    #[cfg(unix)]
    if cfg!(feature = "tray")
        && !has("--headless")
        && !has("--self-check")
        && !has("--foreground")
    {
        daemonize();
    }

    logfile::init();
    linfo!(
        "COM Port Watcher (Rust prototype) v{VERSION} starting — os={}, arch={}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    // Windows: claim our AppUserModelID so toasts post as
    // "COM Port Watcher", not "Windows PowerShell". No-op elsewhere.
    appid::register();

    if has("--self-check") {
        let ports = scan_ports();
        println!("self-check: enumeration OK ({} port(s) visible)", ports.len());
        print_ports(&ports);
        linfo!("Self-check passed");
        return;
    }

    if has("--headless") {
        run_headless(has("--test-notification"));
    } else {
        #[cfg(feature = "tray")]
        tray_app::run(has("--test-notification"));
        #[cfg(not(feature = "tray"))]
        {
            lwarn!("Built without the 'tray' feature — running headless");
            run_headless(has("--test-notification"));
        }
    }
}
