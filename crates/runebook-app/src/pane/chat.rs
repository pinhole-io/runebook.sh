//! Archivist turns and row rewrite.

use super::*;

impl EditorPane {
    /// The Archivist session drawn in the dock panel.
    pub(crate) fn archivist(&self) -> &crate::archivist::Panel {
        &self.archivist
    }

    /// Drop the Archivist transcript. The next send starts a new conversation.
    pub(crate) fn clear_archivist(&mut self) {
        self.archivist.clear_conversation();
    }

    /// Send the draft to the Archivist. A busy turn or a missing key does nothing.
    ///
    /// Held rows are included even when the draft is empty.
    pub(crate) fn send_archivist(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if self.archivist.is_busy() || !self.archivist_ready() {
            return;
        }
        if let Err(err) = self.require_idle() {
            self.notice = Some(err);
            return;
        }
        let draft = self.archivist.draft_text(cx);
        if draft.is_empty() && !self.archivist.has_held() && !self.archivist.has_files() {
            return;
        }
        if !self.flush_edits(cx) {
            return;
        }
        let Some(access) = self.archivist_access() else {
            return;
        };
        let vault = self.vault.clone();
        let epoch = self.epoch;
        if self.archivist.has_files() {
            let held = self.archivist.held_pairs();
            let pairs: Vec<(&str, &str)> = held
                .iter()
                .map(|(note_id, source)| (note_id.as_str(), source.as_str()))
                .collect();
            let instruction = context_message(pairs, &draft).unwrap_or_default();
            let paths = self.archivist.file_paths();
            if !self.archivist.begin_read() {
                return;
            }
            self.archivist.launch_files(
                crate::archivist::FileLaunch {
                    instruction,
                    draft,
                    held,
                    paths,
                },
                vault,
                epoch,
                access,
                cx,
            );
            return;
        }
        let held = self.archivist.take_held();
        let pairs: Vec<(&str, &str)> = held
            .iter()
            .map(|(note_id, source, _)| (note_id.as_str(), source.as_str()))
            .collect();
        let Some(text) = context_message(pairs, &draft) else {
            for (note_id, source, label) in held {
                self.archivist.hold_rows(note_id, source, label);
            }
            return;
        };
        self.archivist.clear_draft(window, cx);
        self.archivist.launch(text, vault, epoch, access, cx);
    }

