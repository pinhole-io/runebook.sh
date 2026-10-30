//! Settings screen in the note area.
//!
//! Sections run Appearance, Notes, Capture, then Models.
//! Each row has a label, a sentence, and a control.
//! The label tooltip adds the fact the sentence leaves out.
//! A short control sits on the right. A long control sits under the sentence.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputContentType, InputState};
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{App, ClickEvent, Entity, WeakEntity, Window, div, px};

use super::{EditorPane, NoteView};
use crate::chrome::panel_header;
use crate::{Root, on_root};
use runebook::{AutosavePace, SpeechModel, TextSize};

use crate::locale;

pub(super) fn panel(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    let background = cx.theme().background;
    let foreground = cx.theme().foreground;
    div()
        .id("settings-panel")
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(background)
        .text_color(foreground)
        .child(header(view.clone(), border))
        .child(
            div()
                .id("settings-body")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p_4()
                .child(
                    div().w_full().flex().justify_center().child(
                        div()
                            .w_full()
                            .max_w(px(520.))
                            .flex()
                            .flex_col()
                            .gap_6()
                            .child(appearance(pane, view.clone(), cx, muted, border))
                            .child(notes(pane, view.clone(), muted, border))
                            .child(capture(pane, view, cx, muted, border))
                            .child(models(pane, cx, muted, border)),
                    ),
                ),
        )
}

fn header(view: WeakEntity<Root>, border: impl Into<gpui_kit::Hsla>) -> impl IntoElement {
    panel_header(div().id("settings-header").px_2(), border.into())
        .child(div().px_2().child(locale::text("settings.title")))
        .child(div().flex_1())
        .child(
            Button::new("settings-done")
                .ghost()
                .small()
                .cursor_pointer()
                .label(locale::text("settings.done"))
                .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
                    if let Some(pane) = this.pane.as_mut() {
                        pane.toggle_settings(window, cx);
                    }
                    cx.notify();
                })),
        )
}

fn appearance(
    pane: &EditorPane,
    view: WeakEntity<Root>,
    cx: &App,
    muted: impl Into<gpui_kit::Hsla> + Clone,
    border: impl Into<gpui_kit::Hsla>,
) -> impl IntoElement {
    let muted = muted.into();
    section(
        locale::text("settings.appearance"),
        locale::text("settings.appearance.desc"),
        muted,
        border,
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(stacked_setting(
                "theme-label",
                locale::text("settings.theme"),
                locale::text("settings.theme.desc"),
                locale::text("settings.theme.tip"),
                muted,
                theme_field(pane, cx),
            ))
            .child(short_setting(
                "text-size-label",
                locale::text("settings.text_size"),
                locale::text("settings.text_size.desc"),
                locale::text("settings.text_size.tip"),
                muted,
                text_size_group(pane, view.clone(), locale::text("settings.text_size.tip")),
            ))
            .child(short_setting(
                "ui-language-label",
                locale::text("settings.ui_language"),
                locale::text("settings.ui_language.desc"),
                locale::text("settings.ui_language.tip"),
                muted,
                locale_group(pane, view, locale::text("settings.ui_language.tip")),
            )),
    )
}

