# Changelog

All notable changes to `keyhop` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-04-22

Browser web-content release. The element picker now actually finds the
links and buttons *inside* a web page (previously you only got hints on
browser chrome), the window picker stops listing background UWP ghosts,
and `keyhop` learned to shut itself down from another terminal.

### Added
- **Browser webpage content detection.** `Ctrl+Shift+Space` on a
  Chromium-based browser (Chrome, Edge, Brave, Opera, Vivaldi, Arc) now
  hints the actual page content — links, buttons, file rows, nav tabs,
  form inputs — instead of just the browser chrome. Two pieces working
  together: the UI Automation walk now descends 32 levels deep on
  browser foregrounds (vs 12 for desktop apps, since DOM trees are
  routinely 15–25 deep), and `keyhop` now sends `WM_GETOBJECT` with
  `UIA_ROOT_OBJECT_ID` / `OBJID_CLIENT` to every renderer HWND before
  walking, which forces Chromium's `BrowserAccessibilityState` to build
  the per-tab accessibility tree on demand.
- **`keyhop --close` / `--quit`.** A second `keyhop` invocation can now
  cleanly shut down the running one without touching the tray icon. A
  hidden top-level IPC window in the running instance receives
  `WM_CLOSE` from the closer process and translates it into the same
  `PostQuitMessage(0)` path the tray's "Quit" entry uses, so all
  global-hotkey registrations and the single-instance mutex get
  released properly.
