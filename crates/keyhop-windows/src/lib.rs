//! Windows backend for [keyhop](https://github.com/rsaz/keyhop).
//!
//! Implements [`keyhop_core::Backend`] against the Windows UI Automation API
//! via the `uiautomation` crate.
//!
//! The backend walks the control-view subtree of the foreground window and
//! collects on-screen, interactable elements (buttons, links, inputs, menu
//! items, ...) into the platform-agnostic [`keyhop_core::Element`] model.

#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::collections::HashMap;

use anyhow::Context;
use keyhop_core::{Action, Backend, Bounds, Element, ElementId, Role};

#[cfg(windows)]
use uiautomation::{
    types::{ControlType, Handle},
    UIAutomation, UIElement, UITreeWalker,
};

#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// Maximum tree depth we descend into. Prevents pathological walks in apps
/// that expose extremely deep accessibility trees.
const MAX_TREE_DEPTH: usize = 12;

/// Hard cap on collected elements per enumeration. A typical foreground
/// window has well under this many interactable items; the cap is mainly a
/// safety valve against runaway browser DOMs exposed via UIA.
const MAX_ELEMENTS: usize = 500;

/// UI Automation-backed implementation of [`Backend`] for Windows.
pub struct WindowsBackend {
    #[cfg(windows)]
    automation: UIAutomation,
    #[cfg(windows)]
    elements: HashMap<ElementId, UIElement>,
    next_id: u64,
}

impl WindowsBackend {
    /// Construct a new backend, initializing the UI Automation client.
    pub fn new() -> anyhow::Result<Self> {
        #[cfg(windows)]
        {
            let automation =
                UIAutomation::new().context("initializing UI Automation client")?;
            Ok(Self {
                automation,
                elements: HashMap::new(),
                next_id: 0,
            })
        }
        #[cfg(not(windows))]
        {
            Ok(Self { next_id: 0 })
        }
    }
}

impl Backend for WindowsBackend {
    #[cfg(windows)]
    fn enumerate_foreground(&mut self) -> anyhow::Result<Vec<Element>> {
        self.elements.clear();
        self.next_id = 0;

        // SAFETY: `GetForegroundWindow` has no preconditions and may return a
        // null handle, which we check below.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            tracing::warn!("no foreground window");
            return Ok(Vec::new());
        }

        let handle = Handle::from(hwnd.0 as isize);
        let root = self
            .automation
            .element_from_handle(handle)
            .context("element_from_handle for foreground window failed")?;
        let walker = self
            .automation
            .get_control_view_walker()
            .context("creating control-view tree walker failed")?;

        let mut out = Vec::with_capacity(64);
        self.walk(&walker, &root, 0, &mut out);
        tracing::debug!(collected = out.len(), "enumerate_foreground done");
        Ok(out)
    }

    #[cfg(not(windows))]
    fn enumerate_foreground(&mut self) -> anyhow::Result<Vec<Element>> {
        anyhow::bail!("WindowsBackend only works on Windows");
    }

    #[cfg(windows)]
    fn perform(&mut self, id: ElementId, action: Action) -> anyhow::Result<()> {
        let el = self
            .elements
            .get(&id)
            .with_context(|| format!("unknown element id {id:?}"))?;

        match action {
            // For now Invoke and Click both go through the synthesized mouse
            // click path. A future revision should prefer the UIA Invoke
            // pattern when available so we don't depend on accurate screen
            // coordinates (matters for virtualized lists, off-screen
            // scrolling, etc.).
            Action::Invoke | Action::Click => {
                el.click().context("element.click() failed")?;
            }
            Action::Focus => {
                el.set_focus().context("element.set_focus() failed")?;
            }
            Action::Type(s) => {
                el.send_text(&s, 0).context("element.send_text() failed")?;
            }
            Action::Scroll { .. } => anyhow::bail!("scroll action not yet implemented"),
            // `Action` is `#[non_exhaustive]`; reject anything we don't know
            // about explicitly so future variants are an obvious compile-time
            // signal rather than a silent no-op.
            _ => anyhow::bail!("unsupported action variant"),
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn perform(&mut self, _id: ElementId, _action: Action) -> anyhow::Result<()> {
        anyhow::bail!("WindowsBackend only works on Windows");
    }
}

#[cfg(windows)]
impl WindowsBackend {
    fn walk(
        &mut self,
        walker: &UITreeWalker,
        el: &UIElement,
        depth: usize,
        out: &mut Vec<Element>,
    ) {
        if out.len() >= MAX_ELEMENTS {
            return;
        }
        if let Some(record) = self.try_record(el) {
            out.push(record);
        }
        if depth >= MAX_TREE_DEPTH {
            return;
        }
        if let Ok(first) = walker.get_first_child(el) {
            let mut current = first;
            loop {
                self.walk(walker, &current, depth + 1, out);
                match walker.get_next_sibling(&current) {
                    Ok(next) => current = next,
                    Err(_) => break,
                }
            }
        }
    }

    fn try_record(&mut self, el: &UIElement) -> Option<Element> {
        if el.is_offscreen().unwrap_or(true) {
            return None;
        }
        let bounds = el.get_bounding_rectangle().ok()?;
        if bounds.get_width() <= 0 || bounds.get_height() <= 0 {
            return None;
        }
        let role = map_role(el.get_control_type().ok()?)?;
        let name = el.get_name().ok().filter(|s| !s.is_empty());

        let id = ElementId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.elements.insert(id, el.clone());

        Some(Element {
            id,
            role,
            name,
            bounds: Bounds {
                x: bounds.get_left(),
                y: bounds.get_top(),
                width: bounds.get_width(),
                height: bounds.get_height(),
            },
        })
    }
}

#[cfg(windows)]
fn map_role(ct: ControlType) -> Option<Role> {
    use ControlType::*;
    Some(match ct {
        Button | SplitButton => Role::Button,
        Hyperlink => Role::Link,
        Edit => Role::TextInput,
        MenuItem => Role::MenuItem,
        TabItem => Role::Tab,
        CheckBox => Role::Checkbox,
        RadioButton => Role::Radio,
        ComboBox => Role::ComboBox,
        ListItem => Role::ListItem,
        TreeItem => Role::TreeItem,
        // Structural / non-interactable controls — skip them. We may revisit
        // some of these later (e.g. expose ScrollBar for keyboard scrolling).
        _ => return None,
    })
}
