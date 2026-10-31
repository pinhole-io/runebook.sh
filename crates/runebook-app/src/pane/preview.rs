//! The source editor's selection menu.
//!
//! The note preview draws its own menu inside the page. This menu stays on
//! the source editor, where a text selection can be copied or rewritten.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::InputState;
use gpui_kit::component::input::{Enter, Escape, Input};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, ClickEvent, Entity, MouseButton, MouseDownEvent, MouseUpEvent, WeakEntity, div, px,
};

use crate::Root;
use crate::locale;
use crate::on_root;

pub(super) fn selection_menu(
    rewrite: Option<Entity<InputState>>,
    view: WeakEntity<Root>,
    cx: &App,
) -> impl IntoElement {
    let prompting = rewrite.is_some();
    let empty = rewrite
        .as_ref()
        .is_some_and(|input| input.read(cx).value().trim().is_empty());
    div()
        .id("note-row-menu")
        .flex()
        .flex_col()
        .gap_2()
        .when(prompting, |menu| menu.w(px(280.)))
        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
            cx.stop_propagation();
        })
        .on_key_up(|_: &gpui_kit::KeyUpEvent, _, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .child(
                    Button::new("row-copy")
                        .small()
                        .cursor_pointer()
                        .label(locale::text("preview.copy"))
                        .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
                            if let Some(pane) = this.pane.as_ref() {
                                pane.copy_selected_rows(cx);
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("row-add-chat")
                        .small()
                        .cursor_pointer()
                        .label(locale::text("preview.add"))
                        .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
                            if let Some(pane) = this.pane.as_mut() {
                                pane.add_selected_rows_to_chat(cx);
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("row-rewrite")
                        .small()
                        .primary()
                        .cursor_pointer()
                        .label(locale::text("preview.rewrite"))
                        .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
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
                    .on_action(on_root(&view, |this, _: &Enter, window, cx| {
                        cx.stop_propagation();
                        if let Some(pane) = this.pane.as_mut() {
                            pane.apply_row_rewrite(window, cx);
                        }
                        cx.notify();
                    }))
                    .on_action(on_root(&view, |this, _: &Escape, _, cx| {
                        cx.stop_propagation();
                        if let Some(pane) = this.pane.as_mut() {
                            pane.clear_note_rows(cx);
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
                            .label(locale::text("preview.apply"))
                            .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
                                if let Some(pane) = this.pane.as_mut() {
                                    pane.apply_row_rewrite(window, cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
        })
}
