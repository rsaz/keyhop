//! Win32-native hint overlay.
//!
//! [`show_overlay`] creates a transparent, always-on-top, layered popup
//! window that covers the virtual desktop, draws a label per hint, and runs
//! its own message loop until the user types a complete label or presses
//! `Esc` (or focus is stolen).
//!
//! Transparency is implemented with the cheapest, most-compatible mechanism:
//! a layered window with a magenta color key. The window body is filled with
//! magenta on every paint; only the actual label rectangles render real
//! pixels.

use std::ffi::c_void;

use anyhow::{anyhow, Context, Result};
use keyhop_core::{Element, ElementId};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    FrameRect, InvalidateRect, SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CALCRECT, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_TOP, FF_DONTCARE, FW_BOLD, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, TRANSPARENT,
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

const CLASS_NAME: PCWSTR = w!("KeyhopOverlayClass");
const WINDOW_TITLE: PCWSTR = w!("Keyhop Overlay");

/// Magenta — used as the layered-window color key for transparency. Anything
/// painted in this exact color becomes see-through.
const TRANSPARENT_KEY: COLORREF = COLORREF(0x00FF00FF);
/// Vimium-style yellow label background. NOTE: COLORREF is little-endian BGR.
const LABEL_BG: COLORREF = COLORREF(0x0000E5FF);
const LABEL_FG: COLORREF = COLORREF(0x00000000);
const LABEL_BORDER: COLORREF = COLORREF(0x00202020);

const FONT_HEIGHT: i32 = 20;
const LABEL_PADDING_X: i32 = 6;
const LABEL_PADDING_Y: i32 = 2;

/// Inputs to [`show_overlay`].
pub struct OverlayConfig {
    /// Pairs of `(element, hint label)`. Labels should already be lowercase
    /// home-row strings produced by [`keyhop_core::HintEngine`].
    pub hints: Vec<(Element, String)>,
}

/// Outcome of an overlay session.
#[derive(Debug)]
pub enum OverlayResult {
    /// User typed a label that matched exactly one element.
    Selected(ElementId),
    /// User pressed `Esc`, focus was stolen, or no hints were supplied.
    Cancelled,
}

struct OverlayState {
    hints: Vec<(Element, String)>,
    typed: String,
    result: Option<OverlayResult>,
    font: HFONT,
}

impl OverlayState {
    unsafe fn new(hints: Vec<(Element, String)>) -> Self {
        Self {
            hints,
            typed: String::new(),
            result: None,
            font: create_label_font(),
        }
    }

    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);

        // Fill entire client area with the color-key value so the underlying
        // desktop shows through.
        let bg = CreateSolidBrush(TRANSPARENT_KEY);
        FillRect(hdc, &ps.rcPaint, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        let old_font = SelectObject(hdc, HGDIOBJ(self.font.0));
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, LABEL_FG);

        for (element, label) in &self.hints {
            self.draw_label(hdc, element, label);
        }

        let _ = SelectObject(hdc, old_font);
        let _ = EndPaint(hwnd, &ps);
    }

    unsafe fn draw_label(&self, hdc: HDC, element: &Element, label: &str) {
        // Hide labels whose prefix doesn't match the typed buffer.
        if !self.typed.is_empty() && !label.starts_with(&self.typed) {
            return;
        }

        let mut wide: Vec<u16> = label.encode_utf16().collect();
        let mut text_rect = RECT::default();
        DrawTextW(
            hdc,
            &mut wide,
            &mut text_rect,
            DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
        );
        let text_w = text_rect.right - text_rect.left;
        let text_h = text_rect.bottom - text_rect.top;

        let label_x = element.bounds.x;
        let label_y = element.bounds.y;
        let label_rect = RECT {
            left: label_x,
            top: label_y,
            right: label_x + text_w + LABEL_PADDING_X * 2,
            bottom: label_y + text_h + LABEL_PADDING_Y * 2,
        };

        let bg_brush: HBRUSH = CreateSolidBrush(LABEL_BG);
        FillRect(hdc, &label_rect, bg_brush);
        let _ = DeleteObject(HGDIOBJ(bg_brush.0));

        let border_brush: HBRUSH = CreateSolidBrush(LABEL_BORDER);
        FrameRect(hdc, &label_rect, border_brush);
        let _ = DeleteObject(HGDIOBJ(border_brush.0));

        let mut text_draw = RECT {
            left: label_rect.left + LABEL_PADDING_X,
            top: label_rect.top + LABEL_PADDING_Y,
            right: label_rect.right - LABEL_PADDING_X,
            bottom: label_rect.bottom - LABEL_PADDING_Y,
        };
        DrawTextW(
            hdc,
            &mut wide,
            &mut text_draw,
            DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
        );
    }

    unsafe fn key_down(&mut self, hwnd: HWND, vk: u32) {
        if vk == VK_ESCAPE.0 as u32 {
            self.result = Some(OverlayResult::Cancelled);
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

            if let Some((el, _)) = self.hints.iter().find(|(_, l)| *l == self.typed) {
                self.result = Some(OverlayResult::Selected(el.id));
                let _ = DestroyWindow(hwnd);
                return;
            }

            let any_prefix = self.hints.iter().any(|(_, l)| l.starts_with(&self.typed));
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
            let _ = DeleteObject(HGDIOBJ(self.font.0));
        }
    }
}

unsafe fn create_label_font() -> HFONT {
    CreateFontW(
        FONT_HEIGHT,
        0,
        0,
        0,
        FW_BOLD.0 as i32,
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
            if state.result.is_none() {
                state.result = Some(OverlayResult::Cancelled);
            }
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

/// Show the hint overlay. Blocks the calling thread on a Win32 message loop
/// until the user picks a hint, presses `Esc`, or the window loses focus.
pub fn show_overlay(config: OverlayConfig) -> Result<OverlayResult> {
    if config.hints.is_empty() {
        return Ok(OverlayResult::Cancelled);
    }

    unsafe {
        // Best-effort: tag the process as PerMonitorV2 DPI-aware so UIA
        // pixel coords line up with our overlay coords on high-DPI displays.
        // Safe to call repeatedly; ignore "already set" / older-OS errors.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let hinstance: HINSTANCE = unsafe { GetModuleHandleW(PCWSTR::null())? }.into();
    unsafe { ensure_class_registered(hinstance)? };

    let (vx, vy, vw, vh) = unsafe { virtual_screen_rect() };
    let state = Box::new(unsafe { OverlayState::new(config.hints) });
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

    let mut state = unsafe { Box::from_raw(state_ptr) };
    let result = state.result.take().unwrap_or(OverlayResult::Cancelled);
    Ok(result)
}
