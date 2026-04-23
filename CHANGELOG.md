# Changelog

All notable changes to `keyhop` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Open settings hotkey** — new global hotkey (`Ctrl+Shift+,` by default) to open the Settings dialog directly from anywhere. Previously, settings could only be opened from the tray menu.

### Changed

- **Settings window improvements** — the Settings dialog now opens centered on screen and always on top (topmost window). Multiple invocations (via hotkey, tray menu, or repeated clicks) now bring the existing window to the foreground instead of creating duplicate dialogs, preventing accidentally opening multiple settings windows. Press `Esc` to close the settings window (same as clicking Cancel).

## [0.4.0] - 2026-04-22

UX & performance release. Closes
[#4](https://github.com/rsaz/keyhop/issues/4) ("less letters to type the
better and multiple other issues / ideas") with six related improvements
that work together to make the picker faster to *use* (fewer keystrokes,
crisper labels) and faster to *run* (cached enumeration, scoped walks).

### Added
- **Variable-length hint labels (closed-form Vimium allocator).** The
  new `HintStrategy::ShortestFirst` (default) picks the smallest label
  length `L` such that `n^L ≥ count`, then splits the labels between
  length `L−1` and length `L` so the average keystroke count is
  minimised — 100 elements on the 9-char home-row alphabet fit in 78
  length-2 + 22 length-3 labels (avg 2.22 keystrokes), where v0.3.0's
  fixed-length scheme needed 3 keys for *every* label (avg 3.0). The
  legacy fixed-length behaviour is still available via
  `[hints] strategy = "fixed_length"`. The allocation guarantees no
  label is a prefix of another, so typing `a` always commits `a` and
  never gets stuck waiting for `aa` / `ab` — verified by regression
  tests over counts 1 through 730.
- **`[hints] min_singles` one-key-reach floor (default `8`).** The
  pure average-keystroke optimum skips length-1 hints entirely once a
  scene has more than ~73 elements on a 9-char alphabet, because
  promoting one element to a single forces `n` length-`L` hints to
  grow by one character. That's optimal in aggregate but
  ergonomically surprising — users expect "shortest first" to mean
  "at least one one-key target". `min_singles` reserves a floor of
  length-1 hints, defaulting to `8` (= `n − 1` for the home-row
  alphabet) so the first eight alphabet letters are always one
  keystroke away and only the last letter is consumed as a multi-char
  prefix. The allocator picks
  `max(min_singles, vimium_natural_singles)` so larger alphabets that
  already produce more singles than the floor (e.g. `alphanumeric`
  with `count = 100` yields 13 natural singles) are never penalised.
  Capped at `n − 1` at runtime — we always keep at least one prefix
  slot when `count > n`. Set `min_singles = 0` in `config.toml` (or
  via the new "Min single-key hints" Settings field) to disable the
  floor and recover the pure math-optimal allocation.
- **Alphabet presets** (`[hints] preset = …`):
  `home_row` (default), `home_row_extended` (`asdfghjkl;'`),
  `lowercase_alpha`, `alphanumeric`, `numbers`, and `custom`. Plus
  three independent modifier flags exposed in Settings:
    - `include_numbers` — append `0123456789`.
    - `include_extended` — append `; '`.
    - `exclude_ambiguous` — strip `O 0` (kept conservative; the new
      Consolas overlay font already differentiates `I` / `l` / `1`
      crisply, so dropping them by default would silently shrink the
      home-row preset from 9 chars to 8).
    - `custom_additions` — free-form characters appended after the
      preset, useful for non-ANSI keyboards.
  See [`src/alphabet_presets.rs`](src/alphabet_presets.rs) for the
  builder; the Settings dialog dropdown materialises the resolved
  alphabet on Save so `config.toml` always has a ready-to-use string.
- **Multi-screen targeting modes.** New `[scope] mode` config:
    - `active_window` (default — v0.3.0 behaviour, no change for
      existing users).
    - `active_monitor` — every visible top-level window on the
      monitor that contains the cursor.
    - `all_windows` — every visible top-level window across every
      monitor, capped at `[scope] max_elements` (default 300) so a
      busy desktop can't render thousands of badges. The foreground
      window is always walked first, so its elements are always
      present even when the cap kicks in.
- **Element-tree caching.** `[performance] enable_caching = true`
  (default) memoises [`crate::Element`] vectors per-HWND for
  `cache_ttl_ms` (default 500ms). The "press Esc, retry" flow no
  longer pays for a fresh UIA walk; multi-window scope modes also
  benefit when the user fires the picker repeatedly on the same set
  of windows. New module [`src/cache.rs`](src/cache.rs) with a
  pluggable `Clock` so unit tests don't have to sleep.
- **Smart multi-monitor badge positioning.** The overlay layout now
  determines each badge's source monitor via `MonitorFromRect`, drops
  candidate placements that would land outside the source monitor's
  work area, and clamps any post-collision fallback back inside it.
  The element-style picker grew an `OutsideBottom` candidate as the
  off-element fallback when `OutsideTop` would clip off the monitor's
  top edge. Also fixes the v0.3.0 case where an element at `y == 0`
  on a non-primary monitor would render its `OutsideTop` badge on
  the previous monitor (or off the virtual desktop entirely).
- **Consolas overlay font.** Replaces the Segoe UI default for
  hint labels. Monospace, ships with every Windows version, and
  draws `I` / `l` / `1` distinctly — directly addresses the most
  common "I typed the wrong letter" complaint. Falls back to the
  closest available face if Consolas is missing.
- **Settings dialog gained six new sections / controls**:
  hint-strategy dropdown (Shortest first / Fixed length), alphabet
  preset dropdown, three preset modifier checkboxes, custom-additions
  edit field, "Min single-key hints" edit (the new `min_singles`
  floor), scope-mode dropdown, max-elements edit, enable-caching
  checkbox, and a cache-TTL edit. The "Exclude ambiguous characters"
  checkbox label reads `(O 0)` to match the actual default exclusion
  list — `I` / `l` / `1` were considered for the list but the new
  Consolas overlay font draws them distinctly, so dropping them by
  default would silently shrink the home-row preset from 9 chars to 8.
- **Settings dialog UI overhaul (two-column, 860×640).** The vertical
  scroll-pile is gone: General / Hints / Colors / Performance now sit
  side-by-side in two columns so every option is visible without
  scrolling. Every label has a hover **tooltip** (300 ms show / 8 s
  linger) explaining what the setting does and what its sensible
  range is — sourced from a single `tips` module so the wording stays
  consistent with the README. Power-user controls were upgraded:
    - **Color picker swatches.** Each `#RRGGBB` field grew a 28×24
      owner-drawn swatch button to its right; clicking it opens the
      Win32 `ChooseColorW` common dialog (with `CC_FULLOPEN` so the
      custom-color matrix is available) and writes the selection
      back into the hex edit. Typing into the edit still works and
      live-updates the swatch on `EN_CHANGE`.
    - **Opacity sliders.** Background and badge opacity are now
      `msctls_trackbar32` trackbars (0–100, page 10, tick 10) with
      a live "`82%`" value label that updates on `WM_HSCROLL` —
      drag, click the trough, or arrow-key from the keyboard. The
      old free-text "0..100 or blank" field is gone, which also
      removes a class of typo-induced parse errors on Save.
    - **Numeric spinners.** `min_singles`, `max_elements`, and
      `cache_ttl_ms` are now `msctls_updown32` UpDown spinners
      auto-buddied to their edit fields (`UDS_AUTOBUDDY |
      UDS_SETBUDDYINT | UDS_ALIGNRIGHT | UDS_ARROWKEYS`), with
      per-field min/max ranges enforced at the control level so
      out-of-range values can't reach `Config::save`.
- **Live config hot-reload.** Save / Reset to Defaults now apply
  in-process — no restart, no overlay rebuild. The Settings dialog
  returns the new `Config` to the main loop, which calls a new
  `apply_config` helper that swaps the `HintEngine`, hint
  styles, scope mode, `WindowsBackend` cache + max-elements, and
  re-registers all hotkeys (dropping the old `Hotkeys` *before*
  registering the new ones so `RegisterHotKey` doesn't trip over
  its own per-process duplicate-chord rule). A "Settings applied"
  toast confirms the change. Hotkey-conflict toasts continue to fire
  if the user picks an already-claimed chord.
- **`config.toml` file watcher.** A new
  [`src/windows/config_watcher.rs`](src/windows/config_watcher.rs)
  module watches `%APPDATA%\keyhop\config.toml` via the cross-platform
  [`notify`](https://crates.io/crates/notify) crate. When the file
  changes — whether the Settings dialog wrote it, the user
  hand-edited it in their text editor, a cloud-sync agent dropped in
  a new copy, or a future CLI subcommand mutated it — the watcher
  posts a debounced (`150 ms`) `WM_USER_RELOAD_CONFIG` thread
  message via `PostThreadMessageW` and the main loop reloads &
  re-applies the config. The debounce window absorbs the
  multi-write atomic-save sequence editors like VS Code, Notepad++,
  and Vim use, so a single Save click triggers a single reload. The
  watcher targets the parent directory (not the file itself) so
  rename-over saves still fire the right events.

### Changed
- **`HintEngine::default()` now uses `ShortestFirst`.** The legacy
  fixed-length behaviour stays available via
  `HintEngine::with_strategy(alphabet, HintStrategy::FixedLength)`
  and via `[hints] strategy = "fixed_length"` in `config.toml`.
- **`Config` gained two new sections (`[scope]`, `[performance]`)
  and the existing `[hints]` section gained seven new fields**
  (`strategy`, `preset`, `include_numbers`, `include_extended`,
  `exclude_ambiguous`, `custom_additions`, `min_singles`). All fields
  are `serde default`, so v0.3.0 `config.toml` files continue to load
  unchanged and just inherit the new defaults.
- **`WindowsBackend::new()` is now a thin wrapper around
  `WindowsBackend::with_config(enable_caching, cache_ttl_ms,
  max_elements_global)`.** The binary uses `with_config` to thread
  the new `[performance]` and `[scope]` knobs into the backend at
  startup; library users can keep calling `new()` and get the
  defaults (caching on, 500ms TTL, 300 element global cap).
- **`handle_pick_element` in `main.rs` now calls
  `backend.enumerate_by_scope(runtime.scope_mode)`** instead of the
  hard-coded `enumerate_foreground`. The active-window scope is the
  default, so existing users see no behaviour change.

### Internals
- New modules: [`src/alphabet_presets.rs`](src/alphabet_presets.rs),
  [`src/cache.rs`](src/cache.rs),
  [`src/windows/config_watcher.rs`](src/windows/config_watcher.rs).
- New dependency: [`notify = "6"`](https://crates.io/crates/notify)
  for the cross-platform `config.toml` file watcher (only used on
  Windows in practice — the watcher itself is `cfg(windows)`-gated).
- New `windows` feature flag: `Win32_UI_Controls_Dialogs` for the
  `ChooseColorW` common dialog used by the Settings color-picker
  swatches. Two trackbar/updown control class names and one
  trackbar message id (`TBM_GETPOS`) are defined locally because
  `windows` 0.58 doesn't generate them.
- `main.rs` gained an `apply_config(new, &mut Runtime, &mut Backend,
  &mut Hotkeys, announce)` helper that centralises every step of a
  hot-reload (engine swap, style swap, backend reconfigure,
  hotkey re-registration, registry sync, optional toast). Both the
  Settings-dialog return path and the file-watcher path call it.
- 30+ new unit tests covering: variable-length label generation
  (single-char, mixed-length, prefix-collision invariants at scale,
  degenerate 1-char alphabet), the alphabet builder (every preset,
  every modifier flag combination, deduping, fallback to default on
  empty result), the cache (TTL expiry, lazy + active sweep,
  enable/disable, mock-clock), and the new monitor-aware layout
  helpers (`rect_inside_monitor`, `screen_to_client_rect`, the new
  `OutsideBottom` anchor).
- New `windows` API surface used:
  `Win32_Graphics_Gdi::{MonitorFromRect, GetMonitorInfoW, MONITORINFO,
  MONITOR_DEFAULTTONEAREST}` for the monitor-aware overlay layout, plus
  `Win32_Graphics_Gdi::{MonitorFromPoint}` and
  `Win32_UI_WindowsAndMessaging::GetCursorPos` for the
  active-monitor scope mode.

## [0.3.0] - 2026-04-22

Browser web-content release. The element picker now actually finds the
links and buttons *inside* a web page (previously you only got hints on
browser chrome), the window picker stops listing background UWP ghosts,
`keyhop` learned to shut itself down from another terminal, and we now
ship a real **MSI installer** so users no longer need the MSVC toolchain
to install.

### Added
- **MSI installer (`Keyhop-0.3.0-x86_64.msi`)** built via `cargo-wix` /
  WiX Toolset 3 and attached to every GitHub Release. The installer:
    - Targets per-machine install (`%ProgramFiles%\Keyhop\bin\`),
      registers a single Add/Remove Programs entry with proper
      Publisher / DisplayName / DisplayVersion / UninstallString, plus
      `ARPHELPLINK`, `ARPURLINFOABOUT`, `ARPURLUPDATEINFO`, and
      `ARPCONTACT` properties so the Apps & Features panel surfaces
      links back to GitHub.
    - Adds `keyhop.exe` to the system `PATH` so `keyhop --close`,
      `keyhop --clear-logs`, and other one-shot subcommands work from
      any terminal post-install.
    - Creates a Start Menu shortcut nested in the binary's MSI
      Component (advertised; ICE69-clean) so uninstall removes it
      atomically with the binary.
    - Supports unattended install via `msiexec /i ... /qn` (silent;
      satisfies Microsoft Store policy 10.2.9), and uses Windows
      Installer's RestartManager to cleanly close a running keyhop.exe
      before an upgrade overwrites it.
- **`.github/workflows/release.yml`** now builds both the portable
  `keyhop.exe` *and* the MSI on each published GitHub Release and
  attaches both as release assets via `softprops/action-gh-release@v3`.
- **`docs/CODE_SIGNING.md`** documents the path to signing the MSI and
  EXE via Microsoft Trusted Signing (deferred work — the v0.3.0 binaries
  ship unsigned, which still passes Store policy 10.2.9 with a
  recommendation but triggers SmartScreen on first install).
- **`.github/dependabot.yml`.** Weekly checks for both `cargo` and
  `github-actions` ecosystems so action drift doesn't recur and Cargo
  advisories surface as PRs.

### Changed
- **CI: bump `actions/checkout` from v4 to v6 and `softprops/action-gh-release` from v2 to v3.** Both v4 / v2 ran on Node.js 20, which GitHub is forcing to Node.js 24 in June 2026 and removing in September 2026. The new versions ship a Node.js 24 runtime and silence the deprecation warning. The bump also fixes the v0.3.0 release upload, which previously failed with `Resource not accessible by integration` because the workflow lacked `permissions: contents: write` — now declared explicitly at the top of `release.yml`.
- **Refresh `Cargo.lock`** (`winnow` 1.0.1 → 1.0.2).
- **README install section rewritten** to recommend the MSI as the
  primary path, position the portable EXE as the alternative, and frame
  `cargo install keyhop` as the developer path (with an explicit note
  about the `link.exe not found` failure mode that surfaced for users
  without Visual Studio Build Tools installed).

### Fixed
- **Hotkey parser: accept literal punctuation as the trailing key
  ([#3](https://github.com/rsaz/keyhop/issues/3)).** `Ctrl+\` (and the
  rest of the printable ANSI punctuation row — `,` `.` `/` `;` `'`
  `[` `]` `-` `=` `` ` ``) are now valid hotkey strings, alongside the
  spelled-out forms (`Backslash`, `Comma`, etc.) that already worked.
  This unblocks chords that sit on both halves of the keyboard, like
  `Ctrl+\`, which several users had asked for. The literal `+` cannot
  be used as the trailing key (it's the segment separator) — bind the
  physical `+/=` key via `Equal` or `=` instead.

### Security
- **Dismiss GHSA-wrw7-89jp-8q8g (`glib < 0.20.0` `VariantStrIter`
  unsoundness, RUSTSEC-2024-0429) as `tolerable_risk`.** The vulnerable
  `glib` 0.18 only enters the dependency graph through
  `tray-icon` → `libappindicator` → `gtk` → `glib` on non-Windows
  targets. `tray-icon` is gated by
  `[target.'cfg(windows)'.dependencies]` in `Cargo.toml` (with
  `default-features = false`, which already strips the `libxdo` X11
  backend), so on `x86_64-pc-windows-msvc` — the only target keyhop
  ships — `glib` is never compiled and the unsound `VariantStrIter`
  code path is unreachable. `tray-icon` 0.22 is the latest release and
  SemVer-locks `gtk-rs` 0.18; an upstream bump to gtk-rs 0.20 is
  required before the lockfile can drop the warning, hence the
  dismissal rather than a `cargo update` fix.

### Added (browser + lifecycle work that originally shipped this version)
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

### Changed (browser + lifecycle)
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

[Unreleased]: https://github.com/rsaz/keyhop/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/rsaz/keyhop/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/rsaz/keyhop/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/rsaz/keyhop/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/rsaz/keyhop/releases/tag/v0.1.0
