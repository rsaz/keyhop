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
        let settings = MenuItem::new("Settings...", true, None);

        // Only show "View Log" in release builds where logs go to a file.
        #[cfg(not(debug_assertions))]
        let view_log = Some(MenuItem::new("View Log", true, None));
        #[cfg(debug_assertions)]
        let view_log: Option<MenuItem> = None;

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

        #[cfg(not(debug_assertions))]
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

/// Build the tray icon procedurally so we don't need to ship a binary
/// `.ico` asset. Layout: 32×32 yellow square, 2-pixel dark border, with a
/// 5×7 pixel "K" glyph scaled 3× and centred.
fn build_icon() -> Result<Icon> {
    let size = TRAY_ICON_PX as usize;
    let mut buf = vec![0u8; size * size * 4];

    // Vimium-ish yellow background (R=255 G=229 B=0).
    const YELLOW: [u8; 4] = [0xFF, 0xE5, 0x00, 0xFF];
    const DARK: [u8; 4] = [0x1A, 0x1A, 0x1A, 0xFF];

    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&YELLOW);
    }

    // 2-pixel dark border so the badge reads against any tray background.
    let border = 2usize;
    for y in 0..size {
        for x in 0..size {
            if x < border || x >= size - border || y < border || y >= size - border {
                let i = (y * size + x) * 4;
                buf[i..i + 4].copy_from_slice(&DARK);
            }
        }
    }

    // 5-wide × 7-tall "K" bitmap. Each row is left-to-right; 1 = ink.
    const K: [[u8; 5]; 7] = [
        [1, 0, 0, 0, 1],
        [1, 0, 0, 1, 0],
        [1, 0, 1, 0, 0],
        [1, 1, 0, 0, 0],
        [1, 0, 1, 0, 0],
        [1, 0, 0, 1, 0],
        [1, 0, 0, 0, 1],
    ];
    let scale = 3usize;
    let glyph_w = 5 * scale;
    let glyph_h = 7 * scale;
    let off_x = (size - glyph_w) / 2;
    let off_y = (size - glyph_h) / 2;
    for (gy, row) in K.iter().enumerate() {
        for (gx, &on) in row.iter().enumerate() {
            if on == 0 {
                continue;
            }
            for sy in 0..scale {
                for sx in 0..scale {
                    let x = off_x + gx * scale + sx;
                    let y = off_y + gy * scale + sy;
                    let i = (y * size + x) * 4;
                    buf[i..i + 4].copy_from_slice(&DARK);
                }
            }
        }
    }

    Icon::from_rgba(buf, TRAY_ICON_PX, TRAY_ICON_PX).context("Icon::from_rgba")
}
