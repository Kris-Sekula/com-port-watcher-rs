//! Linux popup backend: raw X11 windows via x11rb — no toolkit
//! between us and the server, so size, position, mapping and
//! destruction are exactly what the code says. (The tao/GTK
//! backend allocated popup windows at a fallback 200 px height
//! and refused resizes under Xvfb; this backend exists because of
//! that fight.)
//!
//! Rendering happens on the tray thread (shared code in the
//! parent module); a dedicated X thread owns the connection and
//! the windows: it maps them bottom-right, re-puts the image on
//! Expose, destroys on click (reporting the copy text back) or
//! after the auto-close deadline.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tao::event_loop::EventLoopWindowTarget;
use tao::window::WindowId;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, ImageFormat,
    WindowClass,
};
use x11rb::protocol::Event as XEvent;
use x11rb::rust_connection::RustConnection;
use x11rb::COPY_DEPTH_FROM_PARENT;

use super::{
    compute_layout, content_for, corner_position, render, Fonts,
    MAX_VISIBLE, STACK_GAP,
};
use crate::core::{PortEventKind, PortInfo};
use crate::settings::{Corner, Settings};
use crate::{linfo, lwarn};

struct ShownBuf {
    bytes: Vec<u8>,
    w: u32,
    h: u32,
    copy_text: String,
    autoclose: Duration,
}

enum Cmd {
    Show(ShownBuf),
    SetCorner(Corner),
}

pub struct PopupManager {
    fonts: Option<Fonts>,
    cmd_tx: Option<mpsc::Sender<Cmd>>,
    alive: Arc<AtomicBool>,
    settings: Settings,
}

impl PopupManager {
    pub fn new(settings: &Settings) -> PopupManager {
        let fonts = Fonts::load();
        let alive = Arc::new(AtomicBool::new(false));
        let cmd_tx = if fonts.is_some() {
            let (tx, rx) = mpsc::channel::<Cmd>();
            let (init_tx, init_rx) = mpsc::channel::<bool>();
            let alive2 = alive.clone();
            let corner = settings.corner;
            std::thread::spawn(move || x11_thread(rx, alive2, init_tx, corner));
            match init_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(true) => Some(tx),
                _ => {
                    lwarn!("X11 popup backend did not come up — native notifications");
                    None
                }
            }
        } else {
            None
        };
        PopupManager {
            fonts,
            cmd_tx,
            alive,
            settings: settings.clone(),
        }
    }

    /// Apply new user settings: the duration rides along with
    /// each new popup; a corner change re-anchors the visible
    /// stack immediately via the X thread.
    pub fn set_settings(&mut self, settings: &Settings) {
        let corner_changed = settings.corner != self.settings.corner;
        self.settings = settings.clone();
        if corner_changed {
            if let Some(tx) = self.cmd_tx.as_ref() {
                let _ = tx.send(Cmd::SetCorner(settings.corner));
            }
        }
    }

    pub fn available(&self) -> bool {
        self.cmd_tx.is_some() && self.alive.load(Ordering::Relaxed)
    }

    pub fn enqueue(&mut self, kind: PortEventKind, info: &PortInfo) -> bool {
        let (Some(fonts), Some(tx)) = (self.fonts.as_mut(), self.cmd_tx.as_ref()) else {
            return false;
        };
        if !self.alive.load(Ordering::Relaxed) {
            return false;
        }
        let content = content_for(kind, info);
        let layout = compute_layout(fonts, &content, 1.0);
        let (pixels, w, h) = render(fonts, &content, &layout, 1.0);
        // X11 ZPixmap on a little-endian TrueColor visual wants
        // B,G,R,X byte order — exactly the little-endian bytes of
        // our 0x00RRGGBB pixels.
        let mut bytes = Vec::with_capacity(pixels.len() * 4);
        for p in &pixels {
            bytes.extend_from_slice(&p.to_le_bytes());
        }
        let buf = ShownBuf {
            bytes,
            w,
            h,
            copy_text: content.copy_text.clone(),
            autoclose: Duration::from_secs(self.settings.popup_seconds),
        };
        match tx.send(Cmd::Show(buf)) {
            Ok(()) => true,
            Err(_) => {
                self.cmd_tx = None;
                false
            }
        }
    }

    // The X thread owns the whole popup lifecycle (show, expire,
    // click + clipboard), so the tray-loop hooks are no-ops here.
    pub fn pump(&mut self, _target: &EventLoopWindowTarget<()>) {}
    pub fn tick(&mut self, _target: &EventLoopWindowTarget<()>) -> bool {
        false
    }
    pub fn restack(&mut self, _target: &EventLoopWindowTarget<()>) {}
    pub fn repaint(&mut self, _window_id: WindowId) {}
    pub fn note_timer_wake(&mut self, _now: Instant) {}
    pub fn handle_window_event(
        &mut self,
        _window_id: WindowId,
        _event: &tao::event::WindowEvent,
    ) -> bool {
        false // no tao windows on this backend
    }
}

