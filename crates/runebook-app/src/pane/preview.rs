//! Preview rows.
//!
//! A click selects one row. A drag or a shift-click extends the range.
//! The menu opens when the pointer goes up.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Enter, Escape, Input};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, ClickEvent, Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    SharedString, div, px,
};
use runebook::preview_markdown;

use super::EditorPane;
use super::rows::split_rows;
use crate::Root;

pub(super) fn surface(
    pane: &EditorPane,
    view: gpui_kit::WeakEntity<Root>,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let slices: Vec<String> = split_rows(&pane.latest)
        .into_iter()
        .map(|row| pane.latest[row.start..row.end].to_string())
        .collect();
    let bounds = pane
        .row_gesture
        .as_ref()
        .and_then(|gesture| gesture.bounds(slices.len()));
    let menu = bounds.is_some()
        && pane
            .row_gesture
            .as_ref()
            .is_some_and(|gesture| gesture.menu_open());
    let menu_at = bounds.map(|(_, hi)| hi);
    let rewrite = pane.row_rewrite.clone();
    let selection = cx.theme().selection;
    let hover = cx.theme().list_hover;
    let popover = cx.theme().popover;
    let popover_text = cx.theme().popover_foreground;
    let border = cx.theme().border;

    let mut body: Vec<AnyElement> = Vec::with_capacity(slices.len() + 1);
    for (index, slice) in slices.into_iter().enumerate() {
        let selected = bounds.is_some_and(|(lo, hi)| index >= lo && index <= hi);
        body.push(
            note_row(index, slice, selected, selection, hover, view.clone(), cx).into_any_element(),
        );
        if menu && menu_at == Some(index) {
            body.push(
                row_menu(rewrite.clone(), popover, popover_text, border, cx).into_any_element(),
            );
        }
    }

    div()
        .id("preview")
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p_4()
        .flex()
        .flex_col()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.clear_note_rows();
                }
                cx.notify();
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.release_note_row();
                }
                cx.notify();
            }),
        )
        .children(body)
}

fn note_row(
    index: usize,
    source: String,
    selected: bool,
    selection: gpui_kit::Hsla,
    hover: gpui_kit::Hsla,
    view: gpui_kit::WeakEntity<Root>,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let markdown = SharedString::from(preview_markdown(&source));
    div()
        .id(SharedString::from(format!("note-row-{index}")))
        .w_full()
        .min_w(px(0.))
        .px_2()
        .py(px(2.))
        .rounded_md()
        .cursor_pointer()
        .when(selected, |row| row.bg(selection))
        .when(!selected, |row| row.hover(move |style| style.bg(hover)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if let Some(pane) = this.pane.as_mut() {
                    pane.press_note_row(index, event.modifiers.shift);
                }
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
            let changed = this
                .pane
                .as_mut()
                .is_some_and(|pane| pane.drag_note_row(index));
            if changed {
                cx.notify();
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.release_note_row();
                }
                cx.notify();
            }),
        )
        .child(
            crate::mermaid::view(
                SharedString::from(format!("note-row-text-{index}")),
                markdown,
            )
            .w_full()
            .selectable(false)
            .on_link_click(move |url, _, window, cx| {
                let url = url.to_string();
                view.update(cx, |this, cx| {
                    this.follow_link(&url, window, cx);
                })
                .ok();
            }),
        )
}

fn row_menu(
    rewrite: Option<gpui_kit::Entity<gpui_kit::component::input::InputState>>,
    popover: gpui_kit::Hsla,
    popover_text: gpui_kit::Hsla,
    border: gpui_kit::Hsla,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let empty = rewrite
        .as_ref()
        .is_some_and(|input| input.read(cx).value().trim().is_empty());
    div()
        .id("note-row-menu")
        .mt_1()
        .mb_2()
        .p_2()
        .w_full()
        .max_w(px(360.))
        .flex()
        .flex_col()
        .gap_2()
        .rounded_md()
        .border_1()
        .border_color(border)
        .bg(popover)
        .text_color(popover_text)
        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .child(
                    Button::new("row-add-chat")
                        .small()
                        .cursor_pointer()
                        .label("Add to chat")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(pane) = this.pane.as_mut() {
                                pane.add_selected_rows_to_chat();
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("row-rewrite")
                        .small()
                        .primary()
                        .cursor_pointer()
                        .label("Rewrite")
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            if let Some(pane) = this.pane.as_mut() {
                                pane.begin_row_rewrite(window, cx);
                            }
                            cx.notify();
                        })),
                ),
        )
        .when_some(rewrite, |menu, input| {
            menu.child(
                div()
                    .id("row-rewrite-prompt")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_action(cx.listener(|this, _: &Enter, window, cx| {
                        cx.stop_propagation();
                        if let Some(pane) = this.pane.as_mut() {
                            pane.apply_row_rewrite(window, cx);
                        }
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &Escape, _, cx| {
                        cx.stop_propagation();
                        if let Some(pane) = this.pane.as_mut() {
                            pane.clear_note_rows();
                        }
                        cx.notify();
                    }))
                    .child(Input::new(&input).w_full())
                    .child(
                        Button::new("row-rewrite-apply")
                            .small()
                            .primary()
                            .cursor_pointer()
                            .disabled(empty)
                            .label("Apply")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                if let Some(pane) = this.pane.as_mut() {
                                    pane.apply_row_rewrite(window, cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
        })
}
