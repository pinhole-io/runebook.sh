//! Keys section of Settings.
//!
//! Each row edits one command chord. Record listens for the next key.
//! The listener is dropped when the row finishes or Settings closes.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, ClickEvent, Context, Entity, Hsla, SharedString, Subscription, WeakEntity, Window, div, px,
};

use super::EditorPane;
use super::settings::{section as settings_section, stacked_setting};
use crate::keys;
use crate::locale;
use crate::{Root, on_root};
use runebook::AppConfig;

pub(super) struct KeyFields {
    inputs: Vec<Entity<InputState>>,
    /// Subscriptions that commit a chord when the field blurs or confirms.
    /// Held so the listeners stay alive while Settings is open.
    _watches: Vec<Subscription>,
    faults: Vec<Option<String>>,
    recording: Option<usize>,
    intercept: Option<Subscription>,
}

impl EditorPane {
    pub(super) fn prepare_keys(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.close_keys();
        let config = AppConfig::load().unwrap_or_default();
        let rows = keys::rows(&config.keys, cx.platform_info().os);
        let mut inputs = Vec::new();
        let mut watches = Vec::new();
        let mut faults = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(locale::text("settings.keys.unbound"))
                    .default_value(row.field.clone())
            });
            let watch = cx.subscribe(&input, move |root, input, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    return;
                }
                let text = input.read(cx).value().to_string();
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                pane.commit_key(index, &text, cx);
                pane.stop_recording();
                cx.notify();
            });
            inputs.push(input);
            watches.push(watch);
            faults.push(row.fault.as_ref().map(keys::fault_text));
        }
        self.key_fields = Some(KeyFields {
            inputs,
            _watches: watches,
            faults,
            recording: None,
            intercept: None,
        });
    }

    pub(super) fn close_keys(&mut self) {
        self.key_fields = None;
    }

    pub(super) fn record_key(&mut self, index: usize, cx: &mut Context<Root>) {
        self.stop_recording();
        let view = cx.weak_entity();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            let key = event.keystroke.key.as_str();
            if key.is_empty() || is_modifier_key(key) {
                return;
            }
            cx.stop_propagation();
            if key == "escape" && !event.keystroke.modifiers.modified() {
                view.update(cx, |root, cx| {
                    let intercept = root.pane.as_mut().and_then(|pane| pane.take_intercept());
                    cx.defer(move |_| drop(intercept));
                    cx.notify();
                })
                .ok();
                return;
            }
            let chord = event.keystroke.unparse(cx.platform_info().os);
            view.update(cx, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.finish_recording(index, chord, window, cx);
                }
                cx.notify();
            })
            .ok();
        });
        if let Some(fields) = self.key_fields.as_mut() {
            fields.recording = Some(index);
            fields.intercept = Some(intercept);
        }
    }

    fn finish_recording(
        &mut self,
        index: usize,
        chord: String,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let recording = self.key_fields.as_ref().and_then(|fields| fields.recording);
        if recording != Some(index) {
            return;
        }
        let intercept = self.take_intercept();
        if let Some(input) = self
            .key_fields
            .as_ref()
            .and_then(|fields| fields.inputs.get(index))
            .cloned()
        {
            input.update(cx, |state, cx| {
                state.set_value(chord.clone(), window, cx);
            });
        }
        self.commit_key(index, &chord, cx);
        cx.defer(move |_| drop(intercept));
    }

    pub(super) fn reset_key(&mut self, index: usize, window: &mut Window, cx: &mut Context<Root>) {
        let Some(id) = keys::command_id(index) else {
            return;
        };
        let defaults = keys::default_field(id);
        self.stop_recording();
        if let Err(err) = crate::save_setting(|config| {
            config.keys.remove(id);
        }) {
            self.set_fault(index, Some(err));
            return;
        }
        let keys = AppConfig::load().unwrap_or_default().keys;
        keys::apply(cx, &keys);
        if let Some(input) = self
            .key_fields
            .as_ref()
            .and_then(|fields| fields.inputs.get(index))
            .cloned()
        {
            input.update(cx, |state, cx| {
                state.set_value(defaults, window, cx);
            });
        }
        self.set_fault(index, None);
    }

    fn commit_key(&mut self, index: usize, text: &str, cx: &mut Context<Root>) {
        let Some(id) = keys::command_id(index) else {
            return;
        };
        let mut config = AppConfig::load().unwrap_or_default();
        match keys::commit(&mut config.keys, id, text, cx.platform_info().os) {
            Ok(()) => {
                let saved = config.keys.clone();
                match crate::save_setting(|slot| slot.keys = saved.clone()) {
                    Ok(()) => {
                        keys::apply(cx, &saved);
                        self.set_fault(index, None);
                    }
                    Err(err) => self.set_fault(index, Some(err)),
                }
            }
            Err(fault) => self.set_fault(index, Some(keys::fault_text(&fault))),
        }
    }

    fn stop_recording(&mut self) {
        drop(self.take_intercept());
    }

    fn take_intercept(&mut self) -> Option<Subscription> {
        let fields = self.key_fields.as_mut()?;
        fields.recording = None;
        fields.intercept.take()
    }

    fn set_fault(&mut self, index: usize, fault: Option<String>) {
        if let Some(fields) = self.key_fields.as_mut()
            && let Some(slot) = fields.faults.get_mut(index)
        {
            *slot = fault;
        }
    }
}

