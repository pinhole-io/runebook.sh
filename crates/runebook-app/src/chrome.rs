//! Shared height of the panel headers.

use gpui_kit::{Pixels, Styled, px};

/// Height of the file tree, note, and Archivist headers.
///
/// Change this value to change every panel header.
pub(crate) const PANEL_HEADER_HEIGHT: Pixels = px(40.);

/// Smallest files-sidebar width that keeps the New note, New folder, and Search icons on screen.
///
/// Three small icon buttons, two dividers, the gaps, and the header padding.
pub(crate) const SIDEBAR_MIN_WIDTH: Pixels = px(128.);

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
