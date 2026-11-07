//! Windows popup backend: tao windows presented via softbuffer.
//! All methods run on the tray event-loop thread.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tao::dpi::{LogicalSize, PhysicalPosition, PixelUnit};
use tao::event_loop::EventLoopWindowTarget;
use tao::window::{Window, WindowId, WindowSizeConstraints};

use super::{
    compute_layout, content_for, corner_position, render, Fonts, Layout,
    PopupContent, MAX_VISIBLE, STACK_GAP, WIDTH,
};
use crate::core::{PortEventKind, PortInfo};
use crate::settings::Settings;
use crate::{linfo, lwarn};

struct ActivePopup {
    id: WindowId,
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    content: PopupContent,
    layout: Layout,
    scale: f32,
    created: Instant,
    deadline: Instant,
    repainted: bool,
    drift_logged: bool,
}

pub struct PopupManager {
    fonts: Option<Fonts>,
    visible: Vec<ActivePopup>,
    queued: Vec<PopupContent>,
    clipboard_ok: bool,
    clipboard: Option<arboard::Clipboard>,
    last_timer_wake: Instant,
    settings: Settings,
}

impl PopupManager {
    pub fn new(settings: &Settings) -> PopupManager {
        PopupManager {
            fonts: Fonts::load(),
            visible: Vec::new(),
            queued: Vec::new(),
            clipboard_ok: true,
            clipboard: None,
            last_timer_wake: Instant::now(),
            settings: settings.clone(),
        }
    }

    /// Apply new user settings (duration applies to popups shown
    /// from now on; the corner re-anchors the stack on the next
    /// restack, which happens on the next loop wake).
    pub fn set_settings(&mut self, settings: &Settings) {
        self.settings = settings.clone();
    }

    /// True when popups can be shown at all (fonts loaded).
    pub fn available(&self) -> bool {
        self.fonts.is_some()
    }

    /// Queue an event for display. Returns false when popups are
    /// unavailable and the caller should use a native notification.
    pub fn enqueue(&mut self, kind: PortEventKind, info: &PortInfo) -> bool {
        if self.fonts.is_none() {
            return false;
        }
        self.queued.push(content_for(kind, info));
        true
    }

    /// Create windows for queued popups while slots are free.
    /// Called on every event-loop wake. A popup that fails to be
    /// created or painted disables popups and is re-sent as a
    /// native notification, so no event is ever lost to a rendering
    /// problem.
    pub fn pump(&mut self, target: &EventLoopWindowTarget<()>) {
        while self.visible.len() < MAX_VISIBLE && !self.queued.is_empty() {
            let content = self.queued.remove(0);
            let fallback_title = format!("{} — {}", content.header, content.title);
            let mut created = match self.create_window(target, content) {
                Ok(p) => p,
                Err(e) => {
                    lwarn!("Popup window failed ({e}) — native notifications from now on");
                    self.fonts = None;
                    crate::notify::send_raw(&fallback_title, "");
                    self.drain_queue_to_native();
                    return;
                }
            };
            let painted = match self.fonts.as_mut() {
                Some(fonts) => paint(fonts, &mut created),
                None => Err("fonts unavailable".to_string()),
            };
            match painted {
                Ok(()) => {
                    self.visible.push(created);
                    self.restack(target);
                }
                Err(e) => {
                    lwarn!("Popup paint failed ({e}) — native notifications from now on");
                    self.fonts = None;
                    crate::notify::send_raw(&fallback_title, "");
                    self.drain_queue_to_native();
                    return;
                }
            }
        }
    }

    /// After a fatal popup failure, flush anything still queued to
    /// native notifications so no event is silently dropped.
    fn drain_queue_to_native(&mut self) {
        for content in self.queued.drain(..) {
            crate::notify::send_raw(
                &format!("{} — {}", content.header, content.title),
                "",
            );
        }
    }

