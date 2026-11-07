//! Platform-independent core: port model, OS filtering, natural sort,
//! and the debounced add/remove watcher. Direct port of the logic in
//! the Python version, with the same semantics (and equivalent tests):
//! silent baseline, an event fires only after CONFIRM_SCANS
//! consecutive scans agree, so ESP32-style re-enumeration flicker
//! produces no false events.

use std::collections::HashMap;

pub const CONFIRM_SCANS: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortInfo {
    pub device: String, // "COM7" / "/dev/ttyUSB0" / "/dev/cu.usbserial-1"
    pub description: String,
    pub manufacturer: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub serial_number: String,
}

impl PortInfo {
    pub fn new(device: &str) -> Self {
        PortInfo {
            device: device.to_string(),
            description: String::new(),
            manufacturer: String::new(),
            vid: None,
            pid: None,
            serial_number: String::new(),
        }
    }

    /// "COM7 — USB Serial Device (VID:PID=303A:1001)"
    pub fn label(&self) -> String {
        let mut text = self.device.clone();
        if !self.description.is_empty() && self.description != self.device {
            text += &format!(" \u{2014} {}", self.description);
        }
        if let (Some(vid), Some(pid)) = (self.vid, self.pid) {
            text += &format!(" (VID:PID={vid:04X}:{pid:04X})");
        }
        text
    }

    /// "Espressif  ·  S/N ABC123" (empty when neither is known)
    pub fn detail(&self) -> String {
        let mut bits = Vec::new();
        if !self.manufacturer.is_empty() {
            bits.push(self.manufacturer.clone());
        }
        if !self.serial_number.is_empty() {
            bits.push(format!("S/N {}", self.serial_number));
        }
        bits.join("  \u{b7}  ")
    }
}

// ---------------------------------------------------------------------------
// Natural sort: "COM2" < "COM10", "/dev/ttyUSB2" < "/dev/ttyUSB10"
// ---------------------------------------------------------------------------

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum SortPart {
    Num(u64),
    Str(String),
}

fn natural_key(device: &str) -> Vec<SortPart> {
    let mut parts = Vec::new();
    let mut buf = String::new();
    let mut in_num = false;
    for ch in device.chars() {
        if ch.is_ascii_digit() == in_num && !buf.is_empty() {
            // same class, keep accumulating
        } else if !buf.is_empty() {
            parts.push(part_of(&buf, in_num));
            buf.clear();
        }
        in_num = ch.is_ascii_digit();
        buf.push(ch);
    }
    if !buf.is_empty() {
        parts.push(part_of(&buf, in_num));
    }
    parts
}

fn part_of(buf: &str, is_num: bool) -> SortPart {
    if is_num {
        SortPart::Num(buf.parse().unwrap_or(u64::MAX))
    } else {
        SortPart::Str(buf.to_string())
    }
}

pub fn sorted_ports(ports: &[PortInfo]) -> Vec<PortInfo> {
    let mut out = ports.to_vec();
    out.sort_by(|a, b| natural_key(&a.device).cmp(&natural_key(&b.device)));
    out
}

// ---------------------------------------------------------------------------
// OS filtering — which enumerated ports are worth reporting
// ---------------------------------------------------------------------------

/// Pure/platform-parameterised so it is testable on any host.
/// `os` is std::env::consts::OS style: "windows" | "linux" | "macos".
pub fn include_port_for(os: &str, device: &str, description: &str, vid: Option<u16>) -> bool {
    match os {
        "linux" => {
            let base = device.rsplit('/').next().unwrap_or(device);
            if vid.is_some() {
                return true; // anything carrying a USB VID:PID
            }
            ["ttyUSB", "ttyACM", "ttyXRUSB", "ttyAMA", "rfcomm"]
                .iter()
                .any(|p| base.starts_with(p))
        }
        "macos" => {
            let lower_dev = device.to_lowercase();
            let lower_desc = description.to_lowercase();
            if lower_dev.contains("bluetooth") || lower_desc.contains("bluetooth") {
                return false;
            }
            let base = device.rsplit('/').next().unwrap_or(device);
            base.starts_with("cu.") // callout device, not the tty.* twin
        }
        _ => true, // windows: every COM port
    }
}

pub fn include_port(device: &str, description: &str, vid: Option<u16>) -> bool {
    include_port_for(std::env::consts::OS, device, description, vid)
}

// ---------------------------------------------------------------------------
// OS scan via the `serialport` crate (SetupAPI / sysfs+udev / IOKit)
// ---------------------------------------------------------------------------

pub fn scan_ports() -> Vec<PortInfo> {
    let mut out: Vec<PortInfo> = Vec::new();
    let found = match serialport::available_ports() {
        Ok(f) => f,
        Err(e) => {
            crate::lwarn!("available_ports failed: {e}");
            return out;
        }
    };
    for p in found {
        let mut info = PortInfo::new(&p.port_name);
        if let serialport::SerialPortType::UsbPort(usb) = &p.port_type {
            info.vid = Some(usb.vid);
            info.pid = Some(usb.pid);
            info.description = usb.product.clone().unwrap_or_default().trim().to_string();
            info.manufacturer = usb.manufacturer.clone().unwrap_or_default().trim().to_string();
            info.serial_number = usb.serial_number.clone().unwrap_or_default().trim().to_string();
        }
        if include_port(&info.device, &info.description, info.vid) {
            out.push(info);
        }
    }
    // macOS safety net: if both twins slipped through, keep the cu.* one.
    let devices: Vec<String> = out.iter().map(|p| p.device.clone()).collect();
    out.retain(|p| {
        if let Some(rest) = p.device.strip_prefix("/dev/tty.") {
            !devices.iter().any(|d| d == &format!("/dev/cu.{rest}"))
        } else {
            true
        }
    });
    sorted_ports(&out)
}

