//! The contract every platform backend implements.

use crate::action::Action;
use crate::model::{Element, ElementId};

/// A platform-specific provider of interactable elements and action dispatch.
///
/// Implementations live in sibling modules such as [`crate::windows`]. The
/// trait intentionally stays small so it can be kept stable as the
/// higher-level engine evolves.
pub trait Backend {
    /// Enumerate interactable elements visible in the currently focused
    /// window or top-level surface.
    ///
    /// The returned [`ElementId`]s are only guaranteed to be valid until the
    /// next call to `enumerate_foreground` on the same backend.
    fn enumerate_foreground(&mut self) -> anyhow::Result<Vec<Element>>;

    /// Perform `action` against the element previously returned with `id`.
    fn perform(&mut self, id: ElementId, action: Action) -> anyhow::Result<()>;
}
