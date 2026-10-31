//! File tree, note, and Archivist columns in one horizontal split.
//!
//! The saved widths live in the user config. This module accepts or rejects
//! that string. It does not log the text.

use gpui_kit::component::{ResizableState, h_resizable, resizable_panel};
use gpui_kit::prelude::*;
use gpui_kit::{App, Context, Entity, IntoElement, Pixels, Render, WeakEntity, Window, div, px};

use crate::Root;
use crate::chrome::{NOTE_MIN_WIDTH, SIDEBAR_MIN_WIDTH};

/// Layout schema stored in `dock_layout`.
pub(crate) const LAYOUT_VERSION: usize = 1;

/// Largest accepted layout string, in bytes.
pub(crate) const LAYOUT_LIMIT: usize = 64 * 1024;

const DEFAULT_FILES: Pixels = px(260.);
const DEFAULT_ARCHIVIST: Pixels = px(320.);

/// The three columns. The side columns use [`SIDEBAR_MIN_WIDTH`] as their drag floor.
pub(crate) struct Columns {
    root: WeakEntity<Root>,
    sizes: Entity<ResizableState>,
    files: Pixels,
    archivist: Pixels,
}

impl Columns {
    fn new(
        root: WeakEntity<Root>,
        files: Pixels,
        archivist: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        watch(root.clone(), window, cx);
        Self {
            root,
            sizes: cx.new(|_| ResizableState::default()),
            files,
            archivist,
        }
    }
}

impl Render for Columns {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (files, note, archivist) = self.bodies(cx);
        let root = self.root.clone();
        div()
            .size_full()
            .min_h(px(0.))
            .min_w(px(0.))
            .overflow_hidden()
            .child(
                h_resizable("runebook-columns")
                    .with_state(&self.sizes)
                    .on_resize(move |state, _, cx| {
                        let sizes = state.read(cx).sizes();
                        let Some(&files) = sizes.first() else {
                            return;
                        };
                        let Some(&archivist) = sizes.get(2) else {
                            return;
                        };
                        root.update(cx, |this, cx| {
                            this.schedule_dock_save(
                                files.max(SIDEBAR_MIN_WIDTH),
                                archivist.max(SIDEBAR_MIN_WIDTH),
                                cx,
                            );
                        })
                        .ok();
                    })
                    .child(
                        resizable_panel()
                            .size(self.files)
                            .size_range(SIDEBAR_MIN_WIDTH..Pixels::MAX)
                            .fixed()
                            .child(files),
                    )
                    .child(
                        resizable_panel()
                            .size_range(NOTE_MIN_WIDTH..Pixels::MAX)
                            .child(note),
                    )
                    .child(
                        resizable_panel()
                            .size(self.archivist)
                            .size_range(SIDEBAR_MIN_WIDTH..Pixels::MAX)
                            .fixed()
                            .child(archivist),
                    ),
            )
    }
}

impl Columns {
    fn bodies(
        &self,
        cx: &App,
    ) -> (
        gpui_kit::AnyElement,
        gpui_kit::AnyElement,
        gpui_kit::AnyElement,
    ) {
        let Some(root) = self.root.upgrade() else {
            return empty_bodies();
        };
        let Some(pane) = root.read(cx).pane.as_ref() else {
            return empty_bodies();
        };
        let view = self.root.clone();
        (
            crate::pane::files(pane, view.clone(), cx).into_any_element(),
            crate::pane::note(pane, view.clone(), cx),
            crate::archivist::column(
                pane.archivist(),
                pane.archivist_ready(),
                pane.archivist_key_message(),
                view,
                cx,
            )
            .into_any_element(),
        )
    }
}

fn empty_bodies() -> (
    gpui_kit::AnyElement,
    gpui_kit::AnyElement,
    gpui_kit::AnyElement,
) {
    (
        div().size_full().into_any_element(),
        div().size_full().into_any_element(),
        div().size_full().into_any_element(),
    )
}

/// Build the columns and restore saved side widths when the string is accepted.
pub(crate) fn install(
    root: WeakEntity<Root>,
    saved: Option<&str>,
    window: &mut Window,
    cx: &mut Context<Root>,
) -> Entity<Columns> {
    let (files, archivist) = column_widths(saved);
    cx.new(|cx| Columns::new(root, files, archivist, window, cx))
}

/// Files width and Archivist width. A bad string is the default split.
///
/// A current string stores those two widths. An older dock string stores a
/// center split. Both are accepted. A width under the sidebar floor is raised.
pub(crate) fn column_widths(raw: Option<&str>) -> (Pixels, Pixels) {
    let defaults = (DEFAULT_FILES, DEFAULT_ARCHIVIST);
    let Some(raw) = raw.filter(|raw| !raw.is_empty() && raw.len() <= LAYOUT_LIMIT) else {
        return defaults;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return defaults;
    };
    if value.get("version").and_then(|version| version.as_u64()) != Some(LAYOUT_VERSION as u64) {
        return defaults;
    }
    if let (Some(files), Some(archivist)) = (
        value.get("files").and_then(|width| width.as_f64()),
        value.get("archivist").and_then(|width| width.as_f64()),
    ) {
        return (
            floor_width(files, defaults.0),
            floor_width(archivist, defaults.1),
        );
    }
    widths_from_dock(&value).unwrap_or(defaults)
}

