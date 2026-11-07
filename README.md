# COM Port Watcher

A small cross-platform tray utility, written in Rust, that watches
serial ports and pops up a card the moment one appears or
disappears — so plugging in a USB-to-serial adapter no longer
means a trip to Device Manager (or `ls /dev/tty*`) just to learn
which port it got.

One codebase builds for Windows, Linux and macOS. The Linux
release binary is about 3.2 MB.

**Platform status:** Windows and Linux (Debian/Mint) are tested
and in daily use. **The macOS version is not tested** — it is
written against the same APIs as the other two builds, but nobody
on this project has a Mac, so its first build there is the test.

## Features

- **Port watching.** Polls the OS port list every 750 ms
  (SetupAPI on Windows, sysfs on Linux, IOKit on macOS). The ports
  present at startup form a silent baseline — you are only told
  about changes. An add or remove must appear in 2 consecutive
  scans before it is reported (~1.5 s debounce), which filters
  out the flicker of devices that re-enumerate while resetting
  (ESP32 boards in download mode, for example).
- **Per-OS filtering.** Linux ignores the dummy `ttyS*` ports,
  macOS reports the `cu.*` devices only (not their `tty.*` twins)
  and ignores Bluetooth virtual ports, Windows reports all COM
  ports. Ports are listed in natural order (COM2 before COM10).
- **Custom popups** (Windows and Linux). A borderless card in a
  screen corner: green header for "added", orange for "removed",
  the port line in bold — e.g. `COM8 — USB Serial Device
  (VID:PID=303A:1001)` or `/dev/ttyACM0 — USB JTAG/serial debug
  unit (VID:PID=303A:1001)` — and a detail line with the
  manufacturer and serial number. The card closes itself after a
  configurable time; **clicking it copies the port name** (just
  `COM8`, just `/dev/ttyACM0`) to the clipboard and closes it.
  Up to three cards stack; further events queue behind them.
- **Native notifications as fallback.** If the popup surface is
  unavailable (or switched off), events go out as ordinary OS
  notifications: Windows toasts — posted under the app's own
  registered identity (AppUserModelID `Kris.ComPortWatcher`), so
  they are headed "COM Port Watcher", not "Windows PowerShell" —
  Linux desktop notifications, and macOS Notification Center
  banners, which are also the macOS event surface by design.
- **Settings in the tray menu.** *Popup time on screen*:
  3 / 5 / 7 / 10 / 15 / 30 seconds. *Popup corner*: any of the
  four screen corners; changing it moves already-visible cards.
  Both apply to the popup surface (native notifications keep the
  behaviour of the OS notification system).
- **Start at login.** A tray checkbox registers the app with the
  OS: a per-user Run key value on Windows,
  `~/.config/autostart/com-port-watcher-rs.desktop` on Linux
  (honours `XDG_CONFIG_HOME`), a LaunchAgent plist in
  `~/Library/LaunchAgents` on macOS. The checkbox always shows
  the live state read back from the OS.
- **Log file.** Everything is also written to
  `com_port_watcher_rs.log` next to the executable; the tray menu
  has an "Open log file" item.
- **Pause.** A tray checkbox silences notifications (events are
  still logged) until you resume.
- **Headless mode.** `--headless` runs the watcher with no tray
  or windows — log file plus native notifications — and a build
  with `--no-default-features` drops the GUI dependencies
  entirely.

## Installing

There are no prebuilt downloads; you build it once per OS with
the Rust toolchain from https://rustup.rs. Quit any running
instance before rebuilding — the executable is locked while it
runs.

### Windows

```
build_windows.bat            ->  target\release\com-port-watcher-rs.exe
```
(or `cargo build --release` in the project folder.)

