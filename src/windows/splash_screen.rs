//! Splash screen for app initialization.
//!
//! Displays the keyhop logo in a borderless, centered window while the
//! application initializes. The window is automatically closed when
//! [`SplashScreen`] is dropped.

use anyhow::{Context, Result};
use std::sync::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DeleteDC, DeleteObject,
    EndPaint, FillRect, GetDC, InvalidateRect, ReleaseDC, SelectObject, SetStretchBltMode,
    StretchBlt, UpdateWindow, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HALFTONE,
    HBITMAP, HDC, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetSystemMetrics,
    LoadCursorW, PeekMessageW, RegisterClassExW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HWND_TOPMOST, IDC_ARROW, MSG,
    PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNORMAL,
    WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const SPLASH_SIZE: i32 = 480;
/// Inner padding so the logo doesn't touch the splash edges.
const SPLASH_PADDING: i32 = 24;
/// Background colour matching the logo's dark backdrop (BGR for GDI).
const SPLASH_BG_BGR: u32 = 0x000F0F12;

/// Per-window data holding the bitmap to paint. Boxed onto the heap and
/// the pointer stashed in GWLP_USERDATA so the window proc can access it.
struct WindowData {
    bitmap: HBITMAP,
    image_width: i32,
    image_height: i32,
}

/// Owns a splash screen window. Dropping this value destroys the window
/// and releases its GDI resources.
pub struct SplashScreen {
    hwnd: HWND,
    bitmap: HBITMAP,
    _data: Box<WindowData>,
}

// SAFETY: HWND and HBITMAP are just integers; SplashScreen is only ever
// used from the main thread.
unsafe impl Send for SplashScreen {}

impl SplashScreen {
    /// Create and show a centered splash screen with the keyhop logo.
    /// The window is borderless, always-on-top, and sized to 400×400.
    pub fn show() -> Result<Self> {
        let image_data = include_bytes!("../../branding/store-logo-1080.png");
        let image = load_png(image_data).context("failed to load splash image")?;

        unsafe {
            let hinstance = GetModuleHandleW(None).context("GetModuleHandleW failed")?;

            // Register window class (idempotent — the second registration
            // will fail silently and that's fine).
            let class_name = w!("KeyhopSplashClass");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(splash_wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance.into(),
                hIcon: Default::default(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: Default::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: class_name,
                hIconSm: Default::default(),
            };
            RegisterClassExW(&wc);

            let screen_w = GetSystemMetrics(SM_CXSCREEN);
            let screen_h = GetSystemMetrics(SM_CYSCREEN);
            let x = (screen_w - SPLASH_SIZE) / 2;
            let y = (screen_h - SPLASH_SIZE) / 2;

            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class_name,
                w!("Keyhop"),
                WS_POPUP,
                x,
                y,
                SPLASH_SIZE,
                SPLASH_SIZE,
                None,
                None,
                hinstance,
                None,
            )?;

            // Create the bitmap once and stash it in window data so WM_PAINT
            // can re-paint as Windows requests.
            let screen_dc = GetDC(HWND::default());
            let bitmap = create_bitmap_from_image(screen_dc, &image)?;
            ReleaseDC(HWND::default(), screen_dc);

            let data = Box::new(WindowData {
                bitmap,
                image_width: image.width as i32,
                image_height: image.height as i32,
            });
            let data_ptr = Box::into_raw(data);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, data_ptr as isize);

            // Force on top of everything and show. SetWindowPos with
            // HWND_TOPMOST is more reliable than SetForegroundWindow,
            // which Windows refuses when the calling process isn't
            // already foreground (common during startup from Explorer).
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = InvalidateRect(hwnd, None, true);
            let _ = UpdateWindow(hwnd);

            // Pump messages so WM_PAINT actually runs. Without this the
            // window appears blank for the duration of the splash.
            let mut msg = MSG::default();
            for _ in 0..10 {
                while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }

            // Re-box the data so we own it again for the Drop.
            let data = Box::from_raw(data_ptr);

            Ok(Self {
                hwnd,
                bitmap,
                _data: data,
            })
        }
    }
}

