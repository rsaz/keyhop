//! `keyhop` — system-wide keyboard navigation overlay.
//!
//! This binary is intentionally minimal at this stage. It wires up logging,
//! constructs a platform backend, and runs a one-shot enumeration. The full
//! interactive overlay loop (leader hotkey, hint rendering, action dispatch)
//! is the next milestone.

use keyhop_core::{Backend, HintEngine};

fn main() -> anyhow::Result<()> {
    init_tracing();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "keyhop starting");

    let mut backend = build_backend()?;
    let elements = backend.enumerate_foreground()?;

    let hints = HintEngine::default().generate(elements.len());
    tracing::info!(
        element_count = elements.len(),
        hint_count = hints.len(),
        "enumerated foreground"
    );

    if elements.is_empty() {
        println!("No interactable elements discovered yet.");
        println!("Backend enumeration is still a stub — this is expected.");
    } else {
        for (el, hint) in elements.iter().zip(hints.iter()) {
            println!(
                "[{hint:>3}] {role:?}  {name}",
                role = el.role,
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