/// Clipboard copy, run on the X thread at click time. The
/// arboard instance is kept alive for the thread's lifetime: on
/// Linux the selection is served by the owning instance, and
/// dropping it makes the copied text unreadable (verified under
/// Xvfb: set-then-drop → xclip finds no owner).
fn copy_to_clipboard(
    text: &str,
    clipboard: &mut Option<arboard::Clipboard>,
    clipboard_ok: &mut bool,
) {
    if !*clipboard_ok {
        return;
    }
    if clipboard.is_none() {
        match arboard::Clipboard::new() {
            Ok(cb) => *clipboard = Some(cb),
            Err(e) => {
                lwarn!("Clipboard unavailable: {e}");
                *clipboard_ok = false;
                return;
            }
        }
    }
    if let Some(cb) = clipboard.as_mut() {
        match cb.set_text(text.to_string()) {
            Ok(()) => linfo!("Popup clicked — copied {text}"),
            Err(e) => {
                lwarn!("Clipboard set failed: {e}");
                *clipboard_ok = false;
            }
        }
    }
}

struct XPopup {
    win: u32,
    buf: ShownBuf,
    deadline: Instant,
}

fn x11_thread(
    cmd_rx: mpsc::Receiver<Cmd>,
    alive: Arc<AtomicBool>,
    init_tx: mpsc::Sender<bool>,
    mut corner: Corner,
) {
    let (conn, screen_num) = match x11rb::connect(None::<&str>) {
        Ok(pair) => pair,
        Err(e) => {
            lwarn!("X11 connect failed for popups: {e}");
            let _ = init_tx.send(false);
            return;
        }
    };
    let setup = conn.setup().clone();
    let screen = &setup.roots[screen_num];
    let root = screen.root;
    let root_depth = screen.root_depth;
    let visual = screen.root_visual;
    let (screen_w, screen_h) = (
        i32::from(screen.width_in_pixels),
        i32::from(screen.height_in_pixels),
    );
    let gc = conn.generate_id().unwrap_or(0);
    if conn.create_gc(gc, root, &CreateGCAux::new()).is_err() {
        let _ = init_tx.send(false);
        return;
    }
    alive.store(true, Ordering::Relaxed);
    let _ = init_tx.send(true);
    linfo!("X11 popup backend ready");

    let mut visible: Vec<XPopup> = Vec::new();
    let mut queued: Vec<ShownBuf> = Vec::new();
    let mut clipboard_ok = true;
    let mut clipboard: Option<arboard::Clipboard> = None;

    'outer: loop {
        // Commands from the tray thread.
        loop {
            match cmd_rx.try_recv() {
                Ok(Cmd::SetCorner(new_corner)) => {
                    corner = new_corner;
                    restack(&conn, corner, screen_w, screen_h, &visible);
                }
                Ok(Cmd::Show(buf)) => {
                    if visible.len() < MAX_VISIBLE {
                        if let Some(p) = create_popup(&conn, root, visual, buf) {
                            let win = p.win;
                            visible.push(p);
                            restack(&conn, corner, screen_w, screen_h, &visible);
                            let _ = conn.map_window(win);
                            if let Some(p) =
                                visible.iter().find(|p| p.win == win)
                            {
                                put_pixels(&conn, gc, root_depth, p);
                            }
                            let _ = conn.flush();
                        }
                    } else {
                        queued.push(buf);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'outer,
            }
        }
        // X events (non-blocking drain).
        loop {
            match conn.poll_for_event() {
                Ok(Some(XEvent::Expose(ev))) => {
                    if ev.count == 0 {
                        if let Some(p) = visible.iter().find(|p| p.win == ev.window) {
                            put_pixels(&conn, gc, root_depth, p);
                        }
                    }
                }
                Ok(Some(XEvent::ButtonPress(ev))) => {
                    if let Some(idx) =
                        visible.iter().position(|p| p.win == ev.event)
                    {
                        let p = visible.remove(idx);
                        copy_to_clipboard(
                            &p.buf.copy_text,
                            &mut clipboard,
                            &mut clipboard_ok,
                        );
                        let _ = conn.destroy_window(p.win);
                        restack(&conn, corner, screen_w, screen_h, &visible);
                        promote(&conn, gc, root, root_depth, visual, corner, screen_w, screen_h, &mut visible, &mut queued);
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    lwarn!("X11 event error for popups: {e}");
                    break 'outer;
                }
            }
        }
        // Expiry.
        let now = Instant::now();
        let before = visible.len();
        let expired: Vec<u32> = visible
            .iter()
            .filter(|p| p.deadline <= now)
            .map(|p| p.win)
            .collect();
        if !expired.is_empty() {
            visible.retain(|p| p.deadline > now);
            for win in expired {
                let _ = conn.destroy_window(win);
            }
        }
        if visible.len() != before {
            restack(&conn, corner, screen_w, screen_h, &visible);
            promote(&conn, gc, root, root_depth, visual, corner, screen_w, screen_h, &mut visible, &mut queued);
        }
        let _ = conn.flush();
        std::thread::sleep(Duration::from_millis(40));
    }
    alive.store(false, Ordering::Relaxed);
}

/// Create (but do not map) a popup window; the caller positions
/// the whole stack with restack(), then maps and paints.
fn create_popup(
    conn: &RustConnection,
    root: u32,
    visual: u32,
    buf: ShownBuf,
) -> Option<XPopup> {
    let win = conn.generate_id().ok()?;
    let aux = CreateWindowAux::new()
        .background_pixel(super::rgb(super::BODY_BG))
        .override_redirect(1u32)
        .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS);
    if conn
        .create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            root,
            0,
            0,
            buf.w as u16,
            buf.h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &aux,
        )
        .is_err()
    {
        return None;
    }
    let autoclose = buf.autoclose;
    Some(XPopup {
        win,
        buf,
        deadline: Instant::now() + autoclose,
    })
}