fn is_modifier_key(key: &str) -> bool {
    matches!(
        key,
        "control"
            | "ctrl"
            | "alt"
            | "shift"
            | "function"
            | "fn"
            | "platform"
            | "cmd"
            | "super"
            | "win"
            | "meta"
    )
}

pub(super) fn section(
    pane: &EditorPane,
    view: WeakEntity<Root>,
    cx: &App,
    muted: impl Into<Hsla> + Clone,
    border: impl Into<Hsla>,
) -> impl IntoElement {
    let muted = muted.into();
    let danger = cx.theme().danger;
    let rows = pane.key_fields.as_ref().map(|fields| {
        let recording = fields.recording;
        fields
            .inputs
            .iter()
            .enumerate()
            .map(|(index, input)| {
                key_row(
                    index,
                    input,
                    fields.faults.get(index).and_then(Option::as_ref),
                    recording == Some(index),
                    view.clone(),
                    muted,
                    danger,
                )
            })
            .collect::<Vec<_>>()
    });
    settings_section(
        locale::text("settings.keys"),
        locale::text("settings.keys.desc"),
        muted,
        border,
        div()
            .flex()
            .flex_col()
            .gap_4()
            .children(rows.into_iter().flatten()),
    )
}

fn key_row(
    index: usize,
    input: &Entity<InputState>,
    fault: Option<&String>,
    recording: bool,
    view: WeakEntity<Root>,
    muted: Hsla,
    danger: Hsla,
) -> impl IntoElement {
    let id = keys::command_id(index).unwrap_or("key");
    let label = keys::label_key(index).map(locale::text).unwrap_or(id);
    let detail = keys::detail_key(index)
        .map(locale::text)
        .unwrap_or(locale::text("settings.keys.desc"));
    stacked_setting(
        keys::label_id(index),
        label,
        detail,
        locale::text("settings.keys.tip"),
        muted,
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(Input::new(input).w_full()),
                    )
                    .child(record_button(index, view.clone()))
                    .child(reset_button(index, view)),
            )
            .children(fault.map(|text| div().text_sm().text_color(danger).child(text.clone())))
            .when(recording, |column| {
                column.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(locale::text("settings.keys.press")),
                )
            }),
    )
}

fn record_button(index: usize, view: WeakEntity<Root>) -> impl IntoElement {
    Button::new(SharedString::from(format!("key-record-{index}")))
        .ghost()
        .small()
        .cursor_pointer()
        .label(locale::text("settings.keys.record"))
        .on_click(on_root(&view, move |this, _: &ClickEvent, _, cx| {
            if let Some(pane) = this.pane.as_mut() {
                pane.record_key(index, cx);
            }
            cx.notify();
        }))
}

fn reset_button(index: usize, view: WeakEntity<Root>) -> impl IntoElement {
    Button::new(SharedString::from(format!("key-reset-{index}")))
        .ghost()
        .small()
        .cursor_pointer()
        .label(locale::text("settings.keys.reset"))
        .on_click(on_root(&view, move |this, _: &ClickEvent, window, cx| {
            if let Some(pane) = this.pane.as_mut() {
                pane.reset_key(index, window, cx);
            }
            cx.notify();
        }))
}
