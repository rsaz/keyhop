//! Windows backend for [keyhop](https://github.com/rsaz/keyhop).
//!
//! Currently a scaffold: the [`WindowsBackend`] type implements
//! [`keyhop_core::Backend`] but enumeration and action dispatch are not yet
//! wired to UI Automation. See `examples/enumerate_foreground.rs` for a
//! minimal Win32 demo that prints the foreground window title.

#![cfg_attr(not(windows), allow(dead_code))]

use keyhop_core::{Action, Backend, Element, ElementId};

/// UI Automation-backed implementation of [`Backend`] for Windows.
#[derive(Debug, Default)]
pub struct WindowsBackend {
    _private: (),
}

impl WindowsBackend {
    /// Construct a new backend. On non-Windows targets this still compiles
    /// but the backend will return errors from every method.
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self { _private: () })
    }
}

impl Backend for WindowsBackend {
    fn enumerate_foreground(&mut self) -> anyhow::Result<Vec<Element>> {
        // TODO: walk the UI Automation tree of the foreground window and
        // collect elements that support the Invoke / Toggle / SelectionItem
        // patterns.
        tracing::warn!("enumerate_foreground: not yet implemented");
        Ok(Vec::new())
    }

    fn perform(&mut self, _id: ElementId, _action: Action) -> anyhow::Result<()> {
        // TODO: resolve the element handle stored against `id` and dispatch
        // the appropriate UI Automation pattern.
        anyhow::bail!("perform: not yet implemented");
    }
}
