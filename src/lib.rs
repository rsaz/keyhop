//! Public API for [keyhop](https://github.com/rsaz/keyhop).
//!
//! `keyhop` is a system-wide keyboard navigation overlay. Press a leader
//! chord, see hint labels on every interactable control on screen, type the
//! hint, done.
//!
//! The crate ships both:
//!
//! - **A binary** (`keyhop`) — an end-user tool that registers a global
//!   hotkey, walks the foreground window's accessibility tree, and renders
//!   the hint overlay.
//! - **A library** — the platform-agnostic core types and traits
//!   ([`Action`], [`Backend`], [`Element`], [`HintEngine`]), plus a Windows
//!   backend in [`windows`] (only compiled on `target_os = "windows"`).
//!
//! Other backends (Linux X11/Wayland, macOS) will land as future modules
//! gated behind their own `cfg`s.

#![warn(missing_docs)]

pub mod action;
pub mod alphabet_presets;
pub mod backend;
pub mod cache;
pub mod config;
pub mod hint;
pub mod model;

#[cfg(windows)]
pub mod windows;

pub use action::Action;
pub use backend::Backend;
pub use config::Config;
pub use hint::{HintEngine, HintStrategy, DEFAULT_ALPHABET};
pub use model::{Bounds, Element, ElementId, Role};
