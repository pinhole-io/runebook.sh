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
        .child(div().px_2().child("Settings"))
        .child(div().flex_1())
        .child(
            Button::new("settings-done")
                .ghost()
                .small()
                .cursor_pointer()
                .label("Done")
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
        "Appearance",
        "Theme, text size, and the view a note opens in.",
        border,
        muted,
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(field("Theme", muted, theme_group(pane, view.clone())))
            .child(field(
                "Text size",
                muted,
                text_size_group(pane, view.clone()),
            ))
            .child(field("Note view", muted, note_view_group(pane, view))),
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
        "Editor",
        "Source editor and when a change is written.",
        border,
        muted,
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(toggle_row(
                "line-numbers",
                "Line numbers",
                "Show numbers beside the source.",
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
                "Wrap lines",
                "Wrap long source lines instead of scrolling sideways.",
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
                "Autosave",
                muted,
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(autosave_group(pane, view))
                    .child(hint(
                        "Manual writes the note when you leave it or close the window.",
                        muted,
                    )),
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
        "Capture",
        "Microphone, transcription, and playback.",
        border,
        muted,
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(field("Microphone", muted, microphone_field(pane, cx)))
            .child(field(
                "Speech model",
                muted,
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(speech_group(pane, view.clone()))
                    .child(hint(
                        "Fast uses whisper-large-v3-turbo. Accurate uses whisper-large-v3.",
                        muted,
                    )),
            ))
            .child(field(
                "Language",
                muted,
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(text_setting(&pane.model_inputs.speech_language))
                    .child(hint(
                        "A two-letter code. Empty lets Groq detect the language.",
                        muted,
                    )),
            ))
            .child(toggle_row(
                "start-muted",
                "Start media muted",
                "A new clip starts silent. The mute button still works.",
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
        "Models",
        "An empty field uses the environment value, then the default.",
        border,
        muted,
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(field(
                "Tool rounds",
                muted,
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(text_setting(&inputs.tool_rounds))
                    .child(hint("Cap for Research search and the Archivist.", muted)),
            ))
            .child(group_label("Voice", muted))
            .child(field(
                "Groq API key",
                muted,
                secret_setting(&inputs.groq_api_key),
            ))
            .child(group_label("Research", muted))
            .child(field(
                "OpenAI-compatible API key",
                muted,
                secret_setting(&inputs.openai_api_key),
            ))
            .child(field(
                "API host",
                muted,
                text_setting(&inputs.openai_base_url),
            ))
            .child(field(
                "Research model",
                muted,
                text_setting(&inputs.openai_model),
            ))
            .child(group_label("Archivist", muted))
            .child(field(
                "Archivist model",
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
        "Vault",
        "Which note opens with this vault.",
        border,
        muted,
        toggle_row(
            "reopen-note",
            "Reopen the last note",
            "The next launch opens that note when it is still in the vault.",
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
        .child("loading...")
}

fn theme_group(pane: &EditorPane, view: WeakEntity<Root>) -> RadioGroup {
    let selected = match pane.theme {
        ThemeChoice::Dark => 0,
        ThemeChoice::Light => 1,
    };
    RadioGroup::horizontal("theme")
        .selected_index(Some(selected))
        .child(Radio::new("theme-dark").label("Dark").cursor_pointer())
        .child(Radio::new("theme-light").label("Light").cursor_pointer())
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
        .child(Radio::new("text-small").label("Small").cursor_pointer())
        .child(Radio::new("text-medium").label("Medium").cursor_pointer())
        .child(Radio::new("text-large").label("Large").cursor_pointer())
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
        .child(Radio::new("view-preview").label("Preview").cursor_pointer())
        .child(Radio::new("view-source").label("Source").cursor_pointer())
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
                .label("Fast, 200 ms")
                .cursor_pointer(),
        )
        .child(
            Radio::new("save-normal")
                .label("Normal, 400 ms")
                .cursor_pointer(),
        )
        .child(
            Radio::new("save-slow")
                .label("Slow, 1 second")
                .cursor_pointer(),
        )
        .child(Radio::new("save-manual").label("Manual").cursor_pointer())
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
        .child(Radio::new("speech-fast").label("Fast").cursor_pointer())
        .child(
            Radio::new("speech-accurate")
                .label("Accurate")
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
