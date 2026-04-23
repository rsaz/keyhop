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
    config_watcher::{self, WM_USER_RELOAD_CONFIG},
    hotkey::{HotkeyAction, HotkeyConflict, Hotkeys},
    ipc, notification,
    overlay::{pick_hint, Hint, HintStyle},
    settings_window,
    single_instance::InstanceGuard,
    splash_screen::SplashScreen,
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
    use ::windows::Win32::System::Threading::GetCurrentThreadId;
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

    // Show splash screen during initialization. Track when it appeared so
    // we can enforce a minimum display time below — on fast machines the
    // rest of init finishes in <100ms and the splash would otherwise
    // flash by too quickly for the user to register.
    let splash_shown_at = std::time::Instant::now();
    let splash = match SplashScreen::show() {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!(error = ?e, "failed to show splash screen; continuing anyway");
            None
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

    let mut config = Config::load_or_default();
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

    // Phase 4: spin up the foreground-tracking pre-warm worker so the
    // hotkey path can serve from a hot cache. Held in a binding whose
    // `Drop` unhooks `SetWinEventHook` on shutdown. Failure is
    // non-fatal: the synchronous hotkey path still works, just with
    // cold-cache UIA latency on each press.
    let _prewarm = match keyhop::windows::prewarm::Prewarmer::start(
        backend.cache_handle(),
        config.scope.max_elements,
    ) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(error = ?e, "prewarm worker unavailable; hotkeys will run cold");
            None
        }
    };

    // Register hotkeys from config. Conflicts (e.g. another app already
    // owns the chord) are surfaced to the user but don't abort startup —
    // partial registration is better than nothing, and the user can fix
    // the broken chord in Settings.
    let outcome = Hotkeys::register_from_config(&config.hotkeys)?;
    let mut hotkeys = outcome.hotkeys;
    if !outcome.conflicts.is_empty() {
        notify_hotkey_conflicts(&outcome.conflicts);
    }

    // Hot-reload watcher. Best-effort: failure leaves keyhop fully
    // functional but missing the live-edit-`config.toml` convenience.
    // Held in a binding so its `Drop` only fires when `run_windows`
    // returns. The Settings dialog also triggers reload (without
    // depending on the watcher), so users on locked-down profiles
    // where filesystem watching fails still get hot-reload via Save.
    let main_thread_id = unsafe { GetCurrentThreadId() };
    let _config_watcher = match config_watcher::spawn(main_thread_id) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = ?e, "config hot-reload watcher unavailable");
            None
        }
    };

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
    println!("  Open settings: {}", config.hotkeys.open_settings);
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

    // Keep the splash visible for at least 2.5s so the user actually sees
    // the brand mark on fast machines where init completes in <100ms.
    // Pump messages while we wait so the splash stays painted.
    if splash.is_some() {
        const MIN_SPLASH_MS: u128 = 2500;
        let elapsed = splash_shown_at.elapsed().as_millis();
        if elapsed < MIN_SPLASH_MS {
            let remaining = MIN_SPLASH_MS - elapsed;
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_millis(remaining as u64);
            use ::windows::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW as DM, PeekMessageW as PM, TranslateMessage as TM, MSG as M,
                PM_REMOVE as PMR,
            };
            let mut m = M::default();
            while std::time::Instant::now() < deadline {
                unsafe {
                    while PM(&mut m, None, 0, 0, PMR).as_bool() {
                        let _ = TM(&m);
                        DM(&m);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        }
    }
    drop(splash);

    let mut runtime = Runtime {
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

        // Custom thread message from the config-file watcher: re-read
        // the file from disk and hot-apply. Skip Translate/Dispatch
        // for thread messages — they have a NULL hwnd, so DispatchMessage
        // would route them through DefWindowProc and waste cycles.
        if msg.hwnd.0.is_null() && msg.message == WM_USER_RELOAD_CONFIG {
            tracing::info!("config-file change detected, reloading");
            let new_config = Config::load_or_default();
            if new_config != config {
                apply_config(
                    &new_config,
                    &mut config,
                    &mut runtime,
                    &mut backend,
                    &mut hotkeys,
                    /* announce = */ true,
                );
            }
            continue;
        }

        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        for action in hotkeys.poll_actions() {
            match action {
                HotkeyAction::OpenSettings => match settings_window::show(&config) {
                    Ok(Some(new_config)) => {
                        apply_config(
                            &new_config,
                            &mut config,
                            &mut runtime,
                            &mut backend,
                            &mut hotkeys,
                            /* announce = */ true,
                        );
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::error!(error = ?e, "settings window failed");
                        notification::error("Couldn't open Settings", &format!("{e}"));
                    }
                },
                _ => {
                    if let Err(e) = dispatch_hotkey(action, &mut backend, &runtime) {
                        tracing::error!(?action, error = ?e, "hotkey handler failed");
                        notification::error(
                            "keyhop: action failed",
                            &format!("{action:?} failed:\n{e}\n\nSee log for details."),
                        );
                    }
                }
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
                        Ok(Some(new_config)) => {
                            apply_config(
                                &new_config,
                                &mut config,
                                &mut runtime,
                                &mut backend,
                                &mut hotkeys,
                                /* announce = */ true,
                            );
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::error!(error = ?e, "settings window failed");
                            notification::error("Couldn't open Settings", &format!("{e}"));
                        }
                    },
                    TrayCommand::ViewLog => {
                        #[cfg(not(debug_assertions))]
                        {
                            if let Err(e) = open_log_file() {
                                tracing::error!(error = ?e, "failed to open log file");
                                notification::error(
                                    "keyhop: couldn't open log",
                                    &format!("{e}"),
                                );
                            }
                        }
                        #[cfg(debug_assertions)]
                        {
                            // Debug builds write to stderr, so there's no
                            // log file to open. Surface that to the user
                            // rather than silently doing nothing.
                            notification::info(
                                "View Log unavailable",
                                "Debug builds write logs to stderr, not to a file. \
                                Run the release build to use this option.",
                            );
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

/// Hot-apply a freshly loaded [`Config`] to every live subsystem
/// without restarting the process. Called from both the Settings
/// dialog Save handler and the [`config_watcher`]'s file-change
/// thread message; both code paths converge here so the apply logic
/// stays in one place.
///
/// Steps, in dependency order:
///   1. Resolve the alphabet from the preset+modifiers and rebuild
///      the [`HintEngine`].
///   2. Rebuild the element / window [`HintStyle`]s (colors, opacity,
///      leader pref).
///   3. Push the new scope mode into [`Runtime`].
///   4. Push cache + max-elements into the [`WindowsBackend`] without
///      reconstructing it (the UIA client is expensive to recreate).
///   5. Tear down the old [`Hotkeys`] *before* registering the new
///      ones — the OS hotkey table is per-process so re-registering
///      the same chord on top of itself would otherwise fail.
///   6. Update the registry "launch at startup" entry to match.
///   7. Replace the cached `Config` so subsequent Settings opens see
///      the current state, not the stale one we loaded at startup.
///
/// `announce` controls whether a "Settings applied" toast fires;
/// pass `false` for silent reloads (e.g. on first paint).
#[cfg(windows)]
fn apply_config(
    new_config: &Config,
    cached: &mut Config,
    runtime: &mut Runtime,
    backend: &mut keyhop::windows::WindowsBackend,
    hotkeys: &mut Hotkeys,
    announce: bool,
) {
    let resolved_alphabet = keyhop::alphabet_presets::build_alphabet(&new_config.hints);
    runtime.hint_engine =
        keyhop::HintEngine::with_strategy(&resolved_alphabet, new_config.hints.strategy)
            .with_min_singles(new_config.hints.min_singles);
    runtime.element_style = HintStyle::elements_from_config(&new_config.colors.element);
    runtime.window_style = HintStyle::windows_from_config(&new_config.colors.window);
    runtime.scope_mode = new_config.scope.mode;

    backend.reconfigure_cache(
        new_config.performance.enable_caching,
        new_config.performance.cache_ttl_ms,
    );
    backend.set_max_elements_global(new_config.scope.max_elements);

    // Drop the old Hotkeys *before* trying to register the new ones.
    // Windows' RegisterHotKey is per-process, so replacing
    // Ctrl+Shift+Space (old) with Ctrl+Shift+Space (new) would fail
    // on the `register` call if the old chord was still alive at the
    // moment of attempt. The empty-Hotkeys placeholder either built
    // here or already in `*hotkeys` keeps the type intact between
    // the drop and the new assignment.
    if let Ok(empty) = Hotkeys::new() {
        let _drop_old = std::mem::replace(hotkeys, empty);
        // _drop_old goes out of scope at the end of this `if let`,
        // releasing every chord it was holding.
    }
    match Hotkeys::register_from_config(&new_config.hotkeys) {
        Ok(outcome) => {
            *hotkeys = outcome.hotkeys;
            if !outcome.conflicts.is_empty() {
                notify_hotkey_conflicts(&outcome.conflicts);
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "re-registering hotkeys after reload failed");
            notification::error(
                "keyhop: hotkey reload failed",
                &format!("{e}\n\nThe previous chords are no longer active. Open Settings to retry."),
            );
        }
    }

    // Best-effort: a registry write failure here is non-fatal;
    // everything else has already taken effect in-process.
    if let Err(e) = startup::set_enabled(new_config.startup.launch_at_startup) {
        tracing::warn!(error = ?e, "couldn't sync launch-at-startup registry entry");
    }

    *cached = new_config.clone();

    if announce {
        notification::info(
            "Settings applied",
            "Your changes are live now — no restart needed.",
        );
    }
}

/// Common path for "we just registered hotkeys and one or more
/// failed". Surfacing through a notification rather than the log
/// matters because the failure is otherwise silent — the user just
/// sees their chord doing nothing.
#[cfg(windows)]
fn notify_hotkey_conflicts(conflicts: &[HotkeyConflict]) {
    let body = conflicts
        .iter()
        .map(|c| format!("• {} ({}): {}", c.action, c.chord, c.reason))
        .collect::<Vec<_>>()
        .join("\n");
    notification::warn(
        "keyhop: hotkey conflict",
        &format!(
            "Some hotkeys couldn't be registered:\n\n{body}\n\n\
            Open Settings to choose a different chord."
        ),
    );
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
        HotkeyAction::OpenSettings => Ok(()), // Handled separately in main loop
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
