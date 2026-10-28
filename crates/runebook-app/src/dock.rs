//! File tree, note, and Archivist panels in one dock.
//!
//! The layout JSON lives in the user config. This module accepts or rejects
//! that string. It does not log the text.

use gpui_kit::component::dock::{
    BasePanel, DockArea, DockAreaState, DockEvent, DockLayout, DockPlacement, DockSkin, NodeId,
    PaneNode, PaneRef, Panel, PanelEvent, PanelId, panel_handle, register_panel,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Axis, Context, Entity, EventEmitter, FocusHandle, Focusable, Global, IntoElement, Pixels,
    Render, SharedString, WeakEntity, Window, div, px,
};

use crate::Root;

/// Layout schema stored in `dock_layout`.
pub(crate) const LAYOUT_VERSION: usize = 1;

/// Largest accepted layout string, in bytes.
pub(crate) const LAYOUT_LIMIT: usize = 64 * 1024;

const FILES: &str = "files";
const NOTE: &str = "note";
const ARCHIVIST: &str = "archivist";

struct DockHost {
    root: WeakEntity<Root>,
}

impl Global for DockHost {}

enum Slot {
    Files,
    Note,
    Archivist,
}

struct DockPanel {
    slot: Slot,
    root: WeakEntity<Root>,
    focus: FocusHandle,
}

impl DockPanel {
    fn new(
        slot: Slot,
        root: WeakEntity<Root>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        watch(root.clone(), window, cx);
        Self {
            slot,
            root,
            focus: cx.focus_handle(),
        }
    }

    fn name(&self) -> &'static str {
        match self.slot {
            Slot::Files => FILES,
            Slot::Note => NOTE,
            Slot::Archivist => ARCHIVIST,
        }
    }

    fn label(&self) -> &'static str {
        match self.slot {
            Slot::Files => crate::locale::text("dock.files"),
            Slot::Note => crate::locale::text("dock.note"),
            Slot::Archivist => crate::locale::text("dock.archivist"),
        }
    }
}

impl BasePanel for DockPanel {
    fn panel_name(&self) -> &'static str {
        self.name()
    }

    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for DockPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.label())
    }

    fn title_bar(&self, _: &App) -> bool {
        false
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl EventEmitter<PanelEvent> for DockPanel {}

impl Focusable for DockPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DockPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.queue_clips(cx);
        let body = self.body(cx);
        let min_width = match self.slot {
            Slot::Files | Slot::Archivist => crate::chrome::SIDEBAR_MIN_WIDTH,
            Slot::Note => px(0.),
        };
        div()
            .id(self.name())
            .track_focus(&self.focus)
            .size_full()
            .min_w(min_width)
            .min_h(px(0.))
            .child(body)
    }
}

impl DockPanel {
    fn queue_clips(&self, cx: &mut Context<Self>) {
        if !matches!(self.slot, Slot::Note) {
            return;
        }
        let Some(root) = self.root.upgrade() else {
            return;
        };
        let ready = {
            let data = root.read(cx);
            data.pane.is_some()
        };
        if !ready {
            return;
        }
        let view = self.root.clone();
        let root = self.root.clone();
        cx.defer(move |cx| {
            root.update(cx, |root, cx| {
                let Some(pane) = root.pane.as_ref() else {
                    return;
                };
                crate::pane::queue_clips(pane, view, cx);
            })
            .ok();
        });
    }

    fn body(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(root) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };
        let root_ref = root.read(cx);
        let Some(pane) = root_ref.pane.as_ref() else {
            return div().size_full().into_any_element();
        };
        let view = self.root.clone();
        match self.slot {
            Slot::Files => crate::pane::files(pane, view, cx).into_any_element(),
            Slot::Note => crate::pane::note(pane, view, cx).into_any_element(),
            Slot::Archivist => crate::archivist::column(
                pane.archivist(),
                pane.archivist_ready(),
                pane.archivist_key_message(),
                view,
                cx,
            )
            .into_any_element(),
        }
    }
}

