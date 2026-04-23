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

use keyhop::{Action, Backend, Config, Element, HintEngine};
#[cfg(windows)]
use keyhop::config::ScopeMode;

#[cfg(windows)]
use keyhop::windows::{
    hotkey::{HotkeyAction, Hotkeys},
    ipc, notification,
    overlay::{pick_hint, Hint, HintStyle},
    settings_window,
    single_instance::InstanceGuard,
    startup,
    tray::{Tray, TrayCommand},
    window_picker,
};

/// Parsed command-line flags. Hand-rolled so we don't pull in `clap` for
/// a handful of options.
#[derive(Debug, Default, Clone, Copy)]
struct Cli {
    no_tray: bool,
    /// Ask any running keyhop in this user session to shut down cleanly,
    /// then exit. See [`ipc::send_close_signal`] for the wire-level
    /// mechanism. Mutually exclusive with the normal startup path —
    /// when set, we never even acquire the single-instance mutex.
    close: bool,
    /// Delete the on-disk log files in `%LOCALAPPDATA%\keyhop\` and
    /// exit. Useful when triaging a recurring issue ("clear, repro,
    /// share log") or when the file has grown larger than the user
    /// is comfortable keeping around.
    clear_logs: bool,
}

fn main() -> ExitCode {
    let cli = match parse_args() {
        Ok(Some(cli)) => cli,
        Ok(None) => return ExitCode::SUCCESS, // --help / --version handled
        Err(code) => return code,
    };

    // One-shot maintenance commands run before tracing init so the
    // closer / clear-logs paths don't themselves write a single line to
    // the log they're trying to either send-to or delete.
    #[cfg(windows)]
    if cli.close {
        return run_close_command();
    }
    #[cfg(windows)]
    if cli.clear_logs {
        return run_clear_logs_command();
    }

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
            "--close" | "--quit" => cli.close = true,
            "--clear-logs" => cli.clear_logs = true,
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
    println!("        --close       Cleanly shut down a running keyhop instance and exit");
    println!("                      (alias: --quit)");
    println!("        --clear-logs  Delete the on-disk log files and exit");
    println!();
    println!("DEFAULT HOTKEYS:");
    println!("    Ctrl+Shift+Space  Pick element in foreground window");
    println!("    Ctrl+Alt+Space    Pick top-level window across all monitors");
    println!("    Esc               Cancel an open overlay");
    println!();
    println!("LOGS (release builds):");
    println!("    Location:         %LOCALAPPDATA%\\keyhop\\keyhop.log[.YYYY-MM-DD]");
    println!("    Rotation:         daily, keeping the 7 most recent files");
    println!("    Manual purge:     keyhop --clear-logs");
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
            eprintln!("Use the existing tray icon, the `--close` flag, or quit it first.");
            // Exit successfully so launchers / autostart shims don't show
            // an error dialog when the user double-clicks twice.
            return Ok(());
        }
    };

    // Hidden IPC window so a future `keyhop --close` can find this
    // process and ask it to shut down. Held alive until the message
    // loop exits; dropping it destroys the window.
    let _ipc_window = match ipc::create() {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::warn!(error = ?e, "failed to create IPC window; --close will not work");
            None
        }
    };

    let config = Config::load_or_default();
    // Resolve the alphabet from the preset + flags every launch so users
    // who hand-edit `[hints]` to switch presets don't have to delete the
    // stale `alphabet` field by hand. Settings dialog Save also writes
    // a resolved value, so this is just defence-in-depth.
    let resolved_alphabet = keyhop::alphabet_presets::build_alphabet(&config.hints);
    let hint_engine = keyhop::HintEngine::with_strategy(&resolved_alphabet, config.hints.strategy)
        .with_min_singles(config.hints.min_singles);
    let element_style = HintStyle::elements_from_config(&config.colors.element);
    let window_style = HintStyle::windows_from_config(&config.colors.window);

    let mut backend = keyhop::windows::WindowsBackend::with_config(
        config.performance.enable_caching,
        config.performance.cache_ttl_ms,
        config.scope.max_elements,
    )?;

    // Register hotkeys from config. Conflicts (e.g. another app already
    // owns the chord) are surfaced to the user but don't abort startup —
    // partial registration is better than nothing, and the user can fix
    // the broken chord in Settings.
    let outcome = Hotkeys::register_from_config(&config.hotkeys)?;
    let hotkeys = outcome.hotkeys;
    if !outcome.conflicts.is_empty() {
        let body = outcome
            .conflicts
            .iter()
            .map(|c| format!("• {} ({}): {}", c.action, c.chord, c.reason))
            .collect::<Vec<_>>()
            .join("\n");
        notification::warn(
            "keyhop: hotkey conflict",
            &format!(
                "Some hotkeys couldn't be registered:\n\n{body}\n\n\
                Open Settings from the tray to choose a different chord."
            ),
        );
    }

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
    println!("  Pick element : {}", config.hotkeys.pick_element);
    println!("  Pick window  : {}", config.hotkeys.pick_window);
    println!("  Cancel       : Esc (inside overlay)");
    #[cfg(debug_assertions)]
    {
        if tray.is_some() {
            println!(
                "  Quit         : Tray menu → Quit, `keyhop --close`, or Ctrl + C in this terminal"
            );
        } else {
            println!("  Quit         : `keyhop --close`, or Ctrl + C in this terminal");
        }
    }
    #[cfg(not(debug_assertions))]
    {
        if tray.is_some() {
            println!("  Quit         : Tray menu → Quit, or `keyhop --close`");
        } else {
            println!("  Quit         : `keyhop --close`");
        }
    }
    println!();
    println!("Switch focus to any app, then press a leader.");

    let runtime = Runtime {
        hint_engine,
        element_style,
        window_style,
        scope_mode: config.scope.mode,
    };

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
            if let Err(e) = dispatch_hotkey(action, &mut backend, &runtime) {
                tracing::error!(?action, error = ?e, "hotkey handler failed");
                notification::error(
                    "keyhop: action failed",
                    &format!("{action:?} failed:\n{e}\n\nSee log for details."),
                );
            }
        }

        if let Some(tray) = tray.as_ref() {
            for cmd in tray.poll_commands() {
                match cmd {
                    TrayCommand::PickElement => {
                        if let Err(e) = handle_pick_element(&mut backend, &runtime) {
                            tracing::error!(error = ?e, "tray PickElement failed");
                            notification::error(
                                "keyhop: pick element failed",
                                &format!("{e}\n\nSee log for details."),
                            );
                        }
                    }
                    TrayCommand::PickWindow => {
                        if let Err(e) = handle_pick_window(&runtime) {
                            tracing::error!(error = ?e, "tray PickWindow failed");
                            notification::error(
                                "keyhop: pick window failed",
                                &format!("{e}\n\nSee log for details."),
                            );
                        }
                    }
                    TrayCommand::OpenSettings => match settings_window::show(&config) {
                        Ok(true) => {
                            // Apply the new startup state immediately even
                            // though everything else needs a restart, since
                            // it has nothing to do with the message loop.
                            let _ = startup::is_enabled();
                            notification::info(
                                "Settings saved",
                                "Restart keyhop to apply hotkey, alphabet, and color changes.",
                            );
                        }
                        Ok(false) => {}
                        Err(e) => {
                            tracing::error!(error = ?e, "settings window failed");
                            notification::error("Couldn't open Settings", &format!("{e}"));
                        }
                    },
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

/// Per-process configuration that the picker handlers need. Built once at
/// startup from [`Config`] so repeated invocations don't re-parse strings.
#[cfg(windows)]
struct Runtime {
    hint_engine: HintEngine,
    element_style: HintStyle,
    window_style: HintStyle,
    /// Which set of windows the element picker enumerates. Driven by
    /// `[scope]` in `config.toml`; defaults to active-window for
    /// backwards-compatible v0.3.0 behaviour.
    scope_mode: ScopeMode,
}

#[cfg(windows)]
fn dispatch_hotkey(
    action: HotkeyAction,
    backend: &mut keyhop::windows::WindowsBackend,
    runtime: &Runtime,
) -> anyhow::Result<()> {
    match action {
        HotkeyAction::PickElement => handle_pick_element(backend, runtime),
        HotkeyAction::PickWindow => handle_pick_window(runtime),
    }
}

#[cfg(windows)]
fn handle_pick_element(
    backend: &mut keyhop::windows::WindowsBackend,
    runtime: &Runtime,
) -> anyhow::Result<()> {
    let elements: Vec<Element> = backend.enumerate_by_scope(runtime.scope_mode)?;
    if elements.is_empty() {
        tracing::info!("no interactable elements in foreground window");
        notification::info(
            "keyhop",
            "No interactive elements found in the foreground window.",
        );
        return Ok(());
    }

    let labels = runtime.hint_engine.generate(elements.len());
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

    match pick_hint(hints, runtime.element_style)? {
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
fn handle_pick_window(runtime: &Runtime) -> anyhow::Result<()> {
    let windows = window_picker::enumerate_visible()?;
    if windows.is_empty() {
        tracing::info!("no visible top-level windows");
        notification::info("keyhop", "No visible windows to pick from.");
        return Ok(());
    }
    tracing::info!(count = windows.len(), "showing window overlay");

    match window_picker::pick_with_style(windows, runtime.window_style)? {
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
        use tracing_appender::rolling::{RollingFileAppender, Rotation};

        // Daily-rotated logs with a 7-file ceiling. Pre-rotation behaviour
        // (`rolling::never`) appended forever and was the source of
        // multi-hundred-megabyte log files on long-running installs.
        // Daily rotation matches what most desktop apps do (`%TEMP%`,
        // `%LOCALAPPDATA%\Microsoft\...`); seven files is plenty for
        // bug repros without becoming a disk hazard.
        if let Some(log_dir) = get_log_dir() {
            if fs::create_dir_all(&log_dir).is_ok() {
                let appender = RollingFileAppender::builder()
                    .rotation(Rotation::DAILY)
                    .filename_prefix("keyhop")
                    .filename_suffix("log")
                    .max_log_files(7)
                    .build(&log_dir);
                if let Ok(file_appender) = appender {
                    fmt()
                        .with_env_filter(filter)
                        .with_target(false)
                        .with_writer(file_appender)
                        .init();
                    return;
                }
            }
        }
        // If we couldn't open a log file (read-only profile, missing
        // %LOCALAPPDATA%, …) fall back to stderr so debug builds stay
        // useful even in unusual environments.
        fmt().with_env_filter(filter).with_target(false).init();
    }
}

/// Directory holding all keyhop log files. Returns `None` when
/// `%LOCALAPPDATA%` is unset (very rare — typically only on heavily
/// restricted service accounts).
#[cfg(not(debug_assertions))]
fn get_log_dir() -> Option<std::path::PathBuf> {
    use std::env;
    let appdata = env::var("LOCALAPPDATA").ok()?;
    let mut path = std::path::PathBuf::from(appdata);
    path.push("keyhop");
    Some(path)
}

/// Path to the *most recent* log file. With daily rotation this is
/// today's file, named `keyhop.YYYY-MM-DD.log`. We pick "newest by
/// mtime" rather than computing today's name explicitly so the lookup
/// keeps working across timezone changes / clock skew / files written
/// by an earlier process.
#[cfg(not(debug_assertions))]
fn get_latest_log_file() -> Option<std::path::PathBuf> {
    use std::fs;
    let dir = get_log_dir()?;
    let entries = fs::read_dir(&dir).ok()?;
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("keyhop"))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?;
            Some((e.path(), modified))
        })
        .max_by_key(|(_, mtime)| *mtime)
        .map(|(path, _)| path)
}

#[cfg(not(debug_assertions))]
fn open_log_file() -> anyhow::Result<()> {
    if let Some(log_path) = get_latest_log_file() {
        std::process::Command::new("notepad.exe")
            .arg(&log_path)
            .spawn()?;
    }
    Ok(())
}

/// Implementation of `keyhop --close`. Looks for the hidden IPC window
/// the running instance creates ([`ipc::create`]) and posts `WM_CLOSE`
/// to it. The running instance's message loop turns that into a
/// graceful shutdown — the same path the tray-menu "Quit" entry takes.
///
/// Always exits the closer process with code 0 even when no instance
/// was found, so wrapping `keyhop --close` in startup / cleanup
/// scripts doesn't error out when keyhop wasn't running.
#[cfg(windows)]
fn run_close_command() -> ExitCode {
    match ipc::send_close_signal() {
        Ok(true) => {
            println!("keyhop: shutdown signal sent.");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            println!("keyhop: no running instance found.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("keyhop: failed to signal running instance: {e}");
            ExitCode::from(1)
        }
    }
}

/// Implementation of `keyhop --clear-logs`. Removes every file under
/// the log directory whose name starts with `keyhop` (today's file plus
/// any rolled-over daily archives). Leaves the directory itself in
/// place so the next run can write straight back to it without an
/// extra `create_dir_all`.
///
/// In debug builds logs go to stderr only — there's nothing to delete,
/// so the command is a no-op that prints a hint instead of pretending
/// it did something.
#[cfg(windows)]
fn run_clear_logs_command() -> ExitCode {
    #[cfg(debug_assertions)]
    {
        eprintln!("keyhop: --clear-logs has nothing to do in debug builds (logs go to stderr).");
        ExitCode::SUCCESS
    }
    #[cfg(not(debug_assertions))]
    {
        use std::fs;
        let Some(dir) = get_log_dir() else {
            eprintln!("keyhop: %LOCALAPPDATA% is not set; nothing to clear.");
            return ExitCode::from(1);
        };
        if !dir.exists() {
            println!("keyhop: log directory does not exist; nothing to clear.");
            return ExitCode::SUCCESS;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                eprintln!("keyhop: cannot read log directory {}: {e}", dir.display());
                return ExitCode::from(1);
            }
        };
        let mut removed = 0usize;
        let mut errors = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("keyhop") {
                continue;
            }
            match fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(e) => {
                    // Common case: the running keyhop has the current
                    // file open with FILE_SHARE_READ but not _DELETE.
                    // Worth surfacing so the user knows to run --close
                    // first or shut down via the tray.
                    eprintln!("keyhop: could not delete {}: {e}", entry.path().display());
                    errors += 1;
                }
            }
        }
        println!(
            "keyhop: removed {removed} log file(s) from {}.",
            dir.display()
        );
        if errors > 0 {
            eprintln!(
                "keyhop: {errors} file(s) could not be deleted (is keyhop still running? \
                Run `keyhop --close` first)."
            );
            return ExitCode::from(1);
        }
        ExitCode::SUCCESS
    }
}
