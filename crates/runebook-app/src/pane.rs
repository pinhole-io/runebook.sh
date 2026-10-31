//! Vault workspace: file tree, source editor, markdown preview, and row selection.

#[cfg(target_os = "linux")]
mod overlay;
mod page;
#[cfg(target_os = "linux")]
mod place;
mod preview;
mod research;
mod rich;
mod rows;
mod settings;
mod view;

pub(crate) use page::flush_preview;
pub(crate) use view::{files, note, status_bar, toolbar};

use rows::{
    context_message, line_span, rewrite_message, selected_span, selection_label, split_rows,
    usable_selection,
};

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants};
use gpui_kit::component::dialog::{Cancel, Confirm, DialogFooter};
use gpui_kit::component::input::{EditorState, Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, IndexPath, WindowExt};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, ClickEvent, Context, Entity, Focusable, PathPromptOptions, SharedString,
    Subscription, Window, div, px,
};
use runebook::{
    AppConfig, AutosavePace, Error, FolderId, NoteId, NoteKind, NoteViewChoice, SearchHit,
    SpeechModel, TextSize, Vault, VaultIndex, VaultStamp, VaultTree, decode_note, encode_note,
    toggle_task_line,
};

use crate::Root;
use crate::capture::{
    ScreenCapture, ScreenTarget, SpeechRequest, VoiceCapture, append_block, list_input_devices,
    list_record_sources, markdown_link, utc_stamp_now, voice_to_text, window_list_error,
};
use crate::locale;
use crate::models::{self, ModelSettings};
use crate::save_setting;

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
    CreateNote(NoteKind),
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
    stamp: VaultStamp,
    open_note: Option<NoteId>,
    open_kind: NoteKind,
    tree_selection: TreeSelection,
    collapsed: HashSet<FolderId>,
    note_view: NoteView,
    default_note_view: NoteView,
    settings_open: bool,
    microphone: Option<String>,
    microphone_select: Option<Entity<SelectState<SearchableVec<String>>>>,
    microphone_watch: Option<Subscription>,
    mic_load: u64,
    theme_select: Option<Entity<SelectState<SearchableVec<String>>>>,
    theme_watch: Option<Subscription>,
    research_model_select: Option<Entity<SelectState<SearchableVec<String>>>>,
    research_model_watch: Option<Subscription>,
    archivist_model_select: Option<Entity<SelectState<SearchableVec<String>>>>,
    archivist_model_watch: Option<Subscription>,
    model_load: u64,
    model_list_failed: bool,
    theme: String,
    locale: String,
    text_size: TextSize,
    line_numbers: bool,
    soft_wrap: bool,
    autosave: AutosavePace,
    start_muted: bool,
    speech_model: SpeechModel,
    speech_language: String,
    reopen_last_note: bool,
    remembered_note: Option<String>,
    models: ModelSettings,
    model_inputs: ModelInputs,
    _model_watch: Vec<Subscription>,
    editor: Entity<EditorState>,
    saved_text: String,
    latest: String,
    row_gesture: Option<rows::RowGesture>,
    row_rewrite: Option<Entity<InputState>>,
    /// The source editor's selection menu. It opens after the pointer or a key goes up.
    source_menu: bool,
    /// The pointer is down in the source editor, so the menu stays hidden until it goes up.
    source_pressing: bool,
    preview_view: Option<page::PreviewSurface>,
    preview_book: Arc<Mutex<page::MediaBook>>,
    preview_events: VecDeque<page::PageEvent>,
    preview_stamp: u64,
    /// The stamp of the last note pushed to the page.
    preview_sent: u64,
    /// Send `preview_sent` again. The last push had no confirmation.
    preview_retry: bool,
    preview_tries: u8,
    preview_rewrite: bool,
    preview_ready: Arc<AtomicBool>,
    dirty: bool,
    notice: Option<String>,
    media_muted: bool,
    capture: Capture,
    /// Note that owns the live voice or screen capture.
    recording_note: Option<NoteId>,
    prompt: Option<Prompt>,
    rename_blur: Option<Subscription>,
    search: Option<Entity<InputState>>,
    search_hits: Vec<SearchHit>,
    search_truncated: bool,
    search_watch: Option<Subscription>,
    _subscription: Subscription,
    archivist: crate::archivist::Panel,
    epoch: u64,
    research: research::ResearchDesk,
}

pub(super) struct ModelInputs {
    pub(super) tool_rounds: Entity<InputState>,
    pub(super) groq_api_key: Entity<InputState>,
    pub(super) openai_api_key: Entity<InputState>,
    pub(super) openai_base_url: Entity<InputState>,
    pub(super) openai_model: Entity<InputState>,
    pub(super) archivist_model: Entity<InputState>,
    pub(super) speech_language: Entity<InputState>,
}

#[derive(Clone, Copy)]
enum ModelField {
    ToolRounds,
    GroqKey,
    OpenAiKey,
    OpenAiBase,
    OpenAiModel,
    ArchivistModel,
    SpeechLanguage,
}

impl ModelInputs {
    fn get(&self, field: ModelField) -> &Entity<InputState> {
        match field {
            ModelField::ToolRounds => &self.tool_rounds,
            ModelField::GroqKey => &self.groq_api_key,
            ModelField::OpenAiKey => &self.openai_api_key,
            ModelField::OpenAiBase => &self.openai_base_url,
            ModelField::OpenAiModel => &self.openai_model,
            ModelField::ArchivistModel => &self.archivist_model,
            ModelField::SpeechLanguage => &self.speech_language,
        }
    }
}

