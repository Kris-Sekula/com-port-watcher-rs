//! Custom-drawn popup windows (v0.2.0) — the Python version's
//! look, without an OS notification server in between: a
//! borderless, always-on-top card at the bottom-right of the
//! primary monitor, green header for "added", orange for
//! "removed", the port line in bold, details underneath,
//! auto-close after 7 s, and a click copies the port name.
//!
//! This module holds the shared half: system-font loading
//! (fontdue), text layout/wrapping, and rendering a popup into a
//! pixel buffer. The platform backends present that buffer:
//!
//! - Windows: a tao window + softbuffer (popup_win).
//! - Linux: a raw X11 window via x11rb (popup_x11). The tao/GTK
//!   backend was tried first and fought back — GTK allocated the
//!   window at a fallback 200 px height and refused every resize,
//!   and softbuffer needs a mapped window — so Linux bypasses the
//!   toolkit entirely.
//! - macOS: no custom popups (stub); Notification Center banners
//!   are used instead, like the Python version.
//!
//! If fonts or the backend are unavailable, callers fall back to
//! native notifications — a popup failure must never swallow an
//! event.

use std::collections::HashMap;
use std::rc::Rc;

use crate::core::{PortEventKind, PortInfo};
use crate::{linfo, lwarn};

// Logical-pixel layout (scaled by the backend's scale factor).
pub(crate) const WIDTH: f64 = 380.0;
pub(crate) const PAD: f32 = 14.0;
pub(crate) const HEADER_H: f32 = 30.0;
pub(crate) const HEADER_PX: f32 = 13.5;
pub(crate) const TITLE_PX: f32 = 12.5;
pub(crate) const DETAIL_PX: f32 = 11.0;
pub(crate) const LINE_GAP: f32 = 3.0;
pub(crate) const BODY_TOP: f32 = 8.0;
pub(crate) const BODY_BOTTOM: f32 = 10.0;
pub(crate) const STACK_GAP: i32 = 10;
pub(crate) const MARGIN_X: i32 = 12;
pub(crate) const MARGIN_TOP: i32 = 12;
pub(crate) const MARGIN_BOTTOM: i32 = 56; // room for a taskbar/panel
pub(crate) const MAX_VISIBLE: usize = 3;

pub(crate) const GREEN: (u8, u8, u8) = (31, 122, 58); // tray icon green
pub(crate) const ORANGE: (u8, u8, u8) = (198, 102, 24);
pub(crate) const BODY_BG: (u8, u8, u8) = (250, 250, 248);
const BORDER: (u8, u8, u8) = (96, 96, 96);
const TEXT_DARK: (u8, u8, u8) = (26, 26, 26);
const TEXT_DETAIL: (u8, u8, u8) = (68, 68, 68);
const WHITE: (u8, u8, u8) = (255, 255, 255);

pub(crate) fn rgb(c: (u8, u8, u8)) -> u32 {
    ((c.0 as u32) << 16) | ((c.1 as u32) << 8) | c.2 as u32
}

/// System font (Regular + Bold) loaded once, with a per-glyph
/// raster cache. `None` from `load()` means "popups unavailable".
pub(crate) struct Fonts {
    regular: fontdue::Font,
    bold: fontdue::Font,
    cache: HashMap<(bool, char, u32), (fontdue::Metrics, Rc<Vec<u8>>)>,
}