fn widths_from_dock(value: &serde_json::Value) -> Option<(Pixels, Pixels)> {
    let center = value.get("center")?;
    let stack = center.get("info")?.get("stack")?;
    if stack.get("axis").and_then(|axis| axis.as_u64()) != Some(0) {
        return None;
    }
    let sizes = stack.get("sizes")?.as_array()?;
    let children = center.get("children")?.as_array()?;
    if sizes.len() != children.len() {
        return None;
    }
    let mut files = None;
    let mut archivist = None;
    for (child, size) in children.iter().zip(sizes) {
        let width = size.as_f64()?;
        match named_panel(child)? {
            "files" => files = Some(width),
            "archivist" => archivist = Some(width),
            _ => {}
        }
    }
    Some((
        floor_width(files?, DEFAULT_FILES),
        floor_width(archivist?, DEFAULT_ARCHIVIST),
    ))
}

fn named_panel(node: &serde_json::Value) -> Option<&str> {
    if let Some(name) = node.get("panel_name").and_then(|name| name.as_str())
        && matches!(name, "files" | "note" | "archivist")
    {
        return Some(name);
    }
    for child in node.get("children")?.as_array()? {
        if let Some(name) = named_panel(child) {
            return Some(name);
        }
    }
    None
}

fn floor_width(value: f64, fallback: Pixels) -> Pixels {
    if !value.is_finite() || value <= 0. {
        return fallback;
    }
    px(value as f32).max(SIDEBAR_MIN_WIDTH)
}

fn watch<T: 'static>(root: WeakEntity<Root>, window: &mut Window, cx: &mut Context<T>) {
    if let Some(live) = root.upgrade() {
        cx.observe(&live, |_, _, cx| cx.notify()).detach();
        return;
    }
    cx.defer_in(window, move |_, _, cx| {
        if let Some(live) = root.upgrade() {
            cx.observe(&live, |_, _, cx| cx.notify()).detach();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{LAYOUT_LIMIT, column_widths};
    use crate::chrome::SIDEBAR_MIN_WIDTH;
    use gpui_kit::px;

    #[test]
    fn column_widths_rejects_empty_oversized_and_broken_text() {
        assert_eq!(column_widths(None), (px(260.), px(320.)));
        assert_eq!(column_widths(Some("")), (px(260.), px(320.)));
        assert_eq!(column_widths(Some("not-json")), (px(260.), px(320.)));
        let huge = "x".repeat(LAYOUT_LIMIT + 1);
        assert_eq!(column_widths(Some(&huge)), (px(260.), px(320.)));
    }

    #[test]
    fn column_widths_rejects_a_different_version() {
        let raw = r#"{"version":2,"files":200.0,"archivist":220.0}"#;
        assert_eq!(column_widths(Some(raw)), (px(260.), px(320.)));
    }

    #[test]
    fn column_widths_reads_the_current_string() {
        let raw = r#"{"version":1,"files":300.0,"archivist":340.0}"#;
        assert_eq!(column_widths(Some(raw)), (px(300.), px(340.)));
    }

    #[test]
    fn column_widths_raises_a_width_under_the_floor() {
        let raw = r#"{"version":1,"files":40.0,"archivist":10.0}"#;
        assert_eq!(
            column_widths(Some(raw)),
            (SIDEBAR_MIN_WIDTH, SIDEBAR_MIN_WIDTH)
        );
    }

    #[test]
    fn column_widths_reads_an_older_dock_split() {
        let raw = r#"{"version":1,"center":{"panel_name":"StackPanel","children":[{"panel_name":"TabPanel","children":[{"panel_name":"files","children":[],"info":{"panel":null}}],"info":{"tabs":{"active_index":0}}},{"panel_name":"TabPanel","children":[{"panel_name":"note","children":[],"info":{"panel":null}}],"info":{"tabs":{"active_index":0}}},{"panel_name":"TabPanel","children":[{"panel_name":"archivist","children":[],"info":{"panel":null}}],"info":{"tabs":{"active_index":0}}}],"info":{"stack":{"sizes":[256.0,423.0,256.0],"axis":0}}}}"#;
        assert_eq!(column_widths(Some(raw)), (px(256.), px(256.)));
    }

    #[test]
    fn column_widths_reads_side_columns_that_are_not_on_the_edges() {
        let raw = r#"{"version":1,"center":{"children":[{"panel_name":"archivist","children":[]},{"panel_name":"files","children":[]}],"info":{"stack":{"sizes":[340.0,300.0],"axis":0}}}}"#;
        assert_eq!(column_widths(Some(raw)), (px(300.), px(340.)));
    }

    #[test]
    fn column_widths_ignores_a_vertical_dock_split() {
        let raw = r#"{"version":1,"center":{"children":[{"panel_name":"files","children":[]},{"panel_name":"archivist","children":[]}],"info":{"stack":{"sizes":[200.0,200.0],"axis":1}}}}"#;
        assert_eq!(column_widths(Some(raw)), (px(260.), px(320.)));
    }
}
