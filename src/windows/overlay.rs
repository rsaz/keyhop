//! Win32-native hint overlay.
//!
//! [`pick_hint`] creates a transparent, always-on-top, layered popup window
//! that covers the virtual desktop, draws a styled label per hint, and runs
//! its own message loop until the user types a complete label or presses
//! `Esc` (or focus is stolen).
//!
//! The function is generic over what the hints represent: callers supply a
//! `Vec<Hint>` (each with a screen-space rect, a label, and optional
//! "extra" disambiguation text) plus a [`HintStyle`] preset, and get back
//! the *index* of the chosen hint. The caller maps that index back to
//! whatever it cares about — an [`crate::ElementId`] for the element
//! picker, an HWND for the window picker, etc. This keeps the rendering
//! primitive completely free of knowledge about the domain it's pointing
//! at.
//!
//! Layout pass: the renderer estimates each badge's pixel size up-front
//! (no measure-then-place dance) and runs a small collision-resolution
//! pass that stacks colliding hints vertically. This is what keeps two
//! maximized windows on the same monitor (e.g. Edge + Steam) from drawing
//! their badges on top of each other.
//!
//! Transparency is implemented with the cheapest, most-compatible
//! mechanism: a layered window with a magenta color key. The window body
//! is filled with magenta on every paint; only the actual label
//! rectangles render real pixels.

use std::ffi::c_void;

use anyhow::{anyhow, Context, Result};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    FrameRect, InvalidateRect, SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_TOP, FF_DONTCARE, FW_BOLD, FW_NORMAL, HDC, HFONT, HGDIOBJ,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_BACK, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClassInfoExW, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, LoadCursorW, PostQuitMessage, RegisterClassExW,
    SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, TranslateMessage,
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, IDC_ARROW, LWA_COLORKEY, MSG,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WM_DESTROY,
    WM_KEYDOWN, WM_KILLFOCUS, WM_NCCREATE, WM_PAINT, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::Bounds;

const CLASS_NAME: PCWSTR = w!("KeyhopOverlayClass");
const WINDOW_TITLE: PCWSTR = w!("Keyhop Overlay");

/// Magenta — used as the layered-window color key for transparency.
/// Anything painted in this exact color becomes see-through.
const TRANSPARENT_KEY: COLORREF = COLORREF(0x00FF00FF);

/// Pixel gap between the matchable badge and the optional "extra" pill,
/// and the vertical gap between stacked badges after collision resolution.
const PILL_GAP: i32 = 4;

/// Hard cap on how many characters we lay out for the extra text. Past
/// this, `DT_END_ELLIPSIS` truncates visually. Keeps very long window
/// titles from making one badge wider than a monitor.
const MAX_EXTRA_CHARS: usize = 50;

/// One pickable thing on screen. The renderer treats `bounds` as a
/// screen-space anchor and places the label badge at the top-left of that
/// rect (subject to collision-resolution shifting).
#[derive(Debug, Clone)]
pub struct Hint {
    /// Anchor rectangle in screen coordinates (the badge appears at its
    /// top-left). For elements this is the element's bounding rect; for
    /// windows it's the window's frame rect.
    pub bounds: Bounds,
    /// Lowercase home-row label produced by [`crate::HintEngine`]. This is
    /// what the user types to select.
    pub label: String,
    /// Optional non-matchable text drawn in a secondary pill beside the
    /// badge. Used by the window picker to show window titles so the user
    /// can disambiguate two windows whose badges would otherwise be at
    /// the same pixel.
    pub extra: Option<String>,
}

/// Visual presets for the overlay. The element and window pickers use
/// different sizing/colors so the user can tell at a glance which mode
/// they're in.
#[derive(Debug, Clone, Copy)]
pub struct HintStyle {
    /// Pixel height of the bold label font.
    pub font_height: i32,
    /// Background fill of the matchable badge.
    pub badge_bg: COLORREF,
    /// Text color for the matchable badge.
    pub badge_fg: COLORREF,
    /// Border color for both pills.
    pub border: COLORREF,
    /// Background fill of the optional extra/title pill. Ignored when no
    /// hint has an `extra`.
    pub extra_bg: COLORREF,
    /// Text color for the optional extra/title pill.
    pub extra_fg: COLORREF,
    /// Horizontal padding inside each pill.
    pub padding_x: i32,
    /// Vertical padding inside each pill.
    pub padding_y: i32,
}

