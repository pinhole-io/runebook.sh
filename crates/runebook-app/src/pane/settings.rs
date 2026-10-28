//! Settings screen in the note area.
//!
//! Each section owns one kind of choice. A switch row keeps the label and the
//! control on one line. A text field sits under its label.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputContentType, InputState};
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Sizable, Theme};
use gpui_kit::prelude::*;
use gpui_kit::{App, ClickEvent, Entity, WeakEntity, Window, div, px};

use super::{EditorPane, NoteView};
use crate::chrome::panel_header;
use crate::{Root, on_root, theme_mode};
use runebook::{AutosavePace, SpeechModel, TextSize, ThemeChoice};

use crate::locale;

pub(super) fn panel(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    div()
        .id("settings-panel")
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(header(view.clone(), border))
        .child(
            div()
                .id("settings-body")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p_4()
                .child(
                    div()
                        .w_full()
                        .max_w(px(520.))
                        .flex()
                        .flex_col()
                        .gap_6()
                        .child(appearance(pane, view.clone(), muted, border))
                        .child(editor(pane, view.clone(), muted, border))
                        .child(capture(pane, view.clone(), cx, muted, border))
                        .child(models(pane, muted, border))
                        .child(vault(pane, view, muted, border)),
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
                .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
                    if let Some(pane) = this.pane.as_mut() {
                        pane.toggle_settings(cx);
                    }
                    cx.notify();
                })),
        )
}

fn appearance(
    pane: &EditorPane,
    view: WeakEntity<Root>,
    muted: impl Into<gpui_kit::Hsla> + Clone,
    border: impl Into<gpui_kit::Hsla>,
) -> impl IntoElement {
    let muted = muted.into();
    section(
        locale::text("settings.appearance"),
        locale::text("settings.appearance.hint"),
        border,
        muted,
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(field(
                locale::text("settings.theme"),
                muted,
                theme_group(pane, view.clone()),
            ))
            .child(field(
                locale::text("settings.ui_language"),
                muted,
                locale_group(pane, view.clone()),
            ))
            .child(field(
                locale::text("settings.text_size"),
                muted,
                text_size_group(pane, view.clone()),
            ))
            .child(field(
                locale::text("settings.note_view"),
                muted,
                note_view_group(pane, view),
            )),
    )
}

fn editor(
    pane: &EditorPane,
    view: WeakEntity<Root>,
    muted: impl Into<gpui_kit::Hsla> + Clone,
    border: impl Into<gpui_kit::Hsla>,
) -> impl IntoElement {
    let muted = muted.into();
    section(
        locale::text("settings.editor"),
        locale::text("settings.editor.hint"),
        border,
        muted,
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(toggle_row(
                "line-numbers",
                locale::text("settings.line_numbers"),
                locale::text("settings.line_numbers.hint"),
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
                locale::text("settings.wrap"),
                locale::text("settings.wrap.hint"),
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
            .child(field(
                locale::text("settings.autosave"),
                muted,
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(autosave_group(pane, view))
                    .child(hint(locale::text("settings.autosave.hint"), muted)),
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
        locale::text("settings.capture.hint"),
        border,
        muted,
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(field(
                locale::text("settings.microphone"),
                muted,
                microphone_field(pane, cx),
            ))
            .child(field(
                locale::text("settings.speech_model"),
                muted,
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(speech_group(pane, view.clone()))
                    .child(hint(locale::text("settings.speech_model.hint"), muted)),
            ))
            .child(field(
                locale::text("settings.speech_language"),
                muted,
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(text_setting(&pane.model_inputs.speech_language))
                    .child(hint(locale::text("settings.speech_language.hint"), muted)),
            ))
            .child(toggle_row(
                "start-muted",
                locale::text("settings.start_muted"),
                locale::text("settings.start_muted.hint"),
                pane.start_muted,
                muted,
                {
                    let view = view;
                    move |checked, _, app| {
                        let on = *checked;
                        view.update(app, |root, cx| {
                            if let Some(pane) = root.pane.as_mut() {
                                pane.choose_start_muted(on);
                            }
                            cx.notify();
                        })
                        .ok();
                    }
                },
            )),
    )
}

fn models(
    pane: &EditorPane,
    muted: impl Into<gpui_kit::Hsla> + Clone,
    border: impl Into<gpui_kit::Hsla>,
) -> impl IntoElement {
    let muted = muted.into();
    let inputs = &pane.model_inputs;
    section(
        locale::text("settings.models"),
        locale::text("settings.models.hint"),
        border,
        muted,
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(field(
                locale::text("settings.tool_rounds"),
                muted,
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(text_setting(&inputs.tool_rounds))
                    .child(hint(locale::text("settings.tool_rounds.hint"), muted)),
            ))
            .child(group_label(locale::text("settings.voice"), muted))
            .child(field(
                locale::text("settings.groq_key"),
                muted,
                secret_setting(&inputs.groq_api_key),
            ))
            .child(group_label(locale::text("settings.research"), muted))
            .child(field(
                locale::text("settings.openai_key"),
                muted,
                secret_setting(&inputs.openai_api_key),
            ))
            .child(field(
                locale::text("settings.api_host"),
                muted,
                text_setting(&inputs.openai_base_url),
            ))
            .child(field(
                locale::text("settings.research_model"),
                muted,
                text_setting(&inputs.openai_model),
            ))
            .child(group_label(locale::text("settings.archivist"), muted))
            .child(field(
                locale::text("settings.archivist_model"),
                muted,
                text_setting(&inputs.archivist_model),
            )),
    )
}

fn vault(
    pane: &EditorPane,
    view: WeakEntity<Root>,
    muted: impl Into<gpui_kit::Hsla> + Clone,
    border: impl Into<gpui_kit::Hsla>,
) -> impl IntoElement {
    let muted = muted.into();
    section(
        locale::text("settings.vault"),
        locale::text("settings.vault.hint"),
        border,
        muted,
        toggle_row(
            "reopen-note",
            locale::text("settings.reopen"),
            locale::text("settings.reopen.hint"),
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
        ),
    )
}

fn section(
    title: &'static str,
    hint: &'static str,
    border: impl Into<gpui_kit::Hsla>,
    muted: impl Into<gpui_kit::Hsla>,
    body: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .pb_2()
                .border_b_1()
                .border_color(border.into())
                .child(div().text_base().child(title))
                .child(div().text_sm().text_color(muted.into()).child(hint)),
        )
        .child(body)
}

fn hint(text: &'static str, muted: impl Into<gpui_kit::Hsla>) -> impl IntoElement {
    div().text_sm().text_color(muted.into()).child(text)
}

fn field(
    label: &'static str,
    muted: impl Into<gpui_kit::Hsla>,
    control: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().text_color(muted.into()).child(label))
        .child(control)
}

fn group_label(text: &'static str, muted: impl Into<gpui_kit::Hsla>) -> impl IntoElement {
    div().pt_2().text_sm().text_color(muted.into()).child(text)
}

fn toggle_row(
    id: &'static str,
    label: &'static str,
    hint: &'static str,
    on: bool,
    muted: impl Into<gpui_kit::Hsla>,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_base().child(label))
                .child(div().text_sm().text_color(muted.into()).child(hint)),
        )
        .child(
            Switch::new(id)
                .checked(on)
                .cursor_pointer()
                .accessibility_label(label)
                .on_change(on_change),
        )
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

fn locale_group(pane: &EditorPane, view: WeakEntity<Root>) -> RadioGroup {
    let choices = locale::choices();
    let selected = choices.iter().position(|choice| choice.code == pane.locale);
    RadioGroup::horizontal("ui-locale")
        .selected_index(selected)
        .children(
            choices
                .into_iter()
                .map(|choice| Radio::new(choice.code).label(choice.name).cursor_pointer()),
        )
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

fn theme_group(pane: &EditorPane, view: WeakEntity<Root>) -> RadioGroup {
    let selected = match pane.theme {
        ThemeChoice::Dark => 0,
        ThemeChoice::Light => 1,
    };
    RadioGroup::horizontal("theme")
        .selected_index(Some(selected))
        .child(
            Radio::new("theme-dark")
                .label(locale::text("settings.theme.dark"))
                .cursor_pointer(),
        )
        .child(
            Radio::new("theme-light")
                .label(locale::text("settings.theme.light"))
                .cursor_pointer(),
        )
        .on_change(move |index, window, app| {
            let choice = match *index {
                0 => ThemeChoice::Dark,
                1 => ThemeChoice::Light,
                _ => return,
            };
            let mut size = pane_text_size(&view, app);
            Theme::change(theme_mode(choice), Some(window), app);
            view.update(app, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.choose_theme(choice);
                    size = pane.text_size;
                }
                cx.notify();
            })
            .ok();
            crate::theme::apply_text_size(size, app);
        })
}