#[allow(clippy::too_many_arguments)]
fn promote(
    conn: &RustConnection,
    gc: u32,
    root: u32,
    root_depth: u8,
    visual: u32,
    corner: Corner,
    screen_w: i32,
    screen_h: i32,
    visible: &mut Vec<XPopup>,
    queued: &mut Vec<ShownBuf>,
) {
    let mut added = Vec::new();
    while visible.len() < MAX_VISIBLE && !queued.is_empty() {
        let buf = queued.remove(0);
        match create_popup(conn, root, visual, buf) {
            Some(p) => {
                added.push(p.win);
                visible.push(p);
            }
            None => break,
        }
    }
    if !added.is_empty() {
        restack(conn, corner, screen_w, screen_h, visible);
        for win in added {
            let _ = conn.map_window(win);
            if let Some(p) = visible.iter().find(|p| p.win == win) {
                put_pixels(conn, gc, root_depth, p);
            }
        }
        let _ = conn.flush();
    }
}

fn restack(
    conn: &RustConnection,
    corner: Corner,
    screen_w: i32,
    screen_h: i32,
    visible: &[XPopup],
) {
    // Newest popup sits closest to the corner (same convention as
    // the Windows backend); the shared corner_position() does the
    // math so both backends place cards identically.
    let mut closer_h = 0i32;
    for p in visible.iter().rev() {
        let (x, y) = corner_position(
            corner,
            screen_w,
            screen_h,
            p.buf.w as i32,
            p.buf.h as i32,
            closer_h,
        );
        let _ = conn.configure_window(
            p.win,
            &x11rb::protocol::xproto::ConfigureWindowAux::new()
                .x(x)
                .y(y),
        );
        closer_h += p.buf.h as i32 + STACK_GAP;
    }
    let _ = conn.flush();
}

fn put_pixels(conn: &RustConnection, gc: u32, depth: u8, popup: &XPopup) {
    let (w, h) = (popup.buf.w as usize, popup.buf.h as usize);
    if gc == 0 || w == 0 || h == 0 {
        return;
    }
    // Chunk rows so one request stays well under the server's
    // maximum request length.
    let rows_per_chunk = (60_000 / (w * 4)).max(1);
    let mut row = 0usize;
    while row < h {
        let rows = rows_per_chunk.min(h - row);
        let start = row * w * 4;
        let end = (row + rows) * w * 4;
        let _ = conn.put_image(
            ImageFormat::Z_PIXMAP,
            popup.win,
            gc,
            w as u16,
            rows as u16,
            0,
            row as i16,
            0,
            depth,
            &popup.buf.bytes[start..end],
        );
        row += rows;
    }
}