fn font_candidates() -> [(&'static str, &'static str); 5] {
    if cfg!(target_os = "windows") {
        [
            (r"C:\Windows\Fonts\segoeui.ttf", r"C:\Windows\Fonts\segoeuib.ttf"),
            (r"C:\Windows\Fonts\arial.ttf", r"C:\Windows\Fonts\arialbd.ttf"),
            (r"C:\Windows\Fonts\tahoma.ttf", r"C:\Windows\Fonts\tahomabd.ttf"),
            ("", ""),
            ("", ""),
        ]
    } else if cfg!(target_os = "macos") {
        [
            (
                "/System/Library/Fonts/Supplemental/Arial.ttf",
                "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            ),
            (
                "/Library/Fonts/Arial.ttf",
                "/Library/Fonts/Arial Bold.ttf",
            ),
            ("", ""),
            ("", ""),
            ("", ""),
        ]
    } else {
        [
            (
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            ),
            (
                "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
            ),
            (
                "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
                "/usr/share/fonts/truetype/liberation2/LiberationSans-Bold.ttf",
            ),
            (
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
            ),
            (
                "/usr/share/fonts/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/noto/NotoSans-Bold.ttf",
            ),
        ]
    }
}

impl Fonts {
    pub(crate) fn load() -> Option<Fonts> {
        for (reg, bold) in font_candidates() {
            if reg.is_empty() {
                continue;
            }
            let (Ok(r), Ok(b)) = (std::fs::read(reg), std::fs::read(bold)) else {
                continue;
            };
            let settings = fontdue::FontSettings::default();
            if let (Ok(rf), Ok(bf)) = (
                fontdue::Font::from_bytes(r, settings),
                fontdue::Font::from_bytes(b, settings),
            ) {
                linfo!("Popup font: {reg}");
                return Some(Fonts {
                    regular: rf,
                    bold: bf,
                    cache: HashMap::new(),
                });
            }
        }
        lwarn!("No usable system font found — custom popups unavailable, using native notifications");
        None
    }

    pub(crate) fn glyph(
        &mut self,
        bold: bool,
        ch: char,
        px: f32,
    ) -> (fontdue::Metrics, Rc<Vec<u8>>) {
        let key = (bold, ch, px.to_bits());
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let font = if bold { &self.bold } else { &self.regular };
        let (metrics, bitmap) = font.rasterize(ch, px);
        let value = (metrics, Rc::new(bitmap));
        self.cache.insert(key, value.clone());
        value
    }

    pub(crate) fn measure(&mut self, bold: bool, text: &str, px: f32) -> f32 {
        text.chars()
            .map(|c| self.glyph(bold, c, px).0.advance_width)
            .sum()
    }

    pub(crate) fn line_height(&self, bold: bool, px: f32) -> f32 {
        let font = if bold { &self.bold } else { &self.regular };
        font.horizontal_line_metrics(px)
            .map(|m| m.new_line_size)
            .unwrap_or(px * 1.25)
    }

    pub(crate) fn ascent(&self, bold: bool, px: f32) -> f32 {
        let font = if bold { &self.bold } else { &self.regular };
        font.horizontal_line_metrics(px)
            .map(|m| m.ascent)
            .unwrap_or(px * 0.8)
    }
}

/// Top-left position for one popup in a corner stack.
/// `closer_h` is the total height (including stack gaps) of the
/// popups that sit closer to the corner than this one; the stack
/// grows away from the corner — up from a bottom corner, down
/// from a top one. Shared by both backends so they agree exactly.
pub(crate) fn corner_position(
    corner: crate::settings::Corner,
    screen_w: i32,
    screen_h: i32,
    popup_w: i32,
    popup_h: i32,
    closer_h: i32,
) -> (i32, i32) {
    let x = if corner.is_right() {
        screen_w - MARGIN_X - popup_w
    } else {
        MARGIN_X
    };
    let y = if corner.is_bottom() {
        screen_h - MARGIN_BOTTOM - closer_h - popup_h
    } else {
        MARGIN_TOP + closer_h
    };
    (x, y)
}

/// Greedy word wrap against an arbitrary measuring function, so
/// the layout logic is unit-testable without a real font.
pub fn wrap_text(
    text: &str,
    max_width: f32,
    measure: &mut dyn FnMut(&str) -> f32,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw_line in text.split('\n') {
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if measure(&candidate) <= max_width || current.is_empty() {
                current = candidate;
            } else {
                lines.push(std::mem::take(&mut current));
                current = word.to_string();
            }
        }
        lines.push(current);
    }
    lines
}

/// Everything a popup shows, before layout.
pub(crate) struct PopupContent {
    pub(crate) header: String,
    pub(crate) header_color: (u8, u8, u8),
    pub(crate) title: String,
    pub(crate) details: Vec<String>,
    pub(crate) copy_text: String,
}

