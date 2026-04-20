//! Normalized representation of on-screen interactable elements.

/// Stable, opaque identifier for an [`Element`] within a single enumeration
/// pass. Backends are free to choose any encoding; consumers must not assume
/// any structure.
///
/// IDs are not guaranteed to remain valid across enumerations — call
/// [`crate::Backend::enumerate_foreground`] again to refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementId(pub u64);

/// Pixel-space rectangle in physical (DPI-aware) coordinates relative to the
/// virtual desktop origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// Left edge, in physical pixels.
    pub x: i32,
    /// Top edge, in physical pixels.
    pub y: i32,
    /// Width in physical pixels.
    pub width: i32,
    /// Height in physical pixels.
    pub height: i32,
}

impl Bounds {
    /// Returns the geometric center of the rectangle.
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }

    /// Returns true when the rectangle has positive area.
    pub fn is_visible(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// High-level semantic role of an element. Mirrors the most useful subset of
/// the UI Automation / AT-SPI / ARIA role taxonomies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Role {
    /// A clickable button.
    Button,
    /// A hyperlink.
    Link,
    /// A single-line or multi-line text input.
    TextInput,
    /// An item inside a menu.
    MenuItem,
    /// A tab in a tab strip.
    Tab,
    /// A checkbox.
    Checkbox,
    /// A radio option.
    Radio,
    /// A combo / drop-down.
    ComboBox,
    /// A list item.
    ListItem,
    /// A tree item.
    TreeItem,
    /// Anything else that exposes an invoke pattern.
    Other,
}

/// A single interactable thing discovered on screen.
#[derive(Debug, Clone)]
pub struct Element {
    /// Backend-assigned identifier.
    pub id: ElementId,
    /// Semantic role.
    pub role: Role,
    /// Accessible name, when available.
    pub name: Option<String>,
    /// On-screen bounding box.
    pub bounds: Bounds,
}
