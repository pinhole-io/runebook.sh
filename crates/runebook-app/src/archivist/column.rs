//! Right-hand Archivist column.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{
    Enter, InputGroup, InputGroupAddon, InputGroupAddonAlignment, InputGroupButton, Textarea,
};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, ClickEvent, Hsla, SharedString, WeakEntity, div, px};

use super::Panel;
use crate::Root;
use crate::chrome::{SIDEBAR_MIN_WIDTH, panel_header};
use crate::locale;
use crate::on_root;

pub(crate) fn column(
    panel: &Panel,
    ready: bool,
    blocked: &'static str,
    view: WeakEntity<Root>,
    cx: &App,
) -> impl IntoElement {
    let border = cx.theme().sidebar_border;
    let sidebar = cx.theme().sidebar;
    let foreground = cx.theme().sidebar_foreground;
    let muted = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    let busy = panel.busy;

    div()
        .id("archivist")
        .size_full()
        .min_h(px(0.))
        .min_w(SIDEBAR_MIN_WIDTH)
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(sidebar)
        .text_color(foreground)
        .child(
            panel_header(div().id("archivist-header").px_2().text_base(), border)
                .child(locale::text("archivist.title")),
        )
        .child(transcript(panel, muted, danger, busy, cx))
        .when(!panel.held.is_empty(), |column| {
            column.child(held_rows(panel, view.clone(), cx))
        })
        .when(!panel.files.is_empty(), |column| {
            column.child(file_rows(panel, view.clone(), cx))
        })
        .child(composer(panel, ready, blocked, busy, danger, view, cx))
}

fn transcript(
    panel: &Panel,
    muted: impl Into<Hsla>,
    danger: impl Into<Hsla>,
    busy: bool,
    cx: &App,
) -> impl IntoElement {
    let muted = muted.into();
    let danger = danger.into();
    let empty = panel.lines.is_empty();
    let scroll = panel.log_scroll.clone();
    div()
        .id("archivist-log")
        .flex_1()
        .min_h(px(0.))
        .overflow_hidden()
        .relative()
        .child(
            div()
                .id("archivist-log-area")
                .size_full()
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .child(
                    div()
                        .id("archivist-log-content")
                        .w_full()
                        .flex_none()
                        .h_auto()
                        .min_h_full()
                        .pt_2()
                        .pb_4()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .when(empty, |log| {
                            log.child(
                                div()
                                    .px_2()
                                    .text_base()
                                    .text_color(muted)
                                    .child(locale::text("archivist.empty")),
                            )
                        })
                        .children(
                            panel
                                .lines
                                .iter()
                                .enumerate()
                                .map(|(index, line)| line_block(index, line, muted, danger, cx)),
                        )
                        .when(busy && live_visible(&panel.live), |log| {
                            log.child(live_block(&panel.live_rendered, muted, cx))
                        })
                        .when(busy, |log| {
                            log.child(
                                div()
                                    .px_2()
                                    .text_base()
                                    .text_color(muted)
                                    .child(locale::text("archivist.working")),
                            )
                        }),
                ),
        )
        .child(
            div().absolute().inset_0().child(
                Scrollbar::vertical(&scroll)
                    .id("archivist-log-bar")
                    .viewport_from_layout(),
            ),
        )
}

fn live_visible(live: &super::LiveText) -> bool {
    !live.thinking.trim().is_empty() || !live.body.trim().is_empty()
}

fn line_block(index: usize, line: &super::Line, muted: Hsla, danger: Hsla, cx: &App) -> AnyElement {
    match line {
        super::Line::User(text) => {
            plain_block(locale::text("archivist.you"), text, muted, muted).into_any_element()
        }
        super::Line::Error(text) => {
            plain_block(locale::text("archivist.title"), text, danger, danger).into_any_element()
        }
        super::Line::Archivist { thinking, body } => {
            let thinking = thinking.clone();
            let body = body.clone();
            div()
                .px_2()
                .w_full()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap_3()
                .when(!thinking.trim().is_empty(), |block| {
                    block.child(thinking_block(
                        SharedString::from(format!("archivist-{index}-thinking")),
                        thinking.clone(),
                        muted,
                        cx,
                    ))
                })
                .when(!body.trim().is_empty(), |block| {
                    block.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .text_color(muted)
                                    .child(locale::text("archivist.title")),
                            )
                            .child(markdown_block(
                                SharedString::from(format!("archivist-{index}-body")),
                                body.clone(),
                                cx,
                            )),
                    )
                })
                .into_any_element()
        }
    }
}

fn live_block(live: &super::RenderedMarkdown, muted: Hsla, cx: &App) -> impl IntoElement {
    let thinking = live.thinking.clone();
    let body = live.body.clone();
    div()
        .px_2()
        .w_full()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap_3()
        .when(!thinking.trim().is_empty(), |block| {
            block.child(thinking_block(
                SharedString::from("archivist-live-thinking"),
                thinking.clone(),
                muted,
                cx,
            ))
        })
        .when(!body.trim().is_empty(), |block| {
            block.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .text_color(muted)
                            .child(locale::text("archivist.title")),
                    )
                    .child(markdown_block(
                        SharedString::from("archivist-live-body"),
                        body.clone(),
                        cx,
                    )),
            )
        })
}

fn thinking_block(id: SharedString, text: SharedString, muted: Hsla, cx: &App) -> impl IntoElement {
    div()
        .w_full()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap_1()
        .opacity(0.75)
        .child(
            div()
                .text_base()
                .text_color(muted)
                .child(locale::text("archivist.thinking")),
        )
        .child(markdown_block(id, text, cx))
}

