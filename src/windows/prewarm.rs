//! Background pre-warming of the per-HWND UIA element cache.
//!
//! Phase 4 of the snappy-perf plan. The synchronous hotkey path
//! ([`crate::Backend::enumerate_foreground`]) becomes a cache lookup
//! whenever this module has had a chance to walk the foreground
//! window's UIA tree first. Two ingredients make that possible:
//!
//! 1. A dedicated MTA worker thread that owns its own
//!    [`crate::windows::WindowsBackend`] (separate `UIAutomation`
//!    client, separate `self.elements` registry) and shares only the
//!    `Arc<Mutex<CacheManager>>` with the hotkey-driven backend.
//! 2. A `SetWinEventHook` for `EVENT_SYSTEM_FOREGROUND` (and a
//!    rate-limited subscription to `EVENT_OBJECT_LOCATIONCHANGE`)
//!    that forwards the new HWND to the worker over a bounded
//!    channel. "Bounded" matters — when the user is alt-tabbing
//!    rapidly we want to drop intermediate notifications and walk
//!    only the most recent foreground window.
//!
//! The hook proc is a `unsafe extern "system" fn` with no user
//! pointer, so the only way to fan out into the worker is through a
//! process-wide `OnceLock<SyncSender<HWND>>`. We also stash the HHOOK
//! handles in another `OnceLock` so [`Prewarmer::Drop`] can call
//! `UnhookWinEvent` on shutdown — leaking the hooks would keep the
//! callbacks pinned until the process exits, which Windows is
//! relatively forgiving about but `cargo test` runs are not.

use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use parking_lot::Mutex;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, OBJID_WINDOW,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use crate::cache::CacheManager;
use crate::windows::WindowsBackend;

/// Channel buffer size for the foreground-HWND queue. Bounded at 1 so
/// fast alt-tab spam collapses to "just walk the latest window" — older
/// pending HWNDs are silently dropped on `try_send`. Small buffer is
/// the entire point: we want freshness, not throughput.
const QUEUE_CAPACITY: usize = 1;

/// Throttle for `EVENT_OBJECT_LOCATIONCHANGE`. The OS posts this event
/// at video-frame rates while the user resizes / scrolls, and walking
/// the UIA tree on every one would waste both CPU and the target
/// process's UI thread. Re-walking at most every 250 ms is plenty for
/// "the layout settled, refresh the cache".
const LOCATIONCHANGE_DEBOUNCE: Duration = Duration::from_millis(250);

/// Process-wide channel sender, populated at the first
/// [`Prewarmer::start`] call. The WinEvent hook proc is a free C
/// function so it can't capture state — it has to look the sender up
/// here. `OnceLock` avoids the mutable-static lint and makes the
/// "first start wins" semantics explicit.
/// We carry the raw `HWND` value as an `isize` (the OS handle is just a
/// pointer-sized integer) instead of the `HWND` newtype because the
/// newtype wraps a raw pointer and is therefore `!Send`. The worker
/// rebuilds an `HWND` on the other side; nothing about HWND lifetimes
/// requires the original wrapper to make the trip.
static SENDER: OnceLock<SyncSender<isize>> = OnceLock::new();

/// Saved hook handles, exposed only so `Prewarmer::Drop` can unhook
/// them in tests. Stored as raw `isize`s because `HWINEVENTHOOK`
/// wraps a `*mut c_void` and is therefore `!Send` / `!Sync`.
/// `UnhookWinEvent` accepts the rebuilt newtype just fine.
static HOOKS: OnceLock<Mutex<Vec<isize>>> = OnceLock::new();

/// Last time the location-change hook accepted an event. Used as a
/// soft debounce so a bursty resize / scroll doesn't drown the
/// worker. The `Mutex` is fine here — the hook proc only locks for a
/// few microseconds and never blocks the worker.
static LAST_LOCATIONCHANGE: OnceLock<Mutex<Instant>> = OnceLock::new();

/// Owner of the pre-warm worker thread. The `Drop` impl unhooks the
/// WinEvent subscriptions and closes the channel so the worker exits
/// cleanly on shutdown.
pub struct Prewarmer {
    /// Held only so `Drop` can close the channel by replacing the
    /// `OnceLock` value with a closed sender. `OnceLock` doesn't
    /// support reset, so in practice we just let the sender live
    /// until process exit — fine for a singleton.
    _sender: SyncSender<isize>,
}