    /// Open the file picker and park the chosen files.
    pub(crate) fn pick_archivist_files(&mut self, cx: &mut Context<Root>) {
        if self.archivist.is_busy() {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(SharedString::from(locale::text("archivist.add_file"))),
        });
        cx.spawn(async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => Some(paths),
                Ok(Err(err)) => {
                    this.update(cx, |root, cx| {
                        if let Some(pane) = root.pane.as_mut() {
                            pane.archivist.note_error(err.to_string());
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                _ => None,
            };
            let Some(paths) = picked else {
                return;
            };
            this.update(cx, |root, cx| {
                if let Some(pane) = root.pane.as_mut()
                    && let Err(err) = pane.archivist.hold_files(paths)
                {
                    pane.archivist.note_error(err);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn dismiss_archivist_file(&mut self, index: usize) {
        self.archivist.dismiss_file(index);
    }

    pub(crate) fn cancel_archivist_read(&mut self) {
        self.archivist.cancel_read();
    }

    pub(crate) fn fail_archivist_read(&mut self, error: String) {
        self.archivist.fail_read(error);
    }

    /// The file read succeeded. Drop the sent files and start the model turn.
    ///
    /// Clear stops a read before this runs, so that send does not start.
    pub(crate) fn accept_archivist_files(
        &mut self,
        read: crate::archivist::AcceptedRead<'_>,
        vault: Vault,
        epoch: u64,
        access: crate::archivist::ModelAccess,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if !self.archivist.is_reading() {
            return;
        }
        self.archivist.drop_sent_held(read.held);
        self.archivist.drop_sent_files(read.paths);
        self.archivist.clear_draft_if(read.draft, window, cx);
        self.archivist
            .launch_message(read.display, read.message, vault, epoch, access, cx);
    }

    /// Park the selection in the Archivist column. Send delivers it.
    pub(super) fn add_selected_rows_to_chat(&mut self, cx: &mut Context<Root>) {
        let Some((note_id, source, start_line, end_line)) = self.selected_note_text(cx) else {
            return;
        };
        let label = selection_label(&note_id, start_line, end_line);
        self.archivist.hold_rows(note_id, source, label);
        self.clear_note_rows(cx);
    }

    /// Open the rewrite prompt under the selection menu.
    pub(super) fn begin_row_rewrite(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if self.selected_note_text(cx).is_none() {
            return;
        }
        if self.row_rewrite.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder(locale::text("preview.rewrite_placeholder"))
            });
            input.focus_handle(cx).focus(window, cx);
            self.row_rewrite = Some(input);
        } else if let Some(input) = &self.row_rewrite {
            input.focus_handle(cx).focus(window, cx);
        }
    }

    pub(crate) fn dismiss_held_rows(&mut self, index: usize) {
        self.archivist.dismiss_held(index);
    }

    /// Ask the Archivist to rewrite the selected rows.
    pub(super) fn apply_row_rewrite(&mut self, _window: &mut Window, cx: &mut Context<Root>) {
        let Some(input) = self.row_rewrite.clone() else {
            return;
        };
        let change = input.read(cx).value().to_string();
        self.rewrite_selection(&change, cx);
    }

    /// Ask the Archivist to rewrite the selected rows with `change`.
    pub(super) fn rewrite_selection(&mut self, change: &str, cx: &mut Context<Root>) {
        let change = change.trim();
        if change.is_empty() {
            return;
        }
        let Some((note_id, source, _, _)) = self.selected_note_text(cx) else {
            return;
        };
        if !self.archivist_ready() {
            self.notice = Some(self.archivist_key_message().to_string());
            return;
        }
        if self.archivist.is_busy() {
            self.notice = Some(locale::text("notice.archivist_busy").to_string());
            return;
        }
        if let Err(err) = self.require_idle() {
            self.notice = Some(err);
            return;
        }
        if !self.flush_edits(cx) {
            return;
        }
        let Some(message) = rewrite_message(&note_id, &source, change) else {
            return;
        };
        self.archivist.drop_held_note(&note_id);
        self.clear_note_rows(cx);
        let Some(access) = self.archivist_access() else {
            return;
        };
        let vault = self.vault.clone();
        let epoch = self.epoch;
        self.archivist.launch(message, vault, epoch, access, cx);
    }

    pub(crate) fn same_epoch(&self, epoch: u64) -> bool {
        self.epoch == epoch
    }

    pub(crate) fn show_archivist_live(
        &mut self,
        turn: u64,
        live: crate::archivist::LiveText,
    ) -> bool {
        self.archivist.show_live(turn, live)
    }

    pub(crate) fn finish_archivist(
        &mut self,
        turn: u64,
        done: crate::archivist::Turn,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let changes = self.archivist.finish(turn, done);
        self.apply_archivist_changes(&changes, window, cx);
    }

    pub(super) fn apply_archivist_changes(
        &mut self,
        changes: &[crate::archivist::VaultChange],
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        use crate::archivist::VaultChange;
        for change in changes {
            match change {
                VaultChange::RenamedFolder { from, to } => self.rewrite_collapsed(from, to),
                VaultChange::DeletedFolder(id) => self.forget_collapsed(id),
                VaultChange::Wrote(id) => self.reveal(&id.parent()),
                VaultChange::RenamedNote { to, .. } => self.reveal(&to.parent()),
                VaultChange::Folder(id) => {
                    if let Some(parent) = id.parent() {
                        self.reveal(&parent);
                    }
                }
                VaultChange::DeletedNote(_) | VaultChange::Opened(_) => {}
            }
        }
        self.tree_selection = match &self.tree_selection {
            TreeSelection::Note(id) => match follow_note(id.clone(), changes) {
                Some(id) => TreeSelection::Note(id),
                None => TreeSelection::None,
            },
            TreeSelection::Folder(id) => match follow_folder(id.clone(), changes) {
                Some(id) => TreeSelection::Folder(id),
                None => TreeSelection::None,
            },
            TreeSelection::None => TreeSelection::None,
        };
        let opened = last_opened(changes);
        let (open, refresh) = follow_open(self.open_note.clone(), changes);
        if let Err(err) = self.reload() {
            self.open_note = open;
            if self.open_note.is_none() {
                self.dirty = false;
            }
            self.notice = Some(err.to_string());
            return;
        }
        if let Some(id) = opened {
            if !self.flush_edits(cx) {
                return;
            }
            if let Err(err) = self.load_note(id, window, cx) {
                self.notice = Some(err);
            }
            return;
        }
        let Some(id) = open else {
            if self.open_note.is_some() && self.capture_status().is_some() {
                self.notice = Some(locale::text("notice.archivist_deleted").to_string());
                return;
            }
            if !self.take_next_note(changes, window, cx) && self.open_note.is_some() {
                self.clear_editor(window, cx);
            }
            return;
        };
        if !refresh {
            self.open_note = Some(id);
            return;
        }
        if self.capture_status().is_some() {
            self.open_note = Some(id);
            self.notice = Some(locale::text("notice.archivist_changed").to_string());
            return;
        }
        if let Err(err) = self.reload_open_note(id, window, cx) {
            self.notice = Some(err);
        }
    }

    pub(super) fn reload_open_note(
        &mut self,
        id: NoteId,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<(), String> {
        let text = self.vault.read(&id).map_err(|err| err.to_string())?;
        self.clear_note_rows(cx);
        self.reveal(&id.parent());
        self.open_note = Some(id.clone());
        if matches!(
            self.tree_selection,
            TreeSelection::Note(_) | TreeSelection::None
        ) {
            self.tree_selection = TreeSelection::Note(id);
        }
        self.apply_file(&text, window, cx);
        Ok(())
    }
}

pub(super) fn follow_open(
    open: Option<NoteId>,
    changes: &[crate::archivist::VaultChange],
) -> (Option<NoteId>, bool) {
    use crate::archivist::VaultChange;
    let Some(mut current) = open else {
        return (None, false);
    };
    let mut refresh = false;
    for change in changes {
        match change {
            VaultChange::Wrote(id) if &current == id => refresh = true,
            VaultChange::RenamedNote { from, to } if &current == from => {
                current = to.clone();
                refresh = true;
            }
            VaultChange::DeletedNote(id) if &current == id => return (None, false),
            VaultChange::RenamedFolder { from, to } => {
                if let Some(next) = rewrite_note_id(&current, from, to) {
                    current = next;
                    refresh = true;
                }
            }
            VaultChange::DeletedFolder(id) if note_in_folder(&current, id) => return (None, false),
            _ => {}
        }
    }
    (Some(current), refresh)
}

pub(super) fn follow_note(id: NoteId, changes: &[crate::archivist::VaultChange]) -> Option<NoteId> {
    follow_open(Some(id), changes).0
}

pub(super) fn last_opened(changes: &[crate::archivist::VaultChange]) -> Option<NoteId> {
    use crate::archivist::VaultChange;
    let id = changes.iter().rev().find_map(|change| match change {
        VaultChange::Opened(id) => Some(id.clone()),
        _ => None,
    })?;
    follow_note(id, changes)
}

pub(super) fn last_wrote(changes: &[crate::archivist::VaultChange]) -> Option<NoteId> {
    use crate::archivist::VaultChange;
    let id = changes.iter().rev().find_map(|change| match change {
        VaultChange::Wrote(id) => Some(id.clone()),
        _ => None,
    })?;
    follow_note(id, changes)
}

/// The note that should be active after `changes`.
///
/// An open request wins. The current note stays when the changes still track
/// it. A write is next. `fallback` covers a delete that leaves other notes.
pub(super) fn next_open_note(
    open: Option<NoteId>,
    changes: &[crate::archivist::VaultChange],
    fallback: Option<NoteId>,
) -> Option<NoteId> {
    if let Some(id) = last_opened(changes) {
        return Some(id);
    }
    follow_open(open, changes)
        .0
        .or_else(|| last_wrote(changes))
        .or(fallback)
}

pub(super) fn follow_folder(
    mut id: FolderId,
    changes: &[crate::archivist::VaultChange],
) -> Option<FolderId> {
    use crate::archivist::VaultChange;
    for change in changes {
        match change {
            VaultChange::RenamedFolder { from, to }
                if &id == from || folder_contains(from, &id) =>
            {
                id = rewrite_folder_id(&id, from, to)?;
            }
            VaultChange::DeletedFolder(folder) if &id == folder || folder_contains(folder, &id) => {
                return None;
            }
            _ => {}
        }
    }
    Some(id)
}

#[cfg(test)]
mod next_open_tests {
    use super::next_open_note;
    use crate::archivist::VaultChange;
    use runebook::NoteId;

    fn note(id: &str) -> NoteId {
        NoteId::parse(id).expect("test note id")
    }

    #[test]
    fn next_open_note_keeps_the_current_note() {
        let current = note("alpha");
        assert_eq!(
            next_open_note(Some(current.clone()), &[], Some(note("welcome"))),
            Some(current)
        );
    }

    #[test]
    fn next_open_note_switches_on_opened() {
        let opened = note("beta");
        assert_eq!(
            next_open_note(
                Some(note("alpha")),
                &[VaultChange::Opened(opened.clone())],
                Some(note("welcome")),
            ),
            Some(opened)
        );
    }

    #[test]
    fn next_open_note_adopts_a_write_when_nothing_is_open() {
        let wrote = note("beta");
        assert_eq!(
            next_open_note(None, &[VaultChange::Wrote(wrote.clone())], None),
            Some(wrote)
        );
    }

    #[test]
    fn next_open_note_falls_back_after_a_delete() {
        let current = note("alpha");
        let fallback = note("beta");
        assert_eq!(
            next_open_note(
                Some(current.clone()),
                &[VaultChange::DeletedNote(current)],
                Some(fallback.clone()),
            ),
            Some(fallback)
        );
    }

    #[test]
    fn next_open_note_is_none_when_the_index_is_empty() {
        let current = note("alpha");
        assert_eq!(
            next_open_note(
                Some(current.clone()),
                &[VaultChange::DeletedNote(current)],
                None,
            ),
            None
        );
    }
}