pub(crate) fn content_for(kind: PortEventKind, info: &PortInfo) -> PopupContent {
    let (verb, color) = match kind {
        PortEventKind::Added => ("added", GREEN),
        PortEventKind::Removed => ("removed", ORANGE),
    };
    let mut details = Vec::new();
    let detail = info.detail();
    if !detail.is_empty() {
        details.push(detail);
    }
    PopupContent {
        header: format!("{} {}", crate::notify::port_noun(), verb),
        header_color: color,
        title: info.label(),
        details,
        copy_text: info.device.clone(),
    }
}

/// Wrapped text + total height for one popup at a scale factor.
pub(crate) struct Layout {
    pub(crate) title_lines: Vec<String>,
    pub(crate) detail_lines: Vec<String>,
    pub(crate) height_logical: f64,
}

pub(crate) fn compute_layout(
    fonts: &mut Fonts,
    content: &PopupContent,
    scale: f32,
) -> Layout {
    let (title_px, detail_px) = (TITLE_PX * scale, DETAIL_PX * scale);
    let max_text_w = (WIDTH as f32 - 2.0 * PAD) * scale;
    let title_lines = wrap_text(&content.title, max_text_w, &mut |s| {
        fonts.measure(true, s, title_px)
    });
    let mut detail_lines: Vec<String> = Vec::new();
    for d in &content.details {
        detail_lines.extend(wrap_text(d, max_text_w, &mut |s| {
            fonts.measure(false, s, detail_px)
        }));
    }
    let title_h = fonts.line_height(true, title_px);
    let detail_h = fonts.line_height(false, detail_px);
    let body_h = BODY_TOP * scale
        + title_lines.len() as f32 * title_h
        + if detail_lines.is_empty() {
            0.0
        } else {
            LINE_GAP * scale + detail_lines.len() as f32 * detail_h
        }
        + BODY_BOTTOM * scale;
    let height_logical = f64::from(HEADER_H) + f64::from(body_h / scale);
    linfo!(
        "Popup layout: scale={scale}, title lines={}, detail lines={}, height={height_logical:.0}",
        title_lines.len(),
        detail_lines.len()
    );
    Layout {
        title_lines,
        detail_lines,
        height_logical,
    }
}

/// Render one popup into a fresh pixel buffer (0x00RRGGBB per
/// pixel, row-major). Returns (pixels, width, height).
pub(crate) fn render(
    fonts: &mut Fonts,
    content: &PopupContent,
    layout: &Layout,
    scale: f32,
) -> (Vec<u32>, u32, u32) {
    let w = (WIDTH * f64::from(scale)).round() as usize;
    let h = (layout.height_logical * f64::from(scale)).round() as usize;
    let mut frame = vec![0u32; w * h];
    let s = scale;
    let header_h_px = (HEADER_H * s).round() as usize;
    for y in 0..h {
        for x in 0..w {
            let color = if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                BORDER
            } else if y < header_h_px {
                content.header_color
            } else {
                BODY_BG
            };
            frame[y * w + x] = rgb(color);
        }
    }
    let pad = PAD * s;
    // Header text (bold, white), centred in the header by the
    // font's line metrics, computed in logical units so the
    // proportions hold at any scale.
    let line_h = fonts.line_height(true, HEADER_PX);
    let baseline =
        ((HEADER_H - line_h) / 2.0 + fonts.ascent(true, HEADER_PX)) * s;
    draw_text(
        &mut frame, w, h, fonts, true, &content.header, pad, baseline,
        HEADER_PX * s, WHITE,
    );
    // Body: bold title lines, then regular detail lines.
    let mut cursor =
        HEADER_H + BODY_TOP + fonts.ascent(true, TITLE_PX);
    for line in &layout.title_lines {
        draw_text(
            &mut frame, w, h, fonts, true, line, pad, cursor * s,
            TITLE_PX * s, TEXT_DARK,
        );
        cursor += fonts.line_height(true, TITLE_PX);
    }
    cursor += LINE_GAP - fonts.line_height(true, TITLE_PX)
        + fonts.ascent(false, DETAIL_PX);
    for line in &layout.detail_lines {
        draw_text(
            &mut frame, w, h, fonts, false, line, pad, cursor * s,
            DETAIL_PX * s, TEXT_DETAIL,
        );
        cursor += fonts.line_height(false, DETAIL_PX);
    }
    (frame, w as u32, h as u32)
}

