//! Layout for the open vault: toolbar, tree, editor, and preview.

use std::sync::{Arc, OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, EditorState};
#[cfg(target_os = "macos")]
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::component::toolbar::Toolbar;
use gpui_kit::component::{ActiveTheme, ThemeStyled};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, ClickEvent, Image, ImageFormat, KeyUpEvent, MouseDownEvent, MouseUpEvent, Pixels, Point,
    WeakEntity, anchored, canvas, deferred, div, img, point, px, relative,
};

use super::{EditorPane, NoteView};
use crate::chrome::panel_header;

use crate::locale;
use crate::{Root, ToggleNoteView, on_root};

pub(crate) fn toolbar(pane: &EditorPane, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    Toolbar::new("vault-toolbar")
        .w_full()
        .pl_0()
        .content(brand(pane))
        .content(div().flex_1())
        .content(
            div()
                .pr_2()
                .text_xs()
                .text_color(muted)
                .child(locale::text("brand.alpha")),
        )
}

fn toggle_note_view(pane: &EditorPane, view: WeakEntity<Root>) -> impl IntoElement {
    let (icon, label) = match pane.note_view {
        NoteView::Preview => (IconName::SquarePen, locale::text("toolbar.edit_source")),
        NoteView::Source => (IconName::Eye, locale::text("toolbar.show_preview")),
    };
    Button::new("toggle-note-view")
        .ghost()
        .cursor_pointer()
        .icon(icon)
        .tooltip_with_action(label, &ToggleNoteView, None)
        .accessibility_label(label)
        .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
            if let Some(pane) = this.pane.as_mut() {
                pane.toggle_note_view(window, cx);
            }
            cx.notify();
        }))
}

pub(crate) fn note(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> gpui_kit::AnyElement {
    if pane.settings_open {
        return super::settings::panel(pane, view, cx).into_any_element();
    }
    if pane.research.wizard_open() {
        return super::research::wizard(pane, view, cx).into_any_element();
    }
    note_surface(pane, view, cx)
}

fn note_surface(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> gpui_kit::AnyElement {
    let body = if pane.research_note_open() {
        super::research::report(pane, view.clone(), cx).into_any_element()
    } else {
        match pane.note_view {
            NoteView::Preview => super::preview::surface(pane, view.clone(), cx).into_any_element(),
            NoteView::Source => source_surface(pane, view.clone(), cx).into_any_element(),
        }
    };
    let border = cx.theme().border;
    let background = cx.theme().background;
    let foreground = cx.theme().foreground;
    div()
        .flex_1()
        .h_full()
        .min_w(crate::chrome::NOTE_MIN_WIDTH)
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(background)
        .text_color(foreground)
        .child(note_header(pane, border, view, cx))
        .child(body)
        .into_any_element()
}

fn note_header(
    pane: &EditorPane,
    border: gpui_kit::Hsla,
    view: WeakEntity<Root>,
    _cx: &App,
) -> impl IntoElement {
    panel_header(div().id("note-header").px_2(), border)
        .child(div().flex_1())
        .when(!pane.research_note_open(), |header| {
            header.child(toggle_note_view(pane, view))
        })
}

fn source_surface(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let watch = view.clone();
    div()
        .id("source")
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .on_mouse_down(
            gpui_kit::MouseButton::Left,
            on_root(&view, |this, _: &MouseDownEvent, _, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.press_source();
                }
                cx.notify();
            }),
        )
        .on_key_up(on_root(&view, |this, _: &KeyUpEvent, _, cx| {
            let Some(pane) = this.pane.as_mut() else {
                return;
            };
            if pane.source_pressing {
                return;
            }
            if pane.sync_source_menu(cx) {
                cx.notify();
            }
        }))
        .child(
            Editor::new(&pane.editor)
                .bordered(false)
                .readonly(pane.open_note.is_none())
                .h(relative(1.))
                .size_full()
                .flex_1(),
        )
        .children(source_selection_popup(pane, view, cx))
        .child(source_release_watch(watch))
}

/// Window position for the menu: just under the selection, or under the caret.
fn source_menu_point(editor: &EditorState) -> Option<Point<Pixels>> {
    let range = editor.selected_range();
    if range.is_empty() {
        return None;
    }
    let origin = if let Some(bounds) = editor.range_to_bounds(&range) {
        bounds.bottom_left()
    } else {
        editor.cursor_layout()?.0.bottom_left()
    };
    Some(point(origin.x, origin.y + px(4.)))
}

fn source_selection_popup(
    pane: &EditorPane,
    view: WeakEntity<Root>,
    cx: &App,
) -> Option<impl IntoElement> {
    if !pane.source_menu {
        return None;
    }
    let origin = source_menu_point(pane.editor.read(cx))?;
    let menu = super::preview::selection_menu(pane.row_rewrite.clone(), view, cx);
    Some(
        deferred(
            anchored().position(origin).snap_to_window().child(
                div()
                    .id("source-selection-menu")
                    .occlude()
                    .popover_style(cx)
                    .p_3()
                    .child(menu),
            ),
        )
        .with_priority(1),
    )
}

/// Hear the pointer go up even when it leaves the editor.
fn source_release_watch(view: WeakEntity<Root>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            let view = view.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if !phase.bubble() {
                    return;
                }
                view.update(cx, |root, cx| {
                    let Some(pane) = root.pane.as_mut() else {
                        return;
                    };
                    if pane.release_source_press(cx) {
                        cx.notify();
                    }
                })
                .ok();
            });
        },
    )
}

fn brand(pane: &EditorPane) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .pr_2()
        .child(img(mark()).h(px(20.)).w(px(14.)).flex_shrink_0())
        .child(pane.vault.name().to_string())
}

fn mark() -> Arc<Image> {
    static MARK: OnceLock<Arc<Image>> = OnceLock::new();
    MARK.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            include_bytes!("../../../../assets/icon.svg").to_vec(),
        ))
    })
    .clone()
}

pub(crate) fn status_bar(pane: &EditorPane, cx: &App) -> impl IntoElement {
    let note = pane
        .open_note
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| locale::text("status.no_note").to_string());
    let state = if pane.dirty {
        locale::text("status.unsaved")
    } else {
        locale::text("status.saved")
    };
    let path = pane.vault.root().display().to_string();
    let mut text = locale::format(
        "status.bar",
        &[("path", &path), ("note", &note), ("state", state)],
    );
    let join = locale::text("status.join");
    if let Some(status) = pane.capture_status() {
        text.push_str(join);
        text.push_str(status);
    }
    if let Some(notice) = &pane.notice {
        text.push_str(join);
        text.push_str(notice);
    }
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    div()
        .px_2()
        .py_1()
        .text_sm()
        .text_color(muted)
        .border_t_1()
        .border_color(border)
        .child(text)
}