    fn create_window(
        &mut self,
        target: &EventLoopWindowTarget<()>,
        content: PopupContent,
    ) -> Result<ActivePopup, String> {
        let fonts = self.fonts.as_mut().ok_or("fonts unavailable")?;
        let scale = target
            .primary_monitor()
            .map(|m| m.scale_factor() as f32)
            .unwrap_or(1.0);
        let layout = compute_layout(fonts, &content, scale);
        let height_logical = layout.height_logical;
        // Pin min == max == target size so the window can't be
        // re-sized to something the layout didn't plan for.
        let pin = WindowSizeConstraints {
            min_width: Some(PixelUnit::Logical(WIDTH.into())),
            min_height: Some(PixelUnit::Logical(height_logical.into())),
            max_width: Some(PixelUnit::Logical(WIDTH.into())),
            max_height: Some(PixelUnit::Logical(height_logical.into())),
        };
        let window = Rc::new(
            tao::window::WindowBuilder::new()
                .with_title("COM Port Watcher")
                .with_decorations(false)
                .with_always_on_top(true)
                .with_resizable(false)
                .with_focused(false)
                .with_inner_size(LogicalSize::new(WIDTH, height_logical))
                .with_inner_size_constraints(pin)
                .build(target)
                .map_err(|e| e.to_string())?,
        );
        let context =
            softbuffer::Context::new(window.clone()).map_err(|e| e.to_string())?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| e.to_string())?;
        Ok(ActivePopup {
            id: window.id(),
            window,
            surface,
            content,
            layout,
            scale,
            created: Instant::now(),
            deadline: Instant::now()
                + Duration::from_secs(self.settings.popup_seconds),
            repainted: false,
            drift_logged: false,
        })
    }

    pub fn restack(&mut self, target: &EventLoopWindowTarget<()>) {
        let Some(monitor) = target.primary_monitor() else {
            return;
        };
        let size = monitor.size();
        // Newest popup sits closest to the corner; the stack grows
        // away from it. Positions include the monitor's own offset
        // so multi-monitor setups anchor on the primary display.
        let origin = monitor.position();
        let mut closer_h = 0i32;
        for popup in self.visible.iter().rev() {
            let win_size = popup.window.inner_size();
            let (x, y) = corner_position(
                self.settings.corner,
                size.width as i32,
                size.height as i32,
                win_size.width as i32,
                win_size.height as i32,
                closer_h,
            );
            popup.window.set_outer_position(PhysicalPosition::new(
                origin.x + x,
                origin.y + y,
            ));
            closer_h += win_size.height as i32 + STACK_GAP;
        }
    }

    /// Note a WaitUntil-timer wake (StartCause::Poll /
    /// ResumeTimeReached). If the backend starves the loop's timer
    /// on a quiet desktop, a popup's 7 s are measured in accumulated
    /// timer time, so a dropped-timer gap does not eat into its
    /// time on screen.
    pub fn note_timer_wake(&mut self, now: Instant) {
        let gap = now.saturating_duration_since(self.last_timer_wake);
        if gap > Duration::from_millis(500) {
            linfo!(
                "Event-loop timer gap of {:.1}s — extending popup deadlines",
                gap.as_secs_f32()
            );
            for popup in self.visible.iter_mut() {
                popup.deadline += gap;
            }
        }
        self.last_timer_wake = now;
    }

    /// Close expired popups and do the one delayed repaint;
    /// returns true if anything changed.
    pub fn tick(&mut self, target: &EventLoopWindowTarget<()>) -> bool {
        let now = Instant::now();
        let before = self.visible.len();
        self.visible.retain(|p| p.deadline > now);
        let changed = self.visible.len() != before;
        if changed {
            self.restack(target);
        }
        if let Some(fonts) = self.fonts.as_mut() {
            for popup in self.visible.iter_mut() {
                // Enforce the intended size if the backend re-sized
                // the window after creation; a size change also
                // needs a repaint at the new size.
                let want_w = (WIDTH * f64::from(popup.scale)).round() as u32;
                let want_h =
                    (popup.layout.height_logical * f64::from(popup.scale)).round()
                        as u32;
                let size = popup.window.inner_size();
                if size.width != want_w || size.height != want_h {
                    popup.window.set_inner_size(LogicalSize::new(
                        WIDTH,
                        popup.layout.height_logical,
                    ));
                    if !popup.drift_logged {
                        popup.drift_logged = true;
                        let after = popup.window.inner_size();
                        lwarn!(
                            "Popup size drift: got {}x{}, want {}x{}, after set {}x{}",
                            size.width,
                            size.height,
                            want_w,
                            want_h,
                            after.width,
                            after.height
                        );
                    }
                    popup.repainted = false;
                }
                // Drawing done in the same turn as creation can be
                // lost if the window isn't mapped yet — repaint once
                // shortly after creation.
                if !popup.repainted
                    && popup.created.elapsed() > Duration::from_millis(300)
                {
                    popup.repainted = true;
                    if let Err(e) = paint(fonts, popup) {
                        lwarn!("Popup repaint failed: {e}");
                    }
                }
            }
        }
        changed
    }

    /// Repaint one popup (the event loop forwards RedrawRequested
    /// here — the OS asking for a redraw after an expose).
    pub fn repaint(&mut self, window_id: WindowId) {
        let Some(fonts) = self.fonts.as_mut() else {
            return;
        };
        if let Some(popup) = self.visible.iter_mut().find(|p| p.id == window_id) {
            if let Err(e) = paint(fonts, popup) {
                lwarn!("Popup repaint failed: {e}");
            }
        }
    }

    /// Handle a window event; returns true if it belonged to a popup.
    pub fn handle_window_event(
        &mut self,
        window_id: WindowId,
        event: &tao::event::WindowEvent,
    ) -> bool {
        use tao::event::{ElementState, MouseButton, WindowEvent};
        let Some(idx) = self.visible.iter().position(|p| p.id == window_id) else {
            return false;
        };
        match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let popup = self.visible.remove(idx);
                self.copy_to_clipboard(&popup.content.copy_text);
                linfo!("Popup clicked — copied {}", popup.content.copy_text);
                true
            }
            WindowEvent::CloseRequested => {
                self.visible.remove(idx);
                true
            }
            _ => true, // our window; ignore the rest
        }
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        if !self.clipboard_ok {
            return;
        }
        // Keep one instance for the app's lifetime: on Linux the
        // selection dies with the instance (see popup_x11); on
        // Windows it's harmless either way.
        if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(cb) => self.clipboard = Some(cb),
                Err(e) => {
                    lwarn!("Clipboard unavailable: {e}");
                    self.clipboard_ok = false;
                    return;
                }
            }
        }
        if let Some(cb) = self.clipboard.as_mut() {
            if let Err(e) = cb.set_text(text.to_string()) {
                lwarn!("Clipboard set failed: {e}");
                self.clipboard_ok = false;
            }
        }
    }
}