/// Parse a saved layout. A bad string is the default split.
pub(crate) fn accept_layout(raw: Option<&str>) -> Option<DockAreaState> {
    let raw = raw?;
    if raw.is_empty() || raw.len() > LAYOUT_LIMIT {
        return None;
    }
    let state = parse_state(raw)?;
    if state.version != Some(LAYOUT_VERSION) {
        return None;
    }
    Some(state)
}

fn parse_state(raw: &str) -> Option<DockAreaState> {
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let obj = value.as_object_mut()?;
    for key in ["left_dock", "right_dock", "bottom_dock"] {
        obj.entry(key).or_insert(serde_json::Value::Null);
    }
    serde_json::from_value(value).ok()
}

/// Build the dock and restore a saved layout when the string is accepted.
pub(crate) fn install(
    root: WeakEntity<Root>,
    saved: Option<&str>,
    window: &mut Window,
    cx: &mut Context<Root>,
) -> Entity<DockArea> {
    cx.set_global(DockHost { root });
    register_panels(cx);
    let (area, _) = DockSkin::dock_area("runebook", Some(LAYOUT_VERSION), window, cx);
    let loaded = accept_layout(saved)
        .is_some_and(|state| area.update(cx, |area, cx| area.load(state, window, cx).is_ok()));
    if !loaded {
        area.update(cx, |area, cx| {
            area.set_center(default_layout(window, cx), window, cx);
        });
    }
    // The dock floor is 100px. That can hide the files header icons and
    // squeeze the Archivist pane. DockLayout cannot raise one slot's floor,
    // so a finished drag is widened here.
    cx.subscribe_in(&area, window, |_this, area, event, window, cx| {
        if !matches!(event, DockEvent::LayoutChanged) {
            return;
        }
        let area = area.clone();
        cx.defer_in(window, move |this, window, cx| {
            clamp_side_panes(&area, window, cx);
            this.schedule_dock_save(&area, cx);
        });
    })
    .detach();
    let area_for_save = area.clone();
    cx.defer_in(window, move |this, window, cx| {
        clamp_side_panes(&area_for_save, window, cx);
        this.schedule_dock_save(&area_for_save, cx);
    });
    area
}

fn clamp_side_panes(area: &Entity<DockArea>, window: &mut Window, cx: &mut App) {
    let plans = {
        let dock = area.read(cx);
        let Some(tree) = dock.layout(DockPlacement::Center) else {
            return;
        };
        let mut groups: Vec<(NodeId, Vec<usize>, Vec<Option<Pixels>>)> = Vec::new();
        for name in [FILES, ARCHIVIST] {
            let Some(panel) = panel_named(&dock, name, cx) else {
                continue;
            };
            let Some((node, index, sizes)) = pane_column(tree.root(), panel) else {
                continue;
            };
            if let Some(group) = groups.iter_mut().find(|group| group.0 == node) {
                group.1.push(index);
            } else {
                groups.push((node, vec![index], sizes));
            }
        }
        groups
            .into_iter()
            .filter_map(|(node, indexes, sizes)| {
                widen_columns(&sizes, &indexes).map(|sizes| (node, sizes))
            })
            .collect::<Vec<_>>()
    };
    if plans.is_empty() {
        return;
    }
    area.update(cx, |area, cx| {
        for (node, sizes) in plans {
            area.set_split_sizes(node, sizes, window, cx);
        }
    });
}

fn panel_named(area: &DockArea, name: &str, cx: &App) -> Option<PanelId> {
    let tree = area.layout(DockPlacement::Center)?;
    tree.panels().find(|id| {
        area.panel(*id)
            .is_some_and(|view| view.panel_name(cx) == name)
    })
}

