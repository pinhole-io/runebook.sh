//! Search, create, rename, and delete for notes and folders.

use super::*;

impl EditorPane {
    pub(crate) fn begin_create_note(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.prompt_new_note(NoteKind::Book, window, cx);
    }

    /// Ask for a name, then create a note of `kind`.
    pub(super) fn prompt_new_note(
        &mut self,
        kind: NoteKind,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if new_rune_blocked(kind, self.capture_status().is_some()) {
            self.notice = Some(locale::text("notice.new_rune").to_string());
            return;
        }
        let seed_name = match kind {
            NoteKind::Book => "untitled",
            NoteKind::Speech => "speech",
            NoteKind::Sight => "sight",
            NoteKind::Deed => "deed",
            NoteKind::Research => {
                self.open_research_wizard(window, cx);
                return;
            }
        };
        let seed = child_name(&self.parent_folder(), seed_name);
        open_create_dialog(
            kind_title(kind),
            seed,
            NameAction::CreateNote(kind),
            window,
            cx,
        );
    }

    pub(super) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if self.search.is_some() {
            self.close_search();
            return;
        }
        let input = cx
            .new(|cx| InputState::new(window, cx).placeholder(locale::text("search.placeholder")));
        self.search_watch = Some(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(pane) = this.pane.as_mut()
            {
                pane.refresh_search(cx);
                cx.notify();
            }
        }));
        self.search = Some(input);
        self.search_hits.clear();
        self.search_truncated = false;
        cx.defer_in(window, |root, window, cx| {
            if let Some(pane) = root.pane.as_mut() {
                pane.focus_search(window, cx);
            }
        });
    }

    pub(super) fn close_search(&mut self) {
        self.search = None;
        self.search_hits.clear();
        self.search_truncated = false;
        self.search_watch = None;
    }

    pub(super) fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let Some(input) = &self.search else {
            return;
        };
        input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn refresh_search(&mut self, cx: &mut Context<Root>) {
        let Some(input) = &self.search else {
            return;
        };
        let query = input.read(cx).value().trim().to_string();
        if query.is_empty() {
            self.search_hits.clear();
            self.search_truncated = false;
            return;
        }
        match self.vault.search(&query) {
            Ok(found) => {
                self.search_hits = found.hits;
                self.search_truncated = found.truncated;
                if self.notice.as_deref() == Some(search_notice()) {
                    self.notice = None;
                }
            }
            Err(_) => {
                self.search_hits.clear();
                self.search_truncated = false;
                self.notice = Some(search_notice().to_string());
            }
        }
    }

    pub(super) fn begin_create_folder(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let seed = child_name(&self.parent_folder(), "folder");
        open_create_dialog(
            locale::text("tree.new_folder"),
            seed,
            NameAction::CreateFolder,
            window,
            cx,
        );
    }

    pub(super) fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let (seed, action) = match &self.tree_selection {
            TreeSelection::Note(id) => (id.name().to_string(), NameAction::RenameNote(id.clone())),
            TreeSelection::Folder(id) => {
                (id.name().to_string(), NameAction::RenameFolder(id.clone()))
            }
            TreeSelection::None => {
                self.notice = Some(locale::text("tree.select_first").to_string());
                return;
            }
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(seed));
        self.prompt = Some(Prompt::Rename { input, action });
        self.notice = None;
        cx.defer_in(window, |root, window, cx| {
            if let Some(pane) = root.pane.as_mut() {
                pane.focus_rename(window, cx);
            }
        });
    }

    pub(super) fn focus_rename(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let Some(Prompt::Rename { input, .. }) = &self.prompt else {
            return;
        };
        let input = input.clone();
        input.update(cx, |state, cx| state.select_all(window, cx));
        let handle = input.focus_handle(cx);
        handle.focus(window, cx);
        self.rename_blur = Some(cx.on_blur(&handle, window, |this, window, cx| {
            if let Some(pane) = this.pane.as_mut() {
                pane.submit_prompt(window, cx);
            }
            cx.notify();
        }));
    }

    pub(super) fn begin_delete(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let (title, message, action) = match &self.tree_selection {
            TreeSelection::Note(id) => (
                locale::text("tree.delete_note"),
                locale::format("tree.delete_note_body", &[("id", id.as_str())]),
                ConfirmAction::DeleteNote(id.clone()),
            ),
            TreeSelection::Folder(id) => (
                locale::text("tree.delete_folder"),
                locale::format("tree.delete_folder_body", &[("id", id.as_str())]),
                ConfirmAction::DeleteFolder(id.clone()),
            ),
            TreeSelection::None => {
                self.notice = Some(locale::text("tree.select_first").to_string());
                return;
            }
        };
        open_delete_dialog(title, message, action, window, cx);
    }

    pub(super) fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.rename_blur = None;
        let Some(Prompt::Rename { input, action }) = self.prompt.take() else {
            return;
        };
        let text = input.read(cx).value().to_string();
        let retry = action.clone();
        match self.apply_name(action, &text, window, cx) {
            Ok(()) => self.notice = None,
            Err(err) => {
                self.notice = Some(err);
                self.prompt = Some(Prompt::Rename {
                    input,
                    action: retry,
                });
                self.focus_rename(window, cx);
            }
        }
    }

    pub(super) fn cancel_prompt(&mut self) {
        self.rename_blur = None;
        self.prompt = None;
    }

    pub(super) fn apply_name(
        &mut self,
        action: NameAction,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<(), String> {
        if let NameAction::CreateNote(kind) = &action
            && new_rune_blocked(*kind, self.capture_status().is_some())
        {
            return Err(locale::text("notice.new_rune").to_string());
        }
        self.require_idle()?;
        match action {
            NameAction::CreateNote(kind) => {
                let id = NoteId::parse(text.trim()).map_err(|err| err.to_string())?;
                let body = seed_body(kind, id.name());
                let file = encode_note(kind, &body);
                self.vault
                    .create_note(&id, &file)
                    .map_err(|err| err.to_string())?;
                self.reload().map_err(|err| err.to_string())?;
                self.load_note(id, window, cx)?;
                if kind == NoteKind::Speech {
                    self.start_voice();
                }
                if kind == NoteKind::Sight {
                    cx.defer_in(window, |root, window, cx| {
                        prompt_record_source(root, window, cx);
                    });
                }
            }
            NameAction::CreateFolder => {
                let id = FolderId::parse(text.trim()).map_err(|err| err.to_string())?;
                self.vault
                    .create_folder(&id)
                    .map_err(|err| err.to_string())?;
                self.reload().map_err(|err| err.to_string())?;
                if let Some(parent) = id.parent() {
                    self.reveal(&parent);
                }
                self.tree_selection = TreeSelection::Folder(id);
            }
            NameAction::RenameNote(from) => {
                let stem = text.trim();
                if stem == from.name() {
                    return Ok(());
                }
                let to = from
                    .parent()
                    .join_note(stem)
                    .map_err(|err| err.to_string())?;
                self.vault
                    .rename_note(&from, &to)
                    .map_err(|err| err.to_string())?;
                if self.open_note.as_ref() == Some(&from) {
                    self.open_note = Some(to.clone());
                    self.remember_note(&to);
                }
                self.reload().map_err(|err| err.to_string())?;
                self.reveal(&to.parent());
                self.tree_selection = TreeSelection::Note(to);
            }
            NameAction::RenameFolder(from) => {
                let stem = text.trim();
                if stem == from.name() {
                    return Ok(());
                }
                let Some(parent) = from.parent() else {
                    return Err(locale::text("tree.folder_inside").to_string());
                };
                let to = parent.join_folder(stem).map_err(|err| err.to_string())?;
                self.vault
                    .rename_folder(&from, &to)
                    .map_err(|err| err.to_string())?;
                if let Some(note) = self.open_note.clone()
                    && let Some(next) = rewrite_note_id(&note, &from, &to)
                {
                    self.open_note = Some(next.clone());
                    self.remember_note(&next);
                }
                self.rewrite_collapsed(&from, &to);
                self.reload().map_err(|err| err.to_string())?;
                self.tree_selection = TreeSelection::Folder(to);
            }
        }
        Ok(())
    }

    pub(super) fn apply_confirm(
        &mut self,
        action: ConfirmAction,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<(), String> {
        self.require_idle()?;
        match action {
            ConfirmAction::DeleteNote(id) => {
                self.vault.delete_note(&id).map_err(|err| err.to_string())?;
                self.reload().map_err(|err| err.to_string())?;
                if self.open_note.as_ref() == Some(&id) {
                    self.settle_after_delete(window, cx)?;
                } else {
                    self.tree_selection = TreeSelection::None;
                }
            }
            ConfirmAction::DeleteFolder(id) => {
                self.vault
                    .delete_folder(&id)
                    .map_err(|err| err.to_string())?;
                let inside = self
                    .open_note
                    .as_ref()
                    .is_some_and(|note| note_in_folder(note, &id));
                self.forget_collapsed(&id);
                self.reload().map_err(|err| err.to_string())?;
                if inside {
                    self.settle_after_delete(window, cx)?;
                } else {
                    self.tree_selection = TreeSelection::None;
                }
            }
        }
        Ok(())
    }

    /// Load another note after the active one is gone. An empty vault clears the editor.
    pub(super) fn settle_after_delete(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<(), String> {
        if self.take_next_note(&[], window, cx) {
            return Ok(());
        }
        self.clear_editor(window, cx);
        Ok(())
    }

    /// Load the note `next_open_note` chooses. `false` when no note remains.
    pub(super) fn take_next_note(
        &mut self,
        changes: &[crate::archivist::VaultChange],
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        let Some(id) = next_open_note(None, changes, first_note(&self.index)) else {
            return false;
        };
        if let Err(err) = self.load_note(id, window, cx) {
            self.notice = Some(err);
            self.clear_editor(window, cx);
        }
        true
    }

    pub(super) fn move_note(&mut self, from: &NoteId, dest: &FolderId) {
        if let Err(err) = self.require_idle() {
            self.notice = Some(err);
            return;
        }
        if &from.parent() == dest {
            return;
        }
        let to = match dest.join_note(from.name()) {
            Ok(id) => id,
            Err(err) => {
                self.notice = Some(err.to_string());
                return;
            }
        };
        if let Err(err) = self.vault.rename_note(from, &to) {
            self.notice = Some(err.to_string());
            return;
        }
        if self.open_note.as_ref() == Some(from) {
            self.open_note = Some(to.clone());
            self.remember_note(&to);
        }
        if let Err(err) = self.reload() {
            self.notice = Some(err.to_string());
            return;
        }
        self.reveal(dest);
        self.tree_selection = TreeSelection::Note(to);
        self.notice = None;
    }

    pub(super) fn toggle_folder(&mut self, id: FolderId) {
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
    }

    /// Open `folder` and every folder above it.
    pub(super) fn reveal(&mut self, folder: &FolderId) {
        let mut current = Some(folder.clone());
        while let Some(id) = current {
            self.collapsed.remove(&id);
            current = id.parent();
        }
    }

    pub(super) fn rewrite_collapsed(&mut self, from: &FolderId, to: &FolderId) {
        let previous: Vec<FolderId> = self.collapsed.iter().cloned().collect();
        for id in previous {
            let Some(next) = rewrite_folder_id(&id, from, to) else {
                continue;
            };
            self.collapsed.remove(&id);
            self.collapsed.insert(next);
        }
    }

    pub(super) fn forget_collapsed(&mut self, folder: &FolderId) {
        self.collapsed
            .retain(|id| id != folder && !folder_contains(folder, id));
    }

    pub(super) fn parent_folder(&self) -> FolderId {
        match &self.tree_selection {
            TreeSelection::Folder(id) => id.clone(),
            TreeSelection::Note(id) => id.parent(),
            TreeSelection::None => self
                .open_note
                .as_ref()
                .map(NoteId::parent)
                .unwrap_or_else(FolderId::root),
        }
    }

    pub(super) fn select_tree_folder(&mut self, id: FolderId) {
        self.tree_selection = TreeSelection::Folder(id);
    }
}

pub(super) fn search_notice() -> &'static str {
    locale::text("search.failed")
}

/// Opens the create dialog.
///
/// The dialog builder runs again each time the layer redraws, so the error
/// lives in a cell the confirm callback can fill.
pub(super) fn open_create_dialog(
    title: &'static str,
    seed: String,
    action: NameAction,
    window: &mut Window,
    cx: &mut Context<Root>,
) {
    let view = cx.weak_entity();
    let input = cx.new(|cx| InputState::new(window, cx).default_value(seed));
    let error = Rc::new(RefCell::new(None::<String>));
    let initial_focus = Cell::new(true);
    window.open_alert_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            input.focus_handle(cx).focus(window, cx);
        }
        let message = error.borrow().clone();
        let danger = cx.theme().danger;
        let input_for_ok = input.clone();
        let error_for_ok = error.clone();
        let action = action.clone();
        let view = view.clone();
        dialog
            .title(title)
            .confirm()
            .width(px(420.))
            .footer(dialog_footer(
                locale::text("dialog.cancel"),
                locale::text("dialog.create"),
                ButtonVariant::Primary,
            ))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Input::new(&input).w_full())
                    .when_some(message, |column, message| {
                        column.child(div().text_base().text_color(danger).child(message))
                    }),
            )
            .on_ok(move |_event, window, cx| {
                let text = input_for_ok.read(cx).value().to_string();
                view.update(cx, |root, cx| {
                    let Some(pane) = root.pane.as_mut() else {
                        return true;
                    };
                    match pane.apply_name(action.clone(), &text, window, cx) {
                        Ok(()) => {
                            cx.notify();
                            true
                        }
                        Err(err) => {
                            *error_for_ok.borrow_mut() = Some(err);
                            cx.notify();
                            false
                        }
                    }
                })
                .unwrap_or(true)
            })
    });
}

