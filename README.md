# keyhop

> Drive your entire desktop from the keyboard. Press a leader chord, see hint labels on every clickable thing on screen, type the hint, done.

`keyhop` is a system-wide keyboard navigation layer that lets you control your whole computer without ever touching the mouse. Reaching for the mouse forces a constant context switch between thinking and pointing — your hands leave the home row, your eyes hunt for a cursor, and your flow breaks. `keyhop` keeps you on the keyboard so you stay fast, focused, and productive, using OS accessibility APIs (UI Automation on Windows) to target native UI elements semantically.

**Status:** v0.1.0 — Windows backend. Linux backend planned.

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
│  └─ windows/             # Windows backend (cfg(windows) only)
│     ├─ mod.rs            # WindowsBackend (UI Automation tree walk)
│     ├─ hotkey.rs         # global leader hotkeys
│     ├─ overlay.rs        # transparent layered hint overlay
│     ├─ tray.rs           # system tray icon + context menu
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
keyhop                         # release install
cargo run --release            # from source
cargo run                      # debug build (keeps console window)
cargo run --example enumerate_foreground
```

In v0.1.0 the binary launches in console mode — running `keyhop` opens a small terminal window where startup logs and tracing output appear. The yellow `K` tray icon is the user-facing control surface; the console is mainly there for visibility and `Ctrl + C` quit. A future release will switch to the GUI subsystem and hide the console.

### Flags

| Flag                  | What it does                                                |
| --------------------- | ----------------------------------------------------------- |
| `-h`, `--help`        | Print usage and exit.                                       |
| `-V`, `--version`     | Print version and exit.                                     |
| `--no-tray`           | Run without the system tray icon (hotkeys-only mode).       |

## Using it

After launching, `keyhop` registers two global hotkeys and a tray icon:

| Action          | Keys                    | What it does                                                                       |
| --------------- | ----------------------- | ---------------------------------------------------------------------------------- |
| Pick element    | `Ctrl + Shift + Space`  | Hints every interactable control inside the focused window. Type one to invoke it. |
| Pick window     | `Ctrl + Alt + Space`    | Hints every visible top-level window across all monitors. Type one to focus it.    |
| Confirm         | type the hint label     | Commits the selection.                                                             |
| Backspace       | `Backspace`             | Drops the last typed character (inside an overlay).                                |
| Cancel          | `Esc`                   | Dismisses the current overlay without doing anything.                              |
| Quit            | Tray menu → Quit        | Cleanly exits `keyhop` (also `Ctrl + C` if launched from a terminal).              |

The tray icon's right-click menu mirrors the two leader chords plus a Quit entry, so you can drive `keyhop` even if you forget the hotkeys.

Yellow badges = element picker (will *invoke* the control). Orange badges = window picker (will *focus* the window). Both show the hint letters on the home row by default.

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

- [x] Single-crate scaffold
- [x] Foreground window UI Automation tree walk
- [x] Global leader hotkeys + modal input
- [x] Hint overlay (transparent layered window)
- [x] Invoke action dispatch
- [x] Window picker mode (Alt-Tab with hints, all monitors)
- [x] Multi-monitor coordinate fix
- [x] Hint collision resolution
- [x] System tray icon + menu
- [x] CLI flags (`--version`, `--help`, `--no-tray`)
- [x] Single-instance guard
- [ ] GUI-subsystem release build (hide the console window) with parent-console attach for `--help`/`--version`
- [ ] Configurable hotkeys / colors / alphabet (TOML config)
- [ ] More UIA actions (`Focus`, `Type`, `Scroll`)
- [ ] Click-through overlay so non-target apps still see the mouse
- [ ] MSI installer + Winget manifest
- [ ] Linux backend (X11 first)
- [ ] Wayland backend
- [ ] macOS backend (Accessibility API)

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