fn notes(
    pane: &EditorPane,
    view: WeakEntity<Root>,
    muted: impl Into<gpui_kit::Hsla> + Clone,
    border: impl Into<gpui_kit::Hsla>,
) -> impl IntoElement {
    let muted = muted.into();
    section(
        locale::text("settings.notes"),
        locale::text("settings.notes.desc"),
        muted,
        border,
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(short_setting(
                "note-view-label",
                locale::text("settings.note_view"),
                locale::text("settings.note_view.desc"),
                locale::text("settings.note_view.tip"),
                muted,
                note_view_group(pane, view.clone(), locale::text("settings.note_view.tip")),
            ))
            .child(toggle_row(
                "line-numbers",
                "line-numbers-label",
                locale::text("settings.line_numbers"),
                locale::text("settings.line_numbers.desc"),
                locale::text("settings.line_numbers.tip"),
                pane.line_numbers,
                muted,
                {
                    let view = view.clone();
                    move |checked, window, app| {
                        let on = *checked;
                        view.update(app, |root, cx| {
                            if let Some(pane) = root.pane.as_mut() {
                                pane.choose_line_numbers(on, window, cx);
                            }
                            cx.notify();
                        })
                        .ok();
                    }
                },
            ))
            .child(toggle_row(
                "soft-wrap",
                "soft-wrap-label",
                locale::text("settings.wrap"),
                locale::text("settings.wrap.desc"),
                locale::text("settings.wrap.tip"),
                pane.soft_wrap,
                muted,
                {
                    let view = view.clone();
                    move |checked, window, app| {
                        let on = *checked;
                        view.update(app, |root, cx| {
                            if let Some(pane) = root.pane.as_mut() {
                                pane.choose_soft_wrap(on, window, cx);
                            }
                            cx.notify();
                        })
                        .ok();
                    }
                },
            ))
            .child(stacked_setting(
                "autosave-label",
                locale::text("settings.autosave"),
                locale::text("settings.autosave.desc"),
                locale::text("settings.autosave.tip"),
                muted,
                autosave_group(pane, view.clone(), locale::text("settings.autosave.tip")),
            ))
            .child(toggle_row(
                "reopen-note",
                "reopen-label",
                locale::text("settings.reopen"),
                locale::text("settings.reopen.desc"),
                locale::text("settings.reopen.tip"),
                pane.reopen_last_note,
                muted,
                move |checked, _, app| {
                    let on = *checked;
                    view.update(app, |root, cx| {
                        if let Some(pane) = root.pane.as_mut() {
                            pane.choose_reopen_last_note(on);
                        }
                        cx.notify();
                    })
                    .ok();
                },
            )),
    )
}

fn capture(
    pane: &EditorPane,
    view: WeakEntity<Root>,
    cx: &App,
    muted: impl Into<gpui_kit::Hsla> + Clone,
    border: impl Into<gpui_kit::Hsla>,
) -> impl IntoElement {
    let muted = muted.into();
    section(
        locale::text("settings.capture"),
        locale::text("settings.capture.desc"),
        muted,
        border,
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(stacked_setting(
                "microphone-label",
                locale::text("settings.microphone"),
                locale::text("settings.microphone.desc"),
                locale::text("settings.microphone.tip"),
                muted,
                microphone_field(pane, cx),
            ))
            .child(short_setting(
                "speech-model-label",
                locale::text("settings.speech_model"),
                locale::text("settings.speech_model.desc"),
                locale::text("settings.speech_model.tip"),
                muted,
                speech_group(
                    pane,
                    view.clone(),
                    locale::text("settings.speech_model.tip"),
                ),
            ))
            .child(stacked_setting(
                "speech-language-label",
                locale::text("settings.speech_language"),
                locale::text("settings.speech_language.desc"),
                locale::text("settings.speech_language.tip"),
                muted,
                text_setting(&pane.model_inputs.speech_language),
            ))
            .child(toggle_row(
                "start-muted",
                "start-muted-label",
                locale::text("settings.start_muted"),
                locale::text("settings.start_muted.desc"),
                locale::text("settings.start_muted.tip"),
                pane.start_muted,
                muted,
                move |checked, _, app| {
                    let on = *checked;
                    view.update(app, |root, cx| {
                        if let Some(pane) = root.pane.as_mut() {
                            pane.choose_start_muted(on);
                        }
                        cx.notify();
                    })
                    .ok();
                },
            )),
    )
}

