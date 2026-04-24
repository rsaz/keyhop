//! System tray icon and context menu.
//!
//! [`Tray`] mirrors keyhop's two leader chords as menu items so the user can
//! discover and trigger them without remembering the hotkeys, and exposes a
//! Quit entry that asks the message loop to exit. Events are delivered
//! through the `tray-icon` crate's static channel and surfaced as
//! [`TrayCommand`]s by [`Tray::poll_commands`], called from inside the
//! Win32 message loop alongside [`super::hotkey::Hotkeys::poll_actions`].
//!
//! The tray icon is generated procedurally as a 32×32 RGBA buffer (yellow
//! background with a "K" glyph) so we don't have to ship an `.ico` asset.
//! The icon doubles as a visual anchor that keyhop is running.

use anyhow::{Context, Result};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

/// What a tray menu click should do. A superset of
/// [`super::hotkey::HotkeyAction`] — adds entries that have no hotkey
/// equivalent (we don't want global "quit" or "open settings" chords).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    /// Same as `Ctrl+Shift+Space`: pick an interactable element in the
    /// foreground window.
    PickElement,
    /// Same as `Ctrl+Alt+Space`: pick a top-level window across all
    /// monitors and focus it.
    PickWindow,
    /// Open the modal Settings window for editing hotkeys, hint
    /// alphabet, colors, and Windows startup integration.
    OpenSettings,
    /// Open the log file in notepad (release builds only).
    ViewLog,
    /// Ask the message loop to exit cleanly.
    Quit,
}

const TRAY_ICON_PX: u32 = 32;

/// Owns the tray icon and its menu items. Drop removes the icon from the
/// notification area.
pub struct Tray {
    // Field order matters for drop: the tray must drop before the items
    // (and the items before the icon) but in practice tray-icon holds its
    // own Arcs, so we mainly keep these around to extend lifetimes and
    // because their `id()`s feed the event-id comparison below.
    _tray: TrayIcon,
    pick_element_id: MenuId,
    pick_window_id: MenuId,
    settings_id: MenuId,
    view_log_id: Option<MenuId>,
    quit_id: MenuId,
    _items: TrayItems,
}

// Kept alive for the lifetime of `Tray`. `tray-icon`/`muda` hold internal
// references but explicit ownership avoids relying on those internals.
struct TrayItems {
    _pick_element: MenuItem,
    _pick_window: MenuItem,
    _about: MenuItem,
    _settings: MenuItem,
    _view_log: Option<MenuItem>,
    _quit: MenuItem,
}

impl Tray {
    /// Create the tray icon with keyhop's default menu. Must be called on
    /// the thread that runs the Win32 message loop — the OS posts tray
    /// notifications back to the creating thread.
    pub fn build() -> Result<Self> {
        let pick_element = MenuItem::new("Pick element\tCtrl+Shift+Space", true, None);
        let pick_window = MenuItem::new("Pick window\tCtrl+Alt+Space", true, None);
        // Disabled label, just to expose the version inside the menu.
        let about = MenuItem::new(concat!("keyhop v", env!("CARGO_PKG_VERSION")), false, None);
        // Settings shows its default chord too, mirroring the format used
        // by the picker entries above.
        let settings = MenuItem::new("Settings...\tCtrl+Shift+,", true, None);

        // View Log is always present so users always have a one-click
        // path to the file. Debug builds (where logs go to stderr) just
        // show a notification explaining that.
        let view_log = Some(MenuItem::new("View Log", true, None));

        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append(&about).context("appending About item")?;
        menu.append(&PredefinedMenuItem::separator())
            .context("appending separator")?;
        menu.append(&pick_element)
            .context("appending Pick element item")?;
        menu.append(&pick_window)
            .context("appending Pick window item")?;
        menu.append(&PredefinedMenuItem::separator())
            .context("appending separator")?;
        menu.append(&settings).context("appending Settings item")?;

        if let Some(ref view_log_item) = view_log {
            menu.append(view_log_item)
                .context("appending View Log item")?;
        }

        menu.append(&PredefinedMenuItem::separator())
            .context("appending separator")?;
        menu.append(&quit).context("appending Quit item")?;

        let icon = build_icon().context("building tray icon")?;
        let tooltip = format!("keyhop v{} — Ctrl+Shift+Space", env!("CARGO_PKG_VERSION"));
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(tooltip)
            .with_icon(icon)
            .build()
            .context("creating tray icon")?;

        let pick_element_id = pick_element.id().clone();
        let pick_window_id = pick_window.id().clone();
        let settings_id = settings.id().clone();
        let view_log_id = view_log.as_ref().map(|v| v.id().clone());
        let quit_id = quit.id().clone();

        tracing::info!("tray icon registered");

        Ok(Self {
            _tray: tray,
            pick_element_id,
            pick_window_id,
            settings_id,
            view_log_id,
            quit_id,
            _items: TrayItems {
                _pick_element: pick_element,
                _pick_window: pick_window,
                _about: about,
                _settings: settings,
                _view_log: view_log,
                _quit: quit,
            },
        })
    }

