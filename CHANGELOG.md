# Changelog

All notable changes to `keyhop` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-04-20

Initial public release. Windows-only.

### Added
- **Element picker** (`Ctrl + Shift + Space`) — walks the foreground
  window's UI Automation tree and overlays a hint label on every
  interactable control (buttons, links, inputs, menu items, tabs,
  checkboxes, radios, combo boxes, list/tree items). Type a label to
  invoke the element. `Esc` cancels.
- **Window picker** (`Ctrl + Alt + Space`) — enumerates every visible,
  non-cloaked top-level window across all monitors (filtering out
  shell, tool, and progman/workerw windows) and overlays a hint plus a
  truncated title pill. Type a label to bring that window to the
  foreground.
- **System tray icon** — yellow procedurally-generated badge in the
  notification area with a context menu (Pick element, Pick window,
  Quit) that mirrors the hotkeys and provides a clean shutdown path.
- **Transparent layered overlay** with magenta color-key, GDI label
  rendering, per-monitor DPI awareness, and a vertical-stacking
  collision-resolution pass so overlapping anchors don't draw on top of
  each other.
- **Two distinct hint styles**: yellow badges for elements (will
  *invoke*) and orange badges for windows (will *focus*).
- **CLI flags**: `--help` / `-h`, `--version` / `-V`, `--no-tray`.
- **Single-instance guard** via a session-scoped named mutex —
  launching `keyhop` while it's already running prints a friendly
  message and exits cleanly instead of double-registering hotkeys.
- **Library API** re-exports — `Action`, `Backend`, `Element`,
  `HintEngine`, plus the Windows backend in `keyhop::windows`. Marked
  experimental until v1.0.
- **CI** on `windows-latest` (fmt + clippy + build + test) and a
  `enumerate_foreground` example.

### Known limitations
- Windows-only. Linux (X11/Wayland) and macOS backends are roadmap
  items.
- Release builds run with the console subsystem, so launching
  `keyhop.exe` from Explorer opens a small terminal window. A
  GUI-subsystem build with parent-console attach for `--help` and
  `--version` is planned for v0.2.0.
- Hotkeys, hint colors, and the alphabet are not yet user-configurable
  (planned for v0.2.0 via a TOML config file).
- Only the `Invoke` action is dispatched. `Focus`, `Type`, and
  `Scroll` are stubs.

[Unreleased]: https://github.com/rsaz/keyhop/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/rsaz/keyhop/releases/tag/v0.1.0