impl HintStyle {
    /// Compact, dense badges meant to sit on top of small UI controls.
    /// No extra-text rendering needed — element labels carry enough
    /// information on their own (the user is reading the underlying UI).
    pub fn elements() -> Self {
        Self {
            font_height: 20,
            badge_bg: COLORREF(0x0000E5FF), // BGR yellow
            badge_fg: COLORREF(0x00000000),
            border: COLORREF(0x00202020),
            extra_bg: COLORREF(0x00202020),
            extra_fg: COLORREF(0x00FFFFFF),
            padding_x: 6,
            padding_y: 2,
        }
    }

    /// Larger badges with a distinct accent color and a dark "title pill"
    /// for the window name. Used by the window picker so the user
    /// instantly sees what each badge maps to even when two windows are
    /// maximized on the same monitor.
    pub fn windows() -> Self {
        Self {
            font_height: 30,
            badge_bg: COLORREF(0x00FFAA33), // BGR teal-blue accent
            badge_fg: COLORREF(0x00FFFFFF),
            border: COLORREF(0x00101010),
            extra_bg: COLORREF(0x00202020), // dark gray
            extra_fg: COLORREF(0x00FFFFFF),
            padding_x: 12,
            padding_y: 6,
        }
    }

    /// Build the element-picker style, overriding any non-empty colors
    /// from the user config. Empty strings keep the hardcoded defaults so
    /// users can override one swatch at a time without filling out every
    /// field in `config.toml`.
    pub fn elements_from_config(c: &crate::config::BadgeColors) -> Self {
        let mut style = Self::elements();
        apply_color_override(&mut style.badge_bg, &c.badge_bg);
        apply_color_override(&mut style.badge_fg, &c.badge_fg);
        apply_color_override(&mut style.border, &c.border);
        style
    }

    /// Like [`Self::elements_from_config`] but for the window picker.
    pub fn windows_from_config(c: &crate::config::BadgeColors) -> Self {
        let mut style = Self::windows();
        apply_color_override(&mut style.badge_bg, &c.badge_bg);
        apply_color_override(&mut style.badge_fg, &c.badge_fg);
        apply_color_override(&mut style.border, &c.border);
        style
    }
}

fn apply_color_override(target: &mut COLORREF, hex: &str) {
    if hex.trim().is_empty() {
        return;
    }
    match parse_hex_color(hex) {
        Ok(c) => *target = c,
        Err(e) => {
            tracing::warn!(value = %hex, error = ?e, "invalid color in config; keeping default");
        }
    }
}

/// Parse `"#RRGGBB"` or `"#RGB"` (with or without the `#`) into a
/// Win32 [`COLORREF`]. Win32 stores colors as `0x00BBGGRR`, so we swap
/// the byte order from web-style RGB.
pub fn parse_hex_color(s: &str) -> anyhow::Result<COLORREF> {
    let trimmed = s.trim().trim_start_matches('#');
    let (r, g, b) = match trimmed.len() {
        6 => {
            let r = u8::from_str_radix(&trimmed[0..2], 16)?;
            let g = u8::from_str_radix(&trimmed[2..4], 16)?;
            let b = u8::from_str_radix(&trimmed[4..6], 16)?;
            (r, g, b)
        }
        3 => {
            let r = u8::from_str_radix(&trimmed[0..1], 16)? * 0x11;
            let g = u8::from_str_radix(&trimmed[1..2], 16)? * 0x11;
            let b = u8::from_str_radix(&trimmed[2..3], 16)? * 0x11;
            (r, g, b)
        }
        _ => anyhow::bail!("hex color must be #RGB or #RRGGBB, got '{s}'"),
    };
    let bgr = ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
    Ok(COLORREF(bgr))
}

/// One hint after layout resolution, in client coordinates. We compute
/// these once when the overlay is constructed; subsequent paints just
/// blit them.
struct LaidHint {
    label: String,
    extra: Option<String>,
    badge_rect: RECT,
    extra_rect: Option<RECT>,
}

struct OverlayState {
    laid: Vec<LaidHint>,
    style: HintStyle,
    typed: String,
    /// Index into `laid` of the chosen entry, set by `key_down` on a full
    /// label match. `None` means "user cancelled or window closed."
    selected: Option<usize>,
    label_font: HFONT,
    extra_font: HFONT,
}

