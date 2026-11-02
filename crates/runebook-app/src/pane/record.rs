//! Microphone and screen capture for the open note.

use gpui_kit::component::input::Escape;

use super::*;

impl EditorPane {
    pub(super) fn capture_status(&self) -> Option<&'static str> {
        match self.capture {
            Capture::Idle => None,
            Capture::Voice(_) | Capture::Screen(_) | Capture::SavingScreen => {
                Some(locale::text("capture.recording"))
            }
            Capture::Transcribing => Some(locale::text("capture.transcribing")),
        }
    }

    pub(super) fn voice_recording(&self) -> bool {
        matches!(self.capture, Capture::Voice(_))
    }

    pub(super) fn screen_recording(&self) -> bool {
        matches!(self.capture, Capture::Screen(_))
    }

    /// The mic control records only into a Speech note.
    pub(super) fn voice_allowed(&self) -> bool {
        self.open_note.is_some() && self.open_kind == NoteKind::Speech
    }

    /// The video control records only into a Sight note.
    pub(super) fn sight_allowed(&self) -> bool {
        self.open_note.is_some() && self.open_kind == NoteKind::Sight
    }

    /// Start or stop voice capture for the open note.
    pub(super) fn toggle_voice(&mut self, cx: &mut Context<Root>) {
        if self.voice_recording() {
            self.stop_voice(cx);
            return;
        }
        if self.capture_status().is_some() {
            return;
        }
        self.start_voice();
    }

    /// Stop screen capture for the open note.
    ///
    /// A new Sight note asks for the display or window. The file-tree mark stops the capture.
    pub(super) fn toggle_screen(&mut self, cx: &mut Context<Root>) {
        if self.screen_recording() {
            self.stop_screen(cx);
        }
    }

    pub(super) fn start_voice(&mut self) {
        if !self.voice_allowed() {
            self.notice = Some(locale::text("notice.speech_note").to_string());
            return;
        }
        if !self.groq_ready() {
            self.notice = Some(locale::text("capture.groq_key").to_string());
            return;
        }
        match VoiceCapture::start(self.microphone.clone()) {
            Ok(capture) => {
                self.recording_note = self.open_note.clone();
                self.capture = Capture::Voice(capture);
                self.notice = None;
            }
            Err(err) => self.notice = Some(crate::capture::notice(&err)),
        }
    }

    pub(super) fn stop_voice(&mut self, cx: &mut Context<Root>) {
        let capture = match std::mem::replace(&mut self.capture, Capture::Idle) {
            Capture::Voice(capture) => capture,
            other => {
                self.capture = other;
                return;
            }
        };
        let Some(key) = self.groq_key() else {
            self.capture = Capture::Voice(capture);
            self.notice = Some(locale::text("capture.groq_key").to_string());
            return;
        };
        self.capture = Capture::Transcribing;
        let note_id = self.recording_note.clone();
        let speech = SpeechRequest {
            model: self.speech_model.api_name().to_string(),
            language: {
                let language = self.speech_language.trim();
                if language.is_empty() {
                    None
                } else {
                    Some(language.to_string())
                }
            },
        };
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { voice_to_text(capture, key, speech) })
                .await;
            this.update_in(cx, |root, window, cx| {
                let dirty = root
                    .pane
                    .as_mut()
                    .is_some_and(|pane| pane.finish_voice(note_id, outcome, window, cx));
                if dirty {
                    root.schedule_autosave(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn start_screen(&mut self, target: ScreenTarget) {
        if self.capture_status().is_some() {
            return;
        }
        if !self.sight_allowed() {
            self.notice = Some(locale::text("notice.sight_note").to_string());
            return;
        }
        let Some(id) = self.open_note.clone() else {
            self.notice = Some(locale::text("notice.sight_note").to_string());
            return;
        };
        let (path, file_name) = match self.next_recording(&id) {
            Ok(path) => path,
            Err(err) => {
                self.notice = Some(err);
                return;
            }
        };
        match ScreenCapture::start(path, file_name, target) {
            Ok(capture) => {
                self.recording_note = Some(id);
                self.capture = Capture::Screen(capture);
                self.notice = None;
            }
            Err(err) => self.notice = Some(crate::capture::notice(&err)),
        }
    }

    pub(super) fn stop_screen(&mut self, cx: &mut Context<Root>) {
        let capture = match std::mem::replace(&mut self.capture, Capture::Idle) {
            Capture::Screen(capture) => capture,
            other => {
                self.capture = other;
                return;
            }
        };
        let file_name = capture.file_name.clone();
        self.capture = Capture::SavingScreen;
        let note_id = self.recording_note.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { capture.stop() })
                .await;
            this.update_in(cx, |root, window, cx| {
                let dirty = root.pane.as_mut().is_some_and(|pane| {
                    pane.finish_screen(note_id, file_name, outcome, window, cx)
                });
                if dirty {
                    root.schedule_autosave(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn finish_voice(
        &mut self,
        note_id: Option<NoteId>,
        outcome: Result<crate::capture::VoiceOutcome, crate::capture::CaptureError>,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        self.capture = Capture::Idle;
        self.recording_note = None;
        let outcome = match outcome {
            Ok(outcome) if outcome.text.is_empty() => {
                self.notice = Some(locale::text("notice.no_speech").to_string());
                return false;
            }
            Ok(outcome) => outcome,
            Err(err) => {
                self.notice = Some(self.redact_notice(&crate::capture::notice(&err)));
                return false;
            }
        };
        let Some(id) = note_id else {
            self.notice = Some(locale::text("notice.recording_gone").to_string());
            return false;
        };
        let dirty = match self.deliver_block(&id, &outcome.text, window, cx) {
            Ok(dirty) => dirty,
            Err(err) => {
                self.notice = Some(err);
                return false;
            }
        };
        if outcome.truncated {
            self.notice = Some(locale::text("notice.recording_short").to_string());
        }
        dirty
    }

    pub(super) fn finish_screen(
        &mut self,
        note_id: Option<NoteId>,
        file_name: String,
        outcome: Result<(), crate::capture::CaptureError>,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        self.capture = Capture::Idle;
        self.recording_note = None;
        if let Err(err) = outcome {
            self.notice = Some(self.redact_notice(&crate::capture::notice(&err)));
            return false;
        }
        let Some(id) = note_id else {
            self.notice = Some(locale::text("notice.recording_gone").to_string());
            return false;
        };
        let link = markdown_link("screen recording", &file_name);
        match self.deliver_block(&id, &link, window, cx) {
            Ok(dirty) => dirty,
            Err(err) => {
                self.notice = Some(err);
                false
            }
        }
    }

    /// Put `block` in the editor when `id` is open. Otherwise write that note's file.
    pub(super) fn deliver_block(
        &mut self,
        id: &NoteId,
        block: &str,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<bool, String> {
        if self.open_note.as_ref() == Some(id) {
            self.insert_block(block, window, cx);
            return Ok(self.dirty);
        }
        let file = self.vault.read(id).map_err(|err| err.to_string())?;
        let next = note_with_block(&file, block);
        self.vault.write(id, &next).map_err(|err| err.to_string())?;
        Ok(false)
    }

    pub(super) fn insert_block(
        &mut self,
        block: &str,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let next = append_block(&self.latest, block);
        self.clear_note_rows(cx);
        self.latest = next.clone();
        self.dirty = self.open_note.is_some() && next != self.saved_text;
        self.notice = None;
        self.editor
            .update(cx, |state, cx| state.set_value(next, window, cx));
    }

    pub(super) fn next_recording(
        &self,
        id: &NoteId,
    ) -> Result<(std::path::PathBuf, String), String> {
        let base = utc_stamp_now();
        for n in 0..100 {
            let stamp = if n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            let path = self
                .vault
                .recording_path(id, &stamp)
                .map_err(|err| err.to_string())?;
            if path.exists() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| locale::text("notice.recording_name").to_string())?
                .to_string();
            return Ok((path, name));
        }
        Err(locale::text("notice.recording_choose").to_string())
    }

    pub(super) fn require_idle(&self) -> Result<(), String> {
        if self.capture_status().is_none() {
            Ok(())
        } else {
            Err(locale::text("notice.recording_idle").to_string())
        }
    }

    /// Copy the editor buffer into the preview. Returns true when a save should wait.
    pub(crate) fn capture_edit(&mut self, cx: &mut Context<Root>) -> bool {
        let value = self.editor.read(cx).value().to_string();
        if value != self.latest {
            self.clear_note_rows(cx);
        }
        self.latest = value.clone();
        self.dirty = self.open_note.is_some() && value != self.saved_text;
        self.dirty
    }
}

/// Ask which display or window a new Sight note should record.
pub(super) fn prompt_record_source(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) {
    let mut rows = vec![(
        locale::text("screen.full").to_string(),
        ScreenTarget::Display,
    )];
    let list_failed = match list_record_sources() {
        Ok(sources) => {
            for source in sources {
                rows.push((source.title, source.target));
            }
            false
        }
        Err(_) => true,
    };
    if list_failed && let Some(pane) = root.pane.as_mut() {
        pane.notice = Some(window_list_error().to_string());
    }
    let view = cx.weak_entity();
    window.open_dialog(cx, move |dialog, _, dialog_cx| {
        let danger = dialog_cx.theme().danger;
        let mut column = div()
            .id("record-sources")
            .w_full()
            .max_h(px(320.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .on_action(|_: &Escape, window, cx| {
                cx.stop_propagation();
                window.close_dialog(cx);
            });
        if list_failed {
            column = column.child(
                div()
                    .text_base()
                    .text_color(danger)
                    .child(window_list_error()),
            );
        }
        for (index, (label, target)) in rows.iter().cloned().enumerate() {
            column = column.child(record_source_button(view.clone(), index, label, target));
        }
        dialog
            .title(locale::text("screen.choose"))
            .w(px(420.))
            .child(column)
            .footer(
                Button::new("record-cancel")
                    .label(locale::text("dialog.cancel"))
                    .cursor_pointer()
                    .on_click(|_: &ClickEvent, window, cx| {
                        window.close_dialog(cx);
                    }),
            )
    });
}

pub(super) fn record_source_button(
    view: gpui_kit::WeakEntity<Root>,
    index: usize,
    label: String,
    target: ScreenTarget,
) -> impl IntoElement {
    Button::new(SharedString::from(format!("record-source-{index}")))
        .ghost()
        .w_full()
        .justify_start()
        .cursor_pointer()
        .label(label)
        .on_click(move |_: &ClickEvent, window, cx| {
            let target = target.clone();
            view.update(cx, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.start_screen(target);
                }
                cx.notify();
            })
            .ok();
            window.close_dialog(cx);
        })
}

/// Speech and Sight notes start a capture, so they wait until the current one ends.
pub(super) fn new_rune_blocked(kind: NoteKind, recording: bool) -> bool {
    recording && matches!(kind, NoteKind::Speech | NoteKind::Sight)
}

/// Append `block` to a note file and keep that note's kind header.
pub(super) fn note_with_block(file: &str, block: &str) -> String {
    let (kind, body) = decode_note(file);
    encode_note(kind, &append_block(body, block))
}

pub(super) fn recording_changed() -> &'static str {
    locale::text("notice.recording_changed")
}

pub(super) fn recording_removed() -> &'static str {
    locale::text("notice.recording_removed")
}

#[cfg(test)]
mod capture_note_tests {
    use super::{new_rune_blocked, note_with_block};
    use runebook::{NoteKind, decode_note, encode_note};

    #[test]
    fn speech_and_sight_wait_while_a_capture_runs() {
        assert!(new_rune_blocked(NoteKind::Speech, true));
        assert!(new_rune_blocked(NoteKind::Sight, true));
        assert!(!new_rune_blocked(NoteKind::Book, true));
        assert!(!new_rune_blocked(NoteKind::Research, true));
        assert!(!new_rune_blocked(NoteKind::Speech, false));
    }

    #[test]
    fn a_saved_note_keeps_its_header_when_a_block_is_appended() {
        let file = encode_note(NoteKind::Speech, "# speech\n");
        let next = note_with_block(&file, "hello");
        let (kind, body) = decode_note(&next);
        assert_eq!(kind, NoteKind::Speech);
        assert!(body.contains("hello"));
        assert!(body.contains("# speech"));
    }
}
