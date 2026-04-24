//! Filesystem watcher for `%APPDATA%\keyhop\config.toml`.
//!
//! When the file changes on disk — whether the Settings dialog wrote
//! it, the user hand-edited `config.toml` in their editor of choice, a
//! cloud-sync agent dropped in a new copy, or a future CLI subcommand
//! mutated it — we want the running `keyhop` instance to pick up the
//! new values without a restart. This module owns the
//! [`notify`]-backed watcher and the debounce timer that turns a burst
//! of write events into a single "reload now" signal.
//!
//! ## Wire-up
//!
//! 1. [`spawn`] takes the main thread's id (from
//!    [`GetCurrentThreadId`]) and starts a background watcher on the
//!    config file's parent directory (watching the file directly is
//!    fragile because most editors save by writing to a temporary
//!    sibling and atomically renaming over the target).
//! 2. When a `notify` event mentions our `config.toml`, we record the
//!    timestamp and spawn a one-shot debounce thread that sleeps
//!    [`DEBOUNCE_MS`] and then checks: if no newer event arrived
//!    during the sleep, post [`WM_USER_RELOAD_CONFIG`] to the main
//!    thread via [`PostThreadMessageW`]. The main message loop picks
//!    that up in [`crate::main`]'s dispatch and re-applies the config.
//! 3. The returned [`ConfigWatcher`] guard owns the underlying
//!    [`notify::RecommendedWatcher`]; dropping it stops the watch.
//!
//! The debounce window is small enough to feel instant (≤ ~150 ms
//! between Save click and overlay update) but large enough to absorb
//! the multi-write pattern of editors like VS Code that touch the file
//! 2–4 times per save (truncate, write, sync, fsync …).

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_USER};

use crate::config::Config;

/// Custom thread-message id posted to the main thread when a
/// debounced config-file change is ready to apply. We add `1` to
/// `WM_USER` so the value (`0x0401`) is unique within keyhop's
/// message namespace and stays comfortably below `WM_APP` so other
/// libraries (`global-hotkey`, `tray-icon`) can't collide.
pub const WM_USER_RELOAD_CONFIG: u32 = WM_USER + 1;

/// How long after the *last* observed event we wait before triggering
/// a reload. Tuned to absorb editor "atomic save" sequences (VS Code,
/// Notepad++, Vim's swap-file dance) without making the user feel a
/// noticeable lag after Save.
const DEBOUNCE_MS: u64 = 150;

/// Live filesystem watcher. Keep this alive (in `main`) for as long
/// as you want hot-reload to work; dropping it stops the underlying
/// inotify/ReadDirectoryChangesW handle.
pub struct ConfigWatcher {
    /// Held purely so its `Drop` impl tears down the OS watcher.
    /// Renamed to `_watcher` because we never read it after `spawn`.
    _watcher: RecommendedWatcher,
}

/// Start watching `config.toml` for changes, posting
/// [`WM_USER_RELOAD_CONFIG`] to `main_thread_id` (debounced) on each
/// detected modification. Returns `Ok(None)` when no config path is
/// resolvable (e.g. `%APPDATA%` unset) — the caller should treat that
/// as "no hot-reload available, but everything else still works".
pub fn spawn(main_thread_id: u32) -> Result<Option<ConfigWatcher>> {
    let Some(config_path) = Config::file_path() else {
        tracing::info!("no APPDATA path available; config hot-reload disabled");
        return Ok(None);
    };
    let parent = match config_path.parent() {
        Some(p) => p.to_path_buf(),
        None => {
            tracing::warn!(
                ?config_path,
                "config path has no parent; hot-reload disabled"
            );
            return Ok(None);
        }
    };
    // The directory must exist before we hand it to `notify` — first
    // launches that haven't saved a Settings window yet won't have it.
    if let Err(e) = std::fs::create_dir_all(&parent) {
        tracing::warn!(?parent, error = ?e, "couldn't create config dir; hot-reload disabled");
        return Ok(None);
    }

    let target = config_path.clone();
    // Shared "most recent event timestamp" used by the debounce
    // thread to decide whether to fire. `Mutex<Instant>` is fine —
    // contention is at most a few writes per second on the busiest
    // editor save burst.
    let last_event = Arc::new(Mutex::new(Instant::now()));

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(event) = res else {
            return;
        };
        // Most events fire on the parent dir; filter to ones touching
        // our specific file (or its temp siblings during atomic save).
        let touches_config = event.paths.iter().any(|p| {
            p == &target
                || p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("config.toml"))
                    .unwrap_or(false)
        });
        if !touches_config {
            return;
        }

        let now = Instant::now();
        if let Ok(mut le) = last_event.lock() {
            *le = now;
        }

        // One-shot debounce: sleep DEBOUNCE_MS, then fire iff no
        // newer event arrived during the sleep. This collapses an
        // editor's multi-write save sequence into a single reload.
        let last_event_clone = Arc::clone(&last_event);
        let snapshot = now;
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(DEBOUNCE_MS));
            let still_quiet = match last_event_clone.lock() {
                Ok(le) => *le <= snapshot,
                Err(_) => false,
            };
            if !still_quiet {
                return;
            }
            unsafe {
                // PostThreadMessage delivers to the queue without
                // needing an HWND; the main loop's GetMessageW
                // dispatches it (msg.hwnd == NULL means "thread
                // message" and skips DispatchMessage's window lookup).
                let _ =
                    PostThreadMessageW(main_thread_id, WM_USER_RELOAD_CONFIG, WPARAM(0), LPARAM(0));
            }
        });
    })
    .context("creating config-file watcher failed")?;

    watcher
        .watch(&parent, RecursiveMode::NonRecursive)
        .with_context(|| format!("failed to watch config dir {}", parent.display()))?;

    tracing::info!(path = ?config_path, "config hot-reload watcher started");

    Ok(Some(ConfigWatcher { _watcher: watcher }))
}
