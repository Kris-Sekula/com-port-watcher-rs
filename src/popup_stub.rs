//! Stub popup backend for platforms without a custom popup
//! surface (macOS): popups are never available, so every event
//! uses a native notification — Notification Center banners,
//! exactly like the Python version's macOS build.

use std::time::Instant;

use tao::event_loop::EventLoopWindowTarget;
use tao::window::WindowId;

use crate::core::{PortEventKind, PortInfo};
use crate::settings::Settings;

pub struct PopupManager;

impl PopupManager {
    pub fn new(_settings: &Settings) -> PopupManager {
        PopupManager
    }
    pub fn set_settings(&mut self, _settings: &Settings) {}
    pub fn available(&self) -> bool {
        false
    }
    pub fn enqueue(&mut self, _kind: PortEventKind, _info: &PortInfo) -> bool {
        false
    }
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
        false
    }
}