impl OverlayState {
    unsafe fn new(hints: Vec<Hint>, style: HintStyle, origin_x: i32, origin_y: i32) -> Self {
        let label_font = create_font(style.font_height, FW_BOLD.0 as i32);
        // The extra pill renders at ~75% of the label height in regular
        // weight — visible but clearly secondary to the matchable badge.
        let extra_font = create_font((style.font_height * 3) / 4, FW_NORMAL.0 as i32);
        let laid = lay_out(&hints, &style, origin_x, origin_y);
        Self {
            laid,
            style,
            typed: String::new(),
            selected: None,
            label_font,
            extra_font,
        }
    }

    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);

        // Fill entire client area with the color-key value so the
        // underlying desktop shows through.
        let bg = CreateSolidBrush(TRANSPARENT_KEY);
        FillRect(hdc, &ps.rcPaint, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        let _ = SetBkMode(hdc, TRANSPARENT);

        for laid in &self.laid {
            self.draw_hint(hdc, laid);
        }

        let _ = EndPaint(hwnd, &ps);
    }

    unsafe fn draw_hint(&self, hdc: HDC, laid: &LaidHint) {
        // Hide hints whose label doesn't match the typed prefix.
        if !self.typed.is_empty() && !laid.label.starts_with(&self.typed) {
            return;
        }

        // -- matchable badge --
        let bg = CreateSolidBrush(self.style.badge_bg);
        FillRect(hdc, &laid.badge_rect, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        let border = CreateSolidBrush(self.style.border);
        FrameRect(hdc, &laid.badge_rect, border);
        let _ = DeleteObject(HGDIOBJ(border.0));

        let old = SelectObject(hdc, HGDIOBJ(self.label_font.0));
        let _ = SetTextColor(hdc, self.style.badge_fg);
        let mut wide: Vec<u16> = laid.label.encode_utf16().collect();
        let mut text_rect = RECT {
            left: laid.badge_rect.left + self.style.padding_x,
            top: laid.badge_rect.top + self.style.padding_y,
            right: laid.badge_rect.right - self.style.padding_x,
            bottom: laid.badge_rect.bottom - self.style.padding_y,
        };
        DrawTextW(
            hdc,
            &mut wide,
            &mut text_rect,
            DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old);

        // -- optional extra pill --
        if let (Some(extra_rect), Some(extra)) = (&laid.extra_rect, &laid.extra) {
            let bg = CreateSolidBrush(self.style.extra_bg);
            FillRect(hdc, extra_rect, bg);
            let _ = DeleteObject(HGDIOBJ(bg.0));

            let border = CreateSolidBrush(self.style.border);
            FrameRect(hdc, extra_rect, border);
            let _ = DeleteObject(HGDIOBJ(border.0));

            let old = SelectObject(hdc, HGDIOBJ(self.extra_font.0));
            let _ = SetTextColor(hdc, self.style.extra_fg);
            let mut wide_extra: Vec<u16> = extra.encode_utf16().collect();
            let mut extra_text_rect = RECT {
                left: extra_rect.left + self.style.padding_x,
                top: extra_rect.top + self.style.padding_y,
                right: extra_rect.right - self.style.padding_x,
                bottom: extra_rect.bottom - self.style.padding_y,
            };
            DrawTextW(
                hdc,
                &mut wide_extra,
                &mut extra_text_rect,
                DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            let _ = SelectObject(hdc, old);
        }
    }

    unsafe fn key_down(&mut self, hwnd: HWND, vk: u32) {
        if vk == VK_ESCAPE.0 as u32 {
            self.selected = None;
            let _ = DestroyWindow(hwnd);
            return;
        }
        if vk == VK_BACK.0 as u32 {
            self.typed.pop();
            let _ = InvalidateRect(hwnd, None, true);
            return;
        }
        // VK_A..VK_Z map 1:1 to ASCII 'A'..'Z'.
        if (b'A' as u32..=b'Z' as u32).contains(&vk) {
            let ch = (vk as u8 - b'A' + b'a') as char;
            self.typed.push(ch);

            if let Some(idx) = self.laid.iter().position(|h| h.label == self.typed) {
                self.selected = Some(idx);
                let _ = DestroyWindow(hwnd);
                return;
            }

            let any_prefix = self.laid.iter().any(|h| h.label.starts_with(&self.typed));
            if !any_prefix {
                self.typed.clear();
            }
            let _ = InvalidateRect(hwnd, None, true);
        }
    }
}

impl Drop for OverlayState {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.label_font.0));
            let _ = DeleteObject(HGDIOBJ(self.extra_font.0));
        }
    }
}

