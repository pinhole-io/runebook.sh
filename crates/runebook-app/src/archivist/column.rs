//! Right-hand Archivist column.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Enter, Input};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, ClickEvent, Context, Hsla, SharedString, div, px};

use super::{MISSING_KEY, Panel};
use crate::Root;
use crate::capture::groq_key_is_set;

pub(crate) fn column(panel: &Panel, cx: &mut Context<Root>) -> impl IntoElement {
    let border = cx.theme().sidebar_border;
    let sidebar = cx.theme().sidebar;
    let foreground = cx.theme().sidebar_foreground;
    let muted = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    let ready = groq_key_is_set();
    let busy = panel.busy;

    div()
        .id("archivist")
        .w(px(320.))
        .h_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .overflow_hidden()
        .border_l_1()
        .border_color(border)
        .bg(sidebar)
        .text_color(foreground)
        .child(
            div()
                .px_3()
                .py_2()
                .text_sm()
                .border_b_1()
                .border_color(border)
                .child("Archivist"),
        )
        .child(transcript(panel, muted, danger, busy))
        .child(composer(panel, ready, busy, danger, cx))
}

fn transcript(
    panel: &Panel,
    muted: impl Into<Hsla>,
    danger: impl Into<Hsla>,
    busy: bool,
) -> impl IntoElement {
    let muted = muted.into();
    let danger = danger.into();
    let empty = panel.lines.is_empty();
    div()
        .id("archivist-log")
        .flex_1()
        .min_h(px(0.))
        .py_2()
        .flex()
        .flex_col()
        .gap_3()
        .overflow_y_scrollbar()
        .when(empty, |log| {
            log.child(
                div()
                    .px_3()
                    .text_sm()
                    .text_color(muted)
                    .child("Ask the Archivist to add, change, or delete a note."),
            )
        })
        .children(
            panel
                .lines
                .iter()
                .enumerate()
                .map(|(index, line)| line_block(index, line, muted, danger)),
        )
        .when(busy && live_visible(&panel.live), |log| {
            log.child(live_block(&panel.live_rendered, muted))
        })
        .when(busy, |log| {
            log.child(div().px_3().text_sm().text_color(muted).child("Working…"))
        })
}

fn live_visible(live: &super::LiveText) -> bool {
    !live.thinking.trim().is_empty() || !live.body.trim().is_empty()
}

fn line_block(index: usize, line: &super::Line, muted: Hsla, danger: Hsla) -> AnyElement {
    match line {
        super::Line::User(text) => plain_block("You", text, muted, muted).into_any_element(),
        super::Line::Error(text) => {
            plain_block("Archivist", text, danger, danger).into_any_element()
        }
        super::Line::Archivist { thinking, body } => {
            let thinking = thinking.clone();
            let body = body.clone();
            div()
                .px_3()
                .w_full()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap_2()
                .when(!thinking.trim().is_empty(), |block| {
                    block.child(thinking_block(
                        SharedString::from(format!("archivist-{index}-thinking")),
                        thinking.clone(),
                        muted,
                    ))
                })
                .when(!body.trim().is_empty(), |block| {
                    block.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xs().text_color(muted).child("Archivist"))
                            .child(markdown_block(
                                SharedString::from(format!("archivist-{index}-body")),
                                body.clone(),
                            )),
                    )
                })
                .into_any_element()
        }
    }
}

fn live_block(live: &super::RenderedMarkdown, muted: Hsla) -> impl IntoElement {
    let thinking = live.thinking.clone();
    let body = live.body.clone();
    div()
        .px_3()
        .w_full()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap_2()
        .when(!thinking.trim().is_empty(), |block| {
            block.child(thinking_block(
                SharedString::from("archivist-live-thinking"),
                thinking.clone(),
                muted,
            ))
        })
        .when(!body.trim().is_empty(), |block| {
            block.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().text_color(muted).child("Archivist"))
                    .child(markdown_block(
                        SharedString::from("archivist-live-body"),
                        body.clone(),
                    )),
            )
        })
}

fn thinking_block(id: SharedString, text: SharedString, muted: Hsla) -> impl IntoElement {
    div()
        .w_full()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap_1()
        .opacity(0.75)
        .child(div().text_xs().text_color(muted).child("Thinking"))
        .child(markdown_block(id, text))
}

fn markdown_block(id: SharedString, source: SharedString) -> impl IntoElement {
    div()
        .w_full()
        .min_w(px(0.))
        .text_sm()
        .child(crate::mermaid::view(id, source))
}

fn plain_block(
    label: &'static str,
    body: &str,
    label_color: Hsla,
    body_color: Hsla,
) -> impl IntoElement {
    div()
        .px_3()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(label_color).child(label))
        .child(
            div()
                .text_sm()
                .w_full()
                .whitespace_normal()
                .text_color(body_color)
                .child(body.to_string()),
        )
}

fn composer(
    panel: &Panel,
    ready: bool,
    busy: bool,
    danger: impl Into<Hsla>,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let border = cx.theme().sidebar_border;
    let danger = danger.into();
    div()
        .flex_shrink_0()
        .border_t_1()
        .border_color(border)
        .p_2()
        .flex()
        .flex_col()
        .gap_2()
        .when(!ready, |column| {
            column.child(div().text_sm().text_color(danger).child(MISSING_KEY))
        })
        .child(
            div()
                .id("archivist-draft")
                .on_action(cx.listener(|this, _: &Enter, window, cx| {
                    cx.stop_propagation();
                    if let Some(pane) = this.pane.as_mut() {
                        pane.send_archivist(window, cx);
                    }
                    cx.notify();
                }))
                .child(Input::new(&panel.draft).w_full()),
        )
        .child(
            Button::new("archivist-send")
                .small()
                .primary()
                .cursor_pointer()
                .disabled(!ready || busy)
                .label("Send")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    if let Some(pane) = this.pane.as_mut() {
                        pane.send_archivist(window, cx);
                    }
                    cx.notify();
                })),
        )
}