fn text_size_group(pane: &EditorPane, view: WeakEntity<Root>) -> RadioGroup {
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
                .cursor_pointer(),
        )
        .child(
            Radio::new("text-medium")
                .label(locale::text("settings.text.medium"))
                .cursor_pointer(),
        )
        .child(
            Radio::new("text-large")
                .label(locale::text("settings.text.large"))
                .cursor_pointer(),
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

fn note_view_group(pane: &EditorPane, view: WeakEntity<Root>) -> RadioGroup {
    let selected = match pane.default_note_view {
        NoteView::Preview => 0,
        NoteView::Source => 1,
    };
    RadioGroup::horizontal("note-view")
        .selected_index(Some(selected))
        .child(
            Radio::new("view-preview")
                .label(locale::text("settings.view.preview"))
                .cursor_pointer(),
        )
        .child(
            Radio::new("view-source")
                .label(locale::text("settings.view.source"))
                .cursor_pointer(),
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

fn autosave_group(pane: &EditorPane, view: WeakEntity<Root>) -> RadioGroup {
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
                .cursor_pointer(),
        )
        .child(
            Radio::new("save-normal")
                .label(locale::text("settings.autosave.normal"))
                .cursor_pointer(),
        )
        .child(
            Radio::new("save-slow")
                .label(locale::text("settings.autosave.slow"))
                .cursor_pointer(),
        )
        .child(
            Radio::new("save-manual")
                .label(locale::text("settings.autosave.manual"))
                .cursor_pointer(),
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

fn speech_group(pane: &EditorPane, view: WeakEntity<Root>) -> RadioGroup {
    let selected = match pane.speech_model {
        SpeechModel::Fast => 0,
        SpeechModel::Accurate => 1,
    };
    RadioGroup::horizontal("speech-model")
        .selected_index(Some(selected))
        .child(
            Radio::new("speech-fast")
                .label(locale::text("settings.speech_fast"))
                .cursor_pointer(),
        )
        .child(
            Radio::new("speech-accurate")
                .label(locale::text("settings.speech_accurate"))
                .cursor_pointer(),
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

fn pane_text_size(view: &WeakEntity<Root>, app: &mut App) -> TextSize {
    view.update(app, |root, _| {
        root.pane
            .as_ref()
            .map(|pane| pane.text_size)
            .unwrap_or_default()
    })
    .unwrap_or_default()
}
