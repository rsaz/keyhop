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
//! Transparency is implemented with the modern, fast path:
//! `UpdateLayeredWindow(ULW_ALPHA)` driven by a pre-composed 32-bit ARGB
//! DIB. Pixels with `alpha == 0` are fully transparent (no color-key
//! tax, no per-paint magenta fill); badge pixels carry per-pixel alpha
//! that DWM blends directly. The HWND, off-screen DIB, and memory DC
//! are all created once on the first hotkey press and reused for the
//! life of the process — subsequent picks just re-render and re-show.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use anyhow::{anyhow, Context, Result};
use smallvec::SmallVec;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DeleteDC, DeleteObject,
    DrawTextW, FillRect, GetDC, GetMonitorInfoW, MonitorFromRect, ReleaseDC, SelectObject,
    SetBkMode, SetTextColor, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    BLENDFUNCTION, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_TOP, FF_DONTCARE,
    FW_BOLD, FW_NORMAL, HBITMAP, HDC, HFONT, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_BACK, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClassInfoExW, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, LoadCursorW, RegisterClassExW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow,
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, IDC_ARROW, MSG, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_KEYDOWN, WM_KILLFOCUS, WM_NCCREATE, WM_PAINT,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::Bounds;

const CLASS_NAME: PCWSTR = w!("KeyhopOverlayClass");
const WINDOW_TITLE: PCWSTR = w!("Keyhop Overlay");

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
    /// Window-wide opacity for badges (0..=255). 255 = fully opaque,
    /// values around 200-230 let the underlying UI bleed through enough
    /// to see what the badge is sitting on. The transparent color-key
    /// pixels remain fully see-through regardless of this value.
    pub badge_opacity: u8,
    /// Draw a 1px text shadow behind labels for extra contrast on
    /// busy/light backgrounds. Off by default — most styles already
    /// have enough contrast between badge_bg and badge_fg.
    pub text_shadow: bool,
    /// Draw a thin connector line + arrowhead from each badge to the
    /// element it represents. Helps disambiguate badges that are placed
    /// near (but not on) their target — e.g. when collision resolution
    /// pushed the badge into [`BadgePosition::OutsideTop`] above the
    /// element. We always render the arrow when the geometry is long
    /// enough to be readable; degenerate cases (badge fully overlapping
    /// the element) skip it automatically.
    pub show_leader: bool,
    /// Pen color for the leader line + arrowhead. Independent of
    /// `badge_bg` so the leader can stay legible on backgrounds that
    /// would otherwise wash out a translucent badge.
    pub leader_color: COLORREF,
    /// When `true`, the layout pass tries to anchor badges *inside* their
    /// element (top-left first). Use this for the window picker, where
    /// targets are typically full-screen windows and an "above the
    /// element" badge would render above the monitor and be invisible.
    ///
    /// When `false` (default), `OutsideTop` is tried first so that small
    /// element badges sit just above the control they label, leaving the
    /// underlying UI fully visible.
    pub prefer_inside_anchor: bool,
}