fn models(
    pane: &EditorPane,
    cx: &App,
    muted: impl Into<gpui_kit::Hsla> + Clone,
    border: impl Into<gpui_kit::Hsla> + Clone,
) -> impl IntoElement {
    let muted = muted.into();
    let border = border.into();
    let inputs = &pane.model_inputs;
    section(
        locale::text("settings.models"),
        locale::text("settings.models.desc"),
        muted,
        border,
        div()
            .flex()
            .flex_col()
            .gap_4()
            .when(pane.model_list_failed, |column| {
                column.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(locale::text("settings.models.load_failed")),
                )
            })
            .child(model_card(
                locale::text("settings.voice"),
                locale::text("settings.voice.desc"),
                muted,
                border,
                stacked_setting(
                    "groq-key-label",
                    locale::text("settings.groq_key"),
                    locale::text("settings.groq_key.desc"),
                    locale::text("settings.groq_key.tip"),
                    muted,
                    secret_setting(&inputs.groq_api_key),
                ),
            ))
            .child(model_card(
                locale::text("settings.research"),
                locale::text("settings.research.desc"),
                muted,
                border,
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(stacked_setting(
                        "openai-key-label",
                        locale::text("settings.openai_key"),
                        locale::text("settings.openai_key.desc"),
                        locale::text("settings.openai_key.tip"),
                        muted,
                        secret_setting(&inputs.openai_api_key),
                    ))
                    .child(stacked_setting(
                        "api-host-label",
                        locale::text("settings.api_host"),
                        locale::text("settings.api_host.desc"),
                        locale::text("settings.api_host.tip"),
                        muted,
                        text_setting(&inputs.openai_base_url),
                    ))
                    .child(stacked_setting(
                        "research-model-label",
                        locale::text("settings.research_model"),
                        locale::text("settings.research_model.desc"),
                        locale::text("settings.research_model.tip"),
                        muted,
                        listed_model_field(
                            &pane.research_model_select,
                            "research-model",
                            &pane.models.openai_model,
                            cx,
                        ),
                    )),
            ))
            .child(model_card(
                locale::text("settings.archivist"),
                locale::text("settings.archivist.desc"),
                muted,
                border,
                stacked_setting(
                    "archivist-model-label",
                    locale::text("settings.archivist_model"),
                    locale::text("settings.archivist_model.desc"),
                    locale::text("settings.archivist_model.tip"),
                    muted,
                    listed_model_field(
                        &pane.archivist_model_select,
                        "archivist-model",
                        &pane.models.archivist_model,
                        cx,
                    ),
                ),
            ))
            .child(stacked_setting(
                "tool-rounds-label",
                locale::text("settings.tool_rounds"),
                locale::text("settings.tool_rounds.desc"),
                locale::text("settings.tool_rounds.tip"),
                muted,
                text_setting(&inputs.tool_rounds),
            )),
    )
}

fn section(
    title: &'static str,
    sentence: &'static str,
    muted: impl Into<gpui_kit::Hsla>,
    border: impl Into<gpui_kit::Hsla>,
    body: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .pb_2()
                .border_b_1()
                .border_color(border.into())
                .text_base()
                .child(title),
        )
        .child(div().text_sm().text_color(muted.into()).child(sentence))
        .child(body)
}

fn model_card(
    title: &'static str,
    sentence: &'static str,
    muted: impl Into<gpui_kit::Hsla>,
    border: impl Into<gpui_kit::Hsla>,
    body: impl IntoElement,
) -> impl IntoElement {
    div()
        .w_full()
        .p_3()
        .flex()
        .flex_col()
        .gap_3()
        .rounded_md()
        .border_1()
        .border_color(border.into())
        .child(div().text_base().child(title))
        .child(div().text_sm().text_color(muted.into()).child(sentence))
        .child(body)
}

fn short_setting(
    id: &'static str,
    label: &'static str,
    sentence: &'static str,
    tip: &'static str,
    muted: impl Into<gpui_kit::Hsla>,
    control: impl IntoElement,
) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .child(label_block(id, label, sentence, tip, muted)),
        )
        .child(div().flex_shrink_0().child(control))
}