/// Render one popup and present it in its window.
fn paint(fonts: &mut Fonts, popup: &mut ActivePopup) -> Result<(), String> {
    let (pixels, rw, rh) = render(fonts, &popup.content, &popup.layout, popup.scale);
    let size = popup.window.inner_size();
    let (Some(nw), Some(nh)) = (
        NonZeroU32::new(size.width.max(1)),
        NonZeroU32::new(size.height.max(1)),
    ) else {
        return Ok(());
    };
    popup.surface.resize(nw, nh).map_err(|e| e.to_string())?;
    let mut buffer = popup.surface.buffer_mut().map_err(|e| e.to_string())?;
    let frame = &mut *buffer;
    let (wf, hf) = (size.width as usize, size.height as usize);
    if frame.len() < wf * hf {
        return Ok(());
    }
    // Fill with the body colour, then blit the rendered card at
    // the top-left (identical size unless the backend drifted).
    frame[..wf * hf].fill(super::rgb(super::BODY_BG));
    let copy_w = rw.min(size.width) as usize;
    let copy_h = rh.min(size.height) as usize;
    for y in 0..copy_h {
        let src = y * rw as usize;
        let dst = y * wf;
        frame[dst..dst + copy_w].copy_from_slice(&pixels[src..src + copy_w]);
    }
    buffer.present().map_err(|e| e.to_string())
}