pub(super) fn open_delete_dialog(
    title: &'static str,
    message: String,
    action: ConfirmAction,
    window: &mut Window,
    cx: &mut Context<Root>,
) {
    let view = cx.weak_entity();
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let action = action.clone();
        let view = view.clone();
        dialog
            .title(title)
            .description(message.clone())
            .confirm()
            .footer(dialog_footer(
                locale::text("dialog.cancel"),
                locale::text("dialog.delete"),
                ButtonVariant::Danger,
            ))
            .on_ok(move |_event, window, cx| {
                view.update(cx, |root, cx| {
                    let Some(pane) = root.pane.as_mut() else {
                        return;
                    };
                    if let Err(err) = pane.apply_confirm(action.clone(), window, cx) {
                        pane.notice = Some(err);
                    }
                })
                .ok();
                true
            })
    });
}

/// The kit dialog buttons use the default cursor. These buttons use the pointer.
pub(super) fn dialog_footer(
    cancel: &'static str,
    ok: &'static str,
    ok_variant: ButtonVariant,
) -> impl IntoElement {
    DialogFooter::new()
        .child(
            Button::new("cancel")
                .label(cancel)
                .cursor_pointer()
                .on_click(|_: &ClickEvent, window, cx| {
                    window.dispatch_action(Box::new(Cancel), cx);
                }),
        )
        .child(
            Button::new("ok")
                .label(ok)
                .with_variant(ok_variant)
                .cursor_pointer()
                .on_click(|_: &ClickEvent, window, cx| {
                    window.dispatch_action(Box::new(Confirm { secondary: false }), cx);
                }),
        )
}

pub(super) fn child_name(parent: &FolderId, name: &str) -> String {
    if parent.is_root() {
        name.to_string()
    } else {
        format!("{}/{name}", parent.as_str())
    }
}