    /// Drain the tray menu channel and translate events into
    /// [`TrayCommand`]s. Should be called every iteration of the message
    /// loop (after `DispatchMessageW`), exactly like
    /// [`super::hotkey::Hotkeys::poll_actions`].
    pub fn poll_commands(&self) -> Vec<TrayCommand> {
        let receiver = MenuEvent::receiver();
        let mut out = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if event.id == self.pick_element_id {
                out.push(TrayCommand::PickElement);
            } else if event.id == self.pick_window_id {
                out.push(TrayCommand::PickWindow);
            } else if event.id == self.settings_id {
                out.push(TrayCommand::OpenSettings);
            } else if self.view_log_id.as_ref().is_some_and(|id| event.id == *id) {
                out.push(TrayCommand::ViewLog);
            } else if event.id == self.quit_id {
                out.push(TrayCommand::Quit);
            }
            // Other events (e.g. About, separators) are ignored.
        }
        out
    }
}

/// Build a polished tray icon procedurally. Rendered at 4× the target
/// resolution and downsampled with a box filter for free anti-aliasing
/// on the rounded corners and the diagonal strokes of the "K" glyph.
///
/// The icon is a yellow keycap with a subtle vertical gradient for
/// depth, a soft dark outline for legibility against light tray
/// backgrounds, and a bold white "K" centred on it.
fn build_icon() -> Result<Icon> {
    const SUPERSAMPLE: u32 = 4;
    let target = TRAY_ICON_PX;
    let big = target * SUPERSAMPLE;

    let buf_big = render_icon_high_res(big);
    let buf_small = downsample(
        &buf_big,
        big as usize,
        target as usize,
        SUPERSAMPLE as usize,
    );

    Icon::from_rgba(buf_small, target, target).context("Icon::from_rgba")
}

/// Render the icon at high resolution. Working at 128×128 (32 × 4)
/// gives the box-filter downsample enough samples per output pixel
/// to produce smooth edges.
fn render_icon_high_res(size: u32) -> Vec<u8> {
    let s = size as i32;
    let mut buf = vec![0u8; (size * size * 4) as usize];

    // Subtle 1-pixel margin (at the high-res scale that's 4px) so the
    // outline doesn't get clipped after downsampling.
    let margin = (size as i32) / 32; // ~1px equivalent
    let radius = (size as i32) * 6 / 32; // ~6px equivalent at 32x32
    let outline_w = (size as i32) / 32; // ~1px outline

    // Yellow gradient endpoints — top brighter, bottom slightly darker
    // for a subtle keycap-bevel look. Values picked to evoke a real
    // physical keycap rather than a flat sticker.
    const YELLOW_TOP: [u8; 3] = [0xFF, 0xD9, 0x1F];
    const YELLOW_BOT: [u8; 3] = [0xF5, 0xB7, 0x00];
    const OUTLINE: [u8; 3] = [0x1A, 0x1A, 0x1A];
    const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];

    let cap_x0 = margin;
    let cap_y0 = margin;
    let cap_x1 = s - margin;
    let cap_y1 = s - margin;

    for y in 0..s {
        for x in 0..s {
            let i = ((y * s + x) * 4) as usize;

            // Inside the keycap?
            if inside_rounded_rect(x, y, cap_x0, cap_y0, cap_x1, cap_y1, radius) {
                // Vertical gradient
                let t = (y - cap_y0) as f32 / (cap_y1 - cap_y0) as f32;
                let r = lerp(YELLOW_TOP[0], YELLOW_BOT[0], t);
                let g = lerp(YELLOW_TOP[1], YELLOW_BOT[1], t);
                let b = lerp(YELLOW_TOP[2], YELLOW_BOT[2], t);
                buf[i] = r;
                buf[i + 1] = g;
                buf[i + 2] = b;
                buf[i + 3] = 0xFF;
            } else if inside_rounded_rect(
                x,
                y,
                cap_x0 - outline_w,
                cap_y0 - outline_w,
                cap_x1 + outline_w,
                cap_y1 + outline_w,
                radius + outline_w,
            ) {
                // Outline ring
                buf[i] = OUTLINE[0];
                buf[i + 1] = OUTLINE[1];
                buf[i + 2] = OUTLINE[2];
                buf[i + 3] = 0xFF;
            }
        }
    }

    // Draw the "K" using thick strokes at high resolution. Three lines:
    //   1. Vertical stroke on the left
    //   2. Upper diagonal from middle-left to top-right
    //   3. Lower diagonal from middle-left to bottom-right
    // Stroke width and positioning chosen to look balanced after the
    // downsample to 32px.
    let stroke = (size as i32) * 5 / 32; // ~5px-equivalent thickness
    let glyph_inset = (size as i32) * 8 / 32; // padding inside the keycap
    let gx0 = cap_x0 + glyph_inset;
    let gy0 = cap_y0 + glyph_inset - (size as i32) / 32;
    let gx1 = cap_x1 - glyph_inset;
    let gy1 = cap_y1 - glyph_inset + (size as i32) / 32;
    let gmid_y = (gy0 + gy1) / 2;
    let vert_x = gx0;

    // Vertical stroke
    fill_rect(&mut buf, s, vert_x, gy0, vert_x + stroke, gy1, &WHITE);

    // Upper diagonal: from (vert_x + stroke, gmid_y) to (gx1, gy0)
    draw_thick_line(
        &mut buf,
        s,
        vert_x + stroke,
        gmid_y,
        gx1,
        gy0,
        stroke,
        &WHITE,
    );

    // Lower diagonal: from (vert_x + stroke, gmid_y) to (gx1, gy1)
    draw_thick_line(
        &mut buf,
        s,
        vert_x + stroke,
        gmid_y,
        gx1,
        gy1,
        stroke,
        &WHITE,
    );

    buf
}