- **`keyhop --clear-logs`.** Deletes every file under
  `%LOCALAPPDATA%\keyhop\` whose name starts with `keyhop`. If the
  current process holds a file open the command tells you to run
  `--close` first and exits non-zero.
- **Daily log rotation with 7-file retention.** Replaces the previous
  single never-rotated `keyhop.log` (which grew unbounded). Files are
  named `keyhop.YYYY-MM-DD.log`, the oldest is pruned automatically
  when an eighth file would be created.
- **Walk diagnostics.** Every `enumerate_foreground` now logs at debug
  level the elements collected, total nodes visited, deepest depth
  reached, and whether the walk hit the depth or element cap. Triages
  "didn't see X on page Y" reports without a Spy++ session.

### Changed
- **Element-picker badges are smaller and float above the target.**
  Font height dropped from 20px to 16px and horizontal padding from 6px
  to 5px, and the layout pass now tries `OutsideTop` first (then
  `TopLeft` / `TopRight` / `BottomRight`). Hints sit just above the
  control they label so the underlying UI stays visible while you
  decide which key to press.
- **Window-picker badges are anchored inside the window's top-left.**
  Adds a `prefer_inside_anchor: bool` to `HintStyle` so the window
  preset can opt out of `OutsideTop`. Without this, every maximized
  window's badge would render above the top of its monitor (i.e.
  invisible). Window targets are large enough that placing the badge
  on the title bar costs nothing visually.
- **Window picker filters all DWM-cloaked windows.** Previously the
  picker tried to keep `cloaked=2` windows under the theory they were
  on another Windows virtual desktop. In practice that flag is mostly
  set on background-suspended UWP apps (Calculator, Media Player,
  Settings, Photos…) which polluted the picker with apps the user
  doesn't actually have open. Skipping every cloaked window matches
  Alt-Tab's behaviour and clears the noise.
- **Window picker handles minimized windows.** Selecting a minimized
  window now `ShowWindow(SW_RESTORE)`s it before focusing, so the
  picker is usable as a "wake up that minimized window" affordance,
  not just a switcher.

### Internals
- New module `src/windows/ipc.rs` (hidden window IPC for `--close`).
- `MAX_TREE_DEPTH` split into `MAX_TREE_DEPTH_DESKTOP` (12) and
  `MAX_TREE_DEPTH_BROWSER` (32). New `WalkStats` struct propagated
  through the recursive walk for the diagnostic log.
- New `windows` API surface used: `Win32_UI_WindowsAndMessaging::{
  EnumChildWindows, SendMessageTimeoutW, FindWindowW, PostMessageW,
  RegisterClassExW, CreateWindowExW }` (already enabled features).
- Hint-overlay layout tests rewritten to cover both element- and
  window-style position priorities.

### Earlier (carried forward from the unreleased branch)
- **Translucent overlay badges.** The hint overlay now blends with the
  underlying UI instead of fully obscuring it. Defaults are tuned for
  readability (~90% opacity for the element picker, ~94% for the window
  picker); both are user-tunable from `Settings...` or `config.toml` via
  the new `colors.element.opacity` / `colors.window.opacity` fields
  (0..=100, where `0` keeps the preset default).
- **Smarter badge placement.** Each hint now tries several anchor
  positions (top-left → top-right → outside-top → bottom-right) before
  falling back to the original "stack downward" behaviour. Two adjacent
  controls no longer dogpile their badges on top of each other when the
  smarter positions are free.
- **Pattern-based desktop detection.** The element picker now recognises
  custom controls that don't expose a known `ControlType` but do publish
  an action pattern (`Invoke`, `Toggle`, `SelectionItem`, `ExpandCollapse`).
  Modern Win32 / WinUI / Electron apps that wrapped clickable behaviour in
  a `Pane` or `Group` start showing badges where they previously were
  invisible.
- **Expanded `ControlType` mapping.** `Image` and `DataItem` are now
  picked up directly (icon buttons in toolbars, grid cells in data
  views).
- **Browser-aware enumeration.** `keyhop` now detects when the foreground
  process is a known browser (`chrome.exe`, `msedge.exe`, `firefox.exe`,
  `brave.exe`, `opera.exe`, `vivaldi.exe`, `arc.exe`, `iexplore.exe`) and
  switches to a stricter detection path tuned for DOM-style trees:
  size-filters tiny decorative nodes, raises the per-walk element cap
  from 500 to 800, and consults the legacy `IAccessible` role so ARIA
  `role="button"` / `role="link"` elements that don't publish modern
  patterns still get hints.
- **`text_shadow` style flag** on `HintStyle` for an optional 1px text
  shadow on busy backgrounds. Off by default; off in both shipped
  presets.
- **Per-role debug breakdown** logged at `debug` level after every
  enumeration. `RUST_LOG=keyhop=debug keyhop` now prints the
  `Button: N, Link: N, …` histogram so it's easy to spot when a target
  app's controls are getting dropped at the role-mapping stage.
- **Per-element target indicator.** Element-picker badges now paint a
  thin 1px outline (in the badge background color) around the actual
  click target so it's immediately obvious which underlying control
  each card maps to — even when the badge sits in the corner of a
  large button. When smart positioning had to push the badge off the
  element to dodge a collision, a connector line + filled triangular
  arrowhead is also drawn from the badge to the element. On by default
  for the element picker; off for the window picker (which already
  shows a title pill). Toggle from `Settings...` ("Draw arrow from
  each badge to its target element") or `config.toml`
  (`colors.element.show_leader = true|false`, plus an optional
  `colors.element.leader_color` for the connector color).
- **`is_keyboard_focusable` web heuristic.** The browser detection path
  now also accepts elements that the page declared keyboard-tabbable
  (the modern accessibility floor for "this is interactive"). Catches
  click-handler-only `<div>`s, custom widgets, and ARIA composites that
  publish neither an action pattern nor a known role.
- **Looser size floor for proven-interactive web elements.** Elements
  that pass any of the interactivity checks (action pattern, known
  ARIA role, focusable) now only need to clear 10×10 px instead of
  16×16 px, so chip "×" icons, dense toolbar buttons, and small icon
  controls become hintable on real-world pages.
- **Expanded MSAA role coverage** in the web-clickable heuristic
  (`page tab`, `outline item`, `list item`, `cell`, `combobox`,
  `dropdown button`, `menu button`). Picks up Gmail-style row toggles,
  GitHub's file tree, dashboard data grids, and the "more actions"
  menu buttons on most modern web apps.

### Changed
- `BadgeColors` gained an `opacity: u8` field plus optional
  `show_leader: Option<bool>` and `leader_color: String`. Existing
  configs without the new fields deserialize to the preset defaults
  (`opacity = 0`, `show_leader = None`, `leader_color = ""`), so
  upgrades remain non-disruptive.
- The Settings dialog grew an `Opacity (0-100)` section with one edit
  per badge style and a new "Draw arrow from each badge to its target
  element" checkbox; the window is 60px taller to fit them.

### Internals
- 12 new unit tests covering the opacity-config conversion, the smart
  badge-positioning anchors, the multi-position layout pass, and the
  leader-line endpoint / arrowhead geometry helpers.
- New `windows` API surface: `Win32_System_Threading::OpenProcess` /
  `QueryFullProcessImageNameW` (already enabled feature, no Cargo.toml
  changes needed) for the browser process detection. Plus
  `Win32_Foundation::POINT` + `Win32_Graphics_Gdi::{CreatePen, LineTo,
  MoveToEx, Polygon, PS_SOLID}` for the leader-line GDI calls (also
  already enabled).

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

[Unreleased]: https://github.com/rsaz/keyhop/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/rsaz/keyhop/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/rsaz/keyhop/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/rsaz/keyhop/releases/tag/v0.1.0
