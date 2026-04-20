# keyhop

> Drive your entire desktop from the keyboard. Press a leader key, see hint labels on every clickable thing on screen, type the hint, done.

`keyhop` is a system-wide keyboard navigation layer that lets you control your whole computer without ever touching the mouse. Reaching for the mouse forces a constant context switch between thinking and pointing — your hands leave the home row, your eyes hunt for a cursor, and your flow breaks. `keyhop` keeps you on the keyboard so you stay fast, focused, and productive, using OS accessibility APIs to target native UI elements semantically.

**Status:** Pre-alpha. Windows backend in early development. Linux planned.

## Goals

- Native performance and instant feel (sub-50ms hint overlay).
- Semantic targeting via OS accessibility trees (UI Automation on Windows, AT-SPI on Linux).
- Cross-platform core, thin platform-specific backends.
- Publishable as reusable Rust crates so others can build alternative frontends.

## Workspace layout

```
keyhop/
├─ crates/
│  ├─ keyhop-core/      # platform-agnostic types, traits, hint engine
│  ├─ keyhop-windows/   # Windows backend (UI Automation, hooks, overlay)
│  └─ keyhop/           # the binary that wires everything together
```

`keyhop-core` and `keyhop-windows` are designed to be published independently to crates.io.

## Build (Windows)

Requires:
- Rust stable with the `x86_64-pc-windows-msvc` toolchain
- Visual Studio Build Tools with the "Desktop development with C++" workload

```powershell
cargo build --workspace
cargo run -p keyhop
cargo run -p keyhop-windows --example enumerate_foreground
```

## Using it

Run `cargo run -p keyhop` (or `cargo run --release -p keyhop` for the snappy experience). The app sits in the terminal and registers a global hotkey:

| Action  | Keys                  |
| ------- | --------------------- |
| Leader  | `Ctrl + Shift + Space` |
| Confirm | type the hint label    |
| Backspace | `Backspace`          |
| Cancel  | `Esc` (inside overlay) |
| Quit    | `Ctrl+C` in the terminal |

Switch focus to any app, hit the leader, then type the yellow label that appears on the control you want — `keyhop` invokes it via UI Automation.

## Roadmap

- [x] Workspace scaffold
- [x] Foreground window UI Automation tree walk
- [x] Global leader hotkey + modal input
- [x] Hint overlay (transparent layered window)
- [x] Invoke action dispatch
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
