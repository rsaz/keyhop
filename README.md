# keyhop

> Drive your entire desktop from the keyboard. Press a leader chord, see hint labels on every clickable thing on screen, type the hint, done.

`keyhop` is a system-wide keyboard navigation layer that lets you control your whole computer without ever touching the mouse. Reaching for the mouse forces a constant context switch between thinking and pointing — your hands leave the home row, your eyes hunt for a cursor, and your flow breaks. `keyhop` keeps you on the keyboard so you stay fast, focused, and productive, using OS accessibility APIs (UI Automation on Windows) to target native UI elements semantically.

**Status:** v0.4.0 — UX & performance release on top of the v0.3.0 Windows backend. Adds variable-length hint labels (one keystroke for ≤9 targets), a Consolas overlay font, alphabet presets with ambiguous-character exclusion, multi-screen targeting modes (`active_window` / `active_monitor` / `all_windows`), monitor-aware badge positioning, and an in-process element-tree cache. Browser content detection (Chromium), multi-monitor window picker, lifecycle controls (`keyhop --close`, `--clear-logs`, daily log rotation), visual Settings dialog, and the MSI installer (no MSVC toolchain required to install) all carry over from v0.3.0. Linux backend planned.

[![ci](https://github.com/rsaz/keyhop/actions/workflows/ci.yml/badge.svg)](https://github.com/rsaz/keyhop/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/keyhop.svg)](https://crates.io/crates/keyhop)
[![docs.rs](https://docs.rs/keyhop/badge.svg)](https://docs.rs/keyhop)
[![license](https://img.shields.io/crates/l/keyhop.svg)](#license)

## Goals

- Native performance and instant feel (sub-50ms hint overlay).
- Semantic targeting via OS accessibility trees (UI Automation on Windows; AT-SPI on Linux when that backend lands).
- Single, easy-to-install crate with platform backends gated behind `cfg`.

## Crate layout

```
keyhop/
├─ src/
│  ├─ lib.rs               # public API: Action, Backend, Element, HintEngine
│  ├─ main.rs              # the `keyhop` binary (CLI flags, IPC, message loop)
│  ├─ model.rs
│  ├─ action.rs
│  ├─ backend.rs
│  ├─ hint.rs
│  ├─ config.rs            # TOML config (%APPDATA%/keyhop/config.toml)
│  └─ windows/             # Windows backend (cfg(windows) only)
│     ├─ mod.rs            # WindowsBackend (UI Automation tree walk + browser activation)
│     ├─ hotkey.rs         # global leader hotkeys + chord parser
│     ├─ overlay.rs        # transparent layered hint overlay + color parser
│     ├─ tray.rs           # system tray icon + context menu
│     ├─ settings_window.rs # visual Settings dialog (Win32)
│     ├─ startup.rs        # "launch at login" via HKCU Run key
│     ├─ notification.rs   # MessageBox-backed user notifications
│     ├─ window_picker.rs  # Alt-Tab-style window picker (multi-monitor, restores minimized)
│     ├─ single_instance.rs # named-mutex guard so only one keyhop runs
│     └─ ipc.rs            # hidden message-only window for `keyhop --close`
├─ wix/
│  └─ main.wxs             # WiX 3 source for the MSI installer
├─ docs/
│  ├─ CODE_SIGNING.md      # Microsoft Trusted Signing setup (deferred)
│  └─ MICROSOFT_STORE.md   # Partner Center submission runbook (silent install switch, ARP, hosting)
└─ examples/
   └─ enumerate_foreground.rs
```

One package, one publish: `keyhop` ships both the binary and a reusable library API. Linux / Wayland / macOS backends will land as additional `cfg`-gated modules under `src/`.

## Install

### Windows (recommended) — MSI installer

Grab `Keyhop-<version>-x86_64.msi` from the [latest GitHub Release](https://github.com/rsaz/keyhop/releases/latest) and double-click it. The installer:

- Installs to `%ProgramFiles%\Keyhop\bin\keyhop.exe`
- Adds Keyhop to the **system PATH** so `keyhop --close`, `keyhop --clear-logs`, etc. work from any terminal
- Creates a **Start Menu** shortcut
- Registers a single **Add/Remove Programs** entry (uninstall via `appwiz.cpl` or `Settings → Apps`)
- Cleanly closes a running instance during upgrades (Windows Installer's RestartManager)

For unattended installs (CI, fleet rollout, MDM, Microsoft Store automated validation):

```powershell
msiexec /i Keyhop-0.4.0-x86_64.msi /qn
```

UAC will prompt once for admin rights (per-machine install).

### Windows — portable EXE

If you don't want an installer, download `keyhop.exe` from the same [release page](https://github.com/rsaz/keyhop/releases/latest), drop it anywhere, and run it. No registry footprint, no Start Menu entry — you're on your own for placing it on PATH and for upgrades.

### Build from source (developers)

You'll need the MSVC toolchain (the regular Rust install on Windows pulls this in for you):

```powershell
cargo install keyhop                    # from crates.io
# — or —
git clone https://github.com/rsaz/keyhop
cd keyhop
cargo install --path .
```

Requires:
- Rust stable with the `x86_64-pc-windows-msvc` toolchain
- Visual Studio Build Tools with the "Desktop development with C++" workload (provides `link.exe`)

If you see `error: linker 'link.exe' not found`, install the Build Tools — or just use the MSI installer above, which has no toolchain requirement.

## Run

```powershell
keyhop                         # release install (no console, logs to file)
cargo run --release            # from source (no console, logs to file)
cargo run                      # debug build (shows console with live logs)
cargo run --example enumerate_foreground
```

The binary uses the Windows GUI subsystem in release builds, running silently in the background with no console window. Logs are written to `%LOCALAPPDATA%\keyhop\keyhop.log` and can be viewed via the tray menu's "View Log" option. Debug builds (`cargo run` without `--release`) still show a console window for development convenience.

### Flags

| Flag                  | What it does                                                                          |
| --------------------- | ------------------------------------------------------------------------------------- |
| `-h`, `--help`        | Print usage and exit.                                                                 |
| `-V`, `--version`     | Print version and exit.                                                               |
| `--no-tray`           | Run without the system tray icon (hotkeys-only mode).                                 |
| `--close`, `--quit`   | Cleanly shut down the running keyhop instance (uses a hidden IPC window). Exits 0.    |
| `--clear-logs`        | Delete every `keyhop*.log` under `%LOCALAPPDATA%\keyhop\`. Run `--close` first if the live instance has the file open. |

## Using it

After launching, `keyhop` registers two global hotkeys and a tray icon. The default chords below can be changed from `Settings...` in the tray menu — see [Configuration](#configuration).

| Action          | Default keys            | What it does                                                                       |
| --------------- | ----------------------- | ---------------------------------------------------------------------------------- |
| Pick element    | `Ctrl + Shift + Space`  | Hints every interactable control inside the focused window. Type one to invoke it. |
| Pick window     | `Ctrl + Alt + Space`    | Hints every visible top-level window across all monitors. Type one to focus it.    |
| Confirm         | type the hint label     | Commits the selection.                                                             |
| Backspace       | `Backspace`             | Drops the last typed character (inside an overlay).                                |
| Cancel          | `Esc`                   | Dismisses the current overlay without doing anything.                              |
| Settings        | Tray menu → Settings... | Open the visual configuration dialog.                                              |
| Quit            | Tray menu → Quit        | Cleanly exits `keyhop`.                                                            |

The tray icon's right-click menu mirrors the two leader chords, gives you a `Settings...` entry to customize keyhop visually, and (in release builds) a `View Log` shortcut so you can debug without leaving the keyboard.

Yellow badges = element picker (will *invoke* the control). Orange badges = window picker (will *focus* the window). Both show the hint letters on the home row by default — colors and the alphabet are editable from `Settings...`.

### Web browsers

`Ctrl+Shift+Space` works inside Chromium-based browsers (Chrome, Edge, Brave, Opera, Vivaldi, Arc) and finds the actual links, buttons, file rows, nav tabs, and form inputs *inside* the rendered page — not just the browser chrome. keyhop wakes up Chromium's accessibility tree on demand by sending `WM_GETOBJECT` to every renderer HWND before walking, and descends 32 levels deep on browser foregrounds (vs 12 for desktop apps) since DOM trees routinely nest 15–25 deep. Firefox is on the roadmap for v0.4.x; today it surfaces only chrome elements.

## Configuration

`keyhop` is configured through a small visual dialog reachable from the tray icon (`Settings...`), so you never have to touch a config file unless you want to. The dialog lets you change:

- **Hotkeys** — both leader chords. Type any combination of `Ctrl`, `Shift`, `Alt`, `Win`/`Super` modifiers plus a key (`A`-`Z`, `0`-`9`, `F1`-`F24`, `Space`, arrow keys, punctuation, etc.). Example: `Ctrl+Alt+K`.
- **Hint alphabet** — the characters used to build hint labels. Default `asdfghjkl` (the home row). The Settings dialog now also exposes:
  - a **preset** dropdown (`home_row`, `home_row_extended`, `lowercase_alpha`, `alphanumeric`, `numbers`, `custom`),
  - **Include numbers** / **Include extended (`;` `'`)** toggles that append digits or the extended home-row keys,
  - **Exclude ambiguous (O / 0)** to strip easy-to-confuse glyphs,
  - and a **Custom additions** field for keyboard layouts that need extras beyond the preset.
- **Hint strategy** — `Shortest first` (default) hands out single-character hints to the first `alphabet.len()` targets, then two-character hints, and so on (so up to 9 targets only need *one* keystroke). `Fixed length` reproduces the v0.3.0 behaviour where every label is the same length.
- **Scope** — pick which surface the element picker walks: `Active window` (default, v0.3.0 behaviour), `Active monitor` (every visible top-level window on the monitor under your cursor), or `All windows` (every monitor; capped by `max_elements`, default 300, so a busy desktop can't render thousands of badges).
- **Performance** — toggles the in-process element-tree cache (default on, 500 ms TTL). The cache memoises each window's UIA enumeration so the "press Esc, retry" flow is instant on a steady screen.
- **Overlay colors** — element badge background and window badge background, as `#RRGGBB` hex.
- **Overlay opacity** — per-badge-style translucency from `0` to `100` (where `0` means "use the preset default" and `100` means fully opaque). Lower values let the underlying UI bleed through so you can see what you're about to invoke; the defaults sit around 90% for the element picker and 94% for the window picker.
- **Target indicator** — when enabled, every element badge paints a thin outline (in the badge's background color) around the actual click target so it's instantly clear which underlying control each card represents — yellow badge, yellow rectangle around the matching element. When smart positioning had to push the badge off the element to dodge a collision, a connector line ending in a small filled triangle is also drawn from the badge to the element. On by default for the element picker; off for the window picker (which already shows a title pill). Toggle per picker via `colors.element.show_leader` / `colors.window.show_leader` in `config.toml`, or via the "Draw arrow from each badge to its target element" checkbox in the Settings dialog.
- **Launch at Windows startup** — toggles a per-user entry in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (no admin rights required).

`Save` validates everything (invalid hotkey strings or hex colors are rejected up-front), writes `%APPDATA%\keyhop\config.toml`, and shows a confirmation. `Reset to Defaults` deletes the config file. Hotkey, alphabet, and color changes apply on the next launch; the startup toggle takes effect immediately.

If a configured hotkey is already in use by another app at startup, keyhop reports the conflict via a notification dialog and continues running with the surviving chord (e.g. if pick-window conflicts but pick-element doesn't, the latter still works). Open `Settings...` to choose a different combination.

### config.toml format (for power users)

The Settings dialog is the recommended path, but `%APPDATA%\keyhop\config.toml` is plain TOML if you prefer to script it:

```toml
[hotkeys]
pick_element = "Ctrl+Shift+Space"
pick_window  = "Ctrl+Alt+Space"

[hints]
alphabet           = "asdfghjkl"
strategy           = "shortest_first"   # shortest_first | fixed_length
preset             = "home_row"          # home_row | home_row_extended | lowercase_alpha | alphanumeric | numbers | custom
include_numbers    = false               # append 0-9
include_extended   = false               # append ; '
exclude_ambiguous  = true                # strip O / 0 from the resolved alphabet
custom_additions   = ""                  # extra chars appended verbatim

[scope]
mode         = "active_window"           # active_window | active_monitor | all_windows
max_elements = 300                       # hard cap for all_windows mode

[performance]
enable_caching = true                    # memoise UIA element trees per window
cache_ttl_ms   = 500                     # how long a cached enumeration stays fresh

[colors.element]
badge_bg     = "#FFE500"  # leave empty to keep the default
badge_fg     = ""
border       = ""
opacity      = 0          # 0 = preset default; 1..100 = explicit percent
show_leader  = true       # omit the line entirely to keep the preset default
leader_color = ""         # hex pen color for the arrow; empty = preset default

[colors.window]
badge_bg     = "#33AAFF"
badge_fg     = ""
border       = ""
opacity      = 0
show_leader  = false
leader_color = ""

[startup]
launch_at_startup = false
```

Missing keys, missing sections, and an entirely missing file all fall back to the v0.1.0 defaults. Malformed TOML is logged and ignored — keyhop will start with defaults rather than refuse to run.

## Library use

Beyond the binary, `keyhop` exposes a small library API. The Windows backend implements [`Backend`](https://docs.rs/keyhop/latest/keyhop/trait.Backend.html), and the [`HintEngine`](https://docs.rs/keyhop/latest/keyhop/struct.HintEngine.html) is platform-agnostic.

```rust
use keyhop::{Backend, HintEngine};

#[cfg(windows)]
fn enumerate() -> anyhow::Result<()> {
    let mut backend = keyhop::windows::WindowsBackend::new()?;
    let elements = backend.enumerate_foreground()?;
    let hints = HintEngine::default().generate(elements.len());
    for (el, hint) in elements.iter().zip(hints.iter()) {
        println!("{hint}: {:?} {:?}", el.role, el.name);
    }
    Ok(())
}
```

> The library surface is **experimental** while we're pre-1.0 — minor releases may break it. Pin a specific version if you embed it.

## Roadmap

### Shipped (v0.1.0)

- [x] Single-crate scaffold publishable to crates.io
- [x] Foreground window UI Automation tree walk
- [x] Global leader hotkeys + modal input
- [x] Hint overlay (transparent layered window) with collision resolution
- [x] Invoke action dispatch
- [x] Window picker mode (Alt-Tab with hints, all monitors)
- [x] Multi-monitor coordinate handling
- [x] System tray icon + context menu
- [x] CLI flags (`--version`, `--help`, `--no-tray`)
- [x] Single-instance guard
- [x] GUI-subsystem release builds (hidden console, file logging)

### Shipped (v0.2.0)

- [x] Visual Settings dialog (no `.toml` editing required)
- [x] Configurable hotkeys via `%APPDATA%\keyhop\config.toml` with chord parser
- [x] Configurable hint alphabet
- [x] Configurable overlay colors (`#RRGGBB`)
- [x] Hotkey conflict detection with user notification
- [x] Windows startup integration via `HKCU\...\Run` (no admin)
- [x] Scroll action wired through UIA `UIScrollPattern`
- [x] User-facing notifications (no elements found, hotkey conflict, action failed)

### Shipped (v0.3.0)

- [x] Browser webpage content detection (Chrome / Edge / Brave / Opera / Vivaldi / Arc) via on-demand Chromium accessibility-tree activation (`WM_GETOBJECT` to every renderer HWND) plus a 32-level UIA walk on browser foregrounds
- [x] Multi-monitor window picker fixes: badges anchor inside the title bar (visible on maximized windows), DWM-cloaked background UWP apps are filtered out, and selecting a minimized window restores it before focusing
- [x] Element-picker badge UX: smaller font, layout pass tries `OutsideTop` first so the underlying control stays visible
- [x] `keyhop --close` / `--quit` — cleanly shut down a running instance from any terminal (hidden message-only IPC window translates `WM_CLOSE` into the same path the tray's "Quit" entry uses)
- [x] `keyhop --clear-logs` — wipes log files under `%LOCALAPPDATA%\keyhop\`
- [x] Daily log rotation with 7-file retention (replaces the unbounded single `keyhop.log`)
- [x] Walk diagnostics — every `enumerate_foreground` logs nodes visited, deepest depth, and whether the depth/element cap was hit
- [x] **MSI installer** (`Keyhop-<version>-x86_64.msi`) — silent-install, single ARP entry, Start Menu shortcut, PATH integration; built by `cargo wix` and attached to every GitHub Release
- [x] CI: `release.yml` builds and attaches both `keyhop.exe` and the MSI; `actions/checkout` and `softprops/action-gh-release` bumped to Node.js 24 versions
- [x] Dependabot configured (`cargo` + `github-actions`, weekly)

### Shipped (v0.4.0)

- [x] **Variable-length hint labels** with shortest-first allocation — single-character hints for ≤ 9 targets, then two-character for the next tier, etc. Legacy fixed-length still available via `[hints] strategy = "fixed_length"`. ([#4](https://github.com/rsaz/keyhop/issues/4))
- [x] **Alphabet presets** (`home_row`, `home_row_extended`, `lowercase_alpha`, `alphanumeric`, `numbers`, `custom`) plus three independent modifiers (`include_numbers`, `include_extended`, `exclude_ambiguous`) and a free-form `custom_additions` field. All exposed in the Settings dialog.
- [x] **Consolas overlay font** (replaces Segoe UI) — monospace, ships with every Windows version, draws `I` / `l` / `1` distinctly so typed-the-wrong-letter mistakes drop sharply.
- [x] **Multi-screen targeting modes**: `active_window` (default), `active_monitor` (everything on the cursor's monitor), `all_windows` (every visible top-level window across every monitor, capped by `max_elements`).
- [x] **Smart multi-monitor badge positioning** — overlay layout now respects the source monitor's work-area, drops candidate placements that would clip across monitors, and clamps fallbacks back inside the source monitor. The element-style picker also gained an `OutsideBottom` candidate as the off-element fallback when `OutsideTop` would clip.
- [x] **Element-tree caching** — pluggable `Clock`-driven `CacheManager` memoises each window's UIA enumeration for a tunable TTL (default 500 ms), so repeat presses on a steady screen are instant. Toggle via `[performance] enable_caching` / `cache_ttl_ms`.
- [x] **Settings dialog gains six new controls** for hint strategy, alphabet preset + modifiers, custom additions, scope mode, max elements, caching enable, and cache TTL. Window grew to 980×560 px.

### Next up (v0.5.0) — Linux backend + signing

- [ ] Linux backend via AT-SPI (X11 first, then Wayland)
- [ ] Linux global hotkey integration (X11 `XGrabKey` / Wayland portal)
- [ ] Linux overlay rendering (X11 `_NET_WM_STATE_ABOVE` layered window first; layer-shell on Wayland later)
- [ ] Linux tray icon via the StatusNotifierItem / AppIndicator protocol
- [ ] Smoke-test on GNOME (Wayland), KDE (X11), and a tiling WM (i3 or Sway)
- [ ] Microsoft Trusted Signing for the MSI + EXE (kills the SmartScreen "unknown publisher" warning; see [`docs/CODE_SIGNING.md`](docs/CODE_SIGNING.md))
- [ ] Firefox accessibility-tree activation (Gecko uses a different IPC dance than Chromium's `WM_GETOBJECT`)

### v0.6.0 — macOS backend

- [ ] macOS backend via the Accessibility API (`AXUIElement`)
- [ ] macOS global hotkeys via `RegisterEventHotKey` / `MASShortcut`-style API
- [ ] macOS overlay (`NSWindow` with `NSWindowCollectionBehaviorCanJoinAllSpaces`)
- [ ] macOS menu bar item with the same Settings / Pick / Quit affordances as the Windows tray
- [ ] Notarized `.app` bundle build pipeline

### v0.7.0 — Polish + cross-distro install

- [ ] Polished tray icon (multi-resolution `.ico` instead of the procedural badge)
- [ ] Hot-reload config without restarting (re-register hotkeys at runtime)
- [ ] Per-color overrides exposed in the Settings dialog (today only badge backgrounds are editable)
- [ ] Click-through overlay so non-target apps still see the mouse
- [ ] Winget manifest (Windows)
- [ ] Linux installer story: `.deb` + `.rpm` packages, plus a Flatpak manifest

See [CHANGELOG.md](CHANGELOG.md) for release history.

## Contributing

Issues and PRs welcome. Please run before pushing:

```powershell
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

### Building the installer locally

The repo ships a [`Scripts.toml`](Scripts.toml) for [`cargo-run`](https://crates.io/crates/cargo-run) that wraps the MSI build:

```powershell
cargo install --locked cargo-run cargo-wix
scoop install wixtoolset3              # or grab WiX 3.14 from wixtoolset.org

cargo script msi                       # full: cargo build --release + cargo wix
cargo script msi-only                  # MSI only (assumes target/release/keyhop.exe)
cargo script msi-show                  # ls target/wix/*.msi
cargo script msi-clean                 # rm target/wix
```

The `msi` script auto-detects WiX from `$env:WIX`, scoop's `wixtoolset3`, and the standard Program Files install paths.

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