impl HintStyle {
    /// Compact, dense badges meant to sit on top of small UI controls.
    /// No extra-text rendering needed — element labels carry enough
    /// information on their own (the user is reading the underlying UI).
    pub fn elements() -> Self {
        Self {
            font_height: 16,
            badge_bg: COLORREF(0x0000E5FF), // BGR yellow
            badge_fg: COLORREF(0x00000000),
            border: COLORREF(0x00202020),
            extra_bg: COLORREF(0x00202020),
            extra_fg: COLORREF(0x00FFFFFF),
            padding_x: 5,
            padding_y: 2,
            // ~90% opaque: badges are clearly readable but the underlying
            // control still hints through, so the user can see what they're
            // about to invoke before committing.
            badge_opacity: 230,
            text_shadow: false,
            show_leader: true,
            // Same dark grey as the badge border — visually subordinate to
            // the badge itself but contrasts well against typical app UIs.
            leader_color: COLORREF(0x00202020),
            // Element controls are usually small; placing the badge above
            // them keeps the underlying UI visible while the user picks.
            prefer_inside_anchor: false,
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
            // Slightly more opaque than the element picker — the window
            // badges are large and we'd rather lose a hair of see-through
            // than risk the title pill text smudging into a desktop image.
            badge_opacity: 240,
            text_shadow: false,
            // Window badges already include a title pill spelling out
            // exactly which window each one maps to, so the leader line
            // is just visual noise here — turn it off by default.
            show_leader: false,
            leader_color: COLORREF(0x00101010),
            // Window targets are typically maximized — anchoring the
            // badge to the top-left of the window puts it inside the
            // title bar area where it's clearly visible. `OutsideTop`
            // would push it above the monitor for any window at y == 0.
            prefer_inside_anchor: true,
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
        apply_color_override(&mut style.leader_color, &c.leader_color);
        apply_opacity_override(&mut style.badge_opacity, c.opacity);
        if let Some(show) = c.show_leader {
            style.show_leader = show;
        }
        style
    }

    /// Like [`Self::elements_from_config`] but for the window picker.
    pub fn windows_from_config(c: &crate::config::BadgeColors) -> Self {
        let mut style = Self::windows();
        apply_color_override(&mut style.badge_bg, &c.badge_bg);
        apply_color_override(&mut style.badge_fg, &c.badge_fg);
        apply_color_override(&mut style.border, &c.border);
        apply_color_override(&mut style.leader_color, &c.leader_color);
        apply_opacity_override(&mut style.badge_opacity, c.opacity);
        if let Some(show) = c.show_leader {
            style.show_leader = show;
        }
        style
    }
}

/// Convert a 0..=100 percent value from config into a 0..=255 alpha byte.
/// `0` is treated as "use the preset default" so users who never touch the
/// field don't end up with invisible badges.
fn apply_opacity_override(target: &mut u8, percent: u8) {
    if percent == 0 {
        return;
    }
    let clamped = percent.min(100) as u32;
    *target = (clamped * 255 / 100) as u8;
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
    /// Element bounds in client coordinates. Kept around so the painter
    /// can draw a leader line / arrowhead from the badge to the actual
    /// click target — useful when smart positioning shoved the badge off
    /// the element to avoid a collision.
    target_rect: RECT,
}

struct OverlayState {
    laid: Vec<LaidHint>,
    style: HintStyle,
    typed: String,
    /// Index into `laid` of the chosen entry, set by `key_down` on a full
    /// label match. `None` means "user cancelled or window closed."
    selected: Option<usize>,
    /// Set to `true` when the modal pump should exit (selection made,
    /// Esc pressed, or focus lost). Replaces the old DestroyWindow +
    /// PostQuitMessage flow now that the HWND is persistent.
    done: bool,
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
            done: false,
            label_font,
            extra_font,
        }
    }

    /// Render the current state into the supplied off-screen ARGB DIB,
    /// then atomically push the result to DWM via UpdateLayeredWindow.
    /// No WM_PAINT round-trip; no magenta color-key tax.
    unsafe fn render_to_dib(
        &self,
        hwnd: HWND,
        mem_dc: HDC,
        bits: *mut u8,
        width: i32,
        height: i32,
        opacity: u8,
    ) {
        // Reset the DIB to fully transparent. memset on a contiguous
        // BGRA buffer is cheaper than per-pixel iteration and matches
        // what the kernel does internally for fresh DIB sections.
        if !bits.is_null() && width > 0 && height > 0 {
            std::ptr::write_bytes(bits, 0, (width as usize) * (height as usize) * 4);
        }

        let _ = SetBkMode(mem_dc, TRANSPARENT);

        for laid in &self.laid {
            self.draw_hint(mem_dc, bits, width, height, laid);
        }

        // Hand the finished frame to DWM. SourceConstantAlpha applies
        // the per-window opacity over the per-pixel alpha — same visual
        // result as the old LWA_ALPHA + LWA_COLORKEY combo, but with
        // proper anti-aliased edges around every drawn pixel.
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: opacity,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let size = windows::Win32::Foundation::SIZE { cx: width, cy: height };
        let src_pt = POINT { x: 0, y: 0 };
        let _ = UpdateLayeredWindow(
            hwnd,
            HDC::default(),
            None,
            Some(&size as *const _),
            mem_dc,
            Some(&src_pt as *const _),
            COLORREF(0),
            Some(&blend as *const _),
            ULW_ALPHA,
        );
    }

    unsafe fn draw_hint(
        &self,
        hdc: HDC,
        bits: *mut u8,
        dib_w: i32,
        dib_h: i32,
        laid: &LaidHint,
    ) {
        // Hide hints whose label doesn't match the typed prefix.
        if !self.typed.is_empty() && !laid.label.starts_with(&self.typed) {
            return;
        }

        // -- leader outline + connector arrow (drawn first so the badge
        //    paints on top) --
        //
        // These go straight into the DIB byte buffer because GDI's
        // line/frame/polygon operations leave the alpha channel at 0,
        // which would make them invisible after UpdateLayeredWindow.
        if self.style.show_leader {
            self.draw_leader_dib(bits, dib_w, dib_h, laid);
        }

        // -- matchable badge --
        //
        // FillRect writes BGR but zeros the alpha channel; we patch
        // alpha=255 over the entire badge_rect *after* drawing the text
        // so glyph pixels (also alpha=0 from DrawText) inherit the
        // patched alpha. Net result: every pixel inside badge_rect
        // becomes fully opaque ARGB while pixels outside stay invisible.
        let bg = CreateSolidBrush(self.style.badge_bg);
        FillRect(hdc, &laid.badge_rect, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        let old = SelectObject(hdc, HGDIOBJ(self.label_font.0));
        let mut wide: Vec<u16> = laid.label.encode_utf16().collect();
        let text_rect = RECT {
            left: laid.badge_rect.left + self.style.padding_x,
            top: laid.badge_rect.top + self.style.padding_y,
            right: laid.badge_rect.right - self.style.padding_x,
            bottom: laid.badge_rect.bottom - self.style.padding_y,
        };
        if self.style.text_shadow {
            // 1px down-right shadow under the glyph. Drawn dark-grey rather
            // than pure black so it reads as depth, not as a second character.
            let mut shadow_rect = RECT {
                left: text_rect.left + 1,
                top: text_rect.top + 1,
                right: text_rect.right + 1,
                bottom: text_rect.bottom + 1,
            };
            let _ = SetTextColor(hdc, COLORREF(0x00404040));
            DrawTextW(
                hdc,
                &mut wide,
                &mut shadow_rect,
                DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        let _ = SetTextColor(hdc, self.style.badge_fg);
        let mut text_rect_mut = text_rect;
        DrawTextW(
            hdc,
            &mut wide,
            &mut text_rect_mut,
            DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old);

        // Now that all GDI pixels for this badge are written, lift the
        // entire badge rect to alpha=255 in a single sweep.
        set_alpha_in_rect(bits, dib_w, dib_h, &laid.badge_rect, 255);
        // Border: 1px frame in the configured border color, written
        // directly into the DIB so it's already alpha=255.
        frame_rect_argb(bits, dib_w, dib_h, &laid.badge_rect, self.style.border);

        // -- optional extra pill --
        if let (Some(extra_rect), Some(extra)) = (&laid.extra_rect, &laid.extra) {
            let bg = CreateSolidBrush(self.style.extra_bg);
            FillRect(hdc, extra_rect, bg);
            let _ = DeleteObject(HGDIOBJ(bg.0));

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

            set_alpha_in_rect(bits, dib_w, dib_h, extra_rect, 255);
            frame_rect_argb(bits, dib_w, dib_h, extra_rect, self.style.border);
        }
    }

    /// Visually associate the badge with its click target.
    ///
    /// Two complementary signals — both gated on
    /// [`HintStyle::show_leader`]:
    ///
    /// 1. **Target outline.** A 1px frame around the element in the
    ///    leader color. Always drawn (when the feature is on), so the
    ///    user can see at a glance which underlying control each badge
    ///    represents — even when the badge sits in the corner of a
    ///    large button and a connector line would degenerate to a stub.
    /// 2. **Connector arrow.** A line + filled triangular arrowhead
    ///    from the badge to the element. Only drawn when the badge is
    ///    visibly offset from the element (smart positioning kicked
    ///    the badge off the control to dodge a collision); for badges
    ///    that already sit inside the element the outline alone is the
    ///    cleaner visual.
    unsafe fn draw_leader_dib(&self, bits: *mut u8, dib_w: i32, dib_h: i32, laid: &LaidHint) {
        let badge = &laid.badge_rect;
        let target = &laid.target_rect;

        // Outline the target rect in the badge background color so the
        // badge↔target association is immediate (yellow badge = yellow
        // box around its element). Drawn directly into the DIB so it's
        // already alpha=255 — GDI's FrameRect would have left alpha=0.
        frame_rect_argb(bits, dib_w, dib_h, target, self.style.badge_bg);

        // Connector arrow only when the badge is meaningfully detached
        // from its target. When the badge sits inside the target the
        // outline already does the job and a tiny in-element arrow
        // just adds noise.
        let badge_inside_target = badge.left >= target.left
            && badge.right <= target.right
            && badge.top >= target.top
            && badge.bottom <= target.bottom;
        if !badge_inside_target {
            let (start, end) = leader_endpoints(badge, target);
            let dx = end.x - start.x;
            let dy = end.y - start.y;
            let len_sq = dx * dx + dy * dy;
            // Anything shorter than ~6px reads as a stray pixel rather
            // than an arrow — skip the connector but keep the outline.
            if len_sq >= 36 {
                draw_line_argb(
                    bits,
                    dib_w,
                    dib_h,
                    start.x,
                    start.y,
                    end.x,
                    end.y,
                    self.style.leader_color,
                );
                let head = arrowhead_polygon(start, end);
                fill_triangle_argb(bits, dib_w, dib_h, &head, self.style.leader_color);
            }
        }
    }

    fn key_down(&mut self, vk: u32) {
        if vk == VK_ESCAPE.0 as u32 {
            self.selected = None;
            self.done = true;
            return;
        }
        if vk == VK_BACK.0 as u32 {
            self.typed.pop();
            return;
        }
        // VK_A..VK_Z map 1:1 to ASCII 'A'..'Z'.
        if (b'A' as u32..=b'Z' as u32).contains(&vk) {
            let ch = (vk as u8 - b'A' + b'a') as char;
            self.typed.push(ch);

            if let Some(idx) = self.laid.iter().position(|h| h.label == self.typed) {
                self.selected = Some(idx);
                self.done = true;
                return;
            }

            let any_prefix = self.laid.iter().any(|h| h.label.starts_with(&self.typed));
            if !any_prefix {
                self.typed.clear();
            }
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
    // Consolas is a clean monospace font shipped with every supported
    // Windows version. Critically for the hint overlay it draws lower-case
    // `l` and capital `I` distinctly (the Segoe UI default we used through
    // v0.3.0 made the two collapse on small badges, which is one of the
    // top "I typed the wrong letter" complaints in issue #4).
    //
    // CreateFontW silently substitutes the closest available face if
    // Consolas is missing, so the explicit name is safe even on heavily
    // customised installs where shell fonts have been replaced.
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
        w!("Consolas"),
    )
}

/// Anchor strategy for placing a badge relative to its target element.
///
/// We try these in order and pick the first one that doesn't collide with
/// an already-placed badge **and** stays inside the source element's
/// monitor. The order is tuned so the layout still *feels* like "badge
/// sits on the top-left of the thing" most of the time, while giving us
/// escape hatches when two elements share the same anchor or when the
/// element is too close to a monitor edge for the preferred placement
/// to fit.
#[derive(Debug, Clone, Copy)]
enum BadgePosition {
    /// Top-left of the element. The classic vimium/keyhop look — keeps
    /// labels visually associated with the *start* of the control.
    TopLeft,
    /// Top-right of the element. Useful when the top-left of two adjacent
    /// controls would collide (think: two buttons in a toolbar).
    TopRight,
    /// Outside, just above the element. Falls off the element entirely so
    /// nothing visual is obscured. Skipped automatically when the element
    /// is too close to its monitor's top edge.
    OutsideTop,
    /// Outside, just below the element. The fallback when `OutsideTop`
    /// would clip off the top of the monitor (or off into the previous
    /// monitor on multi-monitor setups).
    OutsideBottom,
    /// Bottom-right of the element. Last "still on the element" option
    /// before we resort to vertical stacking.
    BottomRight,
}

/// Default candidate order: badge above the element first, then on it,
/// then below it as the off-element fallback. Used by the element picker
/// — small UI controls benefit from having the badge offset above them
/// so the user can see what they're about to invoke before pressing
/// the matching keys.
const ELEMENT_POSITION_CANDIDATES: [BadgePosition; 5] = [
    BadgePosition::OutsideTop,
    BadgePosition::TopLeft,
    BadgePosition::TopRight,
    BadgePosition::BottomRight,
    BadgePosition::OutsideBottom,
];

/// Window-picker candidate order: badge inside the window's top-left
/// (i.e. on the title bar) first. `OutsideTop` is dropped because for
/// any window at `y == 0` (i.e. maximized on the top of a monitor) it
/// would render above the monitor and be invisible.
const WINDOW_POSITION_CANDIDATES: [BadgePosition; 3] = [
    BadgePosition::TopLeft,
    BadgePosition::TopRight,
    BadgePosition::BottomRight,
];

/// Compute the (x, y) anchor (in client coordinates) for a given strategy.
///
/// `bounds` is the element's screen-space rect, `total_w`/`total_h` are
/// the badge+extra-pill dimensions, and `origin_x`/`origin_y` translate
/// from screen-space into client-space.
fn anchor_for(
    pos: BadgePosition,
    bounds: &Bounds,
    total_w: i32,
    total_h: i32,
    origin_x: i32,
    origin_y: i32,
) -> (i32, i32) {
    let x_base = bounds.x - origin_x;
    let y_base = bounds.y - origin_y;
    match pos {
        BadgePosition::TopLeft => (x_base, y_base),
        BadgePosition::TopRight => (x_base + bounds.width - total_w, y_base),
        BadgePosition::OutsideTop => (x_base, y_base - total_h - PILL_GAP),
        BadgePosition::OutsideBottom => (x_base, y_base + bounds.height + PILL_GAP),
        BadgePosition::BottomRight => (
            x_base + bounds.width - total_w,
            y_base + bounds.height - total_h,
        ),
    }
}

/// Bounds of the monitor whose work area contains the most of `bounds`,
/// in screen-space (physical pixels).
///
/// Returns `None` when the monitor query fails — the caller treats that
/// as "no monitor constraint, place anywhere" rather than refusing to
/// lay out.
unsafe fn monitor_for_bounds(bounds: &Bounds) -> Option<RECT> {
    let rect = RECT {
        left: bounds.x,
        top: bounds.y,
        right: bounds.x + bounds.width,
        bottom: bounds.y + bounds.height,
    };
    let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(monitor, &mut info).as_bool() {
        return None;
    }
    Some(info.rcMonitor)
}

/// Translate a screen-space `RECT` into the same client-space the
/// renderer uses (origin = top-left of the virtual desktop).
fn screen_to_client_rect(rect: RECT, origin_x: i32, origin_y: i32) -> RECT {
    RECT {
        left: rect.left - origin_x,
        top: rect.top - origin_y,
        right: rect.right - origin_x,
        bottom: rect.bottom - origin_y,
    }
}

/// True when `rect` is fully contained inside `monitor` (in the same
/// coordinate space). Used during layout to drop candidate positions
/// that would push the badge off the source element's monitor.
fn rect_inside_monitor(rect: &RECT, monitor: &RECT) -> bool {
    rect.left >= monitor.left
        && rect.top >= monitor.top
        && rect.right <= monitor.right
        && rect.bottom <= monitor.bottom
}

/// Resolve hint anchors into final draw rectangles.
///
/// Two responsibilities:
///
/// 1. Convert each hint's screen-space anchor to client-space (subtract
///    the virtual-desktop origin so labels land on the right monitor on
///    multi-monitor setups whose primary isn't the leftmost).
/// 2. Detect collisions between badges. We try a small set of anchor
///    positions (style-dependent — [`ELEMENT_POSITION_CANDIDATES`] or
///    [`WINDOW_POSITION_CANDIDATES`]) for each hint before falling
///    back to vertical stacking from the original anchor. This keeps two
///    adjacent controls from rendering badges on top of each other while
///    still keeping each badge close to its element.
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
    // Bounding box (in client coords) of every laid hint, kept dense so
    // the spatial grid can index into it; collision queries iterate the
    // grid's bucket lists, not the whole vector.
    let mut placed: Vec<RECT> = Vec::with_capacity(hints.len());
    let mut grid = SpatialGrid::with_capacity(hints.len());

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

        // Determine which monitor owns this element. We constrain badge
        // placement to that monitor so a hint at the right edge of a
        // 1920px monitor never bleeds onto the leftmost pixel column of
        // the next monitor over (a confusing UX bug on multi-monitor
        // setups). When the monitor query fails (rare; usually means
        // some odd virtual display) we fall back to "no constraint" so
        // we still produce *some* layout.
        let monitor_client = unsafe {
            monitor_for_bounds(&h.bounds)
                .map(|m| screen_to_client_rect(m, origin_x, origin_y))
        };

        // Phase 1: try each anchor strategy in order. The first one that
        // doesn't collide with an already-placed badge AND fits on the
        // source element's monitor wins. The order is style-dependent —
        // see `WINDOW_POSITION_CANDIDATES` for why the window picker
        // can't use `OutsideTop`.
        let candidates: &[BadgePosition] = if style.prefer_inside_anchor {
            &WINDOW_POSITION_CANDIDATES
        } else {
            &ELEMENT_POSITION_CANDIDATES
        };
        let mut chosen: Option<(i32, i32)> = None;
        for &pos in candidates {
            let (cx, cy) = anchor_for(pos, &h.bounds, total_w, total_h, origin_x, origin_y);
            let candidate = RECT {
                left: cx,
                top: cy,
                right: cx + total_w,
                bottom: cy + total_h,
            };
            // Cross-monitor guard: if we know the element's monitor and
            // this candidate would land outside it, reject the candidate
            // and try the next strategy. Without this, an element at
            // y == 0 with the element-style preference for `OutsideTop`
            // would render its badge on the previous monitor (or off the
            // virtual desktop entirely).
            if let Some(m) = monitor_client {
                if !rect_inside_monitor(&candidate, &m) {
                    continue;
                }
            }
            if !grid.any_intersects(&candidate, &placed) {
                chosen = Some((cx, cy));
                break;
            }
        }

        // Phase 2: fall back to "stack downward from the original anchor"
        // if every preferred position is already taken. Bounded by a
        // safety cap so a pathological input can't loop forever.
        let (x, mut y) = chosen.unwrap_or_else(|| {
            anchor_for(
                BadgePosition::TopLeft,
                &h.bounds,
                total_w,
                total_h,
                origin_x,
                origin_y,
            )
        });

        if chosen.is_none() {
            let mut attempts = 0;
            loop {
                let candidate = RECT {
                    left: x,
                    top: y,
                    right: x + total_w,
                    bottom: y + total_h,
                };
                if !grid.any_intersects(&candidate, &placed) {
                    break;
                }
                y += row_h + PILL_GAP;
                attempts += 1;
                if attempts > 256 {
                    break;
                }
            }
        }

        // Phase 3: clamp the final placement to the source monitor.
        // Even when phase-1 picked a candidate that fit, phase-2's
        // stacking loop or aggressive fallbacks may have walked the
        // badge below the monitor's bottom edge. Pull it back up
        // instead of letting it leak to the next monitor.
        let (mut x, mut y) = (x, y);
        if let Some(m) = monitor_client {
            if x + total_w > m.right {
                x = m.right - total_w;
            }
            if x < m.left {
                x = m.left;
            }
            if y + total_h > m.bottom {
                y = m.bottom - total_h;
            }
            if y < m.top {
                y = m.top;
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

        let bbox = RECT {
            left: x,
            top: y,
            right: x + total_w,
            bottom: y + total_h,
        };
        let idx = placed.len();
        placed.push(bbox);
        grid.insert(idx, &bbox);
        // Element rect in client space — the leader-line painter needs
        // it later to figure out where each badge should point.
        let target_rect = RECT {
            left: h.bounds.x - origin_x,
            top: h.bounds.y - origin_y,
            right: h.bounds.x - origin_x + h.bounds.width,
            bottom: h.bounds.y - origin_y + h.bounds.height,
        };
        result[i] = Some(LaidHint {
            label: h.label.clone(),
            extra: h.extra.clone(),
            badge_rect,
            extra_rect,
            target_rect,
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

/// Cell side, in client pixels, for [`SpatialGrid`]. Tuned to roughly
/// the size of a window-style badge so the typical badge touches 1-4
/// cells, never enough to make the per-cell list grow past `SmallVec`'s
/// inline capacity.
const GRID_CELL: i32 = 64;

/// Hash-bucketed broad-phase index for badge bounding boxes. Each cell
/// holds the indices (into the caller-owned `Vec<RECT>`) of every
/// placed rect that touches the cell. Collision queries hit only the
/// cells the candidate overlaps, dropping `lay_out`'s collision pass
/// from O(n²) to amortized O(n) for our typical n = 50-800 badges.
///
/// Key choices:
/// - `i32` cell coords let us bucket the full virtual-desktop range
///   without overflow even for the +/-32k px coords on extreme
///   multi-monitor setups.
/// - `SmallVec<[usize; 8]>` inlines the bucket contents until a cell
///   gets unusually crowded, which keeps the per-bucket allocation
///   count at zero for the common case.
struct SpatialGrid {
    cells: HashMap<(i32, i32), SmallVec<[usize; 8]>>,
}

impl SpatialGrid {
    fn with_capacity(hint_count: usize) -> Self {
        // Each badge typically lights up 1-4 cells, so reserving for
        // ~2x the badge count avoids most rehash bumps without
        // over-allocating for tiny picks.
        Self {
            cells: HashMap::with_capacity(hint_count.saturating_mul(2)),
        }
    }

    /// Range of cell coordinates a rect overlaps, expressed as the
    /// half-open `(x_lo..x_hi, y_lo..y_hi)`.
    fn cell_range(rect: &RECT) -> (i32, i32, i32, i32) {
        let x_lo = rect.left.div_euclid(GRID_CELL);
        let y_lo = rect.top.div_euclid(GRID_CELL);
        // -1 on the upper edge so a rect that ends exactly on a cell
        // boundary doesn't claim the next cell over (right/bottom in
        // RECT are exclusive).
        let x_hi = (rect.right - 1).div_euclid(GRID_CELL);
        let y_hi = (rect.bottom - 1).div_euclid(GRID_CELL);
        (x_lo, y_lo, x_hi, y_hi)
    }

    fn insert(&mut self, idx: usize, rect: &RECT) {
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return;
        }
        let (x_lo, y_lo, x_hi, y_hi) = Self::cell_range(rect);
        for cy in y_lo..=y_hi {
            for cx in x_lo..=x_hi {
                self.cells.entry((cx, cy)).or_default().push(idx);
            }
        }
    }

    /// True if any previously-inserted rect (looked up via `placed`)
    /// intersects `candidate`. Visits each candidate-id at most once
    /// per cell, then deduplicates via a tiny stack-allocated set so
    /// rects spanning multiple cells don't get tested twice.
    fn any_intersects(&self, candidate: &RECT, placed: &[RECT]) -> bool {
        if candidate.right <= candidate.left || candidate.bottom <= candidate.top {
            return false;
        }
        let (x_lo, y_lo, x_hi, y_hi) = Self::cell_range(candidate);
        let mut seen: SmallVec<[usize; 16]> = SmallVec::new();
        for cy in y_lo..=y_hi {
            for cx in x_lo..=x_hi {
                let Some(bucket) = self.cells.get(&(cx, cy)) else {
                    continue;
                };
                for &idx in bucket {
                    if seen.contains(&idx) {
                        continue;
                    }
                    seen.push(idx);
                    if rects_intersect(&placed[idx], candidate) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Pick start/end points for the leader line connecting `badge` to
/// `target`. Returns `(start_on_badge, end_on_target)`.
///
/// We classify the relative position of the badge along each axis
/// (left/inside/right and above/inside/below) and pick the edges that
/// face each other. For axis-aligned cases (badge purely above /
/// purely beside the target) this collapses to a clean perpendicular
/// arrow; for diagonal cases (badge in a corner outside the element)
/// we get a clean diagonal pointing at the nearest corner of the
/// element.
fn leader_endpoints(badge: &RECT, target: &RECT) -> (POINT, POINT) {
    // Horizontal classification.
    let (badge_x, target_x) = if badge.right <= target.left {
        (badge.right, target.left)
    } else if badge.left >= target.right {
        (badge.left, target.right)
    } else {
        // Overlap on this axis — meet at the centre of the overlap so
        // the arrow stays inside both rects horizontally.
        let cx = (badge.left.max(target.left) + badge.right.min(target.right)) / 2;
        (cx, cx)
    };

    let (badge_y, target_y) = if badge.bottom <= target.top {
        (badge.bottom, target.top)
    } else if badge.top >= target.bottom {
        (badge.top, target.bottom)
    } else {
        let cy = (badge.top.max(target.top) + badge.bottom.min(target.bottom)) / 2;
        (cy, cy)
    };

    (
        POINT {
            x: badge_x,
            y: badge_y,
        },
        POINT {
            x: target_x,
            y: target_y,
        },
    )
}

/// Build the three vertices of a small filled triangle pointing from
/// `start` toward `end`, with the tip at `end`.
///
/// The arrowhead is a fixed ~6×6 px triangle — big enough to read at a
/// glance without competing with the badge for visual weight.
fn arrowhead_polygon(start: POINT, end: POINT) -> [POINT; 3] {
    const HEAD_LEN: f32 = 7.0;
    const HEAD_HALF_W: f32 = 4.0;

    let dx = (end.x - start.x) as f32;
    let dy = (end.y - start.y) as f32;
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let ux = dx / len;
    let uy = dy / len;
    // Perpendicular to (ux, uy), rotated 90° clockwise.
    let px = -uy;
    let py = ux;

    let base_x = end.x as f32 - ux * HEAD_LEN;
    let base_y = end.y as f32 - uy * HEAD_LEN;

    let left = POINT {
        x: (base_x + px * HEAD_HALF_W).round() as i32,
        y: (base_y + py * HEAD_HALF_W).round() as i32,
    };
    let right = POINT {
        x: (base_x - px * HEAD_HALF_W).round() as i32,
        y: (base_y - py * HEAD_HALF_W).round() as i32,
    };
    [end, left, right]
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        let state_ptr = (*cs).lpCreateParams;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }

    // GWLP_USERDATA points at the OverlayState owned by the active
    // pick_hint() call. It's null between picks (the persistent HWND is
    // hidden), in which case we just forward to DefWindowProc — the
    // window has nothing to render or react to.
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let state = &mut *state_ptr;

    match msg {
        // ULW_ALPHA layered windows don't get WM_PAINT for their content
        // (DWM composites directly from the DIB we hand it via
        // UpdateLayeredWindow). Swallow it so DefWindowProcW doesn't try
        // to BeginPaint/EndPaint on the layered surface.
        WM_PAINT => LRESULT(0),
        WM_KEYDOWN => {
            state.key_down(wp.0 as u32);
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            // Treat focus loss as cancel — no half-committed state.
            state.selected = None;
            state.done = true;
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
///
/// The HWND, off-screen DIB, and memory DC are owned by a thread-local
/// [`PersistentOverlay`] and reused across calls; only the per-pick
/// [`OverlayState`] is allocated/freed each time. This drops the
/// per-pick window-creation latency (~20-40 ms on Windows 11) to zero
/// after the first hotkey press.
pub fn pick_hint(hints: Vec<Hint>, style: HintStyle) -> Result<Option<usize>> {
    if hints.is_empty() {
        return Ok(None);
    }

    PERSISTENT_OVERLAY.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(unsafe { PersistentOverlay::new()? });
        }
        let overlay = slot.as_mut().expect("just initialized");
        unsafe { overlay.run_pick(hints, style) }
    })
}

// Per-thread cache of the persistent overlay. Hotkey handling is
// single-threaded (it runs on the main message loop), so a thread-local
// is enough to make the HWND/DIB/DC truly process-wide.
thread_local! {
    static PERSISTENT_OVERLAY: RefCell<Option<PersistentOverlay>> = const { RefCell::new(None) };
}

/// Persistent rendering resources for the overlay: the layered HWND
/// covering the virtual desktop, an off-screen ARGB DIB sized to that
/// desktop, and a memory DC bound to the DIB. Recreated lazily on a
/// virtual-desktop size change (monitor connect/disconnect).
struct PersistentOverlay {
    hwnd: HWND,
    mem_dc: HDC,
    dib: HBITMAP,
    bits: *mut u8,
    width: i32,
    height: i32,
    origin_x: i32,
    origin_y: i32,
}

impl PersistentOverlay {
    unsafe fn new() -> Result<Self> {
        // Best-effort: tag the process as PerMonitorV2 DPI-aware so
        // source pixel coords line up with our overlay coords on
        // high-DPI displays. Safe to call repeatedly; ignore "already
        // set" / older-OS errors.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let hinstance: HINSTANCE = GetModuleHandleW(PCWSTR::null())?.into();
        ensure_class_registered(hinstance)?;

        let (vx, vy, vw, vh) = virtual_screen_rect();

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS_NAME,
            WINDOW_TITLE,
            WS_POPUP,
            vx,
            vy,
            vw,
            vh,
            None,
            None,
            hinstance,
            None,
        )
        .context("CreateWindowExW failed")?;

        let (mem_dc, dib, bits) = create_dib_surface(vw, vh)
            .context("failed to allocate overlay DIB surface")?;

        let _ = hinstance;
        Ok(Self {
            hwnd,
            mem_dc,
            dib,
            bits,
            width: vw,
            height: vh,
            origin_x: vx,
            origin_y: vy,
        })
    }

    /// Reallocate DIB + reposition HWND if the virtual desktop changed
    /// (e.g. monitor hot-plug). Idempotent when nothing has moved.
    unsafe fn ensure_geometry(&mut self) -> Result<()> {
        let (vx, vy, vw, vh) = virtual_screen_rect();
        if vx == self.origin_x && vy == self.origin_y && vw == self.width && vh == self.height {
            return Ok(());
        }
        let _ = SetWindowPos(self.hwnd, None, vx, vy, vw, vh, SWP_NOZORDER | SWP_NOACTIVATE);
        if vw != self.width || vh != self.height {
            // Tear down the old DIB before allocating the new one so we
            // don't double the peak memory footprint on reconnects.
            let _ = DeleteDC(self.mem_dc);
            let _ = DeleteObject(HGDIOBJ(self.dib.0));
            let (mem_dc, dib, bits) = create_dib_surface(vw, vh)
                .context("failed to reallocate overlay DIB surface")?;
            self.mem_dc = mem_dc;
            self.dib = dib;
            self.bits = bits;
            self.width = vw;
            self.height = vh;
        }
        self.origin_x = vx;
        self.origin_y = vy;
        Ok(())
    }

    unsafe fn run_pick(&mut self, hints: Vec<Hint>, style: HintStyle) -> Result<Option<usize>> {
        self.ensure_geometry()?;

        let (vx, vy, vw, vh) = (self.origin_x, self.origin_y, self.width, self.height);
        tracing::debug!(
            vx,
            vy,
            vw,
            vh,
            hint_count = hints.len(),
            "overlay virtual desktop rect"
        );

        let opacity = style.badge_opacity;
        let mut state = OverlayState::new(hints, style, vx, vy);

        // Bind the state pointer to the persistent HWND so wnd_proc can
        // route key events into it. We clear the pointer before
        // dropping the state so a stray late message can't dereference
        // freed memory.
        SetWindowLongPtrW(
            self.hwnd,
            GWLP_USERDATA,
            (&mut state as *mut OverlayState) as isize,
        );

        // Initial render + show. ShowWindow with SW_SHOWNOACTIVATE keeps
        // the previously focused app visually unchanged underneath the
        // overlay; we still SetForegroundWindow + SetFocus so key
        // events route to us.
        state.render_to_dib(self.hwnd, self.mem_dc, self.bits, self.width, self.height, opacity);
        let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        let _ = SetForegroundWindow(self.hwnd);
        let _ = SetFocus(self.hwnd);

        // Modal pump: spin until the state flags `done`. After every
        // dispatched message we re-render so live prefix-typing visibly
        // hides non-matching badges. Clippy can't see through the
        // wnd_proc indirection that flips `state.done`, so silence the
        // "not mutated in the loop body" lint here.
        let mut last_typed_len = state.typed.len();
        let mut msg = MSG::default();
        #[allow(clippy::while_immutable_condition)]
        while !state.done {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if state.done {
                break;
            }
            if state.typed.len() != last_typed_len {
                state.render_to_dib(
                    self.hwnd,
                    self.mem_dc,
                    self.bits,
                    self.width,
                    self.height,
                    opacity,
                );
                last_typed_len = state.typed.len();
            }
        }

        // Hide first, then unbind the state pointer — the order matters
        // because ShowWindow can pump a few WM_KILLFOCUS / WM_NCACTIVATE
        // messages synchronously and we want them to find a still-valid
        // state (or a null pointer that wnd_proc safely no-ops on).
        let _ = ShowWindow(self.hwnd, SW_HIDE);
        SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);

        Ok(state.selected)
    }
}

impl Drop for PersistentOverlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.mem_dc);
            let _ = DeleteObject(HGDIOBJ(self.dib.0));
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
        }
    }
}

/// Allocate a top-down 32-bit BGRA DIB section, bind it to a fresh
/// memory DC, and return raw pointers to all three. The caller owns
/// the lifetimes and must DeleteDC + DeleteObject when done.
unsafe fn create_dib_surface(width: i32, height: i32) -> Result<(HDC, HBITMAP, *mut u8)> {
    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(screen_dc);
    let _ = ReleaseDC(None, screen_dc);
    if mem_dc.is_invalid() {
        return Err(anyhow!("CreateCompatibleDC returned NULL"));
    }

    // Negative biHeight = top-down, which lines up with our (x, y)
    // pixel-addressing convention so we don't have to flip rows when
    // patching alpha.
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(mem_dc, &mut bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        .context("CreateDIBSection failed")?;
    if bits.is_null() {
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem_dc);
        return Err(anyhow!("CreateDIBSection returned NULL bits"));
    }
    let _ = SelectObject(mem_dc, HGDIOBJ(dib.0));

    Ok((mem_dc, dib, bits as *mut u8))
}

// -- Direct ARGB rasterization helpers --------------------------------
//
// GDI on a 32-bit DIB writes BGR but leaves the alpha byte at 0, so
// anything we want visible after UpdateLayeredWindow either has to be
// patched up post-hoc (`set_alpha_in_rect`) or written directly to the
// pixel buffer with the right ARGB tuple (`*_argb` helpers below).
//
// All of these treat `bits` as a top-down BGRA buffer of `dib_w * dib_h`
// pixels and silently clip writes that fall outside the buffer.

/// Lift the alpha byte to `alpha` for every pixel in `rect ∩ dib`.
/// Used after a FillRect/DrawText sequence so the just-drawn region
/// becomes opaque in one sweep.
unsafe fn set_alpha_in_rect(bits: *mut u8, dib_w: i32, dib_h: i32, rect: &RECT, alpha: u8) {
    if bits.is_null() {
        return;
    }
    let x0 = rect.left.max(0);
    let y0 = rect.top.max(0);
    let x1 = rect.right.min(dib_w);
    let y1 = rect.bottom.min(dib_h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let stride = (dib_w as usize) * 4;
    for y in y0..y1 {
        let row = bits.add((y as usize) * stride);
        // We could SIMD this but the badge rects are tiny (≤200 px
        // wide × ~30 px tall) — the scalar loop fits in L1 trivially.
        let mut p = row.add((x0 as usize) * 4 + 3);
        for _ in x0..x1 {
            *p = alpha;
            p = p.add(4);
        }
    }
}

/// Write a single fully-opaque pixel at `(x, y)` in `color` (BGR).
#[inline]
unsafe fn put_pixel_argb(bits: *mut u8, dib_w: i32, dib_h: i32, x: i32, y: i32, color: COLORREF) {
    if x < 0 || y < 0 || x >= dib_w || y >= dib_h || bits.is_null() {
        return;
    }
    let stride = (dib_w as usize) * 4;
    let p = bits.add((y as usize) * stride + (x as usize) * 4);
    let bgr = color.0;
    *p = (bgr & 0xFF) as u8; // B
    *p.add(1) = ((bgr >> 8) & 0xFF) as u8; // G
    *p.add(2) = ((bgr >> 16) & 0xFF) as u8; // R
    *p.add(3) = 0xFF; // A
}

/// 1px-thick filled rect (interior) in ARGB. Used for the four edges
/// of [`frame_rect_argb`] and as a building block for short connector
/// arrows; cheap enough to inline into the line/polygon helpers.
unsafe fn fill_rect_argb(bits: *mut u8, dib_w: i32, dib_h: i32, rect: &RECT, color: COLORREF) {
    let x0 = rect.left.max(0);
    let y0 = rect.top.max(0);
    let x1 = rect.right.min(dib_w);
    let y1 = rect.bottom.min(dib_h);
    if x0 >= x1 || y0 >= y1 || bits.is_null() {
        return;
    }
    let stride = (dib_w as usize) * 4;
    let bgr = color.0;
    let b = (bgr & 0xFF) as u8;
    let g = ((bgr >> 8) & 0xFF) as u8;
    let r = ((bgr >> 16) & 0xFF) as u8;
    for y in y0..y1 {
        let row = bits.add((y as usize) * stride);
        let mut p = row.add((x0 as usize) * 4);
        for _ in x0..x1 {
            *p = b;
            *p.add(1) = g;
            *p.add(2) = r;
            *p.add(3) = 0xFF;
            p = p.add(4);
        }
    }
}

/// 1px frame around `rect`, drawn directly into the DIB. Mirrors
/// `FrameRect`'s semantics (rect.right and rect.bottom are exclusive)
/// so callers can keep the same coordinates they used with GDI.
unsafe fn frame_rect_argb(bits: *mut u8, dib_w: i32, dib_h: i32, rect: &RECT, color: COLORREF) {
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    let top = RECT { left: rect.left, top: rect.top, right: rect.right, bottom: rect.top + 1 };
    let bottom = RECT {
        left: rect.left,
        top: rect.bottom - 1,
        right: rect.right,
        bottom: rect.bottom,
    };
    let left = RECT { left: rect.left, top: rect.top, right: rect.left + 1, bottom: rect.bottom };
    let right = RECT {
        left: rect.right - 1,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    };
    fill_rect_argb(bits, dib_w, dib_h, &top, color);
    fill_rect_argb(bits, dib_w, dib_h, &bottom, color);
    fill_rect_argb(bits, dib_w, dib_h, &left, color);
    fill_rect_argb(bits, dib_w, dib_h, &right, color);
}

/// Bresenham line rasterizer that writes ARGB pixels directly. We use
/// it instead of `LineTo` because GDI line drawing leaves the alpha
/// channel at 0 (so the line would be invisible after
/// UpdateLayeredWindow even though the pixels were written).
unsafe fn draw_line_argb(
    bits: *mut u8,
    dib_w: i32,
    dib_h: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: COLORREF,
) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        put_pixel_argb(bits, dib_w, dib_h, x, y, color);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Filled triangle in ARGB via a simple scanline fill. The arrowhead
/// triangles we draw are at most ~7 px on a side, so a barycentric or
/// edge-walk approach would be over-engineering — bounding-box +
/// orientation test is plenty.
unsafe fn fill_triangle_argb(
    bits: *mut u8,
    dib_w: i32,
    dib_h: i32,
    pts: &[POINT; 3],
    color: COLORREF,
) {
    let min_x = pts.iter().map(|p| p.x).min().unwrap_or(0).max(0);
    let max_x = pts.iter().map(|p| p.x).max().unwrap_or(0).min(dib_w - 1);
    let min_y = pts.iter().map(|p| p.y).min().unwrap_or(0).max(0);
    let max_y = pts.iter().map(|p| p.y).max().unwrap_or(0).min(dib_h - 1);
    if min_x > max_x || min_y > max_y {
        return;
    }
    // Edge-function sign test: a point is inside the triangle iff it
    // lies on the same side of all three edges. We pick the convention
    // by sampling the centroid so the test works for both winding
    // orders (the arrowhead helper doesn't guarantee CW vs CCW).
    let edge = |ax: i32, ay: i32, bx: i32, by: i32, cx: i32, cy: i32| -> i32 {
        (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)
    };
    let cx = (pts[0].x + pts[1].x + pts[2].x) / 3;
    let cy = (pts[0].y + pts[1].y + pts[2].y) / 3;
    let s0 = edge(pts[0].x, pts[0].y, pts[1].x, pts[1].y, cx, cy);
    let s1 = edge(pts[1].x, pts[1].y, pts[2].x, pts[2].y, cx, cy);
    let s2 = edge(pts[2].x, pts[2].y, pts[0].x, pts[0].y, cx, cy);
    let want_pos = s0 >= 0 && s1 >= 0 && s2 >= 0;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let e0 = edge(pts[0].x, pts[0].y, pts[1].x, pts[1].y, x, y);
            let e1 = edge(pts[1].x, pts[1].y, pts[2].x, pts[2].y, x, y);
            let e2 = edge(pts[2].x, pts[2].y, pts[0].x, pts[0].y, x, y);
            let inside = if want_pos {
                e0 >= 0 && e1 >= 0 && e2 >= 0
            } else {
                e0 <= 0 && e1 <= 0 && e2 <= 0
            };
            if inside {
                put_pixel_argb(bits, dib_w, dib_h, x, y, color);
            }
        }
    }
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

#[cfg(test)]
mod opacity_tests {
    use super::*;

    #[test]
    fn opacity_zero_keeps_default() {
        let mut alpha: u8 = 230;
        apply_opacity_override(&mut alpha, 0);
        assert_eq!(alpha, 230, "0% must mean 'preset default'");
    }

    #[test]
    fn opacity_full_is_max() {
        let mut alpha: u8 = 100;
        apply_opacity_override(&mut alpha, 100);
        assert_eq!(alpha, 255);
    }

    #[test]
    fn opacity_half_is_about_half() {
        let mut alpha: u8 = 0;
        apply_opacity_override(&mut alpha, 50);
        assert!(
            alpha > 120 && alpha < 135,
            "50% should be ~127, got {alpha}"
        );
    }

    #[test]
    fn opacity_over_one_hundred_clamps() {
        let mut alpha: u8 = 0;
        apply_opacity_override(&mut alpha, 250);
        assert_eq!(alpha, 255, "values > 100% must clamp to fully opaque");
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn element_style() -> HintStyle {
        HintStyle::elements()
    }

    fn window_style() -> HintStyle {
        HintStyle::windows()
    }

    fn hint(x: i32, y: i32, w: i32, h: i32, label: &str) -> Hint {
        Hint {
            bounds: Bounds {
                x,
                y,
                width: w,
                height: h,
            },
            label: label.to_string(),
            extra: None,
        }
    }

    #[test]
    fn element_style_places_single_hint_outside_top() {
        // The element style prefers `OutsideTop` so the badge sits just
        // above the control instead of covering it. For an element at
        // (100, 200) the badge top should be lifted by `font_height +
        // padding_y * 2 + PILL_GAP`.
        let style = element_style();
        let laid = lay_out(&[hint(100, 200, 50, 30, "a")], &style, 0, 0);
        let row_h = style.font_height + style.padding_y * 2;
        assert_eq!(laid[0].badge_rect.left, 100);
        assert_eq!(laid[0].badge_rect.top, 200 - row_h - PILL_GAP);
    }

    #[test]
    fn window_style_places_single_hint_inside_top_left() {
        // The window style prefers `TopLeft` because window targets are
        // typically maximized — `OutsideTop` would push the badge above
        // the monitor and make it invisible.
        let style = window_style();
        let laid = lay_out(&[hint(100, 200, 800, 600, "a")], &style, 0, 0);
        assert_eq!(laid[0].badge_rect.left, 100);
        assert_eq!(laid[0].badge_rect.top, 200);
    }

    #[test]
    fn second_hint_sharing_anchor_picks_alternate_position() {
        // Two hints at the same anchor — the smart-positioning pass should
        // shove the second one to a different corner instead of stacking
        // it directly below. We use the window style here so the test
        // reasons in concrete top-left coordinates without having to
        // subtract the OutsideTop offset.
        let style = window_style();
        let laid = lay_out(
            &[hint(0, 0, 200, 100, "a"), hint(0, 0, 200, 100, "b")],
            &style,
            0,
            0,
        );
        // First hint takes the top-left.
        assert_eq!(laid[0].badge_rect.left, 0);
        assert_eq!(laid[0].badge_rect.top, 0);
        // Second hint must end up somewhere that doesn't overlap the
        // first — exact corner depends on which candidate frees first,
        // but it should NOT be at (0, 0).
        assert!(
            laid[1].badge_rect.left != 0 || laid[1].badge_rect.top != 0,
            "second hint should pick a non-colliding alternate position"
        );
    }

    #[test]
    fn anchor_for_top_right_subtracts_badge_width() {
        let bounds = Bounds {
            x: 100,
            y: 200,
            width: 300,
            height: 50,
        };
        let (x, _y) = anchor_for(BadgePosition::TopRight, &bounds, 60, 24, 0, 0);
        assert_eq!(x, 100 + 300 - 60);
    }

    #[test]
    fn anchor_for_outside_top_lifts_above() {
        let bounds = Bounds {
            x: 0,
            y: 100,
            width: 50,
            height: 30,
        };
        let (_x, y) = anchor_for(BadgePosition::OutsideTop, &bounds, 40, 24, 0, 0);
        assert!(y < 100, "OutsideTop must place the badge above the element");
    }

    #[test]
    fn anchor_for_outside_bottom_drops_below() {
        let bounds = Bounds {
            x: 0,
            y: 100,
            width: 50,
            height: 30,
        };
        let (_x, y) = anchor_for(BadgePosition::OutsideBottom, &bounds, 40, 24, 0, 0);
        assert!(
            y > 100 + 30,
            "OutsideBottom must place the badge fully below the element"
        );
    }

    #[test]
    fn rect_inside_monitor_accepts_contained_rect() {
        let monitor = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let inside = RECT {
            left: 100,
            top: 100,
            right: 200,
            bottom: 130,
        };
        assert!(rect_inside_monitor(&inside, &monitor));
    }

    #[test]
    fn rect_inside_monitor_rejects_overlap_left_edge() {
        let monitor = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let leaks = RECT {
            left: -10,
            top: 100,
            right: 50,
            bottom: 130,
        };
        assert!(!rect_inside_monitor(&leaks, &monitor));
    }

    #[test]
    fn rect_inside_monitor_rejects_overlap_top_edge() {
        // Element at y=0 with OutsideTop placement would push the badge
        // above the monitor — that's exactly the case smart positioning
        // exists to detect.
        let monitor = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let leaks = RECT {
            left: 100,
            top: -30,
            right: 200,
            bottom: 0,
        };
        assert!(!rect_inside_monitor(&leaks, &monitor));
    }

    #[test]
    fn screen_to_client_rect_subtracts_origin() {
        let r = RECT {
            left: 1920,
            top: 0,
            right: 3840,
            bottom: 1080,
        };
        let client = screen_to_client_rect(r, 100, 50);
        assert_eq!(client.left, 1820);
        assert_eq!(client.top, -50);
        assert_eq!(client.right, 3740);
        assert_eq!(client.bottom, 1030);
    }

    #[test]
    fn lay_out_records_target_rect_in_client_space() {
        // origin offset must be subtracted from every target rect so the
        // leader painter doesn't have to know about virtual-desktop coords.
        let laid = lay_out(&[hint(150, 250, 60, 40, "a")], &element_style(), 100, 200);
        assert_eq!(laid[0].target_rect.left, 50);
        assert_eq!(laid[0].target_rect.top, 50);
        assert_eq!(laid[0].target_rect.right, 110);
        assert_eq!(laid[0].target_rect.bottom, 90);
    }
}

#[cfg(test)]
mod spatial_grid_tests {
    use super::*;

    fn r(l: i32, t: i32, w: i32, h: i32) -> RECT {
        RECT { left: l, top: t, right: l + w, bottom: t + h }
    }

    #[test]
    fn empty_grid_reports_no_collision() {
        let grid = SpatialGrid::with_capacity(0);
        let placed: Vec<RECT> = Vec::new();
        assert!(!grid.any_intersects(&r(0, 0, 50, 20), &placed));
    }

    #[test]
    fn detects_collision_within_same_cell() {
        let mut grid = SpatialGrid::with_capacity(2);
        let placed = vec![r(0, 0, 30, 30)];
        grid.insert(0, &placed[0]);
        // Candidate overlaps placed[0] inside the same 64-px cell.
        assert!(grid.any_intersects(&r(10, 10, 20, 20), &placed));
    }

    #[test]
    fn ignores_rect_in_unrelated_cell() {
        let mut grid = SpatialGrid::with_capacity(2);
        let placed = vec![r(0, 0, 20, 20)];
        grid.insert(0, &placed[0]);
        // Candidate at (500, 500) — far away, no shared cell.
        assert!(!grid.any_intersects(&r(500, 500, 20, 20), &placed));
    }

    #[test]
    fn dedupes_rects_spanning_multiple_cells() {
        // A wide rect spans cells (0,0) and (1,0). The candidate also
        // spans both cells — without dedupe we'd test the placed rect
        // twice. Correctness is unaffected, but the test pins the
        // optimization in place via a side-channel: a degenerate
        // candidate that fits between two large rects must not falsely
        // report collision because of double-visiting.
        let mut grid = SpatialGrid::with_capacity(2);
        let placed = vec![r(0, 0, 100, 20), r(0, 100, 100, 20)];
        for (i, rect) in placed.iter().enumerate() {
            grid.insert(i, rect);
        }
        // Candidate sits in the gap between the two placed rects but
        // spans the same horizontal cells.
        assert!(!grid.any_intersects(&r(20, 40, 60, 20), &placed));
    }

    #[test]
    fn handles_negative_coordinates() {
        // Multi-monitor setups with the primary in the middle put the
        // virtual desktop's leftmost monitor at negative client coords.
        let mut grid = SpatialGrid::with_capacity(2);
        let placed = vec![r(-100, -50, 40, 40)];
        grid.insert(0, &placed[0]);
        assert!(grid.any_intersects(&r(-90, -40, 20, 20), &placed));
        assert!(!grid.any_intersects(&r(200, 200, 20, 20), &placed));
    }
}

#[cfg(test)]
mod leader_tests {
    use super::*;

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    #[test]
    fn endpoints_for_badge_above_target_meet_at_facing_edges() {
        // Badge sits directly above the element with a vertical gap.
        let badge = rect(100, 50, 140, 70);
        let target = rect(100, 100, 200, 200);
        let (start, end) = leader_endpoints(&badge, &target);
        // Start should leave the bottom of the badge…
        assert_eq!(start.y, 70);
        // …and land on the top of the target.
        assert_eq!(end.y, 100);
        // X stays inside the horizontal overlap.
        assert!(start.x >= 100 && start.x <= 140);
        assert!(end.x >= 100 && end.x <= 140);
    }

    #[test]
    fn endpoints_for_badge_inside_target_collapse_axes() {
        // Badge sits in the top-left of the target — overlapping on both
        // axes. The painter skips drawing in this case, but
        // `leader_endpoints` should still return non-degenerate points
        // for any caller that asks (no panics, in particular).
        let badge = rect(100, 100, 140, 120);
        let target = rect(100, 100, 300, 300);
        let (start, end) = leader_endpoints(&badge, &target);
        // Both x and y axes overlap, so start == end (zero-length line).
        assert_eq!(start.x, end.x);
        assert_eq!(start.y, end.y);
    }

    #[test]
    fn arrowhead_tip_lands_on_end_point() {
        let start = POINT { x: 0, y: 0 };
        let end = POINT { x: 100, y: 0 };
        let head = arrowhead_polygon(start, end);
        assert_eq!(head[0].x, 100);
        assert_eq!(head[0].y, 0);
        // The other two points form the back of the triangle, behind the tip.
        assert!(head[1].x < 100);
        assert!(head[2].x < 100);
    }

    #[test]
    fn arrowhead_for_diagonal_keeps_tip_at_end() {
        let start = POINT { x: 0, y: 0 };
        let end = POINT { x: 30, y: 40 };
        let head = arrowhead_polygon(start, end);
        assert_eq!(head[0].x, 30);
        assert_eq!(head[0].y, 40);
    }
}