// ---------------------------------------------------------------------------
// Debounced watcher
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortEventKind {
    Added,
    Removed,
}

#[derive(Debug)]
pub struct DebouncedWatcher {
    confirm_scans: u32,
    confirmed: HashMap<String, PortInfo>,
    pending_add: HashMap<String, u32>,
    pending_remove: HashMap<String, u32>,
    baselined: bool,
}

impl Default for DebouncedWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl DebouncedWatcher {
    pub fn new() -> Self {
        DebouncedWatcher {
            confirm_scans: CONFIRM_SCANS,
            confirmed: HashMap::new(),
            pending_add: HashMap::new(),
            pending_remove: HashMap::new(),
            baselined: false,
        }
    }

    pub fn confirmed_ports(&self) -> Vec<PortInfo> {
        sorted_ports(&self.confirmed.values().cloned().collect::<Vec<_>>())
    }

    /// Feed one scan; returns confirmed (kind, port) events.
    pub fn update(&mut self, current: &[PortInfo]) -> Vec<(PortEventKind, PortInfo)> {
        if !self.baselined {
            self.confirmed = current.iter().map(|p| (p.device.clone(), p.clone())).collect();
            self.baselined = true;
            return Vec::new();
        }
        let current_map: HashMap<&str, &PortInfo> =
            current.iter().map(|p| (p.device.as_str(), p)).collect();
        let mut events = Vec::new();

        // Additions
        for info in current {
            if self.confirmed.contains_key(&info.device) {
                self.pending_add.remove(&info.device);
                continue;
            }
            let count = self.pending_add.get(&info.device).copied().unwrap_or(0) + 1;
            if count >= self.confirm_scans {
                self.confirmed.insert(info.device.clone(), info.clone());
                self.pending_add.remove(&info.device);
                events.push((PortEventKind::Added, info.clone()));
            } else {
                self.pending_add.insert(info.device.clone(), count);
            }
        }
        self.pending_add.retain(|dev, _| current_map.contains_key(dev.as_str()));

        // Removals
        let confirmed_snapshot: Vec<PortInfo> = self.confirmed.values().cloned().collect();
        for info in confirmed_snapshot {
            if current_map.contains_key(info.device.as_str()) {
                self.pending_remove.remove(&info.device);
                continue;
            }
            let count = self.pending_remove.get(&info.device).copied().unwrap_or(0) + 1;
            if count >= self.confirm_scans {
                self.confirmed.remove(&info.device);
                self.pending_remove.remove(&info.device);
                events.push((PortEventKind::Removed, info));
            } else {
                self.pending_remove.insert(info.device.clone(), count);
            }
        }
        events
    }
}

// ---------------------------------------------------------------------------
// Tests — the same cases the Python suite covers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: u32) -> PortInfo {
        let mut info = PortInfo::new(&format!("COM{n}"));
        info.description = "USB Serial Device".into();
        info.vid = Some(0x303A);
        info.pid = Some(0x1001);
        info
    }
    fn kinds(events: &[(PortEventKind, PortInfo)]) -> Vec<(PortEventKind, String)> {
        events.iter().map(|(k, i)| (*k, i.device.clone())).collect()
    }

    #[test]
    fn debounce_flow() {
        let mut w = DebouncedWatcher::new();
        assert!(w.update(&[p(3)]).is_empty(), "baseline silent");
        assert!(w.update(&[p(3), p(7)]).is_empty(), "add needs 2 scans");
        assert_eq!(
            kinds(&w.update(&[p(3), p(7)])),
            vec![(PortEventKind::Added, "COM7".to_string())]
        );
        assert!(w.update(&[p(3), p(7)]).is_empty(), "no repeats");
        assert!(w.update(&[p(3)]).is_empty(), "flicker 1 silent");
        assert!(w.update(&[p(3), p(7)]).is_empty(), "flicker recovery silent");
        assert!(w.update(&[p(3)]).is_empty(), "remove needs 2 scans");
        assert_eq!(
            kinds(&w.update(&[p(3)])),
            vec![(PortEventKind::Removed, "COM7".to_string())]
        );
    }

    #[test]
    fn label_format() {
        assert_eq!(p(7).label(), "COM7 \u{2014} USB Serial Device (VID:PID=303A:1001)");
        let bare = PortInfo::new("COM4");
        assert_eq!(bare.label(), "COM4");
    }

    #[test]
    fn filtering() {
        assert!(include_port_for("linux", "/dev/ttyUSB0", "", None));
        assert!(include_port_for("linux", "/dev/ttyACM1", "", None));
        assert!(!include_port_for("linux", "/dev/ttyS0", "", None));
        assert!(include_port_for("linux", "/dev/ttyS9", "", Some(0x0403)));
        assert!(include_port_for("macos", "/dev/cu.usbserial-110", "", Some(0x10C4)));
        assert!(!include_port_for("macos", "/dev/tty.usbserial-110", "", Some(0x10C4)));
        assert!(!include_port_for("macos", "/dev/cu.Bluetooth-Incoming-Port", "Bluetooth", None));
        assert!(include_port_for("windows", "COM12", "USB Serial Device", None));
    }

    #[test]
    fn natural_sort() {
        let ports = vec![p(10), p(2), p(1)];
        let devs: Vec<String> = sorted_ports(&ports).iter().map(|x| x.device.clone()).collect();
        assert_eq!(devs, vec!["COM1", "COM2", "COM10"]);
    }
}
