//! Vault workspace: file tree, source editor, markdown preview, and row selection.

mod preview;
mod rows;
mod view;

use rows::{context_message, rewrite_message, selected_source};

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants};
use gpui_kit::component::dialog::{Cancel, Confirm, DialogFooter};
use gpui_kit::component::input::{EditorState, Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, IndexPath, WindowExt};
use gpui_kit::prelude::*;
use gpui_kit::{AppContext, ClickEvent, Context, Entity, Focusable, Subscription, Window, div, px};
use runebook::{
    AppConfig, Error, FolderId, NoteId, NoteViewChoice, ThemeChoice, Vault, VaultIndex, VaultTree,
};

use crate::Root;
use crate::capture::{
    MISSING_GROQ_KEY, ScreenCapture, ScreenTarget, VoiceCapture, WINDOW_LIST_ERROR, append_block,
    groq_key_is_set, list_input_devices, markdown_link, utc_stamp_now, voice_to_text,
};
use crate::save_setting;

pub(super) use view::screen;

#[derive(Clone, PartialEq, Eq)]
enum TreeSelection {
    None,
    Note(NoteId),
    Folder(FolderId),
}

/// Which note surface is on screen. A new window starts on the preview.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NoteView {
    Preview,
    Source,
}

enum Prompt {
    Rename {
        input: Entity<InputState>,
        action: NameAction,
    },
}

#[derive(Clone)]
enum NameAction {
    CreateNote,
    CreateFolder,
    RenameNote(NoteId),
    RenameFolder(FolderId),
}

#[derive(Clone)]
enum ConfirmAction {
    DeleteNote(NoteId),
    DeleteFolder(FolderId),
}

pub(super) struct EditorPane {
    vault: Vault,
    tree: VaultTree,
    index: VaultIndex,
    open_note: Option<NoteId>,
    tree_selection: TreeSelection,
    collapsed: HashSet<FolderId>,
    note_view: NoteView,
    default_note_view: NoteView,
    settings_open: bool,
    microphone: Option<String>,
    microphone_select: Option<Entity<SelectState<SearchableVec<String>>>>,
    microphone_watch: Option<Subscription>,
    mic_load: u64,
    theme: ThemeChoice,
    editor: Entity<EditorState>,
    saved_text: String,
    latest: String,
    row_gesture: Option<rows::RowGesture>,
    row_rewrite: Option<Entity<InputState>>,
    dirty: bool,
    notice: Option<String>,
    capture: Capture,
    prompt: Option<Prompt>,
    rename_blur: Option<Subscription>,
    _subscription: Subscription,
    archivist: crate::archivist::Panel,
    epoch: u64,
}

enum Capture {
    Idle,
    Voice(VoiceCapture),
    Screen(ScreenCapture),
    Transcribing,
    SavingScreen,
}

