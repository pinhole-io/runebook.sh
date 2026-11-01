//! The open note buffer: load, flush, and row edits.

use super::*;

impl EditorPane {
    /// Swap the rendered note and the source editor.
    ///
    /// The editor entity stays alive in both modes. Focusing it on the way to
    /// the source lets typing start on the next frame. A Research note stays
    /// on the report.
    pub(crate) fn toggle_note_view(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if self.research_note_open() {
            return;
        }
        self.clear_note_rows(cx);
        self.note_view = match self.note_view {
            NoteView::Preview => NoteView::Source,
            NoteView::Source => NoteView::Preview,
        };
        if self.note_view == NoteView::Source {
            self.editor.focus_handle(cx).focus(window, cx);
        }
    }

    pub(super) fn research_note_open(&self) -> bool {
        self.open_note.is_some() && self.open_kind == NoteKind::Research
    }

    /// Write the open note when the buffer changed.
    ///
    /// Returns false when the write failed. The caller should stay on this note.
    pub(crate) fn flush_edits(&mut self, cx: &mut Context<Root>) -> bool {
        let Some(id) = self.open_note.clone() else {
            self.dirty = false;
            return true;
        };
        let value = self.editor.read(cx).value().to_string();
        if value != self.latest {
            self.clear_note_rows(cx);
        }
        self.latest = value.clone();
        if value == self.saved_text {
            self.dirty = false;
            return true;
        }
        let file = encode_note(self.open_kind, &value);
        match self.vault.write(&id, &file) {
            Ok(()) => {
                self.saved_text = value;
                self.dirty = false;
                self.notice = None;
                if let Ok(index) = self.vault.notes() {
                    self.index = index;
                }
                true
            }
            Err(err) => {
                self.notice = Some(err.to_string());
                false
            }
        }
    }

