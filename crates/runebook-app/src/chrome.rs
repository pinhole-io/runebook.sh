//! Shared height of the panel headers.

use gpui_kit::{Pixels, Styled, px};

/// Height of the file tree, note, and Archivist headers.
///
/// Change this value to change every panel header.
pub(crate) const PANEL_HEADER_HEIGHT: Pixels = px(40.);

/// Apply the shared panel header size to `header`.
pub(crate) fn panel_header<E: Styled>(header: E, border: gpui_kit::Hsla) -> E {
    header
        .w_full()
        .h(PANEL_HEADER_HEIGHT)
        .flex_shrink_0()
        .flex()
        .items_center()
        .overflow_hidden()
        .border_b_1()
        .border_color(border)
}
