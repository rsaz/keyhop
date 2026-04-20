# keyhop

> Vimium for your entire desktop. Press a leader key, see hint labels on every clickable thing on screen, type the hint, done.

`keyhop` is a system-wide keyboard navigation layer that lets you drive your whole computer without a mouse. Inspired by [Vimium](https://vimium.github.io/) for the browser, but extended to native applications via OS accessibility APIs.

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
│  └─ keyhop-app/       # the binary that wires everything together
```

`keyhop-core` and `keyhop-windows` are designed to be published independently to crates.io.

## Build (Windows)

Requires:
- Rust stable with the `x86_64-pc-windows-msvc` toolchain
- Visual Studio Build Tools with the "Desktop development with C++" workload

```powershell
cargo build --workspace
cargo run -p keyhop-app
cargo run -p keyhop-windows --example enumerate_foreground
```

## Roadmap

- [x] Workspace scaffold
- [ ] Foreground window UI Automation tree walk
- [ ] Hint overlay (transparent click-through window)
- [ ] Global leader hotkey + modal input
- [ ] Action dispatch (Invoke, Focus, Type, Scroll)
- [ ] Configuration file (TOML)
- [ ] Linux backend (X11 first)
- [ ] Wayland backend

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