    pub(crate) fn load_note(
        &mut self,
        id: NoteId,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<(), String> {
        let text = self.vault.read(&id).map_err(|err| err.to_string())?;
        self.clear_note_rows(cx);
        self.reveal(&id.parent());
        self.open_note = Some(id.clone());
        self.remember_note(&id);
        self.tree_selection = TreeSelection::Note(id);
        self.apply_file(&text, window, cx);
        self.notice = None;
        self.settings_open = false;
        self.show_note_view(self.default_note_view, window, cx);
        Ok(())
    }

    pub(crate) fn create_and_open(
        &mut self,
        id: NoteId,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<(), String> {
        self.require_idle()?;
        self.vault
            .create_note(&id, body)
            .map_err(|err| err.to_string())?;
        self.reload().map_err(|err| err.to_string())?;
        self.load_note(id, window, cx)
    }

    pub(super) fn clear_editor(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.dismiss_media(window, cx);
        self.open_note = None;
        self.open_kind = NoteKind::Book;
        self.tree_selection = TreeSelection::None;
        self.clear_note_rows(cx);
        self.saved_text.clear();
        self.latest.clear();
        self.dirty = false;
        self.editor
            .update(cx, |state, cx| state.set_value("", window, cx));
    }

    pub(super) fn press_note_row(
        &mut self,
        index: usize,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        self.row_rewrite = None;
        self.row_gesture = rows::RowGesture::press(self.row_gesture.take(), index, shift);
        // Leave the source editor and the Archivist field so Ctrl+C copies these rows.
        self.preview_focus.focus(window, cx);
    }

    pub(super) fn drag_note_row(&mut self, index: usize) -> bool {
        self.row_gesture
            .as_mut()
            .is_some_and(|gesture| gesture.drag_over(index))
    }

    pub(super) fn release_note_row(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if let Some(gesture) = self.row_gesture.as_mut() {
            gesture.release();
        }
        if self.row_gesture.is_some() {
            self.preview_focus.focus(window, cx);
        }
    }

    pub(super) fn clear_note_rows(&mut self, cx: &mut Context<Root>) {
        // Collapse only while the source menu is up. A later clear, such as a
        // reload, must not move a caret the menu was not showing.
        let drop_editor = self.source_menu && self.note_view == NoteView::Source;
        self.row_gesture = None;
        self.row_rewrite = None;
        self.source_menu = false;
        self.source_pressing = false;
        if !drop_editor {
            return;
        }
        self.editor.update(cx, |editor, cx| {
            let range = editor.selected_range();
            if range.is_empty() {
                return;
            }
            let at = editor.cursor();
            editor.set_selected_range(at..at, cx);
        });
    }

    /// Hide the source menu while the pointer is down.
    pub(super) fn press_source(&mut self) {
        self.source_pressing = true;
        self.source_menu = false;
        self.row_rewrite = None;
    }

    /// Open or close the source menu from the editor selection.
    ///
    /// Returns true when the menu state changed and the window should redraw.
    pub(super) fn sync_source_menu(&mut self, cx: &App) -> bool {
        let open = self.note_view == NoteView::Source
            && self.open_note.is_some()
            && usable_selection(&self.editor.read(cx).selected_text().to_string()).is_some();
        let changed = self.source_menu != open;
        self.source_menu = open;
        if !open && self.row_rewrite.take().is_some() {
            return true;
        }
        changed
    }

    /// The pointer went up. Show the menu when the editor still has a selection.
    pub(super) fn release_source_press(&mut self, cx: &App) -> bool {
        let was_pressing = self.source_pressing;
        self.source_pressing = false;
        if !was_pressing && !self.source_menu {
            return false;
        }
        self.sync_source_menu(cx) || was_pressing
    }

    pub(super) fn selected_note_text(&self, cx: &App) -> Option<(String, String, usize, usize)> {
        let note_id = self.open_note.as_ref()?.as_str().to_string();
        if self.note_view == NoteView::Source {
            if !self.source_menu {
                return None;
            }
            let editor = self.editor.read(cx);
            let range = editor.selected_range();
            let full = editor.text().to_string();
            let text = editor.selected_text().to_string();
            let text = usable_selection(&text)?;
            let (start_line, end_line) = line_span(&full, range.start, range.end);
            return Some((note_id, text, start_line, end_line));
        }
        let gesture = self.row_gesture.as_ref()?;
        let (source, start_line, end_line) = selected_span(&self.latest, gesture)?;
        Some((note_id, source, start_line, end_line))
    }

    /// Write the selection to the clipboard. The selection stays.
    ///
    /// Returns false when nothing is selected, so the caller can let another
    /// copy handler run.
    pub(super) fn copy_selected_rows(&self, cx: &mut Context<Root>) -> bool {
        let Some((_, source, _, _)) = self.selected_note_text(cx) else {
            return false;
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(source));
        true
    }

    /// Flip the checkbox on preview row `index`.
    ///
    /// Returns true when the body changed and a save should wait.
    pub(super) fn toggle_task(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        let Some(next) = toggled_task_body(&self.latest, index) else {
            return false;
        };
        self.clear_note_rows(cx);
        self.latest = next.clone();
        self.dirty = self.open_note.is_some() && next != self.saved_text;
        self.editor
            .update(cx, |state, cx| state.set_value(next, window, cx));
        self.dirty
    }

    pub(super) fn apply_file(&mut self, text: &str, window: &mut Window, cx: &mut Context<Root>) {
        self.dismiss_media(window, cx);
        let (kind, body) = note_body(text);
        self.open_kind = kind;
        self.saved_text = body.clone();
        self.latest = body.clone();
        self.dirty = false;
        self.editor
            .update(cx, |state, cx| state.set_value(body, window, cx));
    }
}

impl Drop for EditorPane {
    fn drop(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(id) = self.open_note.clone() {
            let file = encode_note(self.open_kind, &self.latest);
            let _ = self.vault.write(&id, &file);
        }
    }
}

pub(super) fn starting_note(index: &VaultIndex, prefs: &AppConfig) -> Option<NoteId> {
    if prefs.reopen_last_note
        && let Some(raw) = prefs.last_note.as_deref()
        && let Ok(id) = NoteId::parse(raw)
        && index.contains(&id)
    {
        return Some(id);
    }
    first_note(index)
}

pub(super) fn first_note(index: &VaultIndex) -> Option<NoteId> {
    let welcome = NoteId::parse("welcome").expect("welcome is one safe path segment");
    if index.contains(&welcome) {
        Some(welcome)
    } else {
        index.ids().next().cloned()
    }
}

pub(super) fn note_body(text: &str) -> (NoteKind, String) {
    let (kind, body) = decode_note(text);
    (kind, body.to_string())
}

pub(super) fn seed_body(kind: NoteKind, name: &str) -> String {
    match kind {
        NoteKind::Book | NoteKind::Speech | NoteKind::Sight | NoteKind::Research => {
            format!("# {name}\n")
        }
        NoteKind::Deed => "- [ ] \n".to_string(),
    }
}

pub(super) fn kind_title(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::Book => locale::text("kind.book"),
        NoteKind::Speech => locale::text("kind.speech"),
        NoteKind::Sight => locale::text("kind.sight"),
        NoteKind::Deed => locale::text("kind.deed"),
        NoteKind::Research => locale::text("kind.research"),
    }
}

pub(super) fn toggled_task_body(source: &str, index: usize) -> Option<String> {
    let rows = split_rows(source);
    let row = rows.get(index)?;
    let slice = source.get(row.start..row.end)?;
    let line_end = slice.find('\n').unwrap_or(slice.len());
    let raw = &slice[..line_end];
    let logical = raw.strip_suffix('\r').unwrap_or(raw);
    let toggled = toggle_task_line(logical)?;
    let mut next = String::with_capacity(source.len());
    next.push_str(&source[..row.start]);
    next.push_str(&toggled);
    next.push_str(&source[row.start + raw.len()..]);
    Some(next)
}

#[cfg(test)]
mod task_tests {
    use super::toggled_task_body;

    #[test]
    fn toggle_flips_one_task_line_and_leaves_the_heading() {
        let source = "# Task list\n\n- [x] Completed task\n- [ ] Incomplete task\n";
        assert!(toggled_task_body(source, 0).is_none());
        let next = toggled_task_body(source, 1).expect("completed task");
        assert_eq!(
            next,
            "# Task list\n\n- [ ] Completed task\n- [ ] Incomplete task\n"
        );
        let next = toggled_task_body(&next, 2).expect("incomplete task");
        assert_eq!(
            next,
            "# Task list\n\n- [ ] Completed task\n- [x] Incomplete task\n"
        );
    }
}
