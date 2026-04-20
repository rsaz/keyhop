//! Windows backend for keyhop.
//!
//! Implements [`crate::Backend`] against the Windows UI Automation API via
//! the `uiautomation` crate, plus the global hotkey ([`hotkey`]) and
//! transparent overlay ([`overlay`]) building blocks the binary needs.
//!
//! The backend walks the control-view subtree of the foreground window and
//! collects on-screen, interactable elements (buttons, links, inputs, menu
//! items, ...) into the platform-agnostic [`crate::Element`] model.

pub mod hotkey;
pub mod overlay;
pub mod single_instance;
pub mod tray;
pub mod window_picker;

use std::collections::HashMap;

use anyhow::Context;

use uiautomation::{
    patterns::{
        UIExpandCollapsePattern, UIInvokePattern, UILegacyIAccessiblePattern,
        UISelectionItemPattern, UITogglePattern,
    },
    types::{ControlType, Handle},
    UIAutomation, UIElement, UITreeWalker,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::{Action, Backend, Bounds, Element, ElementId, Role};

/// Maximum tree depth we descend into. Prevents pathological walks in apps
/// that expose extremely deep accessibility trees.
const MAX_TREE_DEPTH: usize = 12;

/// Hard cap on collected elements per enumeration. A typical foreground
/// window has well under this many interactable items; the cap is mainly a
/// safety valve against runaway browser DOMs exposed via UIA.
const MAX_ELEMENTS: usize = 500;

/// UI Automation-backed implementation of [`Backend`] for Windows.
pub struct WindowsBackend {
    automation: UIAutomation,
    elements: HashMap<ElementId, UIElement>,
    next_id: u64,
}

impl WindowsBackend {
    /// Construct a new backend, initializing the UI Automation client.
    pub fn new() -> anyhow::Result<Self> {
        // PerMonitorV2 ensures `GetBoundingRectangle` returns physical
        // pixels matching the overlay's coordinate space on high-DPI
        // displays. Best-effort: ignore "already set" / older-OS errors.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        let automation = UIAutomation::new().context("initializing UI Automation client")?;
        Ok(Self {
            automation,
            elements: HashMap::new(),
            next_id: 0,
        })
    }
}

impl Backend for WindowsBackend {
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

    fn perform(&mut self, id: ElementId, action: Action) -> anyhow::Result<()> {
        let el = self
            .elements
            .get(&id)
            .with_context(|| format!("unknown element id {id:?}"))?;

        match action {
            // Invoke goes through the UIA pattern cascade — no cursor warp,
            // works on virtualized / off-screen items, respects the app's
            // accessibility hooks. Mouse-click is the explicit last resort
            // for legacy controls that don't expose any pattern.
            Action::Invoke => {
                let path = invoke_smart(el).context("invoking element failed")?;
                if path == InvokePath::MouseFallback {
                    tracing::warn!(?id, "no UIA pattern matched; used mouse-click fallback");
                } else {
                    tracing::debug!(?id, ?path, "element invoked via UIA pattern");
                }
            }
            // Click is the explicit "synthesize a real mouse click" action.
            // Different semantics from Invoke — caller is opting in to
            // cursor motion, useful for testing or for apps that only
            // respond to genuine input.
            Action::Click => {
                el.click().context("element.click() failed")?;
            }
            Action::Focus => {
                el.set_focus().context("element.set_focus() failed")?;
            }
            Action::Type(s) => {
                el.send_text(&s, 0).context("element.send_text() failed")?;
            }
            Action::Scroll { .. } => anyhow::bail!("scroll action not yet implemented"),
        }
        Ok(())
    }
}

/// Which path through [`invoke_smart`] succeeded. Surfaced via tracing so we
/// can spot apps that consistently fall through to the mouse fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InvokePath {
    Invoke,
    Toggle,
    SelectionItem,
    ExpandCollapse,
    LegacyDefaultAction,
    MouseFallback,
}

/// Try every UIA pattern that could plausibly "invoke" `el`, in order of
/// specificity, and return which one worked. Falls back to a synthesized
/// mouse click only when every pattern path has been exhausted.
///
/// Pattern cascade rationale:
///
/// 1. [`UIInvokePattern`] — the canonical "do the primary action" interface.
///    Buttons, links, menu items, split buttons all expose it.
/// 2. [`UITogglePattern`] — checkboxes, toggle buttons.
/// 3. [`UISelectionItemPattern`] — tabs, radios, list/tree items.
/// 4. [`UIExpandCollapsePattern`] — combo boxes, tree expanders.
/// 5. [`UILegacyIAccessiblePattern::do_default_action`] — broad
///    MSAA-compatible fallback that often works when the modern patterns
///    aren't exposed.
/// 6. [`UIElement::click`] — synthesized mouse click. Last resort: warps
///    the cursor and is the only thing that works for some pre-UIA Win32
///    controls. We prefer never to hit this path.
fn invoke_smart(el: &UIElement) -> anyhow::Result<InvokePath> {
    if let Ok(p) = el.get_pattern::<UIInvokePattern>() {
        if p.invoke().is_ok() {
            return Ok(InvokePath::Invoke);
        }
    }
    if let Ok(p) = el.get_pattern::<UITogglePattern>() {
        if p.toggle().is_ok() {
            return Ok(InvokePath::Toggle);
        }
    }
    if let Ok(p) = el.get_pattern::<UISelectionItemPattern>() {
        if p.select().is_ok() {
            return Ok(InvokePath::SelectionItem);
        }
    }
    if let Ok(p) = el.get_pattern::<UIExpandCollapsePattern>() {
        // Expand-only: for an already-expanded element this is a no-op on
        // most controls. Collapsing on Invoke would surprise the user (you
        // pressed a hint to *do* the thing, not to undo it).
        if p.expand().is_ok() {
            return Ok(InvokePath::ExpandCollapse);
        }
    }
    if let Ok(p) = el.get_pattern::<UILegacyIAccessiblePattern>() {
        if p.do_default_action().is_ok() {
            return Ok(InvokePath::LegacyDefaultAction);
        }
    }
    el.click().context("element.click() failed")?;
    Ok(InvokePath::MouseFallback)
}

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
