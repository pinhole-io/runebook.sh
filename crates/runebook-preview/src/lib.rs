//! Preview text for a Runebook note.
//!
//! This crate rewrites HTML, expands emoji shortcodes, splits markdown rows,
//! and attaches link definitions. It does not open a vault and it does not paint a window.

mod anchors;
mod emoji;
mod fence;
mod html_style;
mod rows;

pub use anchors::{Prepared, link_urls, prepare};
pub use emoji::expand;
pub use fence::{closing_fence, leading_indent, opening_fence};
pub use html_style::{HTML_WIDGET, def_list, figure_html, style_html, visible_text};
pub use rows::{SourceRow, heading_needs_room, list_pad, split_rows};