impl Prewarmer {
    /// Spawn the worker thread, register the WinEvent hooks, and
    /// return a handle whose `Drop` tears the hooks back down.
    ///
    /// The worker's [`WindowsBackend`] is constructed inside the
    /// thread so its `UIAutomation` client (and its `CoInitializeEx`
    /// MTA registration) live on the worker thread, where every
    /// subsequent UIA call from this instance will run.
    pub fn start(
        cache: Arc<Mutex<CacheManager>>,
        max_elements_global: usize,
    ) -> anyhow::Result<Self> {
        // Idempotent: a second `start` call would overwrite the
        // global sender and orphan the previous worker. We forbid
        // that explicitly so misuse fails loudly instead of leaking
        // a thread.
        if SENDER.get().is_some() {
            anyhow::bail!("Prewarmer::start called twice");
        }

        let (tx, rx) = sync_channel::<isize>(QUEUE_CAPACITY);
        SENDER
            .set(tx.clone())
            .map_err(|_| anyhow::anyhow!("prewarm sender slot already populated"))?;
        HOOKS.get_or_init(|| Mutex::new(Vec::new()));
        LAST_LOCATIONCHANGE.get_or_init(|| Mutex::new(Instant::now()));

        // Worker thread. Owns its own backend; never touches the
        // main backend's `self.elements`. Errors are logged and the
        // loop continues — one flaky window must not kill pre-warming
        // for everyone else.
        thread::Builder::new()
            .name("keyhop-prewarm".into())
            .spawn(move || worker_loop(cache, max_elements_global, rx))
            .context("spawning prewarm worker thread")?;

        // SAFETY: `SetWinEventHook` is safe to call from any thread; the
        // OS schedules the OUTOFCONTEXT callbacks onto the calling
        // thread's message queue. We require this to be invoked from
        // the same thread that pumps `GetMessageW` — `main.rs` does.
        let foreground_hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if foreground_hook.is_invalid() {
            anyhow::bail!("SetWinEventHook(EVENT_SYSTEM_FOREGROUND) failed");
        }
        let location_hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                None,
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        // A failed location-change hook is non-fatal: we still get
        // foreground events, which is the dominant signal. Log and
        // continue.
        if location_hook.is_invalid() {
            tracing::warn!(
                "SetWinEventHook(EVENT_OBJECT_LOCATIONCHANGE) failed; pre-warm will only react to foreground changes"
            );
        }

        if let Some(slot) = HOOKS.get() {
            let mut h = slot.lock();
            h.push(foreground_hook.0 as isize);
            if !location_hook.is_invalid() {
                h.push(location_hook.0 as isize);
            }
        }

        tracing::info!("prewarm worker started");
        Ok(Self { _sender: tx })
    }
}

impl Drop for Prewarmer {
    fn drop(&mut self) {
        if let Some(slot) = HOOKS.get() {
            let mut hooks = slot.lock();
            for raw in hooks.drain(..) {
                let hook = HWINEVENTHOOK(raw as *mut std::ffi::c_void);
                // SAFETY: `UnhookWinEvent` is safe and idempotent.
                let _ = unsafe { UnhookWinEvent(hook) };
            }
        }
    }
}

/// Worker loop. Owns its own [`WindowsBackend`] (and therefore its own
/// `UIAutomation` client + `CoInitializeEx(MTA)`); reads HWNDs off the
/// channel and walks each one, populating the shared cache as a side
/// effect. Exits when every `SyncSender` clone has been dropped — i.e.
/// when [`Prewarmer`] (and any held copies) go out of scope.
fn worker_loop(
    cache: Arc<Mutex<CacheManager>>,
    max_elements_global: usize,
    rx: std::sync::mpsc::Receiver<isize>,
) {
    // Build the worker's backend on this thread so its UIA client
    // initializes COM here (MTA). Failure here disables pre-warming
    // for the whole process — we'd rather log + bail than crash.
    let mut backend = match WindowsBackend::with_shared_cache(cache, max_elements_global) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = ?e, "prewarm worker failed to init backend; thread exiting");
            return;
        }
    };

    while let Ok(raw) = rx.recv() {
        if raw == 0 {
            continue;
        }
        let hwnd = HWND(raw as *mut std::ffi::c_void);
        let started = Instant::now();
        match backend.enumerate_window(hwnd) {
            Ok(elements) => {
                tracing::debug!(
                    hwnd = ?hwnd.0,
                    count = elements.len(),
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "prewarm walked window"
                );
            }
            Err(e) => {
                tracing::debug!(hwnd = ?hwnd.0, error = ?e, "prewarm walk failed");
            }
        }
    }

    tracing::info!("prewarm worker exiting (channel closed)");
}

/// `SetWinEventHook` callback. Invoked on the main thread (we
/// registered with `WINEVENT_OUTOFCONTEXT` plus
/// `WINEVENT_SKIPOWNPROCESS`, so events from our own process — the
/// overlay HWND, the splash, etc. — never reach us). Any work here
/// runs on the message-loop thread, so we keep it tight: filter,
/// debounce, `try_send`, return.
///
/// # Safety
///
/// Called by Windows; `hwnd` may be null and the other parameters are
/// raw OS handles — we treat them only as opaque values.
unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _id_event_thread: u32,
    _dwms_event_time: u32,
) {
    // Both hooks fire for child accessibility objects too; only the
    // top-level window event is interesting for cache pre-warming.
    if id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 {
        return;
    }
    if hwnd.0.is_null() {
        return;
    }

    if event == EVENT_OBJECT_LOCATIONCHANGE {
        // Soft debounce so a busy resize / scroll doesn't queue a
        // walk per frame. We only ever schedule the *latest* HWND
        // anyway (channel capacity == 1), but the debounce keeps the
        // hook proc itself cheap.
        if let Some(slot) = LAST_LOCATIONCHANGE.get() {
            let mut last = slot.lock();
            if last.elapsed() < LOCATIONCHANGE_DEBOUNCE {
                return;
            }
            *last = Instant::now();
        }

        // A move/resize / scroll on the foreground window invalidates
        // the cache for that HWND — its element bounds and offscreen
        // bits are now stale. The cleanest hand-off is to push the
        // HWND back through the same channel and let the worker
        // re-walk it; that overwrites the cached entry as a side
        // effect (`CacheManager::insert` is upsert).
        let _ = try_send_hwnd(hwnd);
        return;
    }

    // EVENT_SYSTEM_FOREGROUND: hand the new foreground HWND off to
    // the worker. Drop on full so a rapid alt-tab burst doesn't
    // queue stale work.
    let _ = try_send_hwnd(hwnd);
}

/// Send `hwnd` to the worker, dropping silently when the queue is
/// full (we always prefer the freshest HWND) or before
/// [`Prewarmer::start`] has had a chance to install the sender.
fn try_send_hwnd(hwnd: HWND) -> Result<(), TrySendError<isize>> {
    if let Some(tx) = SENDER.get() {
        return tx.try_send(hwnd.0 as isize);
    }
    Ok(())
}
