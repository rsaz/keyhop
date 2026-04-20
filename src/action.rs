//! Actions that can be performed against an [`crate::Element`].

/// Things keyhop can do to a target element. Backends choose the best native
/// mechanism (UI Automation invoke pattern, synthesized input, etc.) and may
/// return an error for actions a given element does not support.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action {
    /// Trigger the element's primary action (button press, link follow, ...).
    Invoke,
    /// Move keyboard focus to the element without invoking it.
    Focus,
    /// Synthesize a left mouse click at the element's center.
    Click,
    /// Type the given string into the element.
    Type(String),
    /// Scroll the element's container by the given delta in physical pixels.
    Scroll {
        /// Horizontal delta in pixels.
        dx: i32,
        /// Vertical delta in pixels.
        dy: i32,
    },
}