unsafe fn create_font(height: i32, weight: i32) -> HFONT {
    CreateFontW(
        height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET.0.into(),
        OUT_DEFAULT_PRECIS.0.into(),
        CLIP_DEFAULT_PRECIS.0.into(),
        CLEARTYPE_QUALITY.0.into(),
        u32::from(DEFAULT_PITCH.0 | (FF_DONTCARE.0 << 4)),
        w!("Segoe UI"),
    )
}

/// Resolve hint anchors into final draw rectangles.
///
/// Two responsibilities:
///
/// 1. Convert each hint's screen-space anchor to client-space (subtract
///    the virtual-desktop origin so labels land on the right monitor on
///    multi-monitor setups whose primary isn't the leftmost).
/// 2. Detect collisions between badges and shift the colliding ones
///    *down* by one badge-height + a small gap. Without this, two
///    maximized windows on the same monitor would draw their badges on
///    top of each other.
///
/// We estimate text widths from the font height rather than measuring
/// (which would need an HDC and font selection here). For Segoe UI Bold a
/// glyph is roughly 0.62 × font height; for the regular extra-pill text
/// it's around 0.50. Generous enough to avoid clipping; close enough that
/// the layout feels tight.
fn lay_out(hints: &[Hint], style: &HintStyle, origin_x: i32, origin_y: i32) -> Vec<LaidHint> {
    let label_glyph_w = ((style.font_height as f32) * 0.62) as i32;
    let extra_glyph_w = ((style.font_height as f32) * 0.50) as i32;
    let row_h = style.font_height + style.padding_y * 2;

    // Process hints in reading order (top-then-left) so collision
    // resolution stacks predictably from the natural anchor downward.
    let mut order: Vec<usize> = (0..hints.len()).collect();
    order.sort_by_key(|&i| (hints[i].bounds.y - origin_y, hints[i].bounds.x - origin_x));

    let mut result: Vec<Option<LaidHint>> = (0..hints.len()).map(|_| None).collect();
    // Bounding box (in client coords) of every laid hint, used purely for
    // collision tests.
    let mut placed: Vec<RECT> = Vec::with_capacity(hints.len());

    for &i in &order {
        let h = &hints[i];

        let label_chars = h.label.chars().count() as i32;
        let badge_w = label_chars * label_glyph_w + style.padding_x * 2;

        let extra_w = h.extra.as_ref().map(|s| {
            let chars = s.chars().count().min(MAX_EXTRA_CHARS) as i32;
            chars * extra_glyph_w + style.padding_x * 2
        });

        let total_w = badge_w + extra_w.map(|w| PILL_GAP + w).unwrap_or(0);
        let total_h = row_h;

        let x = h.bounds.x - origin_x;
        let mut y = h.bounds.y - origin_y;

        // Walk down until we don't collide with an already-placed hint.
        // Bounded by an obvious safety cap so a pathological input
        // (thousands of windows at the same anchor) can't loop forever.
        let mut attempts = 0;
        loop {
            let candidate = RECT {
                left: x,
                top: y,
                right: x + total_w,
                bottom: y + total_h,
            };
            if !placed.iter().any(|p| rects_intersect(p, &candidate)) {
                break;
            }
            y += row_h + PILL_GAP;
            attempts += 1;
            if attempts > 256 {
                break;
            }
        }

        let badge_rect = RECT {
            left: x,
            top: y,
            right: x + badge_w,
            bottom: y + row_h,
        };
        let extra_rect = extra_w.map(|w| RECT {
            left: x + badge_w + PILL_GAP,
            top: y,
            right: x + badge_w + PILL_GAP + w,
            bottom: y + row_h,
        });

        placed.push(RECT {
            left: x,
            top: y,
            right: x + total_w,
            bottom: y + total_h,
        });
        result[i] = Some(LaidHint {
            label: h.label.clone(),
            extra: h.extra.clone(),
            badge_rect,
            extra_rect,
        });
    }

    result
        .into_iter()
        .map(|o| o.expect("laid every hint"))
        .collect()
}