fn model_input(
    window: &mut Window,
    cx: &mut Context<Root>,
    value: &str,
    placeholder: &str,
    masked: bool,
) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
            .masked(masked)
    })
}

fn watch_model(
    input: &Entity<InputState>,
    field: ModelField,
    cx: &mut Context<Root>,
) -> Subscription {
    cx.subscribe(input, move |this, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change)
            && let Some(pane) = this.pane.as_mut()
        {
            pane.apply_model_field(field, cx);
            cx.notify();
        }
    })
}

enum Capture {
    Idle,
    Voice(VoiceCapture),
    Screen(ScreenCapture),
    Transcribing,
    SavingScreen,
}

fn search_notice() -> &'static str {
    locale::text("search.failed")
}

impl EditorPane {
    pub(super) fn open(
        vault: Vault,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<Self, Error> {
        let tree = vault.tree()?;
        let index = vault.notes()?;
        let stamp = vault.stamp()?;
        let prefs = AppConfig::load().unwrap_or_default();
        let default_note_view = note_view_from(prefs.note_view);
        let models = ModelSettings::from_config(&prefs);
        let model_inputs = ModelInputs {
            tool_rounds: model_input(window, cx, &models.tool_rounds.to_string(), "250", false),
            groq_api_key: model_input(
                window,
                cx,
                &models::field_secret(&models.groq_api_key, models::GROQ_KEY_ENVS),
                "GROQ_API_KEY",
                true,
            ),
            openai_api_key: model_input(
                window,
                cx,
                &models::field_secret(&models.openai_api_key, models::OPENAI_KEY_ENVS),
                "OPENAI_API_KEY",
                true,
            ),
            openai_base_url: model_input(
                window,
                cx,
                &models.openai_base_url,
                "https://api.openai.com",
                false,
            ),
            openai_model: model_input(window, cx, &models.openai_model, "gpt-5.6-luna", false),
            archivist_model: model_input(
                window,
                cx,
                &models.archivist_model,
                "openai:gpt-6-luna",
                false,
            ),
            speech_language: model_input(
                window,
                cx,
                &prefs.speech_language,
                locale::text("capture.speech_placeholder"),
                false,
            ),
        };
        let model_watch = vec![
            watch_model(&model_inputs.tool_rounds, ModelField::ToolRounds, cx),
            watch_model(&model_inputs.groq_api_key, ModelField::GroqKey, cx),
            watch_model(&model_inputs.openai_api_key, ModelField::OpenAiKey, cx),
            watch_model(&model_inputs.openai_base_url, ModelField::OpenAiBase, cx),
            watch_model(&model_inputs.openai_model, ModelField::OpenAiModel, cx),
            watch_model(
                &model_inputs.archivist_model,
                ModelField::ArchivistModel,
                cx,
            ),
            watch_model(
                &model_inputs.speech_language,
                ModelField::SpeechLanguage,
                cx,
            ),
        ];
        let preview = page::install(window, cx);
        let open_note = starting_note(&index, &prefs);
        let (open_kind, text) = match &open_note {
            Some(id) => note_body(&vault.read(id)?),
            None => (NoteKind::Book, String::new()),
        };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .line_number(prefs.line_numbers)
                .soft_wrap(prefs.soft_wrap)
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
            source_menu: false,
            source_pressing: false,
            preview_view: preview.view,
            preview_book: preview.book,
            preview_events: VecDeque::new(),
            preview_stamp: 0,
            preview_sent: 0,
            preview_retry: false,
            preview_tries: 0,
            preview_rewrite: false,
            preview_ready: preview.ready,
            saved_text: text.clone(),
            latest: text,
            open_note,
            open_kind,
            vault,
            tree,
            index,
            stamp,
            note_view: default_note_view,
            default_note_view,
            settings_open: false,
            microphone: saved_microphone(prefs.microphone),
            microphone_select: None,
            microphone_watch: None,
            mic_load: 0,
            theme_select: None,
            theme_watch: None,
            research_model_select: None,
            research_model_watch: None,
            archivist_model_select: None,
            archivist_model_watch: None,
            model_load: 0,
            model_list_failed: false,
            theme: crate::theme::catalog().resolve(&prefs.theme).name.clone(),
            locale: locale::activate(&prefs.locale).to_string(),
            text_size: prefs.text_size,
            line_numbers: prefs.line_numbers,
            soft_wrap: prefs.soft_wrap,
            autosave: prefs.autosave,
            start_muted: prefs.media_muted,
            speech_model: prefs.speech_model,
            speech_language: prefs.speech_language,
            reopen_last_note: prefs.reopen_last_note,
            remembered_note: prefs.last_note.clone(),
            models,
            model_inputs,
            _model_watch: model_watch,
            editor,
            dirty: false,
            notice: None,
            media_muted: prefs.media_muted,
            capture: Capture::Idle,
            recording_note: None,
            prompt: None,
            rename_blur: None,
            search: None,
            search_hits: Vec::new(),
            search_truncated: false,
            search_watch: None,
            _subscription: subscription,
            archivist: crate::archivist::Panel::new(window, cx),
            epoch: next_epoch(),
            research: research::ResearchDesk::new(),
        };
        if pane.note_view == NoteView::Source {
            pane.editor.focus_handle(cx).focus(window, cx);
        }
        Ok(pane)
    }

