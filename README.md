# keyhop

> Drive your entire desktop from the keyboard. Press a leader key, see hint labels on every clickable thing on screen, type the hint, done.

`keyhop` is a system-wide keyboard navigation layer that lets you control your whole computer without ever touching the mouse. Reaching for the mouse forces a constant context switch between thinking and pointing — your hands leave the home row, your eyes hunt for a cursor, and your flow breaks. `keyhop` keeps you on the keyboard so you stay fast, focused, and productive, using OS accessibility APIs to target native UI elements semantically.

**Status:** Pre-alpha. Windows backend in early development. Linux planned.

## Goals

- Native performance and instant feel (sub-50ms hint overlay).
- Semantic targeting via OS accessibility trees (UI Automation on Windows, AT-SPI on Linux).
- Single, easy-to-install crate with platform backends gated behind `cfg`.

## Crate layout

```
keyhop/
├─ src/
│  ├─ lib.rs        # public API: Action, Backend, Element, HintEngine
│  ├─ main.rs       # the `keyhop` binary
│  ├─ model.rs
│  ├─ action.rs
│  ├─ backend.rs
│  ├─ hint.rs
│  └─ windows/      # Windows backend (cfg(windows) only)
│     ├─ mod.rs     # WindowsBackend (UI Automation)
│     ├─ hotkey.rs  # global leader hotkey
│     └─ overlay.rs # transparent layered overlay
└─ examples/
   └─ enumerate_foreground.rs
```

One package, one publish: `keyhop` ships both the binary and a reusable library API. Linux / Wayland / macOS backends will land as additional `cfg`-gated modules under `src/`.

## Install / build (Windows)

Requires:
- Rust stable with the `x86_64-pc-windows-msvc` toolchain
- Visual Studio Build Tools with the "Desktop development with C++" workload

```powershell
cargo install --path .            # install the binary into ~/.cargo/bin
cargo run --release               # run from source
cargo run --example enumerate_foreground
```

## Using it

Run `cargo run` (or `cargo run --release` for the snappy experience). The app sits in the terminal and registers two global hotkeys:

| Action          | Keys                    | What it does                                                                 |
| --------------- | ----------------------- | ---------------------------------------------------------------------------- |
| Pick element    | `Ctrl + Shift + Space`  | Hints every interactable control inside the focused window. Type one to invoke it. |
| Pick window     | `Ctrl + Alt + Space`    | Hints every visible top-level window across all monitors. Type one to focus it.   |
| Confirm         | type the hint label     | Commits the selection.                                                       |
| Backspace       | `Backspace`             | Drops the last typed character (inside an overlay).                          |
| Cancel          | `Esc`                   | Dismisses the current overlay without doing anything.                        |
| Quit            | `Ctrl + C`              | Stops `keyhop` (in the terminal).                                            |

Switch focus to any app, hit the leader, then type the label that appears on whatever you want — `keyhop` invokes the control (yellow badges) or focuses the window (orange badges).

## Roadmap

- [x] Single-crate scaffold
- [x] Foreground window UI Automation tree walk
- [x] Global leader hotkey + modal input
- [x] Hint overlay (transparent layered window)
- [x] Invoke action dispatch
- [x] Window picker mode (Alt-Tab with hints, all monitors)
- [ ] System tray icon + menu
- [ ] Click-through overlay so non-target apps still see the mouse
- [ ] Multi-monitor / per-monitor DPI tuning
- [ ] More actions (Focus, Type, Scroll)
- [ ] Configuration file (TOML)
- [ ] Linux backend (X11 first)
- [ ] Wayland backend

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