fn markdown_block(id: SharedString, source: SharedString, cx: &App) -> impl IntoElement {
    div()
        .w_full()
        .min_w(px(0.))
        .text_base()
        .child(crate::mermaid::view(id, source, cx))
}

fn plain_block(
    label: &'static str,
    body: &str,
    label_color: Hsla,
    body_color: Hsla,
) -> impl IntoElement {
    div()
        .px_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_base().text_color(label_color).child(label))
        .child(
            div()
                .text_base()
                .w_full()
                .min_w(px(0.))
                .whitespace_normal()
                .text_color(body_color)
                .child(body.to_string()),
        )
}

fn held_rows(panel: &Panel, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    let cards: Vec<AnyElement> = panel
        .held
        .iter()
        .enumerate()
        .map(|(index, row)| {
            held_card(index, &row.note_id, &row.source, muted, view.clone()).into_any_element()
        })
        .collect();
    div()
        .flex_shrink_0()
        .border_t_1()
        .border_color(border)
        .px_2()
        .py_2()
        .flex()
        .flex_col()
        .gap_2()
        .children(cards)
        .child(
            div()
                .text_base()
                .text_color(muted)
                .child(locale::text("archivist.rows")),
        )
}

fn held_card(
    index: usize,
    note_id: &str,
    source: &str,
    muted: Hsla,
    view: WeakEntity<Root>,
) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_base().text_color(muted).child(locale::format(
                    "archivist.selected_rows",
                    &[("note", note_id)],
                )))
                .child(
                    div()
                        .text_base()
                        .w_full()
                        .whitespace_normal()
                        .child(held_excerpt(source)),
                ),
        )
        .child(
            Button::new(SharedString::from(format!("held-remove-{index}")))
                .ghost()
                .small()
                .compact()
                .cursor_pointer()
                .label(locale::text("archivist.remove"))
                .on_click(on_root(&view, move |this, _: &ClickEvent, _, cx| {
                    if let Some(pane) = this.pane.as_mut() {
                        pane.dismiss_held_rows(index);
                    }
                    cx.notify();
                })),
        )
}

fn file_rows(panel: &Panel, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    let busy = panel.busy;
    let cards: Vec<AnyElement> = panel
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            file_card(index, &file.name, busy, muted, view.clone()).into_any_element()
        })
        .collect();
    div()
        .flex_shrink_0()
        .border_t_1()
        .border_color(border)
        .px_2()
        .py_2()
        .flex()
        .flex_col()
        .gap_2()
        .children(cards)
        .child(
            div()
                .text_base()
                .text_color(muted)
                .child(locale::text("archivist.files")),
        )
}

fn file_card(
    index: usize,
    name: &str,
    busy: bool,
    muted: Hsla,
    view: WeakEntity<Root>,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_base()
                .text_color(muted)
                .child(name.to_string()),
        )
        .child(
            Button::new(SharedString::from(format!("file-remove-{index}")))
                .ghost()
                .small()
                .compact()
                .cursor_pointer()
                .disabled(busy)
                .label(locale::text("archivist.remove"))
                .on_click(on_root(&view, move |this, _: &ClickEvent, _, cx| {
                    if let Some(pane) = this.pane.as_mut() {
                        pane.dismiss_archivist_file(index);
                    }
                    cx.notify();
                })),
        )
}

fn held_excerpt(source: &str) -> String {
    let flat = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = flat.chars();
    let short: String = chars.by_ref().take(120).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

fn composer(
    panel: &Panel,
    ready: bool,
    blocked: &'static str,
    busy: bool,
    danger: impl Into<Hsla>,
    view: WeakEntity<Root>,
    cx: &App,
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
            column.child(div().text_base().text_color(danger).child(blocked))
        })
        .child(
            div()
                .id("archivist-draft")
                .on_action(on_root(&view, |this, action: &Enter, window, cx| {
                    if action.shift {
                        return;
                    }
                    cx.stop_propagation();
                    if let Some(pane) = this.pane.as_mut() {
                        pane.send_archivist(window, cx);
                    }
                    cx.notify();
                }))
                .child(
                    InputGroup::new("archivist-composer")
                        .large()
                        .w_full()
                        .input(Textarea::new(&panel.draft).min_h(px(0.)))
                        .addon(
                            InputGroupAddon::new("archivist-file-addon").child(
                                InputGroupButton::new("archivist-add-file")
                                    .small()
                                    .icon(IconName::FilePlus)
                                    .tooltip(locale::text("archivist.add_file"))
                                    .accessibility_label(locale::text("archivist.add_file"))
                                    .cursor_pointer()
                                    .disabled(busy)
                                    .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
                                        if let Some(pane) = this.pane.as_mut() {
                                            pane.pick_archivist_files(cx);
                                        }
                                        cx.notify();
                                    })),
                            ),
                        )
                        .addon(
                            InputGroupAddon::new("archivist-send-addon")
                                .align(InputGroupAddonAlignment::InlineEnd)
                                .child(
                                    InputGroupButton::new("archivist-send")
                                        .small()
                                        .icon(IconName::Send)
                                        .tooltip(locale::text("archivist.send"))
                                        .accessibility_label(locale::text("archivist.send"))
                                        .cursor_pointer()
                                        .disabled(!ready || busy)
                                        .on_click(on_root(
                                            &view,
                                            |this, _: &ClickEvent, window, cx| {
                                                if let Some(pane) = this.pane.as_mut() {
                                                    pane.send_archivist(window, cx);
                                                }
                                                cx.notify();
                                            },
                                        )),
                                ),
                        ),
                ),
        )
}