impl EditorPane {
    pub(super) fn open(
        vault: Vault,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<Self, Error> {
        let tree = vault.tree()?;
        let index = vault.notes()?;
        let prefs = AppConfig::load().unwrap_or_default();
        let default_note_view = note_view_from(prefs.note_view);
        let open_note = first_note(&index);
        let text = match &open_note {
            Some(id) => vault.read(id)?,
            None => String::new(),
        };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .line_number(true)
                .soft_wrap(true)
                .default_value(text.clone())
        });
        let subscription = cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.on_edit(cx);
            }
        });
        let pane = Self {
            tree_selection: open_note
                .clone()
                .map(TreeSelection::Note)
                .unwrap_or(TreeSelection::None),
            collapsed: HashSet::new(),
            row_gesture: None,
            row_rewrite: None,
            saved_text: text.clone(),
            latest: text,
            open_note,
            vault,
            tree,
            index,
            note_view: default_note_view,
            default_note_view,
            settings_open: false,
            microphone: saved_microphone(prefs.microphone),
            microphone_select: None,
            microphone_watch: None,
            mic_load: 0,
            theme: prefs.theme,
            editor,
            dirty: false,
            notice: None,
            capture: Capture::Idle,
            prompt: None,
            rename_blur: None,
            _subscription: subscription,
            archivist: crate::archivist::Panel::new(window, cx),
            epoch: next_epoch(),
        };
        if pane.note_view == NoteView::Source {
            pane.editor.focus_handle(cx).focus(window, cx);
        }
        Ok(pane)
    }

    /// Swap the rendered note and the source editor.
    ///
    /// The editor entity stays alive in both modes. Focusing it on the way to
    /// the source lets typing start on the next frame.
    pub(super) fn toggle_note_view(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.note_view = match self.note_view {
            NoteView::Preview => NoteView::Source,
            NoteView::Source => NoteView::Preview,
        };
        if self.note_view == NoteView::Source {
            self.editor.focus_handle(cx).focus(window, cx);
        }
    }

    /// Open or close settings in the note area.
    ///
    /// Opening settings clears the microphone list and loads it again.
    fn toggle_settings(&mut self, cx: &mut Context<Root>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.begin_microphone_load(cx);
        }
    }

    fn begin_microphone_load(&mut self, cx: &mut Context<Root>) {
        self.microphone_select = None;
        self.microphone_watch = None;
        self.mic_load = self.mic_load.wrapping_add(1);
        let load = self.mic_load;
        cx.spawn(async move |this, cx| {
            let names = cx
                .background_executor()
                .spawn(async { list_input_devices() })
                .await;
            this.update_in(cx, |root, window, cx| {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                if pane.mic_load != load || !pane.settings_open {
                    return;
                }
                pane.show_microphones(names, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_microphones(
        &mut self,
        mut names: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if let Some(saved) = self.microphone.clone()
            && !names.iter().any(|item| item == &saved)
        {
            names.push(saved);
        }
        let selected_label = self
            .microphone
            .clone()
            .unwrap_or_else(|| SYSTEM_MICROPHONE.to_string());
        let mut items = Vec::with_capacity(names.len() + 1);
        items.push(SYSTEM_MICROPHONE.to_string());
        items.extend(names);
        let selected = items
            .iter()
            .position(|item| item == &selected_label)
            .unwrap_or(0);
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(items),
                Some(IndexPath::new(selected)),
                window,
                cx,
            )
        });
        self.microphone_watch = Some(cx.subscribe(
            &select,
            |root, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                let SelectEvent::Confirm(value) = event;
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                pane.apply_microphone(value.clone());
                cx.notify();
            },
        ));
        self.microphone_select = Some(select);
    }

    fn apply_microphone(&mut self, value: Option<String>) {
        let name = match value.as_deref() {
            Some(SYSTEM_MICROPHONE) | None => None,
            Some(name) => Some(name.to_string()),
        };
        self.microphone = name.clone();
        self.store_setting(|config| config.microphone = name);
    }

    /// Save the window theme. The caller already applied it to the window.
    fn choose_theme(&mut self, choice: ThemeChoice) {
        self.theme = choice;
        self.store_setting(|config| config.theme = choice);
    }

    /// Save the default note view and show that view on the open note.
    fn choose_note_view(&mut self, view: NoteView, window: &mut Window, cx: &mut Context<Root>) {
        self.default_note_view = view;
        self.show_note_view(view, window, cx);
        let choice = note_view_choice(view);
        self.store_setting(|config| config.note_view = choice);
    }

    fn show_note_view(&mut self, view: NoteView, window: &mut Window, cx: &mut Context<Root>) {
        self.note_view = view;
        if view == NoteView::Source {
            self.editor.focus_handle(cx).focus(window, cx);
        }
    }

    fn store_setting(&mut self, change: impl FnOnce(&mut AppConfig)) {
        if let Err(err) = save_setting(change) {
            self.notice = Some(err);
        }
    }

    pub(super) fn root(&self) -> &Path {
        self.vault.root()
    }

    /// Resolve a media link in the open note.
    pub(super) fn media_path(&self, href: &str) -> Result<std::path::PathBuf, String> {
        let Some(id) = self.open_note.as_ref() else {
            return Err("Open a note first".to_string());
        };
        self.vault
            .media_file(id, href)
            .map_err(|err| err.to_string())
    }

    pub(super) fn index(&self) -> &VaultIndex {
        &self.index
    }

    pub(super) fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    pub(super) fn capture_status(&self) -> Option<&'static str> {
        match self.capture {
            Capture::Idle => None,
            Capture::Voice(_) | Capture::Screen(_) | Capture::SavingScreen => Some("Recording"),
            Capture::Transcribing => Some("Transcribing"),
        }
    }

    pub(super) fn voice_recording(&self) -> bool {
        matches!(self.capture, Capture::Voice(_))
    }

    pub(super) fn screen_recording(&self) -> bool {
        matches!(self.capture, Capture::Screen(_))
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
    /// The video button starts a recording from its source menu.
    pub(super) fn toggle_screen(&mut self, cx: &mut Context<Root>) {
        if self.screen_recording() {
            self.stop_screen(cx);
        }
    }

    fn note_window_list(&mut self, failed: bool) {
        if failed {
            self.notice = Some(WINDOW_LIST_ERROR.to_string());
        } else if self.notice.as_deref() == Some(WINDOW_LIST_ERROR) {
            self.notice = None;
        }
    }

    fn start_voice(&mut self) {
        if self.open_note.is_none() {
            self.notice = Some("Open a note first".to_string());
            return;
        }
        if !groq_key_is_set() {
            self.notice = Some(MISSING_GROQ_KEY.to_string());
            return;
        }
        match VoiceCapture::start(self.microphone.clone()) {
            Ok(capture) => {
                self.capture = Capture::Voice(capture);
                self.notice = None;
            }
            Err(err) => self.notice = Some(err),
        }
    }

    fn stop_voice(&mut self, cx: &mut Context<Root>) {
        let capture = match std::mem::replace(&mut self.capture, Capture::Idle) {
            Capture::Voice(capture) => capture,
            other => {
                self.capture = other;
                return;
            }
        };
        self.capture = Capture::Transcribing;
        let note_id = self.open_note.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { voice_to_text(capture) })
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

    fn start_screen(&mut self, target: ScreenTarget) {
        if self.capture_status().is_some() {
            return;
        }
        let Some(id) = self.open_note.clone() else {
            self.notice = Some("Open a note first".to_string());
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
                self.capture = Capture::Screen(capture);
                self.notice = None;
            }
            Err(err) => self.notice = Some(err),
        }
    }

    fn stop_screen(&mut self, cx: &mut Context<Root>) {
        let capture = match std::mem::replace(&mut self.capture, Capture::Idle) {
            Capture::Screen(capture) => capture,
            other => {
                self.capture = other;
                return;
            }
        };
        let file_name = capture.file_name.clone();
        self.capture = Capture::SavingScreen;
        let note_id = self.open_note.clone();
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

    fn finish_voice(
        &mut self,
        note_id: Option<NoteId>,
        outcome: Result<crate::capture::VoiceOutcome, String>,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        self.capture = Capture::Idle;
        let outcome = match outcome {
            Ok(outcome) if outcome.text.is_empty() => {
                self.notice = Some("The recording had no speech".to_string());
                return false;
            }
            Ok(outcome) => outcome,
            Err(err) => {
                self.notice = Some(err);
                return false;
            }
        };
        if !self.same_open_note(note_id.as_ref()) {
            self.notice = Some("The note changed before the transcript was ready".to_string());
            return false;
        }
        self.insert_block(&outcome.text, window, cx);
        if outcome.truncated {
            self.notice =
                Some("The recording was shortened to fit the transcription limit".to_string());
        }
        self.dirty
    }

    fn finish_screen(
        &mut self,
        note_id: Option<NoteId>,
        file_name: String,
        outcome: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        self.capture = Capture::Idle;
        if let Err(err) = outcome {
            self.notice = Some(err);
            return false;
        }
        if !self.same_open_note(note_id.as_ref()) {
            self.notice = Some("The note changed before the recording was saved".to_string());
            return false;
        }
        let link = markdown_link("screen recording", &file_name);
        self.insert_block(&link, window, cx);
        self.dirty
    }

    fn same_open_note(&self, note_id: Option<&NoteId>) -> bool {
        match (self.open_note.as_ref(), note_id) {
            (Some(open), Some(id)) => open == id,
            _ => false,
        }
    }

    fn insert_block(&mut self, block: &str, window: &mut Window, cx: &mut Context<Root>) {
        let next = append_block(&self.latest, block);
        self.clear_note_rows();
        self.latest = next.clone();
        self.dirty = self.open_note.is_some() && next != self.saved_text;
        self.notice = None;
        self.editor
            .update(cx, |state, cx| state.set_value(next, window, cx));
    }

    fn next_recording(&self, id: &NoteId) -> Result<(std::path::PathBuf, String), String> {
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
                .ok_or_else(|| "The recording name is not valid text".to_string())?
                .to_string();
            return Ok((path, name));
        }
        Err("Could not choose a recording name".to_string())
    }

    fn require_idle(&self) -> Result<(), String> {
        if self.capture_status().is_none() {
            Ok(())
        } else {
            Err("Stop the recording before you change notes".to_string())
        }
    }

    /// Copy the editor buffer into the preview. Returns true when a save should wait.
    pub(super) fn capture_edit(&mut self, cx: &mut Context<Root>) -> bool {
        let value = self.editor.read(cx).value().to_string();
        if value != self.latest {
            self.clear_note_rows();
        }
        self.latest = value.clone();
        self.dirty = self.open_note.is_some() && value != self.saved_text;
        self.dirty
    }

    /// Write the open note when the buffer changed.
    ///
    /// Returns false when the write failed. The caller should stay on this note.
    pub(super) fn flush_edits(&mut self, cx: &mut Context<Root>) -> bool {
        let Some(id) = self.open_note.clone() else {
            self.dirty = false;
            return true;
        };
        let value = self.editor.read(cx).value().to_string();
        if value != self.latest {
            self.clear_note_rows();
        }
        self.latest = value.clone();
        if value == self.saved_text {
            self.dirty = false;
            return true;
        }
        match self.vault.write(&id, &value) {
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

    pub(super) fn load_note(
        &mut self,
        id: NoteId,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<(), String> {
        self.require_idle()?;
        let text = self.vault.read(&id).map_err(|err| err.to_string())?;
        self.clear_note_rows();
        self.reveal(&id.parent());
        self.open_note = Some(id.clone());
        self.tree_selection = TreeSelection::Note(id);
        self.saved_text = text.clone();
        self.latest = text.clone();
        self.dirty = false;
        self.notice = None;
        self.settings_open = false;
        self.editor
            .update(cx, |state, cx| state.set_value(text, window, cx));
        self.show_note_view(self.default_note_view, window, cx);
        Ok(())
    }

    pub(super) fn create_and_open(
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

    pub(super) fn begin_create_note(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let seed = child_name(&self.parent_folder(), "untitled");
        open_create_dialog("New note", seed, NameAction::CreateNote, window, cx);
    }

    fn begin_create_folder(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let seed = child_name(&self.parent_folder(), "folder");
        open_create_dialog("New folder", seed, NameAction::CreateFolder, window, cx);
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let (seed, action) = match &self.tree_selection {
            TreeSelection::Note(id) => (id.name().to_string(), NameAction::RenameNote(id.clone())),
            TreeSelection::Folder(id) => {
                (id.name().to_string(), NameAction::RenameFolder(id.clone()))
            }
            TreeSelection::None => {
                self.notice = Some("Select a note or folder first".to_string());
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

    fn focus_rename(&mut self, window: &mut Window, cx: &mut Context<Root>) {
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

    fn begin_delete(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let (title, message, action) = match &self.tree_selection {
            TreeSelection::Note(id) => (
                "Delete note",
                format!("Delete note {id}? This removes the file."),
                ConfirmAction::DeleteNote(id.clone()),
            ),
            TreeSelection::Folder(id) => (
                "Delete folder",
                format!("Delete folder {id}? This removes the notes inside it."),
                ConfirmAction::DeleteFolder(id.clone()),
            ),
            TreeSelection::None => {
                self.notice = Some("Select a note or folder first".to_string());
                return;
            }
        };
        open_delete_dialog(title, message, action, window, cx);
    }

    fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Root>) {
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

    fn cancel_prompt(&mut self) {
        self.rename_blur = None;
        self.prompt = None;
    }

    fn apply_name(
        &mut self,
        action: NameAction,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<(), String> {
        self.require_idle()?;
        match action {
            NameAction::CreateNote => {
                let id = NoteId::parse(text.trim()).map_err(|err| err.to_string())?;
                let body = format!("# {}\n", id.name());
                self.vault
                    .create_note(&id, &body)
                    .map_err(|err| err.to_string())?;
                self.reload().map_err(|err| err.to_string())?;
                self.load_note(id, window, cx)?;
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
                    return Err("Select a folder inside the vault".to_string());
                };
                let to = parent.join_folder(stem).map_err(|err| err.to_string())?;
                self.vault
                    .rename_folder(&from, &to)
                    .map_err(|err| err.to_string())?;
                if let Some(note) = self.open_note.clone()
                    && let Some(next) = rewrite_note_id(&note, &from, &to)
                {
                    self.open_note = Some(next);
                }
                self.rewrite_collapsed(&from, &to);
                self.reload().map_err(|err| err.to_string())?;
                self.tree_selection = TreeSelection::Folder(to);
            }
        }
        Ok(())
    }

    fn apply_confirm(
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
                    self.clear_editor(window, cx);
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
                    self.clear_editor(window, cx);
                } else {
                    self.tree_selection = TreeSelection::None;
                }
            }
        }
        Ok(())
    }

    fn clear_editor(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.open_note = None;
        self.tree_selection = TreeSelection::None;
        self.clear_note_rows();
        self.saved_text.clear();
        self.latest.clear();
        self.dirty = false;
        self.editor
            .update(cx, |state, cx| state.set_value("", window, cx));
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
        }
        if let Err(err) = self.reload() {
            self.notice = Some(err.to_string());
            return;
        }
        self.reveal(dest);
        self.tree_selection = TreeSelection::Note(to);
        self.notice = None;
    }

    fn toggle_folder(&mut self, id: FolderId) {
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
    }

    /// Open `folder` and every folder above it.
    fn reveal(&mut self, folder: &FolderId) {
        let mut current = Some(folder.clone());
        while let Some(id) = current {
            self.collapsed.remove(&id);
            current = id.parent();
        }
    }

    fn rewrite_collapsed(&mut self, from: &FolderId, to: &FolderId) {
        let previous: Vec<FolderId> = self.collapsed.iter().cloned().collect();
        for id in previous {
            let Some(next) = rewrite_folder_id(&id, from, to) else {
                continue;
            };
            self.collapsed.remove(&id);
            self.collapsed.insert(next);
        }
    }

    fn forget_collapsed(&mut self, folder: &FolderId) {
        self.collapsed
            .retain(|id| id != folder && !folder_contains(folder, id));
    }

    fn reload(&mut self) -> Result<(), Error> {
        self.tree = self.vault.tree()?;
        self.index = self.vault.notes()?;
        Ok(())
    }

    /// Send the draft to the Archivist. A busy turn or a missing key does nothing.
    ///
    /// Held rows are included even when the draft is empty.
    pub(crate) fn send_archivist(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if !self.archivist.is_open() || self.archivist.is_busy() || !groq_key_is_set() {
            return;
        }
        if let Err(err) = self.require_idle() {
            self.notice = Some(err);
            return;
        }
        let draft = self.archivist.draft_text(cx);
        if draft.is_empty() && !self.archivist.has_held() {
            return;
        }
        if !self.flush_edits(cx) {
            return;
        }
        let held = self.archivist.take_held();
        let pairs: Vec<(&str, &str)> = held
            .iter()
            .map(|(note_id, source)| (note_id.as_str(), source.as_str()))
            .collect();
        let Some(text) = context_message(pairs, &draft) else {
            for (note_id, source) in held {
                self.archivist.hold_rows(note_id, source);
            }
            return;
        };
        self.archivist.clear_draft(window, cx);
        let vault = self.vault.clone();
        let epoch = self.epoch;
        self.archivist.launch(text, vault, epoch, cx);
    }

    fn press_note_row(&mut self, index: usize, shift: bool) {
        self.row_rewrite = None;
        self.row_gesture = Some(rows::RowGesture::press(
            self.row_gesture.take(),
            index,
            shift,
        ));
    }

    fn drag_note_row(&mut self, index: usize) -> bool {
        self.row_gesture
            .as_mut()
            .is_some_and(|gesture| gesture.drag_over(index))
    }

    fn release_note_row(&mut self) {
        if let Some(gesture) = self.row_gesture.as_mut() {
            gesture.release();
        }
    }

    fn clear_note_rows(&mut self) {
        self.row_gesture = None;
        self.row_rewrite = None;
    }

    fn selected_note_text(&self) -> Option<(String, String)> {
        let note_id = self.open_note.as_ref()?.as_str().to_string();
        let gesture = self.row_gesture.as_ref()?;
        let source = selected_source(&self.latest, gesture)?;
        Some((note_id, source))
    }

    /// Park the selection in the Archivist column. Send delivers it.
    fn add_selected_rows_to_chat(&mut self) {
        let Some((note_id, source)) = self.selected_note_text() else {
            return;
        };
        self.archivist.show();
        self.archivist.hold_rows(note_id, source);
        self.clear_note_rows();
    }

    /// Open the rewrite prompt under the row menu.
    fn begin_row_rewrite(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if self.selected_note_text().is_none() {
            return;
        }
        if self.row_rewrite.is_none() {
            let input = cx
                .new(|cx| InputState::new(window, cx).placeholder("How should these rows change?"));
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
    fn apply_row_rewrite(&mut self, _window: &mut Window, cx: &mut Context<Root>) {
        let Some(input) = self.row_rewrite.clone() else {
            return;
        };
        let change = input.read(cx).value().trim().to_string();
        if change.is_empty() {
            return;
        }
        let Some((note_id, source)) = self.selected_note_text() else {
            return;
        };
        if !groq_key_is_set() {
            self.archivist.show();
            self.notice = Some(crate::archivist::MISSING_KEY.to_string());
            return;
        }
        if self.archivist.is_busy() {
            self.notice = Some("The Archivist is still working.".to_string());
            return;
        }
        if let Err(err) = self.require_idle() {
            self.notice = Some(err);
            return;
        }
        if !self.flush_edits(cx) {
            return;
        }
        let Some(message) = rewrite_message(&note_id, &source, &change) else {
            return;
        };
        self.archivist.drop_held_note(&note_id);
        self.clear_note_rows();
        self.archivist.show();
        let vault = self.vault.clone();
        let epoch = self.epoch;
        self.archivist.launch(message, vault, epoch, cx);
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

    fn apply_archivist_changes(
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
            if self.capture_status().is_some() {
                self.notice = Some("Stop the recording before you change notes".to_string());
                return;
            }
            if !self.flush_edits(cx) {
                return;
            }
            if let Err(err) = self.load_note(id, window, cx) {
                self.notice = Some(err);
            }
            return;
        }
        if self.open_note.is_some() && open.is_none() {
            if self.capture_status().is_some() {
                self.notice =
                    Some("Stop the recording. The Archivist deleted this note.".to_string());
            } else {
                self.clear_editor(window, cx);
            }
            return;
        }
        let Some(id) = open else {
            return;
        };
        if !refresh {
            self.open_note = Some(id);
            return;
        }
        if self.capture_status().is_some() {
            self.open_note = Some(id);
            self.notice =
                Some("Stop the recording to reload the note the Archivist changed.".to_string());
            return;
        }
        if let Err(err) = self.reload_open_note(id, window, cx) {
            self.notice = Some(err);
        }
    }

    fn reload_open_note(
        &mut self,
        id: NoteId,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<(), String> {
        let text = self.vault.read(&id).map_err(|err| err.to_string())?;
        self.clear_note_rows();
        self.reveal(&id.parent());
        self.open_note = Some(id.clone());
        if matches!(
            self.tree_selection,
            TreeSelection::Note(_) | TreeSelection::None
        ) {
            self.tree_selection = TreeSelection::Note(id);
        }
        self.saved_text = text.clone();
        self.latest = text.clone();
        self.dirty = false;
        self.editor
            .update(cx, |state, cx| state.set_value(text, window, cx));
        Ok(())
    }

    fn parent_folder(&self) -> FolderId {
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

    fn select_tree_folder(&mut self, id: FolderId) {
        self.tree_selection = TreeSelection::Folder(id);
    }
}

impl Drop for EditorPane {
    fn drop(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(id) = self.open_note.clone() {
            let _ = self.vault.write(&id, &self.latest);
        }
    }
}

const SYSTEM_MICROPHONE: &str = "System default";

fn saved_microphone(name: Option<String>) -> Option<String> {
    name.filter(|value| !value.trim().is_empty())
}

fn note_view_from(choice: NoteViewChoice) -> NoteView {
    match choice {
        NoteViewChoice::Preview => NoteView::Preview,
        NoteViewChoice::Source => NoteView::Source,
    }
}

fn note_view_choice(view: NoteView) -> NoteViewChoice {
    match view {
        NoteView::Preview => NoteViewChoice::Preview,
        NoteView::Source => NoteViewChoice::Source,
    }
}

fn first_note(index: &VaultIndex) -> Option<NoteId> {
    let welcome = NoteId::parse("welcome").expect("welcome is one safe path segment");
    if index.contains(&welcome) {
        Some(welcome)
    } else {
        index.ids().next().cloned()
    }
}

/// Opens the create dialog.
///
/// The dialog builder runs again each time the layer redraws, so the error
/// lives in a cell the confirm callback can fill.
fn open_create_dialog(
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
            .footer(dialog_footer("Cancel", "Create", ButtonVariant::Primary))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Input::new(&input).w_full())
                    .when_some(message, |column, message| {
                        column.child(div().text_sm().text_color(danger).child(message))
                    }),
            )
            .on_ok(move |_event, window, cx| {
                let text = input_for_ok.read(cx).value().to_string();
                view.update(cx, |root, cx| {
                    let Some(pane) = root.pane.as_mut() else {
                        return true;
                    };
                    match pane.apply_name(action.clone(), &text, window, cx) {
                        Ok(()) => true,
                        Err(err) => {
                            *error_for_ok.borrow_mut() = Some(err);
                            false
                        }
                    }
                })
                .unwrap_or(true)
            })
    });
}

fn open_delete_dialog(
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
            .footer(dialog_footer("Cancel", "Delete", ButtonVariant::Danger))
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
fn dialog_footer(
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

fn child_name(parent: &FolderId, name: &str) -> String {
    if parent.is_root() {
        name.to_string()
    } else {
        format!("{}/{name}", parent.as_str())
    }
}

fn rewrite_note_id(note: &NoteId, from: &FolderId, to: &FolderId) -> Option<NoteId> {
    let prefix = format!("{}/", from.as_str());
    let rest = note.as_str().strip_prefix(&prefix)?;
    let next = if to.is_root() {
        rest.to_string()
    } else {
        format!("{}/{rest}", to.as_str())
    };
    NoteId::parse(&next).ok()
}

fn rewrite_folder_id(id: &FolderId, from: &FolderId, to: &FolderId) -> Option<FolderId> {
    if id == from {
        return Some(to.clone());
    }
    let prefix = format!("{}/", from.as_str());
    let rest = id.as_str().strip_prefix(&prefix)?;
    let next = if to.is_root() {
        rest.to_string()
    } else {
        format!("{}/{rest}", to.as_str())
    };
    FolderId::parse(&next).ok()
}

fn folder_contains(folder: &FolderId, child: &FolderId) -> bool {
    let prefix = format!("{}/", folder.as_str());
    child.as_str().starts_with(&prefix)
}

fn note_in_folder(note: &NoteId, folder: &FolderId) -> bool {
    let prefix = format!("{}/", folder.as_str());
    note.as_str().starts_with(&prefix)
}

fn next_epoch() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn follow_open(
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

fn follow_note(id: NoteId, changes: &[crate::archivist::VaultChange]) -> Option<NoteId> {
    follow_open(Some(id), changes).0
}

fn last_opened(changes: &[crate::archivist::VaultChange]) -> Option<NoteId> {
    use crate::archivist::VaultChange;
    let id = changes.iter().rev().find_map(|change| match change {
        VaultChange::Opened(id) => Some(id.clone()),
        _ => None,
    })?;
    follow_note(id, changes)
}

fn follow_folder(mut id: FolderId, changes: &[crate::archivist::VaultChange]) -> Option<FolderId> {
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
