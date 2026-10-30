//! Shared sizes for the panel chrome.

use gpui_kit::{Pixels, Styled, px};

/// Height of the file tree, note, and Archivist headers.
///
/// Change this value to change every panel header.
pub(crate) const PANEL_HEADER_HEIGHT: Pixels = px(40.);

/// Smallest width for the files sidebar and the Archivist pane.
///
/// Four 24px icon buttons, three 1px dividers, six 8px gaps, and 16px of
/// header padding need 163px. The extra pixels keep those icons fully visible.
pub(crate) const SIDEBAR_MIN_WIDTH: Pixels = px(228.);

/// Smallest width for the note column.
pub(crate) const NOTE_MIN_WIDTH: Pixels = px(172.);

/// Height of one sidebar footer row.
pub(crate) const FOOTER_ROW_HEIGHT: Pixels = px(28.);

/// Space above and below the two footer rows.
///
/// This matches `py_1` when one rem is 16px.
pub(crate) const FOOTER_PAD_Y: Pixels = px(4.);

/// Border-box height of the two-button sidebar footer.
///
/// `border_t_1` is 1px. The Archivist strip uses this same height.
/// The field inside that strip stays one line tall.
pub(crate) fn sidebar_footer_height() -> Pixels {
    px(1.) + FOOTER_PAD_Y + FOOTER_ROW_HEIGHT + FOOTER_ROW_HEIGHT + FOOTER_PAD_Y
}

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
