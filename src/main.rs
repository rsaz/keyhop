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
//! overlay; the tray's `Quit` entry exits the message loop.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use keyhop::{Action, Backend, Element, HintEngine};

#[cfg(windows)]
use keyhop::windows::{
    hotkey::{HotkeyAction, Hotkeys},
    overlay::{pick_hint, Hint, HintStyle},
    single_instance::InstanceGuard,
    tray::{Tray, TrayCommand},
    window_picker,
};

/// Parsed command-line flags. Hand-rolled so we don't pull in `clap` for
/// three options.
#[derive(Debug, Default, Clone, Copy)]
struct Cli {
    no_tray: bool,
}

fn main() -> ExitCode {
    let cli = match parse_args() {
        Ok(Some(cli)) => cli,
        Ok(None) => return ExitCode::SUCCESS, // --help / --version handled
        Err(code) => return code,
    };

    init_tracing();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "keyhop starting");

    let result: anyhow::Result<()>;
    #[cfg(windows)]
    {
        result = run_windows(cli);
    }
    #[cfg(not(windows))]
    {
        let _ = cli;
        result = Err(anyhow::anyhow!(
            "no backend available for this platform yet"
        ));
    }

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = ?e, "keyhop exited with error");
            ExitCode::from(1)
        }
    }
}

fn parse_args() -> Result<Option<Cli>, ExitCode> {
    let mut cli = Cli::default();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("keyhop {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--no-tray" => cli.no_tray = true,
            other => {
                eprintln!("keyhop: unknown argument: {other}");
                eprintln!("Try 'keyhop --help' for a list of options.");
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok(Some(cli))
}

fn print_help() {
    println!(
        "keyhop {} — system-wide keyboard navigation overlay",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("USAGE:");
    println!("    keyhop [FLAGS]");
    println!();
    println!("FLAGS:");
    println!("    -h, --help        Print help and exit");
    println!("    -V, --version     Print version and exit");
    println!("        --no-tray     Run without the system tray icon (hotkeys-only mode)");
    println!();
    println!("DEFAULT HOTKEYS:");
    println!("    Ctrl+Shift+Space  Pick element in foreground window");
    println!("    Ctrl+Alt+Space    Pick top-level window across all monitors");
    println!("    Esc               Cancel an open overlay");
    println!();
    println!("ENVIRONMENT:");
    println!("    RUST_LOG          Tracing filter, e.g. `keyhop=debug` (default: info)");
    println!();
    println!("PROJECT:");
    println!("    Repository:       {}", env!("CARGO_PKG_REPOSITORY"));
}

#[cfg(windows)]
fn run_windows(cli: Cli) -> anyhow::Result<()> {
    use ::windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, PostQuitMessage, TranslateMessage, MSG,
    };

    // Refuse to start a second copy in the same session — two instances
    // would race for the same global hotkeys and double-stack tray icons.
    let _instance = match InstanceGuard::acquire()? {
        Some(guard) => guard,
        None => {
            eprintln!("keyhop is already running in this session.");
            eprintln!("Use the existing tray icon, or quit it first.");
            // Exit successfully so launchers / autostart shims don't show
            // an error dialog when the user double-clicks twice.
            return Ok(());
        }
    };

    let mut backend = keyhop::windows::WindowsBackend::new()?;
    let hotkeys = Hotkeys::register_defaults()?;

    // The tray is opt-in via `--no-tray` and best-effort otherwise: if it
    // can't be created (e.g. headless CI, no Explorer shell) we still want
    // the hotkeys to work.
    let tray = if cli.no_tray {
        tracing::info!("--no-tray: running without system tray icon");
        None
    } else {
        match Tray::build() {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(error = ?e, "tray icon unavailable; continuing with hotkeys only");
                None
            }
        }
    };

    println!("keyhop {} is running.", env!("CARGO_PKG_VERSION"));
    println!("  Pick element : Ctrl + Shift + Space");
    println!("  Pick window  : Ctrl + Alt   + Space");
    println!("  Cancel       : Esc (inside overlay)");
    #[cfg(debug_assertions)]
    {
        if tray.is_some() {
            println!("  Quit         : Tray menu → Quit, or Ctrl + C in this terminal");
        } else {
            println!("  Quit         : Ctrl + C in this terminal");
        }
    }
    #[cfg(not(debug_assertions))]
    {
        if tray.is_some() {
            println!("  Quit         : Tray menu → Quit");
        } else {
            println!("  Quit         : (no tray available)");
        }
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
                    #[cfg(not(debug_assertions))]
                    TrayCommand::ViewLog => {
                        if let Err(e) = open_log_file() {
                            tracing::error!(error = ?e, "failed to open log file");
                        }
                    }
                    #[cfg(debug_assertions)]
                    TrayCommand::ViewLog => {
                        // ViewLog shouldn't be available in debug builds, but handle it gracefully
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

    #[cfg(debug_assertions)]
    {
        fmt().with_env_filter(filter).with_target(false).init();
    }

    #[cfg(not(debug_assertions))]
    {
        use std::fs;
        use tracing_appender::rolling;
        
        if let Some(log_path) = get_log_file_path() {
            if let Some(parent) = log_path.parent() {
                if fs::create_dir_all(parent).is_ok() {
                    let file_appender = rolling::never(parent, "keyhop.log");
                    fmt()
                        .with_env_filter(filter)
                        .with_target(false)
                        .with_writer(file_appender)
                        .init();
                    return;
                }
            }
        }
        fmt().with_env_filter(filter).with_target(false).init();
    }
}

#[cfg(not(debug_assertions))]
fn get_log_file_path() -> Option<std::path::PathBuf> {
    use std::env;
    let appdata = env::var("LOCALAPPDATA").ok()?;
    let mut path = std::path::PathBuf::from(appdata);
    path.push("keyhop");
    path.push("keyhop.log");
    Some(path)
}

#[cfg(not(debug_assertions))]
fn open_log_file() -> anyhow::Result<()> {
    if let Some(log_path) = get_log_file_path() {
        if log_path.exists() {
            std::process::Command::new("notepad.exe")
                .arg(&log_path)
                .spawn()?;
        }
    }
    Ok(())
}