Toolchain notes:
- The MSVC toolchain (rustup's default) needs the Visual Studio
  Build Tools with the "Desktop development with C++" workload,
  for `link.exe`.
- The GNU toolchain (`rustup default stable-x86_64-pc-windows-gnu`)
  links without Visual Studio, but the `windows-*` crates run
  `dlltool.exe` in their build scripts: `rustup component add
  rust-mingw` provides one, or install MSYS2 and
  `pacman -S mingw-w64-x86_64-binutils` and put
  `C:\msys64\mingw64\bin` on PATH.
- The release build is a GUI-subsystem app: no console window
  stays open behind the tray icon.

### Linux

```sh
sudo apt install libgtk-3-dev libayatana-appindicator3-dev libxdo-dev pkg-config   # Debian/Ubuntu build deps
./build_linux.sh             ->  target/release/com-port-watcher-rs
```
The `-dev` packages are build-time only; the binary runs against
the desktop's ordinary GTK/AppIndicator libraries. If you copy a
prebuilt binary to a machine that has never built it, it also
needs the xdo runtime library: `sudo apt install libxdo3`.

Launching the binary from a terminal detaches it into the
background (the tray icon stays; the terminal is free). Use
`--foreground` to keep it attached, and stop it from the tray
menu's Quit or with `pkill com-port-watcher-rs`.

Popups on Linux are drawn as raw X11 windows. Under a Wayland
session they go through XWayland and the compositor has the
final say on placement.

### macOS — not tested

On a Mac with the Xcode Command Line Tools:

```sh
./build_macos.sh             ->  target/release/com-port-watcher-rs
```

**This build has never been run on a Mac** (see the platform
status note at the top). It is expected to work as a menu-bar
app with Notification Center banners; the binary is unpackaged
and unsigned, and `.app` bundling is not set up.

## Using it

Run the executable; a plug icon appears in the tray / menu bar.
Nothing pops up for the ports already plugged in — only changes
are reported.

Tray menu:

- the ports currently present (informational)
- **Test notification** — shows a sample event so you can see the
  surface and check placement
- **Pause notifications**
- **Custom popups** — switch between the custom cards and native
  OS notifications
- **Settings** — *Popup time on screen* and *Popup corner*
- **Start at login**
- **Open log file**
- **Quit**

Settings are stored in `com_port_watcher_rs.cfg` next to the
executable, as plain `key=value` lines you can also edit by hand:

```
popup_seconds=7
popup_corner=bottom-right
custom_popups=true
```

`popup_corner` accepts `bottom-right`, `bottom-left`,
`top-right`, `top-left`. Unknown keys and bad values are ignored
and fall back to defaults, so a hand-edited file can't break
startup.

Command line:

```
com-port-watcher-rs                     # tray app (Unix: detaches to background)
com-port-watcher-rs --foreground        # tray app, stay in the terminal
com-port-watcher-rs --test-notification # tray app + one sample event at startup
com-port-watcher-rs --list              # print the ports visible right now
com-port-watcher-rs --self-check        # enumerate once and report
com-port-watcher-rs --headless          # no tray; log + native notifications
com-port-watcher-rs --version
```

## Source layout

```
src/main.rs       CLI modes + headless loop
src/core.rs       port model, OS filtering, natural sort, debounced watcher
src/tray_app.rs   tray event loop + menu, scan worker thread
src/notify.rs     native notifications (notify-rust)
src/popup.rs      shared popup core: system fonts, text layout, rendering
src/popup_win.rs  Windows popup backend (tao window + softbuffer)
src/popup_x11.rs  Linux popup backend (raw X11 windows, own thread)
src/popup_stub.rs popup stub for macOS (native banners instead)
src/settings.rs   persisted settings (the .cfg file)
src/autostart.rs  Start-at-login registration per OS
src/appid.rs      Windows AppUserModelID registration (no-op elsewhere)
src/logfile.rs    dependency-free file logger
assets/           toast/app icon, drawn by tools/make_icon.py
```

`cargo test` runs the unit tests (18: watcher debounce and
filtering, text wrapping, corner maths, settings parsing,
autostart entry files).