/// The horizontal slot whose size is this panel's column width.
fn pane_column(node: &PaneNode, panel: PanelId) -> Option<(NodeId, usize, Vec<Option<Pixels>>)> {
    let PaneRef::Split {
        axis,
        children,
        sizes,
    } = node.kind()
    else {
        return None;
    };
    if axis == Axis::Horizontal {
        for (index, child) in children.iter().enumerate() {
            if !subtree_has(child, panel) {
                continue;
            }
            if let Some(nested) = pane_column(child, panel) {
                return Some(nested);
            }
            return Some((node.id(), index, sizes.to_vec()));
        }
        return None;
    }
    for child in children {
        if let Some(found) = pane_column(child, panel) {
            return Some(found);
        }
    }
    None
}

fn subtree_has(node: &PaneNode, files: PanelId) -> bool {
    match node.kind() {
        PaneRef::Tabs { panels, .. } => panels.contains(&files),
        PaneRef::Split { children, .. } => children.iter().any(|child| subtree_has(child, files)),
    }
}

/// Grow each listed slot to [`crate::chrome::SIDEBAR_MIN_WIDTH`].
///
/// The extra width comes from the other slots. A listed slot keeps its width
/// down to the floor, and only lends what it has above that floor. The total
/// stays the same, so the dock does not scale the column back down. `None`
/// means every listed slot is already wide enough, a slot has not been
/// measured yet, or the row cannot spare the width.
fn widen_columns(sizes: &[Option<Pixels>], indexes: &[usize]) -> Option<Vec<Pixels>> {
    if indexes.is_empty() || sizes.iter().any(Option::is_none) {
        return None;
    }
    let min = crate::chrome::SIDEBAR_MIN_WIDTH;
    let mut next: Vec<Pixels> = sizes.iter().copied().flatten().collect();
    let mut growth = px(0.);
    for &index in indexes {
        let current = *next.get(index)?;
        if current < min {
            growth += min - current;
            next[index] = min;
        }
    }
    if growth <= px(0.) {
        return None;
    }
    growth = give_width(&mut next, growth, indexes, false, px(0.));
    if growth > px(0.) {
        growth = give_width(&mut next, growth, indexes, true, min);
    }
    if growth > px(0.) {
        return None;
    }
    Some(next)
}

/// Move `growth` out of the slots selected by `protected`, down to `floor`.
fn give_width(
    sizes: &mut [Pixels],
    mut growth: Pixels,
    indexes: &[usize],
    protected: bool,
    floor: Pixels,
) -> Pixels {
    for (slot, size) in sizes.iter_mut().enumerate() {
        if growth <= px(0.) {
            break;
        }
        if indexes.contains(&slot) != protected {
            continue;
        }
        let spare = if *size > floor { *size - floor } else { px(0.) };
        let give = if spare < growth { spare } else { growth };
        *size -= give;
        growth -= give;
    }
    growth
}

fn register_panels(cx: &mut App) {
    register_panel(cx, FILES, |_, window, cx| {
        panel_handle(open_panel(Slot::Files, window, cx))
    });
    register_panel(cx, NOTE, |_, window, cx| {
        panel_handle(open_panel(Slot::Note, window, cx))
    });
    register_panel(cx, ARCHIVIST, |_, window, cx| {
        panel_handle(open_panel(Slot::Archivist, window, cx))
    });
}

fn open_panel(slot: Slot, window: &mut Window, cx: &mut App) -> Entity<DockPanel> {
    let root = cx.global::<DockHost>().root.clone();
    cx.new(|cx| DockPanel::new(slot, root, window, cx))
}