fn stacked_setting(
    id: &'static str,
    label: &'static str,
    sentence: &'static str,
    tip: &'static str,
    muted: impl Into<gpui_kit::Hsla>,
    control: impl IntoElement,
) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(label_block(id, label, sentence, tip, muted))
        .child(control)
}

fn toggle_row(
    id: &'static str,
    label_id: &'static str,
    label: &'static str,
    sentence: &'static str,
    tip: &'static str,
    on: bool,
    muted: impl Into<gpui_kit::Hsla>,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    short_setting(
        label_id,
        label,
        sentence,
        tip,
        muted,
        Switch::new(id)
            .checked(on)
            .cursor_pointer()
            .accessibility_label(label)
            .tooltip(tip)
            .on_change(on_change),
    )
}

fn label_block(
    id: &'static str,
    label: &'static str,
    sentence: &'static str,
    tip: &'static str,
    muted: impl Into<gpui_kit::Hsla>,
) -> impl IntoElement {
    let muted = muted.into();
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap_1()
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        .child(div().text_sm().child(label))
        .child(div().text_sm().text_color(muted).child(sentence))
}

fn text_setting(state: &Entity<InputState>) -> impl IntoElement {
    Input::new(state).w_full()
}

fn secret_setting(state: &Entity<InputState>) -> impl IntoElement {
    Input::new(state)
        .w_full()
        .content_type(InputContentType::Password)
        .mask_toggle()
}

fn listed_model_field(
    select: &Option<Entity<SelectState<SearchableVec<String>>>>,
    id: &'static str,
    saved: &str,
    cx: &App,
) -> gpui_kit::AnyElement {
    let Some(select) = select else {
        let label = if saved.is_empty() {
            locale::text("settings.model.default").to_string()
        } else {
            saved.to_string()
        };
        return pending_select(id, &label, cx).into_any_element();
    };
    Select::new(select)
        .id(id)
        .w_full()
        .cursor_pointer()
        .search_placeholder(locale::text("settings.model.search"))
        .into_any_element()
}

fn theme_field(pane: &EditorPane, cx: &App) -> gpui_kit::AnyElement {
    let Some(select) = pane.theme_select.as_ref() else {
        return pending_select("theme", &pane.theme, cx).into_any_element();
    };
    Select::new(select)
        .id("theme")
        .w_full()
        .cursor_pointer()
        .into_any_element()
}

fn microphone_field(pane: &EditorPane, cx: &App) -> gpui_kit::AnyElement {
    let Some(select) = pane.microphone_select.as_ref() else {
        return microphone_loading(cx).into_any_element();
    };
    Select::new(select)
        .id("microphone")
        .w_full()
        .cursor_pointer()
        .into_any_element()
}

fn pending_select(id: &'static str, label: &str, cx: &App) -> impl IntoElement {
    let border = cx.theme().border;
    let muted = cx.theme().muted_foreground;
    div()
        .id(id)
        .w_full()
        .h(px(32.))
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(border)
        .text_base()
        .text_color(muted)
        .child(label.to_string())
}

fn microphone_loading(cx: &App) -> impl IntoElement {
    let border = cx.theme().border;
    let muted = cx.theme().muted_foreground;
    div()
        .id("microphone")
        .w_full()
        .h(px(32.))
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(border)
        .text_base()
        .text_color(muted)
        .child(locale::text("settings.microphone.loading"))
}