    /// Swap the rendered note and the source editor.
    ///
    /// The editor entity stays alive in both modes. Focusing it on the way to
    /// the source lets typing start on the next frame. A Research note stays
    /// on the report.
    pub(super) fn toggle_note_view(&mut self, window: &mut Window, cx: &mut Context<Root>) {
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

    fn research_note_open(&self) -> bool {
        self.open_note.is_some() && self.open_kind == NoteKind::Research
    }

    /// Open or close settings in the note area.
    ///
    /// Opening settings builds the theme list, the microphone list, and the model list.
    fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.show_themes(window, cx);
            self.begin_microphone_load(cx);
            self.schedule_model_list_load(Duration::ZERO, cx);
        }
    }

    /// Fill the theme menu from the catalog. A theme added while Settings is open
    /// shows up the next time Settings opens.
    fn show_themes(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let names = crate::theme::catalog().names();
        let selected = names
            .iter()
            .position(|name| name == &self.theme)
            .unwrap_or(0);
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(names),
                Some(IndexPath::new(selected)),
                window,
                cx,
            )
        });
        self.theme_watch = Some(cx.subscribe(
            &select,
            |root, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                let SelectEvent::Confirm(Some(name)) = event else {
                    return;
                };
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                pane.apply_theme(name.clone(), cx);
                cx.notify();
            },
        ));
        self.theme_select = Some(select);
    }

    /// Load `GET /v1/models` for the current host and key.
    ///
    /// `delay` lets a host or key edit settle before the request. A newer load
    /// cancels the menu update from an older one. The key stays in the request
    /// header and is not written to an error.
    fn schedule_model_list_load(&mut self, delay: Duration, cx: &mut Context<Root>) {
        self.model_load = self.model_load.wrapping_add(1);
        let load = self.model_load;
        self.model_list_failed = false;
        let base = models::resolve_openai_base(&self.models.openai_base_url);
        let key = models::resolve_openai_key(&self.models.openai_api_key).unwrap_or_default();
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let current = this
                .update(cx, |root, _| {
                    root.pane
                        .as_ref()
                        .is_some_and(|pane| pane.model_load == load && pane.settings_open)
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let listed = cx
                .background_executor()
                .spawn(async move { models::fetch_model_ids(&base, &key) })
                .await;
            this.update_in(cx, |root, window, cx| {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                if pane.model_load != load || !pane.settings_open {
                    return;
                }
                pane.show_model_selects(listed, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_model_selects(
        &mut self,
        listed: models::ListedModels,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let (failed, ids) = match listed {
            models::ListedModels::Ready(ids) => (false, ids),
            models::ListedModels::Failed => (true, Vec::new()),
        };
        self.model_list_failed = failed;
        let research = self.models.openai_model.clone();
        let archivist = self.models.archivist_model.clone();
        let names = models::model_choices(
            locale::text("settings.model.default"),
            &ids,
            &[research.as_str(), archivist.as_str()],
        );
        let (research_select, research_watch) = Self::model_select(
            names.clone(),
            &research,
            ModelField::OpenAiModel,
            window,
            cx,
        );
        let (archivist_select, archivist_watch) =
            Self::model_select(names, &archivist, ModelField::ArchivistModel, window, cx);
        self.research_model_select = Some(research_select);
        self.research_model_watch = Some(research_watch);
        self.archivist_model_select = Some(archivist_select);
        self.archivist_model_watch = Some(archivist_watch);
    }

    fn model_select(
        names: Vec<String>,
        saved: &str,
        field: ModelField,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> (Entity<SelectState<SearchableVec<String>>>, Subscription) {
        let selected_label = if saved.is_empty() {
            locale::text("settings.model.default").to_string()
        } else {
            saved.to_string()
        };
        let selected = names
            .iter()
            .position(|item| item == &selected_label)
            .unwrap_or(0);
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(names),
                Some(IndexPath::new(selected)),
                window,
                cx,
            )
            .searchable(true)
        });
        let watch = cx.subscribe(
            &select,
            move |root, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                let SelectEvent::Confirm(value) = event;
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                let name = value
                    .clone()
                    .unwrap_or_else(|| locale::text("settings.model.default").to_string());
                pane.apply_listed_model(field, name);
                cx.notify();
            },
        );
        (select, watch)
    }

    /// Save a model from the menu. The default row stores an empty field.
    fn apply_listed_model(&mut self, field: ModelField, name: String) {
        let value = if name == locale::text("settings.model.default") {
            String::new()
        } else {
            name
        };
        match field {
            ModelField::OpenAiModel => {
                if value == self.models.openai_model {
                    return;
                }
                self.models.openai_model = value;
            }
            ModelField::ArchivistModel => {
                if value == self.models.archivist_model {
                    return;
                }
                self.models.archivist_model = value;
                self.archivist.end_session();
            }
            _ => return,
        }
        self.store_models();
    }

    /// Apply `name` and save it. An unknown name is ignored.
    fn apply_theme(&mut self, name: String, cx: &mut Context<Root>) {
        let Some(loaded) = crate::theme::catalog().by_name(&name).cloned() else {
            return;
        };
        let size = self.text_size;
        crate::theme::apply(&loaded, cx);
        self.choose_theme(name);
        crate::theme::apply_text_size(size, cx);
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
            .unwrap_or_else(|| locale::text("mic.system").to_string());
        let mut items = Vec::with_capacity(names.len() + 1);
        items.push(locale::text("mic.system").to_string());
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
            Some(name) if name == locale::text("mic.system") => None,
            None => None,
            Some(name) => Some(name.to_string()),
        };
        self.microphone = name.clone();
        self.store_setting(|config| config.microphone = name);
    }

    /// Save the window language and refresh text that was captured earlier.
    fn choose_locale(&mut self, code: &str, window: &mut Window, cx: &mut Context<Root>) {
        let code = locale::activate(code);
        if self.locale == code {
            return;
        }
        self.locale = code.to_string();
        self.refresh_placeholders(window, cx);
        if self.settings_open {
            self.begin_microphone_load(cx);
        }
        let saved = self.locale.clone();
        self.store_setting(move |config| config.locale = saved);
    }

    fn refresh_placeholders(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let speech = locale::text("capture.speech_placeholder");
        self.model_inputs
            .speech_language
            .update(cx, |state, cx| state.set_placeholder(speech, window, cx));
        if let Some(search) = self.search.clone() {
            let label = locale::text("search.placeholder");
            search.update(cx, |state, cx| state.set_placeholder(label, window, cx));
        }
        if let Some(rewrite) = self.row_rewrite.clone() {
            let label = locale::text("preview.rewrite_placeholder");
            rewrite.update(cx, |state, cx| state.set_placeholder(label, window, cx));
        }
        self.archivist.set_draft_placeholder(window, cx);
    }

    /// Save the window theme. The caller already applied it to the window.
    fn choose_theme(&mut self, name: String) {
        self.theme = name.clone();
        self.store_setting(move |config| config.theme = name);
    }

    /// Save the text size. The caller already applied it to the window.
    fn choose_text_size(&mut self, size: TextSize) {
        self.text_size = size;
        self.store_setting(move |config| config.text_size = size);
    }

    /// Show or hide source line numbers, and save that choice.
    fn choose_line_numbers(&mut self, on: bool, window: &mut Window, cx: &mut Context<Root>) {
        if self.line_numbers == on {
            return;
        }
        self.line_numbers = on;
        self.editor
            .update(cx, |state, cx| state.set_line_number(on, window, cx));
        self.store_setting(move |config| config.line_numbers = on);
    }

    /// Wrap or scroll long source lines, and save that choice.
    fn choose_soft_wrap(&mut self, on: bool, window: &mut Window, cx: &mut Context<Root>) {
        if self.soft_wrap == on {
            return;
        }
        self.soft_wrap = on;
        self.editor
            .update(cx, |state, cx| state.set_soft_wrap(on, window, cx));
        self.store_setting(move |config| config.soft_wrap = on);
    }

    /// Save when edits are written. Manual waits for leave or close.
    fn choose_autosave(&mut self, pace: AutosavePace) {
        self.autosave = pace;
        self.store_setting(move |config| config.autosave = pace);
    }

    /// Delay before the next autosave. Manual is `None`.
    pub(super) fn autosave_delay(&self) -> Option<Duration> {
        self.autosave.delay_ms().map(Duration::from_millis)
    }

    /// Save the Groq transcription model.
    fn choose_speech_model(&mut self, model: SpeechModel) {
        self.speech_model = model;
        self.store_setting(move |config| config.speech_model = model);
    }

    /// Start new clips muted, and apply that to the clip that is playing.
    fn choose_start_muted(&mut self, on: bool) {
        self.start_muted = on;
        self.media_muted = on;
        self.preview_stamp = 0;
        self.store_setting(move |config| config.media_muted = on);
    }

    /// Save whether the next launch opens the last note.
    fn choose_reopen_last_note(&mut self, on: bool) {
        self.reopen_last_note = on;
        self.store_setting(move |config| config.reopen_last_note = on);
        if on && let Some(id) = self.open_note.clone() {
            self.remember_note(&id);
        }
    }

    /// Remember `id` for the next launch.
    fn remember_note(&mut self, id: &NoteId) {
        let saved = id.to_string();
        if self.remembered_note.as_deref() == Some(saved.as_str()) {
            return;
        }
        self.remembered_note = Some(saved.clone());
        self.store_setting(move |config| config.last_note = Some(saved));
    }

    /// Save the default note view and show that view on the open note.
    fn choose_note_view(&mut self, view: NoteView, window: &mut Window, cx: &mut Context<Root>) {
        self.default_note_view = view;
        self.show_note_view(view, window, cx);
        let choice = note_view_choice(view);
        self.store_setting(|config| config.note_view = choice);
    }

    fn show_note_view(&mut self, view: NoteView, window: &mut Window, cx: &mut Context<Root>) {
        if self.research_note_open() {
            self.note_view = NoteView::Preview;
            return;
        }
        self.note_view = view;
        if view == NoteView::Source {
            self.editor.focus_handle(cx).focus(window, cx);
        }
    }

    fn store_setting(&mut self, change: impl FnOnce(&mut AppConfig)) {
        if let Err(err) = save_setting(change) {
            self.notice = Some(self.redact_notice(&err));
        }
    }

    fn redact_notice(&self, text: &str) -> String {
        let mut secrets = vec![
            self.models.groq_api_key.clone(),
            self.models.openai_api_key.clone(),
        ];
        secrets.extend(models::env_secrets());
        models::redact_secrets(text, secrets.iter().map(String::as_str))
    }

    fn saved_secrets(&self) -> Vec<String> {
        [
            self.models.groq_api_key.as_str(),
            self.models.openai_api_key.as_str(),
        ]
        .into_iter()
        .map(str::trim)
        .filter(|value| value.len() >= 8)
        .map(str::to_string)
        .collect()
    }

    pub(super) fn groq_ready(&self) -> bool {
        models::resolve_groq_key(&self.models.groq_api_key).is_some()
    }

    pub(super) fn archivist_ready(&self) -> bool {
        self.archivist_access().is_some()
    }

    pub(super) fn archivist_key_message(&self) -> &'static str {
        crate::archivist::missing_model_key(&models::resolve_archivist_model(
            &self.models.archivist_model,
        ))
    }

    fn archivist_access(&self) -> Option<crate::archivist::ModelAccess> {
        let model = models::resolve_archivist_model(&self.models.archivist_model);
        let groq_key = self.groq_key().unwrap_or_default();
        let openai_key =
            models::resolve_openai_key(&self.models.openai_api_key).unwrap_or_default();
        let openai_base_url = models::resolve_openai_base(&self.models.openai_base_url);
        if !crate::archivist::has_model_key(&model, &groq_key, &openai_key) {
            return None;
        }
        Some(crate::archivist::ModelAccess {
            groq_key,
            openai_key,
            openai_base_url,
            model,
            tool_rounds: self.models.tool_rounds,
        })
    }

    fn groq_key(&self) -> Option<String> {
        models::resolve_groq_key(&self.models.groq_api_key)
    }

    fn apply_model_field(&mut self, field: ModelField, cx: &mut Context<Root>) {
        let raw = self.model_inputs.get(field).read(cx).value().to_string();
        let Some(text) = models::bounded(&raw) else {
            self.notice = Some(locale::text("notice.value_long").to_string());
            return;
        };
        match field {
            ModelField::ToolRounds => {
                let Some(rounds) = models::accept_tool_rounds(text) else {
                    self.notice = Some(locale::text("notice.tool_rounds").to_string());
                    return;
                };
                if rounds == self.models.tool_rounds {
                    return;
                }
                self.models.tool_rounds = rounds;
                self.archivist.end_session();
            }
            ModelField::GroqKey => {
                if text == self.models.groq_api_key {
                    return;
                }
                self.models.groq_api_key = text.to_string();
                self.archivist.end_session();
            }
            ModelField::OpenAiKey => {
                if text == self.models.openai_api_key {
                    return;
                }
                self.models.openai_api_key = text.to_string();
            }
            ModelField::OpenAiBase => {
                if text == self.models.openai_base_url {
                    return;
                }
                self.models.openai_base_url = text.to_string();
            }
            ModelField::OpenAiModel => {
                if text == self.models.openai_model {
                    return;
                }
                self.models.openai_model = text.to_string();
            }
            ModelField::ArchivistModel => {
                if text == self.models.archivist_model {
                    return;
                }
                self.models.archivist_model = text.to_string();
                self.archivist.end_session();
            }
            ModelField::SpeechLanguage => {
                self.apply_speech_language(text);
                return;
            }
        }
        self.store_models();
        if self.settings_open && matches!(field, ModelField::OpenAiKey | ModelField::OpenAiBase) {
            self.schedule_model_list_load(Duration::from_millis(400), cx);
        }
    }

    fn apply_speech_language(&mut self, text: &str) {
        let next = match models::accept_language(text) {
            models::LanguageEdit::Pending => return,
            models::LanguageEdit::Reject => {
                self.notice = Some(locale::text("notice.language").to_string());
                return;
            }
            models::LanguageEdit::Set(value) => value,
        };
        if self.notice.as_deref() == Some(locale::text("notice.language")) {
            self.notice = None;
        }
        if next == self.speech_language {
            return;
        }
        self.speech_language = next.clone();
        self.store_setting(move |config| config.speech_language = next);
    }

    fn store_models(&mut self) {
        let models = self.models.clone();
        self.store_setting(move |config| {
            config.tool_rounds = models.tool_rounds;
            config.groq_api_key = models.groq_api_key.clone();
            config.openai_api_key = models.openai_api_key.clone();
            config.openai_base_url = models.openai_base_url.clone();
            config.openai_model = models.openai_model.clone();
            config.archivist_model = models.archivist_model.clone();
        });
    }

    pub(super) fn root(&self) -> &Path {
        self.vault.root()
    }

    /// Resolve a media link in the open note.
    pub(super) fn media_path(&self, href: &str) -> Result<std::path::PathBuf, String> {
        let Some(id) = self.open_note.as_ref() else {
            return Err(locale::text("media.open_note").to_string());
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

    fn start_voice(&mut self) {
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

    fn start_screen(&mut self, target: ScreenTarget) {
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

    fn finish_voice(
        &mut self,
        note_id: Option<NoteId>,
        outcome: Result<crate::capture::VoiceOutcome, String>,
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
                self.notice = Some(self.redact_notice(&err));
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

    fn finish_screen(
        &mut self,
        note_id: Option<NoteId>,
        file_name: String,
        outcome: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        self.capture = Capture::Idle;
        self.recording_note = None;
        if let Err(err) = outcome {
            self.notice = Some(err);
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
    fn deliver_block(
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

    fn insert_block(&mut self, block: &str, window: &mut Window, cx: &mut Context<Root>) {
        let next = append_block(&self.latest, block);
        self.clear_note_rows(cx);
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
                .ok_or_else(|| locale::text("notice.recording_name").to_string())?
                .to_string();
            return Ok((path, name));
        }
        Err(locale::text("notice.recording_choose").to_string())
    }

    fn require_idle(&self) -> Result<(), String> {
        if self.capture_status().is_none() {
            Ok(())
        } else {
            Err(locale::text("notice.recording_idle").to_string())
        }
    }

    /// Copy the editor buffer into the preview. Returns true when a save should wait.
    pub(super) fn capture_edit(&mut self, cx: &mut Context<Root>) -> bool {
        let value = self.editor.read(cx).value().to_string();
        if value != self.latest {
            self.clear_note_rows(cx);
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

    pub(super) fn load_note(
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

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let Some(input) = &self.search else {
            return;
        };
        input.focus_handle(cx).focus(window, cx);
    }

    fn refresh_search(&mut self, cx: &mut Context<Root>) {
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

    fn begin_create_folder(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let seed = child_name(&self.parent_folder(), "folder");
        open_create_dialog(
            locale::text("tree.new_folder"),
            seed,
            NameAction::CreateFolder,
            window,
            cx,
        );
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Root>) {
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
    fn settle_after_delete(
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
    fn take_next_note(
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

    fn clear_editor(&mut self, window: &mut Window, cx: &mut Context<Root>) {
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
        let tree = self.vault.tree()?;
        let index = self.vault.notes()?;
        let stamp = self.vault.stamp()?;
        self.tree = tree;
        self.index = index;
        self.stamp = stamp;
        Ok(())
    }

    /// The stamp from the last scan. The file watch compares against this.
    pub(super) fn file_stamp(&self) -> &VaultStamp {
        &self.stamp
    }

    /// Read the vault again and show files that changed outside the app.
    pub(super) fn refresh_from_disk(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if self.prompt.is_some() {
            return;
        }
        if let Err(err) = self.reload() {
            if self.fail_disk_scan(err.to_string()) {
                present_scan(window);
            }
            return;
        }
        self.finish_disk_refresh(window, cx);
        present_scan(window);
    }

    /// Install a scan that started from `base`. A newer local edit wins.
    pub(super) fn apply_disk_refresh(
        &mut self,
        base: &VaultStamp,
        refresh: DiskRefresh,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        if self.prompt.is_some() || &self.stamp != base {
            return false;
        }
        self.tree = refresh.tree;
        self.index = refresh.index;
        self.stamp = refresh.stamp;
        self.finish_disk_refresh(window, cx);
        true
    }

    /// Record a failed scan. Returns true when the notice changed.
    pub(super) fn fail_disk_scan(&mut self, message: String) -> bool {
        let notice = locale::format("tree.refresh_error", &[("message", &message)]);
        if self.notice.as_deref() == Some(notice.as_str()) {
            return false;
        }
        self.notice = Some(notice);
        true
    }

    fn finish_disk_refresh(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.clear_disk_notice();
        self.reconcile_open_file(window, cx);
        if self.search.is_some() {
            self.refresh_search(cx);
        }
    }

    fn clear_disk_notice(&mut self) {
        if self.notice.as_deref().is_some_and(|text| {
            let prefix = tree_refresh_prefix();
            !prefix.is_empty() && text.starts_with(prefix)
        }) {
            self.notice = None;
        }
    }

    /// Drop a selection the scan removed, then reload a clean open note.
    fn reconcile_open_file(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.drop_missing_selection();
        let Some(id) = self.open_note.clone() else {
            return;
        };
        let matches_saved = if self.index.contains(&id) {
            match self.vault.read(&id) {
                Ok(text) => {
                    let (kind, body) = note_body(&text);
                    Some(kind == self.open_kind && body == self.saved_text)
                }
                Err(_) => None,
            }
        } else {
            None
        };
        match open_file_sync(self.dirty, self.capture_status().is_some(), matches_saved) {
            OpenFileSync::Keep => {}
            OpenFileSync::Reload => {
                if let Err(err) = self.reload_open_note(id, window, cx) {
                    self.notice = Some(err);
                }
            }
            OpenFileSync::MissingMove => {
                if let Err(err) = self.settle_after_delete(window, cx) {
                    self.notice = Some(err);
                }
            }
            OpenFileSync::Notice(text) => {
                self.notice = Some(text.to_string());
                if matches_saved.is_none() && matches!(self.tree_selection, TreeSelection::Note(_))
                {
                    self.tree_selection = TreeSelection::None;
                }
            }
        }
    }

    fn drop_missing_selection(&mut self) {
        match &self.tree_selection {
            TreeSelection::Note(id) if !tree_contains_note(&self.tree.children, id) => {
                self.tree_selection = TreeSelection::None;
            }
            TreeSelection::Folder(id)
                if !id.is_root() && !tree_contains_folder(&self.tree.children, id) =>
            {
                self.tree_selection = TreeSelection::None;
            }
            _ => {}
        }
    }

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
            self.archivist
                .launch_files(instruction, draft, held, paths, vault, epoch, access, cx);
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
        display: String,
        message: superglue::openai::ChatMessage,
        held: &[(String, String)],
        paths: &[std::path::PathBuf],
        draft: &str,
        vault: Vault,
        epoch: u64,
        access: crate::archivist::ModelAccess,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if !self.archivist.is_reading() {
            return;
        }
        self.archivist.drop_sent_held(held);
        self.archivist.drop_sent_files(paths);
        self.archivist.clear_draft_if(draft, window, cx);
        self.archivist
            .launch_message(display, message, vault, epoch, access, cx);
    }

    fn clear_note_rows(&mut self, cx: &mut Context<Root>) {
        // Collapse only while the source menu is up. A later clear, such as a
        // reload, must not move a caret the menu was not showing.
        let drop_editor = self.source_menu && self.note_view == NoteView::Source;
        self.row_gesture = None;
        self.row_rewrite = None;
        self.preview_rewrite = false;
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
    fn press_source(&mut self) {
        self.source_pressing = true;
        self.source_menu = false;
        self.row_rewrite = None;
    }

    /// Open or close the source menu from the editor selection.
    ///
    /// Returns true when the menu state changed and the window should redraw.
    fn sync_source_menu(&mut self, cx: &App) -> bool {
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
    fn release_source_press(&mut self, cx: &App) -> bool {
        let was_pressing = self.source_pressing;
        self.source_pressing = false;
        if !was_pressing && !self.source_menu {
            return false;
        }
        self.sync_source_menu(cx) || was_pressing
    }

    fn selected_note_text(&self, cx: &App) -> Option<(String, String, usize, usize)> {
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
    fn copy_selected_rows(&self, cx: &mut Context<Root>) -> bool {
        let Some((_, source, _, _)) = self.selected_note_text(cx) else {
            return false;
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(source));
        true
    }

    /// Park the selection in the Archivist column. Send delivers it.
    fn add_selected_rows_to_chat(&mut self, cx: &mut Context<Root>) {
        let Some((note_id, source, start_line, end_line)) = self.selected_note_text(cx) else {
            return;
        };
        let label = selection_label(&note_id, start_line, end_line);
        self.archivist.hold_rows(note_id, source, label);
        self.clear_note_rows(cx);
    }

    /// Open the rewrite prompt under the selection menu.
    fn begin_row_rewrite(&mut self, window: &mut Window, cx: &mut Context<Root>) {
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
    fn apply_row_rewrite(&mut self, _window: &mut Window, cx: &mut Context<Root>) {
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
        let Some(message) = rewrite_message(&note_id, &source, &change) else {
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

    fn reload_open_note(
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

    fn apply_file(&mut self, text: &str, window: &mut Window, cx: &mut Context<Root>) {
        let (kind, body) = note_body(text);
        self.open_kind = kind;
        self.saved_text = body.clone();
        self.latest = body.clone();
        self.dirty = false;
        self.editor
            .update(cx, |state, cx| state.set_value(body, window, cx));
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
            let file = encode_note(self.open_kind, &self.latest);
            let _ = self.vault.write(&id, &file);
        }
    }
}

/// Draw the window after a vault scan.
///
/// Wayland stops asking for frames while the window is idle. `notify`
/// marks the view dirty and does not start a frame, so the last picture
/// stays up until a resize. This asks for the next frame.
pub(super) fn present_scan(window: &mut Window) {
    window.refresh();
    window.on_next_frame(|_, _| {});
}

fn tree_refresh_prefix() -> &'static str {
    locale::text("tree.refresh_error")
        .split("{message}")
        .next()
        .unwrap_or("")
}

fn starting_note(index: &VaultIndex, prefs: &AppConfig) -> Option<NoteId> {
    if prefs.reopen_last_note
        && let Some(raw) = prefs.last_note.as_deref()
        && let Ok(id) = NoteId::parse(raw)
        && index.contains(&id)
    {
        return Some(id);
    }
    first_note(index)
}

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

/// Ask which display or window a new Sight note should record.
fn prompt_record_source(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) {
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
            .gap_1();
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

fn record_source_button(
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

/// Speech and Sight notes start a capture, so they wait until the current one ends.
fn new_rune_blocked(kind: NoteKind, recording: bool) -> bool {
    recording && matches!(kind, NoteKind::Speech | NoteKind::Sight)
}

fn note_body(text: &str) -> (NoteKind, String) {
    let (kind, body) = decode_note(text);
    (kind, body.to_string())
}

/// Append `block` to a note file and keep that note's kind header.
fn note_with_block(file: &str, block: &str) -> String {
    let (kind, body) = decode_note(file);
    encode_note(kind, &append_block(body, block))
}

fn seed_body(kind: NoteKind, name: &str) -> String {
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

fn toggled_task_body(source: &str, index: usize) -> Option<String> {
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

/// Absolute path of `id` inside the vault at `root`.
#[cfg(any(test, target_os = "macos"))]
fn folder_on_disk(root: &Path, id: &FolderId) -> std::path::PathBuf {
    if id.is_root() {
        root.to_path_buf()
    } else {
        root.join(id.as_str())
    }
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

fn disk_changed() -> &'static str {
    locale::text("notice.disk_changed")
}

fn disk_removed() -> &'static str {
    locale::text("notice.disk_removed")
}

fn recording_changed() -> &'static str {
    locale::text("notice.recording_changed")
}

fn recording_removed() -> &'static str {
    locale::text("notice.recording_removed")
}

/// One background scan of the vault. The watch applies it on the UI thread.
pub(super) struct DiskRefresh {
    tree: VaultTree,
    index: VaultIndex,
    stamp: VaultStamp,
}

/// Read the vault when `base` is stale.
///
/// `Ok(None)` means the file tree is already current.
///
/// # Errors
///
/// Returns the vault error when the folder cannot be read.
pub(super) fn scan_vault(root: &Path, base: &VaultStamp) -> Result<Option<DiskRefresh>, Error> {
    let vault = Vault::open(root)?;
    let stamp = vault.stamp()?;
    if &stamp == base {
        return Ok(None);
    }
    Ok(Some(DiskRefresh {
        tree: vault.tree()?,
        index: vault.notes()?,
        stamp,
    }))
}

enum OpenFileSync {
    Keep,
    Reload,
    MissingMove,
    Notice(&'static str),
}

/// What to do with the open note after a scan.
///
/// `matches_saved` is `None` when the file is gone.
/// A clean note reloads. Unsaved text and a live recording stay on screen.
fn open_file_sync(dirty: bool, capturing: bool, matches_saved: Option<bool>) -> OpenFileSync {
    match matches_saved {
        Some(true) => OpenFileSync::Keep,
        Some(false) if capturing => OpenFileSync::Notice(recording_changed()),
        Some(false) if dirty => OpenFileSync::Notice(disk_changed()),
        Some(false) => OpenFileSync::Reload,
        None if capturing => OpenFileSync::Notice(recording_removed()),
        None if dirty => OpenFileSync::Notice(disk_removed()),
        None => OpenFileSync::MissingMove,
    }
}

fn tree_contains_note(entries: &[runebook::TreeEntry], id: &NoteId) -> bool {
    entries.iter().any(|entry| match entry {
        runebook::TreeEntry::Note { id: note, .. } => note == id,
        runebook::TreeEntry::Folder { children, .. } => tree_contains_note(children, id),
    })
}

fn tree_contains_folder(entries: &[runebook::TreeEntry], id: &FolderId) -> bool {
    entries.iter().any(|entry| match entry {
        runebook::TreeEntry::Folder {
            id: folder,
            children,
            ..
        } => folder == id || tree_contains_folder(children, id),
        runebook::TreeEntry::Note { .. } => false,
    })
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

fn last_wrote(changes: &[crate::archivist::VaultChange]) -> Option<NoteId> {
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
fn next_open_note(
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

#[cfg(test)]
mod disk_sync_tests {
    use super::{
        OpenFileSync, disk_changed, disk_removed, open_file_sync, recording_changed,
        recording_removed,
    };

    fn notice(sync: OpenFileSync) -> Option<&'static str> {
        match sync {
            OpenFileSync::Notice(text) => Some(text),
            _ => None,
        }
    }

    #[test]
    fn clean_note_reloads_when_the_file_changed() {
        assert!(matches!(
            open_file_sync(false, false, Some(false)),
            super::OpenFileSync::Reload
        ));
        assert!(matches!(
            open_file_sync(false, false, Some(true)),
            super::OpenFileSync::Keep
        ));
    }

    #[test]
    fn unsaved_text_stays_when_the_file_changed() {
        assert_eq!(
            notice(open_file_sync(true, false, Some(false))),
            Some(disk_changed())
        );
        assert!(matches!(
            open_file_sync(true, false, Some(true)),
            super::OpenFileSync::Keep
        ));
    }

    #[test]
    fn missing_clean_note_moves_and_a_dirty_note_stays() {
        assert!(matches!(
            open_file_sync(false, false, None),
            super::OpenFileSync::MissingMove
        ));
        assert_eq!(
            notice(open_file_sync(true, false, None)),
            Some(disk_removed())
        );
    }

    #[test]
    fn recording_blocks_a_reload() {
        assert_eq!(
            notice(open_file_sync(false, true, Some(false))),
            Some(recording_changed())
        );
        assert_eq!(
            notice(open_file_sync(true, true, None)),
            Some(recording_removed())
        );
    }
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

#[cfg(test)]
mod capture_note_tests {
    use super::{new_rune_blocked, note_with_block};
    use runebook::{NoteKind, decode_note, encode_note};

    #[test]
    fn speech_and_sight_wait_while_a_capture_runs() {
        assert!(new_rune_blocked(NoteKind::Speech, true));
        assert!(new_rune_blocked(NoteKind::Sight, true));
        assert!(!new_rune_blocked(NoteKind::Book, true));
        assert!(!new_rune_blocked(NoteKind::Deed, true));
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

#[cfg(test)]
mod folder_path_tests {
    use super::folder_on_disk;
    use runebook::FolderId;
    use std::path::Path;

    #[test]
    fn the_root_id_is_the_vault_root() {
        let root = Path::new("/tmp/notes");
        assert_eq!(folder_on_disk(root, &FolderId::root()), root);
    }

    #[test]
    fn a_nested_id_joins_the_vault_root() {
        let id = FolderId::parse("journal/2026").unwrap();
        assert_eq!(
            folder_on_disk(Path::new("/tmp/notes"), &id),
            Path::new("/tmp/notes/journal/2026")
        );
    }
}
