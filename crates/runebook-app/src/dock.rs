//! File tree, note, and Archivist panels in one dock.
//!
//! The layout JSON lives in the user config. This module accepts or rejects
//! that string. It does not log the text.

use gpui_kit::component::dock::{
    BasePanel, DockArea, DockAreaState, DockEvent, DockLayout, DockSkin, Panel, PanelEvent,
    panel_handle, register_panel,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Global, IntoElement, Render,
    SharedString, WeakEntity, Window, div, px,
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
            Slot::Files => "Notes",
            Slot::Note => "Note",
            Slot::Archivist => "Archivist",
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

    fn visible(&self, cx: &App) -> bool {
        if !matches!(self.slot, Slot::Archivist) {
            return true;
        }
        self.root.upgrade().is_some_and(|root| {
            root.read(cx)
                .pane
                .as_ref()
                .is_some_and(|pane| pane.archivist_open())
        })
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
        div()
            .id(self.name())
            .track_focus(&self.focus)
            .size_full()
            .min_w(px(0.))
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
            Slot::Archivist => {
                crate::archivist::column(pane.archivist(), pane.groq_ready(), view, cx)
                    .into_any_element()
            }
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
    cx.subscribe(&area, |this, area, event: &DockEvent, cx| {
        if matches!(event, DockEvent::LayoutChanged) {
            this.schedule_dock_save(&area, cx);
        }
    })
    .detach();
    let area_for_save = area.clone();
    cx.defer_in(window, move |this, _, cx| {
        this.schedule_dock_save(&area_for_save, cx);
    });
    area
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
    use super::{LAYOUT_LIMIT, accept_layout};

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
}
