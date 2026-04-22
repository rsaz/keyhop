# keyhop

> Drive your entire desktop from the keyboard. Press a leader chord, see hint labels on every clickable thing on screen, type the hint, done.

`keyhop` is a system-wide keyboard navigation layer that lets you control your whole computer without ever touching the mouse. Reaching for the mouse forces a constant context switch between thinking and pointing — your hands leave the home row, your eyes hunt for a cursor, and your flow breaks. `keyhop` keeps you on the keyboard so you stay fast, focused, and productive, using OS accessibility APIs (UI Automation on Windows) to target native UI elements semantically.

**Status:** v0.2.0 — Windows backend with visual configuration, customizable hotkeys/colors/alphabet, and Windows startup integration. Linux backend planned.

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
│  ├─ main.rs              # the `keyhop` binary
│  ├─ model.rs
│  ├─ action.rs
│  ├─ backend.rs
│  ├─ hint.rs
│  ├─ config.rs            # TOML config (%APPDATA%/keyhop/config.toml)
│  └─ windows/             # Windows backend (cfg(windows) only)
│     ├─ mod.rs            # WindowsBackend (UI Automation tree walk)
│     ├─ hotkey.rs         # global leader hotkeys + chord parser
│     ├─ overlay.rs        # transparent layered hint overlay + color parser
│     ├─ tray.rs           # system tray icon + context menu
│     ├─ settings_window.rs # visual Settings dialog (Win32)
│     ├─ startup.rs        # "launch at login" via HKCU Run key
│     ├─ notification.rs   # MessageBox-backed user notifications
│     └─ window_picker.rs  # Alt-Tab-style window picker
└─ examples/
   └─ enumerate_foreground.rs
```

One package, one publish: `keyhop` ships both the binary and a reusable library API. Linux / Wayland / macOS backends will land as additional `cfg`-gated modules under `src/`.

## Install

From crates.io (recommended once published):

```powershell
cargo install keyhop
```

From source:

```powershell
git clone https://github.com/rsaz/keyhop
cd keyhop
cargo install --path .
```

Requires:
- Rust stable with the `x86_64-pc-windows-msvc` toolchain
- Visual Studio Build Tools with the "Desktop development with C++" workload

## Run

```powershell
keyhop                         # release install (no console, logs to file)
cargo run --release            # from source (no console, logs to file)
cargo run                      # debug build (shows console with live logs)
cargo run --example enumerate_foreground
```

The binary uses the Windows GUI subsystem in release builds, running silently in the background with no console window. Logs are written to `%LOCALAPPDATA%\keyhop\keyhop.log` and can be viewed via the tray menu's "View Log" option. Debug builds (`cargo run` without `--release`) still show a console window for development convenience.

### Flags

| Flag                  | What it does                                                |
| --------------------- | ----------------------------------------------------------- |
| `-h`, `--help`        | Print usage and exit.                                       |
| `-V`, `--version`     | Print version and exit.                                     |
| `--no-tray`           | Run without the system tray icon (hotkeys-only mode).       |

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

Yellow badges = element picker (will *invoke* the control). Orange badges = window picker (will *focus* the window). Both show the hint letters on the home row by default — but as of v0.2.0 you can change the colors and the alphabet from `Settings...`.

## Configuration

`keyhop` is configured through a small visual dialog reachable from the tray icon (`Settings...`), so you never have to touch a config file unless you want to. The dialog lets you change:

- **Hotkeys** — both leader chords. Type any combination of `Ctrl`, `Shift`, `Alt`, `Win`/`Super` modifiers plus a key (`A`-`Z`, `0`-`9`, `F1`-`F24`, `Space`, arrow keys, punctuation, etc.). Example: `Ctrl+Alt+K`.
- **Hint alphabet** — the characters used to build hint labels. Default `asdfghjkl` (the home row).
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
alphabet = "asdfghjkl"

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

### Next up (v0.3.0) — Linux backend

- [ ] Linux backend via AT-SPI (X11 first, then Wayland)
- [ ] Linux global hotkey integration (X11 `XGrabKey` / Wayland portal)
- [ ] Linux overlay rendering (X11 `_NET_WM_STATE_ABOVE` layered window first; layer-shell on Wayland later)
- [ ] Linux tray icon via the StatusNotifierItem / AppIndicator protocol
- [ ] Smoke-test on GNOME (Wayland), KDE (X11), and a tiling WM (i3 or Sway)

### v0.4.0 — macOS backend

- [ ] macOS backend via the Accessibility API (`AXUIElement`)
- [ ] macOS global hotkeys via `RegisterEventHotKey` / `MASShortcut`-style API
- [ ] macOS overlay (`NSWindow` with `NSWindowCollectionBehaviorCanJoinAllSpaces`)
- [ ] macOS menu bar item with the same Settings / Pick / Quit affordances as the Windows tray
- [ ] Notarized `.app` bundle build pipeline

### v0.5.0 — Polish, click-through, installers

- [ ] Polished tray icon (multi-resolution `.ico` instead of the procedural badge)
- [ ] Hot-reload config without restarting (re-register hotkeys at runtime)
- [ ] Per-color overrides exposed in the Settings dialog (today only badge backgrounds are editable)
- [ ] Click-through overlay so non-target apps still see the mouse
- [ ] MSI installer + Winget manifest (Windows)
- [ ] Linux installer story: `.deb` + `.rpm` packages, plus a Flatpak manifest

See [CHANGELOG.md](CHANGELOG.md) for release history.

## Contributing

Issues and PRs welcome. Please run before pushing:

```powershell
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
