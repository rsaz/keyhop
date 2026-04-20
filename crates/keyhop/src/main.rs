//! `keyhop` — system-wide keyboard navigation overlay.
//!
//! Current behavior: wait briefly so the user can switch to whichever window
//! they want to inspect, enumerate the interactable elements via the
//! configured backend, and print each one with its assigned hint label.
//!
//! The interactive overlay loop (global hotkey, transparent overlay window,
//! action dispatch) is the next milestone.

use std::{thread, time::Duration};

use keyhop_core::{Backend, HintEngine};

fn main() -> anyhow::Result<()> {
    init_tracing();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "keyhop starting");

    let mut backend = build_backend()?;

    println!("keyhop · enumeration preview");
    println!("Switch to the window you want to navigate.");
    for s in (1..=3).rev() {
        println!("  enumerating in {s}...");
        thread::sleep(Duration::from_secs(1));
    }

    let elements = backend.enumerate_foreground()?;
    let hints = HintEngine::default().generate(elements.len());

    println!();
    if elements.is_empty() {
        println!("No interactable elements discovered.");
        println!("Try focusing an app like Notepad, File Explorer, or a browser.");
    } else {
        println!("Found {} interactable elements:", elements.len());
        for (el, hint) in elements.iter().zip(hints.iter()) {
            println!(
                "  [{hint:>3}] {role:<10?} {w:>4}x{h:<4}  {name}",
                role = el.role,
                w = el.bounds.width,
                h = el.bounds.height,
                name = el.name.as_deref().unwrap_or("<unnamed>")
            );
        }
    }

    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}

#[cfg(windows)]
fn build_backend() -> anyhow::Result<Box<dyn Backend>> {
    Ok(Box::new(keyhop_windows::WindowsBackend::new()?))
}

#[cfg(not(windows))]
fn build_backend() -> anyhow::Result<Box<dyn Backend>> {
    anyhow::bail!("no backend available for this platform yet")
}