impl Drop for SplashScreen {
    fn drop(&mut self) {
        unsafe {
            // Clear the window-data pointer before destruction so the
            // window proc can't dereference freed memory if any final
            // messages run.
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.bitmap);
        }
    }
}

/// Window procedure. Paints the cached bitmap on every WM_PAINT.
unsafe extern "system" fn splash_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    const WM_PAINT: u32 = 0x000F;
    const WM_ERASEBKGND: u32 = 0x0014;

    match msg {
        // Returning 1 here tells Windows we've handled the erase, which
        // suppresses the default white flash before WM_PAINT runs.
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let data_ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
            ) as *const WindowData;

            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);

            // Fill the entire window with the dark backdrop first so the
            // logo sits on a continuous frame (the source PNG already
            // includes its own dark gradient — anything left over after
            // the centred logo also needs to be dark).
            let bg_brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(SPLASH_BG_BGR));
            let full_rect = windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: SPLASH_SIZE,
                bottom: SPLASH_SIZE,
            };
            FillRect(hdc, &full_rect, bg_brush);
            let _ = DeleteObject(bg_brush);

            if !data_ptr.is_null() {
                let data = &*data_ptr;
                let mem_dc = CreateCompatibleDC(hdc);
                let old = SelectObject(mem_dc, data.bitmap);

                // HALFTONE produces a high-quality bicubic-style downscale
                // — much smoother than the default COLORONCOLOR which
                // looks pixelated when shrinking a 1080² source to 432².
                SetStretchBltMode(hdc, HALFTONE);

                let target = SPLASH_SIZE - 2 * SPLASH_PADDING;
                let _ = StretchBlt(
                    hdc,
                    SPLASH_PADDING,
                    SPLASH_PADDING,
                    target,
                    target,
                    mem_dc,
                    0,
                    0,
                    data.image_width,
                    data.image_height,
                    SRCCOPY,
                );
                SelectObject(mem_dc, old);
                let _ = DeleteDC(mem_dc);
            }

            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Decoded RGBA image data.
struct Image {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

fn load_png(data: &[u8]) -> Result<Image> {
    let decoder = png::Decoder::new(data);
    let mut reader = decoder.read_info().context("PNG decode failed")?;

    let info = reader.info();
    let width = info.width;
    let height = info.height;

    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).context("PNG frame read failed")?;

    let rgba_data = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            for chunk in buf[..info.buffer_size()].chunks(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            rgba
        }
        _ => anyhow::bail!("Unsupported PNG color type"),
    };

    Ok(Image {
        width,
        height,
        data: rgba_data,
    })
}

/// Create a top-down 32-bit DIB section from RGBA image data. The DIB
/// owns its pixel memory; freeing the bitmap with `DeleteObject` releases
/// it. Note that Windows DIBs are BGRA, so we swap channels here.
unsafe fn create_bitmap_from_image(hdc: HDC, image: &Image) -> Result<HBITMAP> {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: image.width as i32,
            biHeight: -(image.height as i32), // negative = top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0 as u32,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default(); 1],
    };

    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let bitmap = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;

    if !bits.is_null() {
        let pixel_count = (image.width * image.height) as usize;
        let dest = std::slice::from_raw_parts_mut(bits as *mut u8, pixel_count * 4);

        for i in 0..pixel_count {
            let src = i * 4;
            let dst = i * 4;
            dest[dst] = image.data[src + 2];
            dest[dst + 1] = image.data[src + 1];
            dest[dst + 2] = image.data[src];
            dest[dst + 3] = image.data[src + 3];
        }
    }

    Ok(bitmap)
}

// Suppress unused warning from Mutex import on builds where it isn't used.
#[allow(dead_code)]
fn _unused_mutex_marker(_: Mutex<()>) {}