fn locale_group(pane: &EditorPane, view: WeakEntity<Root>, tip: &'static str) -> RadioGroup {
    let choices = locale::choices();
    let selected = choices.iter().position(|choice| choice.code == pane.locale);
    RadioGroup::horizontal("ui-locale")
        .selected_index(selected)
        .children(choices.into_iter().map(|choice| {
            Radio::new(choice.code)
                .label(choice.name)
                .cursor_pointer()
                .tooltip(tip)
        }))
        .on_change(move |index, window, app| {
            let Some(code) = locale::choices().get(*index).map(|choice| choice.code) else {
                return;
            };
            view.update(app, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.choose_locale(code, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
}

fn text_size_group(pane: &EditorPane, view: WeakEntity<Root>, tip: &'static str) -> RadioGroup {
    let selected = match pane.text_size {
        TextSize::Small => 0,
        TextSize::Medium => 1,
        TextSize::Large => 2,
    };
    RadioGroup::horizontal("text-size")
        .selected_index(Some(selected))
        .child(
            Radio::new("text-small")
                .label(locale::text("settings.text.small"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .child(
            Radio::new("text-medium")
                .label(locale::text("settings.text.medium"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .child(
            Radio::new("text-large")
                .label(locale::text("settings.text.large"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .on_change(move |index, _, app| {
            let size = match *index {
                0 => TextSize::Small,
                1 => TextSize::Medium,
                2 => TextSize::Large,
                _ => return,
            };
            view.update(app, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.choose_text_size(size);
                }
                cx.notify();
            })
            .ok();
            crate::theme::apply_text_size(size, app);
        })
}

fn note_view_group(pane: &EditorPane, view: WeakEntity<Root>, tip: &'static str) -> RadioGroup {
    let selected = match pane.default_note_view {
        NoteView::Preview => 0,
        NoteView::Source => 1,
    };
    RadioGroup::horizontal("note-view")
        .selected_index(Some(selected))
        .child(
            Radio::new("view-preview")
                .label(locale::text("settings.view.preview"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .child(
            Radio::new("view-source")
                .label(locale::text("settings.view.source"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .on_change(move |index, window, app| {
            let next = match *index {
                0 => NoteView::Preview,
                1 => NoteView::Source,
                _ => return,
            };
            view.update(app, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.choose_note_view(next, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
}

fn autosave_group(pane: &EditorPane, view: WeakEntity<Root>, tip: &'static str) -> RadioGroup {
    let selected = match pane.autosave {
        AutosavePace::Fast => 0,
        AutosavePace::Normal => 1,
        AutosavePace::Slow => 2,
        AutosavePace::Manual => 3,
    };
    RadioGroup::vertical("autosave")
        .selected_index(Some(selected))
        .child(
            Radio::new("save-fast")
                .label(locale::text("settings.autosave.fast"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .child(
            Radio::new("save-normal")
                .label(locale::text("settings.autosave.normal"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .child(
            Radio::new("save-slow")
                .label(locale::text("settings.autosave.slow"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .child(
            Radio::new("save-manual")
                .label(locale::text("settings.autosave.manual"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .on_change(move |index, _, app| {
            let pace = match *index {
                0 => AutosavePace::Fast,
                1 => AutosavePace::Normal,
                2 => AutosavePace::Slow,
                3 => AutosavePace::Manual,
                _ => return,
            };
            view.update(app, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.choose_autosave(pace);
                }
                cx.notify();
            })
            .ok();
        })
}

fn speech_group(pane: &EditorPane, view: WeakEntity<Root>, tip: &'static str) -> RadioGroup {
    let selected = match pane.speech_model {
        SpeechModel::Fast => 0,
        SpeechModel::Accurate => 1,
    };
    RadioGroup::horizontal("speech-model")
        .selected_index(Some(selected))
        .child(
            Radio::new("speech-fast")
                .label(locale::text("settings.speech_fast"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .child(
            Radio::new("speech-accurate")
                .label(locale::text("settings.speech_accurate"))
                .cursor_pointer()
                .tooltip(tip),
        )
        .on_change(move |index, _, app| {
            let model = match *index {
                0 => SpeechModel::Fast,
                1 => SpeechModel::Accurate,
                _ => return,
            };
            view.update(app, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.choose_speech_model(model);
                }
                cx.notify();
            })
            .ok();
        })
}
