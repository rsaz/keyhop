//! `keyhop` — system-wide keyboard navigation overlay.
//!
//! Registers `Ctrl+Shift+Space` as the global leader chord. Each press snaps
//! the foreground window via UI Automation, generates short hint labels, and
//! shows a transparent overlay listing them. Typing a label invokes the
//! corresponding control; `Esc` cancels.

use keyhop_core::{Action, Backend, HintEngine};

#[cfg(windows)]
use keyhop_windows::{
    hotkey::LeaderHotkey,
    overlay::{show_overlay, OverlayConfig, OverlayResult},
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
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    let mut backend = keyhop_windows::WindowsBackend::new()?;
    let leader = LeaderHotkey::register_default()?;

    println!("keyhop is running.");
    println!("  Leader : Ctrl+Shift+Space");
    println!("  Cancel : Esc (inside overlay)");
    println!("  Quit   : Ctrl+C in this terminal");
    println!();
    println!("Switch focus to any app, then press the leader.");

    // Win32 message loop. `GetMessageW` is required on this thread for the
    // global-hotkey crate's hidden message window to receive `WM_HOTKEY`.
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

        // Hotkey events surface as WM_HOTKEY messages handled inside the
        // crate; the resulting `GlobalHotKeyEvent`s arrive on the channel.
        if leader.poll_pressed() {
            if let Err(e) = handle_leader(&mut backend) {
                tracing::error!(error = ?e, "handle_leader failed");
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
fn handle_leader(backend: &mut keyhop_windows::WindowsBackend) -> anyhow::Result<()> {
    let elements = backend.enumerate_foreground()?;
    if elements.is_empty() {
        tracing::info!("no interactable elements in foreground window");
        return Ok(());
    }

    let labels = HintEngine::default().generate(elements.len());
    let hints: Vec<_> = elements.into_iter().zip(labels).collect();
    tracing::info!(count = hints.len(), "showing overlay");

    match show_overlay(OverlayConfig { hints })? {
        OverlayResult::Selected(id) => {
            tracing::info!(?id, "hint selected — invoking");
            if let Err(e) = backend.perform(id, Action::Invoke) {
                tracing::warn!(error = ?e, "perform failed");
            }
        }
        OverlayResult::Cancelled => {
            tracing::info!("overlay cancelled");
        }
    }
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}