/// Box-filter downsample from `src_size`² to `dst_size`² (premultiplied
/// average over `factor`² source pixels, in straight RGBA space). Cheap
/// and gives respectable anti-aliasing for the icon rendering above.
fn downsample(src: &[u8], src_size: usize, dst_size: usize, factor: usize) -> Vec<u8> {
    let mut out = vec![0u8; dst_size * dst_size * 4];
    for dy in 0..dst_size {
        for dx in 0..dst_size {
            let mut r: u32 = 0;
            let mut g: u32 = 0;
            let mut b: u32 = 0;
            let mut a: u32 = 0;
            for sy in 0..factor {
                for sx in 0..factor {
                    let src_x = dx * factor + sx;
                    let src_y = dy * factor + sy;
                    let i = (src_y * src_size + src_x) * 4;
                    let alpha = src[i + 3] as u32;
                    // Premultiply so transparent pixels don't bleed
                    // their colour into the average.
                    r += src[i] as u32 * alpha / 255;
                    g += src[i + 1] as u32 * alpha / 255;
                    b += src[i + 2] as u32 * alpha / 255;
                    a += alpha;
                }
            }
            let count = (factor * factor) as u32;
            let dst_i = (dy * dst_size + dx) * 4;
            let out_a = a / count;
            // Un-premultiply so the saved RGBA buffer matches what
            // tray-icon expects.
            if out_a > 0 {
                out[dst_i] = ((r / count) * 255 / out_a).min(255) as u8;
                out[dst_i + 1] = ((g / count) * 255 / out_a).min(255) as u8;
                out[dst_i + 2] = ((b / count) * 255 / out_a).min(255) as u8;
            }
            out[dst_i + 3] = out_a as u8;
        }
    }
    out
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    (a as f32 * (1.0 - t) + b as f32 * t).round() as u8
}

fn fill_rect(buf: &mut [u8], stride: i32, x0: i32, y0: i32, x1: i32, y1: i32, rgb: &[u8; 3]) {
    for y in y0..y1 {
        for x in x0..x1 {
            if x < 0 || y < 0 || x >= stride || y >= stride {
                continue;
            }
            let i = ((y * stride + x) * 4) as usize;
            buf[i] = rgb[0];
            buf[i + 1] = rgb[1];
            buf[i + 2] = rgb[2];
            buf[i + 3] = 0xFF;
        }
    }
}

/// Draw a thick line by stamping a square of side `thickness` at every
/// pixel along the Bresenham path between the endpoints. Crude but
/// produces clean diagonals at supersampled resolution.
fn draw_thick_line(
    buf: &mut [u8],
    stride: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    thickness: i32,
    rgb: &[u8; 3],
) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    let half = thickness / 2;

    loop {
        // Stamp a filled square centred on (x, y)
        for oy in -half..=half {
            for ox in -half..=half {
                let px = x + ox;
                let py = y + oy;
                if px < 0 || py < 0 || px >= stride || py >= stride {
                    continue;
                }
                let i = ((py * stride + px) * 4) as usize;
                buf[i] = rgb[0];
                buf[i + 1] = rgb[1];
                buf[i + 2] = rgb[2];
                buf[i + 3] = 0xFF;
            }
        }
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

/// Inside test for a rounded rectangle. Standard "rect with quarter-
/// circle corners" SDF — coordinates outside the bounding box are out;
/// inside the corner-radius square we test against the corner circle.
fn inside_rounded_rect(x: i32, y: i32, x0: i32, y0: i32, x1: i32, y1: i32, r: i32) -> bool {
    if x < x0 || x >= x1 || y < y0 || y >= y1 {
        return false;
    }
    let cx = if x < x0 + r {
        x0 + r
    } else if x >= x1 - r {
        x1 - r - 1
    } else {
        return true;
    };
    let cy = if y < y0 + r {
        y0 + r
    } else if y >= y1 - r {
        y1 - r - 1
    } else {
        return true;
    };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= r * r
}
