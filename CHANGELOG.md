# Changelog

All notable changes to `keyhop` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-04-21

Configuration & robustness release. Everything stays inside the tray —
the new Settings dialog means you no longer have to hand-edit any files.

### Added
- **Visual Settings dialog** reachable from the tray icon
  (`Settings...`). Lets you change hotkeys, hint alphabet, overlay
  colors, and Windows startup integration without ever touching a
  config file. Validates input on Save, shows clear error dialogs for
  invalid hotkeys / hex colors, and offers a `Reset to Defaults`
  button.
- **TOML config file** at `%APPDATA%\keyhop\config.toml`. Missing or
  malformed configs gracefully fall back to the v0.1.0 defaults so
  upgrades are non-disruptive.
- **Customizable hotkey chords** via a new parser that accepts
  `Ctrl`/`Control`/`Shift`/`Alt`/`Win`/`Super` modifiers plus any
  letter, digit, F-key, arrow, space, enter, or punctuation key
  (`Ctrl+Alt+K`, `Win+F12`, etc.) — case-insensitive, whitespace
  tolerant.
- **Hotkey conflict detection** — if another app already owns one of
  your chords, keyhop surfaces a notification dialog naming the
  conflicting chord and continues running with whatever did register.
- **Customizable hint alphabet** (default `asdfghjkl`).
- **Customizable overlay colors** (`#RRGGBB` hex). Element badge
  background and window badge background are exposed in the dialog;
  power users can override foreground and border colors directly in
  the TOML file.
- **Windows startup integration** — `Launch keyhop at Windows startup`
  checkbox writes / removes a per-user entry in
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (no admin
  required). The checkbox state mirrors the registry, not the config,
  so it stays accurate even if the registry is changed externally.
- **`Action::Scroll` implemented** for the Windows backend via the
  UIA `UIScrollPattern`. Pixel deltas are mapped to `LargeIncrement` /
  `SmallIncrement` based on magnitude (threshold matches Windows'
  `WHEEL_DELTA == 120` convention).
- **User-facing notifications** for "no interactive elements found",
  "no visible windows", action failures, and hotkey conflicts. All
  surface through `MessageBoxW` so they're visible even when the
  release build runs without a console.
- **`Settings...` and `View Log` items in the tray menu** (the latter
  in release builds only).

### Changed
- Hint engine, element style, and window style are now built once at
  startup from the loaded config and threaded through the picker
  handlers, replacing the previous `HintEngine::default()` /
  `HintStyle::elements()` calls scattered through `main.rs`.
- Startup banner now prints whichever hotkey chords are currently
  registered, so it stays accurate when the user customizes them.

### Internals
- New modules: `src/config.rs`, `src/windows/notification.rs`,
  `src/windows/settings_window.rs`, `src/windows/startup.rs`.
- New deps: `serde`, `toml`. New `windows` feature flags:
  `Win32_System_Registry`, `Win32_UI_Shell`, `Win32_UI_Controls`.
- 13 new unit tests covering hotkey-chord parsing, hex-color parsing,
  and TOML config round-tripping.

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

[Unreleased]: https://github.com/rsaz/keyhop/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/rsaz/keyhop/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/rsaz/keyhop/releases/tag/v0.1.0
