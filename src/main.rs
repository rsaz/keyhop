//! `keyhop` — system-wide keyboard navigation overlay.
//!
//! Two leader chords:
//!
//! - `Ctrl + Shift + Space` — pick an interactable element inside the
//!   currently-focused window and invoke it.
//! - `Ctrl + Alt + Space` — pick a top-level window across all monitors
//!   and bring it to the foreground.
//!
//! Both actions are also exposed through a system tray icon (right-click
//! the yellow "K" badge in the notification area). `Esc` cancels either
//! overlay; the tray's `Quit` entry — or `Ctrl+C` in this terminal — exits
//! the message loop.

use keyhop::{Action, Backend, Element, HintEngine};

#[cfg(windows)]
use keyhop::windows::{
    hotkey::{HotkeyAction, Hotkeys},
    overlay::{pick_hint, Hint, HintStyle},
    tray::{Tray, TrayCommand},
    window_picker,
};

fn main() -> anyhow::Result<()> {
    init_tracing();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "keyhop starting");

    #[cfg(windows)]
    {
        run_windows()
    }

    #[cfg(not(windows))]
    {
        anyhow::bail!("no backend available for this platform yet")
    }
}

#[cfg(windows)]
fn run_windows() -> anyhow::Result<()> {
    use ::windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, PostQuitMessage, TranslateMessage, MSG,
    };

    let mut backend = keyhop::windows::WindowsBackend::new()?;
    let hotkeys = Hotkeys::register_defaults()?;
    // The tray is best-effort: if it can't be created (e.g. headless CI,
    // no shell), we still want the hotkeys to work.
    let tray = match Tray::build() {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::warn!(error = ?e, "tray icon unavailable; continuing with hotkeys only");
            None
        }
    };

    println!("keyhop is running.");
    println!("  Pick element : Ctrl + Shift + Space");
    println!("  Pick window  : Ctrl + Alt   + Space");
    println!("  Cancel       : Esc (inside overlay)");
    if tray.is_some() {
        println!("  Quit         : Tray menu → Quit, or Ctrl + C in this terminal");
    } else {
        println!("  Quit         : Ctrl + C in this terminal");
    }
    println!();
    println!("Switch focus to any app, then press a leader.");

    // Win32 message loop. `GetMessageW` is required on this thread for the
    // global-hotkey crate's hidden message window to receive `WM_HOTKEY`,
    // and for tray-icon's notification window to receive shell callbacks.
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 == 0 || r.0 == -1 {
            // 0 = WM_QUIT (clean shutdown via PostQuitMessage), -1 = error.
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        for action in hotkeys.poll_actions() {
            if let Err(e) = dispatch_hotkey(action, &mut backend) {
                tracing::error!(?action, error = ?e, "hotkey handler failed");
            }
        }

        if let Some(tray) = tray.as_ref() {
            for cmd in tray.poll_commands() {
                match cmd {
                    TrayCommand::PickElement => {
                        if let Err(e) = handle_pick_element(&mut backend) {
                            tracing::error!(error = ?e, "tray PickElement failed");
                        }
                    }
                    TrayCommand::PickWindow => {
                        if let Err(e) = handle_pick_window() {
                            tracing::error!(error = ?e, "tray PickWindow failed");
                        }
                    }
                    TrayCommand::Quit => {
                        tracing::info!("quit requested from tray");
                        // PostQuitMessage queues WM_QUIT; the next
                        // GetMessageW returns 0 and we break above.
                        unsafe { PostQuitMessage(0) };
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
fn dispatch_hotkey(
    action: HotkeyAction,
    backend: &mut keyhop::windows::WindowsBackend,
) -> anyhow::Result<()> {
    match action {
        HotkeyAction::PickElement => handle_pick_element(backend),
        HotkeyAction::PickWindow => handle_pick_window(),
    }
}

#[cfg(windows)]
fn handle_pick_element(backend: &mut keyhop::windows::WindowsBackend) -> anyhow::Result<()> {
    let elements: Vec<Element> = backend.enumerate_foreground()?;
    if elements.is_empty() {
        tracing::info!("no interactable elements in foreground window");
        return Ok(());
    }

    let labels = HintEngine::default().generate(elements.len());
    let hints: Vec<Hint> = elements
        .iter()
        .zip(labels.iter())
        .map(|(el, label)| Hint {
            bounds: el.bounds,
            label: label.clone(),
            extra: None,
        })
        .collect();
    tracing::info!(count = hints.len(), "showing element overlay");

    match pick_hint(hints, HintStyle::elements())? {
        Some(idx) => {
            let chosen = &elements[idx];
            tracing::info!(id = ?chosen.id, role = ?chosen.role, "element selected — invoking");
            if let Err(e) = backend.perform(chosen.id, Action::Invoke) {
                tracing::warn!(error = ?e, "perform failed");
            }
        }
        None => tracing::info!("element overlay cancelled"),
    }
    Ok(())
}

#[cfg(windows)]
fn handle_pick_window() -> anyhow::Result<()> {
    let windows = window_picker::enumerate_visible()?;
    if windows.is_empty() {
        tracing::info!("no visible top-level windows");
        return Ok(());
    }
    tracing::info!(count = windows.len(), "showing window overlay");

    match window_picker::pick(windows)? {
        Some(chosen) => {
            tracing::info!(title = %chosen.title, "window selected — focusing");
            window_picker::focus(chosen.hwnd)?;
        }
        None => tracing::info!("window overlay cancelled"),
    }
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}