fn rects_intersect(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        let state_ptr = (*cs).lpCreateParams;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }

    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let state = &mut *state_ptr;

    match msg {
        WM_PAINT => {
            state.paint(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            state.key_down(hwnd, wp.0 as u32);
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            // Treat focus loss as cancel — no half-committed state.
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn ensure_class_registered(hinstance: HINSTANCE) -> Result<()> {
    let mut existing = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        ..Default::default()
    };
    if GetClassInfoExW(hinstance, CLASS_NAME, &mut existing).is_ok() {
        return Ok(());
    }
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance,
        hCursor: LoadCursorW(None, IDC_ARROW)?,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    let atom = RegisterClassExW(&class);
    if atom == 0 {
        return Err(anyhow!("RegisterClassExW returned 0"));
    }
    Ok(())
}

unsafe fn virtual_screen_rect() -> (i32, i32, i32, i32) {
    (
        GetSystemMetrics(SM_XVIRTUALSCREEN),
        GetSystemMetrics(SM_YVIRTUALSCREEN),
        GetSystemMetrics(SM_CXVIRTUALSCREEN),
        GetSystemMetrics(SM_CYVIRTUALSCREEN),
    )
}

/// Show the hint overlay and block on a Win32 message loop until the
/// user picks a hint, presses `Esc`, or the window loses focus.
///
/// Returns `Ok(Some(idx))` where `idx` indexes into the input `hints` vec
/// when the user picks one, and `Ok(None)` on cancel / focus loss / empty
/// input.
pub fn pick_hint(hints: Vec<Hint>, style: HintStyle) -> Result<Option<usize>> {
    if hints.is_empty() {
        return Ok(None);
    }

    unsafe {
        // Best-effort: tag the process as PerMonitorV2 DPI-aware so
        // source pixel coords line up with our overlay coords on
        // high-DPI displays. Safe to call repeatedly; ignore "already
        // set" / older-OS errors.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let hinstance: HINSTANCE = unsafe { GetModuleHandleW(PCWSTR::null())? }.into();
    unsafe { ensure_class_registered(hinstance)? };

    let (vx, vy, vw, vh) = unsafe { virtual_screen_rect() };
    tracing::debug!(
        vx,
        vy,
        vw,
        vh,
        hint_count = hints.len(),
        "overlay virtual desktop rect"
    );
    let state = Box::new(unsafe { OverlayState::new(hints, style, vx, vy) });
    let state_ptr = Box::into_raw(state);

    let hwnd_result = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS_NAME,
            WINDOW_TITLE,
            WS_POPUP | WS_VISIBLE,
            vx,
            vy,
            vw,
            vh,
            None,
            None,
            hinstance,
            Some(state_ptr as *const c_void),
        )
    };
    let hwnd = match hwnd_result {
        Ok(h) => h,
        Err(e) => {
            // Reclaim the leaked Box to avoid leaking the state on failure.
            unsafe { drop(Box::from_raw(state_ptr)) };
            return Err(e).context("CreateWindowExW failed");
        }
    };

    unsafe {
        SetLayeredWindowAttributes(hwnd, TRANSPARENT_KEY, 0, LWA_COLORKEY)?;
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(hwnd);
    }

    // Modal message pump. Returns when WndProc posts WM_QUIT.
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 == 0 || r.0 == -1 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    let state = unsafe { Box::from_raw(state_ptr) };
    Ok(state.selected)
}

#[cfg(test)]
mod color_tests {
    use super::*;

    #[test]
    fn parses_six_digit_hex() {
        let c = parse_hex_color("#FFE500").unwrap();
        assert_eq!(c.0, 0x0000E5FF);
    }

    #[test]
    fn parses_three_digit_hex() {
        let c = parse_hex_color("#F00").unwrap();
        assert_eq!(c.0, 0x000000FF);
    }

    #[test]
    fn parses_without_hash() {
        let c = parse_hex_color("FFFFFF").unwrap();
        assert_eq!(c.0, 0x00FFFFFF);
    }

    #[test]
    fn rejects_bad_length() {
        assert!(parse_hex_color("#FFFF").is_err());
    }

    #[test]
    fn rejects_non_hex() {
        assert!(parse_hex_color("#GGHHII").is_err());
    }
}
