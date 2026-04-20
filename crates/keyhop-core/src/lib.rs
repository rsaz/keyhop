//! Platform-agnostic core for [keyhop](https://github.com/rsaz/keyhop).
//!
//! This crate defines the shared types and traits used by every platform
//! backend: the element model, action vocabulary, the [`Backend`] trait, and
//! the [`HintEngine`] that produces short keyboard labels for on-screen
//! targets.
//!
//! No OS calls live here. Backends (e.g. `keyhop-windows`) implement
//! [`Backend`] against this surface.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod action;
pub mod backend;
pub mod hint;
pub mod model;

pub use action::Action;
pub use backend::Backend;
pub use hint::HintEngine;
pub use model::{Bounds, Element, ElementId, Role};