fn draw_text(
    frame: &mut [u32],
    frame_w: usize,
    frame_h: usize,
    fonts: &mut Fonts,
    bold: bool,
    text: &str,
    x: f32,
    baseline: f32,
    px: f32,
    color: (u8, u8, u8),
) {
    let mut pen_x = x;
    for ch in text.chars() {
        let (metrics, bitmap) = fonts.glyph(bold, ch, px);
        let gx = (pen_x + metrics.xmin as f32).round() as i32;
        let gy = (baseline - metrics.ymin as f32 - metrics.height as f32).round() as i32;
        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let coverage = bitmap[(row * metrics.width + col) as usize];
                if coverage == 0 {
                    continue;
                }
                let (px_x, px_y) = (gx + col as i32, gy + row as i32);
                if px_x < 0 || px_y < 0 || px_x >= frame_w as i32 || px_y >= frame_h as i32 {
                    continue;
                }
                let idx = px_y as usize * frame_w + px_x as usize;
                let bg = frame[idx];
                let a = coverage as u32;
                let blend = |fg: u8, bgc: u32, shift: u32| -> u32 {
                    let b = (bgc >> shift) & 0xFF;
                    (fg as u32 * a + b * (255 - a)) / 255
                };
                frame[idx] = (blend(color.0, bg, 16) << 16)
                    | (blend(color.1, bg, 8) << 8)
                    | blend(color.2, bg, 0);
            }
        }
        pen_x += metrics.advance_width;
    }
}

#[cfg(target_os = "windows")]
#[path = "popup_win.rs"]
mod backend;
#[cfg(target_os = "linux")]
#[path = "popup_x11.rs"]
mod backend;
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
#[path = "popup_stub.rs"]
mod backend;

pub(crate) use backend::PopupManager;

#[cfg(test)]
mod tests {
    use super::{corner_position, wrap_text};
    use crate::settings::Corner;

    #[test]
    fn corner_positions_anchor_correctly() {
        // 1280x1024 screen, 380x93 popup, nothing closer to the corner.
        assert_eq!(
            corner_position(Corner::BottomRight, 1280, 1024, 380, 93, 0),
            (888, 875)
        );
        assert_eq!(
            corner_position(Corner::BottomLeft, 1280, 1024, 380, 93, 0),
            (12, 875)
        );
        assert_eq!(
            corner_position(Corner::TopRight, 1280, 1024, 380, 93, 0),
            (888, 12)
        );
        assert_eq!(
            corner_position(Corner::TopLeft, 1280, 1024, 380, 93, 0),
            (12, 12)
        );
    }

    #[test]
    fn corner_stacks_grow_away_from_the_corner() {
        // 103 px closer to the corner (93 + 10 gap): bottom stacks
        // move up, top stacks move down.
        assert_eq!(
            corner_position(Corner::BottomRight, 1280, 1024, 380, 93, 103).1,
            875 - 103
        );
        assert_eq!(
            corner_position(Corner::TopLeft, 1280, 1024, 380, 93, 103).1,
            12 + 103
        );
    }

    fn mono(s: &str) -> f32 {
        s.len() as f32 * 10.0
    }

    #[test]
    fn wrap_breaks_on_words() {
        let lines = wrap_text("alpha beta gamma delta", 110.0, &mut mono);
        assert_eq!(lines, vec!["alpha beta", "gamma delta"]);
    }

    #[test]
    fn wrap_keeps_short_text_whole() {
        let lines = wrap_text("COM8 — test", 400.0, &mut mono);
        assert_eq!(lines, vec!["COM8 — test"]);
    }

    #[test]
    fn wrap_respects_explicit_newlines() {
        let lines = wrap_text("one\ntwo three", 400.0, &mut mono);
        assert_eq!(lines, vec!["one", "two three"]);
    }

    #[test]
    fn wrap_long_word_goes_on_own_line() {
        let lines = wrap_text("short averylongword indeed", 110.0, &mut mono);
        assert_eq!(lines[0], "short");
        assert_eq!(lines[1], "averylongword");
    }
}