fn default_layout(window: &mut Window, cx: &mut App) -> DockLayout {
    let files = open_panel(Slot::Files, window, cx);
    let note = open_panel(Slot::Note, window, cx);
    let archivist = open_panel(Slot::Archivist, window, cx);
    DockLayout::h_split()
        .child(
            DockLayout::tabs().panel_view(panel_handle(files), cx),
            Some(px(260.)),
        )
        .child(DockLayout::tabs().panel_view(panel_handle(note), cx), None)
        .child(
            DockLayout::tabs().panel_view(panel_handle(archivist), cx),
            Some(px(320.)),
        )
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
    use super::{LAYOUT_LIMIT, accept_layout, widen_columns};
    use gpui_kit::px;

    #[test]
    fn accept_layout_rejects_empty_oversized_and_broken_text() {
        assert!(accept_layout(None).is_none());
        assert!(accept_layout(Some("")).is_none());
        assert!(accept_layout(Some("not-json")).is_none());
        let huge = "x".repeat(LAYOUT_LIMIT + 1);
        assert!(accept_layout(Some(&huge)).is_none());
    }

    #[test]
    fn accept_layout_rejects_a_different_version() {
        let raw =
            r#"{"version":2,"center":{"panel_name":"files","children":[],"info":{"panel":null}}}"#;
        assert!(accept_layout(Some(raw)).is_none());
    }

    #[test]
    fn accept_layout_keeps_version_one() {
        let raw =
            r#"{"version":1,"center":{"panel_name":"files","children":[],"info":{"panel":null}}}"#;
        let state = accept_layout(Some(raw)).expect("version 1 layout");
        assert_eq!(state.version, Some(1));
        assert_eq!(state.center.panel_name, "files");
    }

    #[test]
    fn widen_columns_keeps_a_wide_files_column() {
        let sizes = [Some(px(260.)), Some(px(400.)), Some(px(320.))];
        assert!(widen_columns(&sizes, &[0]).is_none());
    }

    #[test]
    fn widen_columns_grows_the_files_column_from_its_neighbor() {
        let sizes = [Some(px(40.)), Some(px(500.)), Some(px(320.))];
        let next = widen_columns(&sizes, &[0]).expect("narrow column");
        let min = crate::chrome::SIDEBAR_MIN_WIDTH;
        assert_eq!(next, vec![min, px(500.) - (min - px(40.)), px(320.)]);
    }

    #[test]
    fn widen_columns_grows_a_files_column_that_is_not_first() {
        let sizes = [Some(px(320.)), Some(px(40.)), Some(px(400.))];
        let next = widen_columns(&sizes, &[1]).expect("narrow column");
        let min = crate::chrome::SIDEBAR_MIN_WIDTH;
        assert_eq!(next, vec![px(320.) - (min - px(40.)), min, px(400.)]);
    }

    #[test]
    fn widen_columns_skips_an_unmeasured_slot() {
        let sizes = [Some(px(100.)), None, Some(px(320.))];
        assert!(widen_columns(&sizes, &[0]).is_none());
    }

    #[test]
    fn widen_columns_refuses_when_the_row_cannot_spare_the_width() {
        let sizes = [Some(px(40.)), Some(px(10.)), Some(px(10.))];
        assert!(widen_columns(&sizes, &[0]).is_none());
    }

    #[test]
    fn widen_columns_keeps_both_side_panes_at_the_floor() {
        let sizes = [Some(px(40.)), Some(px(700.)), Some(px(40.))];
        let next = widen_columns(&sizes, &[0, 2]).expect("narrow sides");
        let min = crate::chrome::SIDEBAR_MIN_WIDTH;
        let extra = min - px(40.);
        assert_eq!(next, vec![min, px(700.) - extra - extra, min]);
    }

    #[test]
    fn widen_columns_borrows_surplus_above_the_other_floor() {
        let sizes = [Some(px(40.)), Some(px(10.)), Some(px(400.))];
        let next = widen_columns(&sizes, &[0, 2]).expect("note is small");
        let min = crate::chrome::SIDEBAR_MIN_WIDTH;
        let from_note = px(10.);
        let from_archivist = (min - px(40.)) - from_note;
        assert_eq!(next, vec![min, px(0.), px(400.) - from_archivist]);
    }

    #[test]
    fn widen_columns_refuses_when_both_floors_do_not_fit() {
        let sizes = [Some(px(40.)), Some(px(10.)), Some(px(40.))];
        assert!(widen_columns(&sizes, &[0, 2]).is_none());
    }
}
