//! Vault workspace: file tree, source editor, markdown preview, and row selection.

mod preview;
mod research;
mod rows;
mod view;

pub(crate) use preview::queue_clips;
pub(crate) use view::{files, note, status_bar, toolbar};

use rows::{context_message, rewrite_message, selected_source, split_rows};

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants};
use gpui_kit::component::dialog::{Cancel, Confirm, DialogFooter};
use gpui_kit::component::input::{EditorState, Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, IndexPath, WindowExt};
use gpui_kit::prelude::*;
use gpui_kit::{
    AppContext, ClickEvent, Context, Entity, Focusable, Image, ImageFormat, RenderImage,
    SharedString, Subscription, Window, div, px,
};
use runebook::{
    AppConfig, Error, FolderId, NoteId, NoteKind, NoteViewChoice, ThemeChoice, Vault, VaultIndex,
    VaultTree, decode_note, encode_note, toggle_task_line,
};

use crate::Root;
use crate::capture::{
    MISSING_GROQ_KEY, ScreenCapture, ScreenTarget, VoiceCapture, WINDOW_LIST_ERROR, append_block,
    list_input_devices, list_record_sources, markdown_link, utc_stamp_now, voice_to_text,
};
use crate::models::{self, ModelSettings};
use crate::playback::{self, ClipKind};
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
    theme: ThemeChoice,
    models: ModelSettings,
    model_inputs: ModelInputs,
    _model_watch: Vec<Subscription>,
    editor: Entity<EditorState>,
    saved_text: String,
    latest: String,
    row_gesture: Option<rows::RowGesture>,
    row_rewrite: Option<Entity<InputState>>,
    dirty: bool,
    notice: Option<String>,
    playback: Option<playback::Playback>,
    playback_watch: u64,
    hold: Option<ClipHold>,
    media_muted: bool,
    seek_drag: bool,
    seek_preview: Option<(String, Duration)>,
    timings: HashMap<String, TimingState>,
    posters: HashMap<String, PosterState>,
    poster_epoch: u64,
    film: Option<Film>,
    loading: Option<String>,
    load_stop: Option<Arc<AtomicBool>>,
    picture_width: u32,
    capture: Capture,
    prompt: Option<Prompt>,
    rename_blur: Option<Subscription>,
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
}

#[derive(Clone, Copy)]
enum ModelField {
    ToolRounds,
    GroqKey,
    OpenAiKey,
    OpenAiBase,
    OpenAiModel,
    ArchivistModel,
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

enum PosterState {
    Pending,
    Ready(Arc<Image>),
    Missing,
}

struct ClipHold {
    href: String,
    position: Duration,
    duration: Duration,
}

struct Film {
    href: String,
    frames: Vec<playback::FilmFrame>,
    interval: Duration,
    timing: playback::ClipTiming,
}

/// Note width in device pixels, minus the sidebar, capped for the atlas.
fn note_frame_width(window: &Window) -> u32 {
    let note = (window.viewport_size().width.as_f32() - 260.0).max(0.0);
    let device = (note * window.scale_factor()).round().max(0.0) as u32;
    playback::frame_width(device)
}

fn resume_point(hold: &ClipHold) -> Duration {
    if !hold.duration.is_zero() && hold.position + Duration::from_millis(250) >= hold.duration {
        Duration::ZERO
    } else {
        hold.position
    }
}

fn drop_frames(frames: Vec<playback::FilmFrame>, window: &mut Window) {
    for frame in frames {
        let _ = window.drop_image(frame.image);
    }
}

enum TimingState {
    Pending,
    Ready(playback::ClipTiming),
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
        ];
        let open_note = first_note(&index);
        let show_archivist = open_note.is_none();
        let (open_kind, text) = match &open_note {
            Some(id) => note_body(&vault.read(id)?),
            None => (NoteKind::Book, String::new()),
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
            open_kind,
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
            models,
            model_inputs,
            _model_watch: model_watch,
            editor,
            dirty: false,
            notice: None,
            playback: None,
            playback_watch: 0,
            hold: None,
            media_muted: false,
            seek_drag: false,
            seek_preview: None,
            timings: HashMap::new(),
            posters: HashMap::new(),
            poster_epoch: 0,
            film: None,
            loading: None,
            load_stop: None,
            picture_width: note_frame_width(window),
            capture: Capture::Idle,
            prompt: None,
            rename_blur: None,
            _subscription: subscription,
            archivist: crate::archivist::Panel::new(show_archivist, window, cx),
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

    fn archivist_access(&self) -> Option<crate::archivist::ModelAccess> {
        let key = self.groq_key()?;
        Some(crate::archivist::ModelAccess {
            key,
            model: models::resolve_archivist_model(&self.models.archivist_model),
            tool_rounds: self.models.tool_rounds,
        })
    }

    fn groq_key(&self) -> Option<String> {
        models::resolve_groq_key(&self.models.groq_api_key)
    }

    fn apply_model_field(&mut self, field: ModelField, cx: &mut Context<Root>) {
        let raw = self.model_inputs.get(field).read(cx).value().to_string();
        let Some(text) = models::bounded(&raw) else {
            self.notice = Some("That value is too long.".to_string());
            return;
        };
        match field {
            ModelField::ToolRounds => {
                let Some(rounds) = models::accept_tool_rounds(text) else {
                    self.notice = Some("Tool rounds must be at least 1.".to_string());
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
        }
        self.store_models();
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

    /// The still preview for `href`.
    pub(super) fn picture(&self, href: &str) -> Option<Arc<Image>> {
        match self.posters.get(href) {
            Some(PosterState::Ready(image)) => Some(Arc::clone(image)),
            _ => None,
        }
    }

    /// True when a still preview was already requested for `href`.
    pub(super) fn poster_started(&self, href: &str) -> bool {
        self.posters.contains_key(href)
    }

    /// Load the first frame of `href` so the preview can show it before play.
    pub(super) fn request_poster(&mut self, href: &str, cx: &mut Context<Root>) {
        if self.posters.contains_key(href) {
            return;
        }
        let Some(id) = self.open_note.clone() else {
            return;
        };
        let path = match self.vault.media_file(&id, href) {
            Ok(path) => path,
            Err(_) => {
                self.posters.insert(href.to_string(), PosterState::Missing);
                return;
            }
        };
        self.posters.insert(href.to_string(), PosterState::Pending);
        let href = href.to_string();
        let epoch = self.poster_epoch;
        let width = self.picture_width;
        cx.spawn(async move |this, cx| {
            let bytes = cx
                .background_executor()
                .spawn(async move { playback::poster_jpeg(&path, Duration::ZERO, width) })
                .await;
            this.update(cx, |root, cx| {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                pane.store_poster(epoch, href, bytes);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn store_poster(&mut self, epoch: u64, href: String, bytes: Option<Vec<u8>>) {
        if self.poster_epoch != epoch {
            return;
        }
        if matches!(self.posters.get(&href), Some(PosterState::Ready(_))) {
            return;
        }
        let Some(bytes) = bytes else {
            self.posters.insert(href, PosterState::Missing);
            return;
        };
        self.posters.insert(
            href,
            PosterState::Ready(Arc::new(Image::from_bytes(ImageFormat::Jpeg, bytes))),
        );
    }

    /// Remember the note width so a later still uses the same scale.
    pub(super) fn sync_picture_width(&mut self, window: &Window) {
        self.picture_width = note_frame_width(window);
    }

    /// Stop the speaker. The decoded frames stay until the note changes.
    fn end_playback(&mut self, _window: &mut Window, _cx: &mut Context<Root>) {
        self.playback_watch = self.playback_watch.wrapping_add(1);
        self.playback.take();
    }

    fn pause_playback(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let Some(play) = self.playback.as_ref() else {
            return;
        };
        let hold = ClipHold {
            href: play.href().to_string(),
            position: play.position(),
            duration: play.duration(),
        };
        self.end_playback(window, cx);
        self.hold = Some(hold);
    }

    fn dismiss_media(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let href = self.film.as_ref().map(|film| film.href.clone());
        let position = href.as_deref().map(|href| self.clip_position(href));
        self.cancel_load();
        self.end_playback(window, cx);
        let poster = position.and_then(|position| self.poster_from_film(position));
        self.drop_film(window);
        self.hold = None;
        self.seek_drag = false;
        self.seek_preview = None;
        self.timings.clear();
        self.posters.clear();
        self.poster_epoch = self.poster_epoch.wrapping_add(1);
        if let Some((href, image)) = poster {
            self.posters.insert(href, PosterState::Ready(image));
        }
    }

    fn cancel_load(&mut self) {
        if let Some(stop) = self.load_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        if self.loading.take().is_some() {
            self.playback_watch = self.playback_watch.wrapping_add(1);
        }
    }

    fn drop_film(&mut self, window: &mut Window) {
        if let Some(film) = self.film.take() {
            drop_frames(film.frames, window);
        }
    }

    fn poster_from_film(&self, position: Duration) -> Option<(String, Arc<Image>)> {
        let film = self.film.as_ref()?;
        let slot = playback::frame_slot(position, film.interval, film.frames.len())?;
        let image = Arc::new(Image::from_bytes(
            ImageFormat::Jpeg,
            film.frames[slot].jpeg.clone(),
        ));
        Some((film.href.clone(), image))
    }

    /// The href of the clip that is playing, when one is playing.
    pub(super) fn playing_href(&self) -> Option<&str> {
        self.playback.as_ref().map(playback::Playback::href)
    }

    /// The decoded frame for `href`, when that clip is loaded and not still loading.
    pub(super) fn live_picture(&self, href: &str) -> Option<Arc<RenderImage>> {
        if self.is_loading(href) {
            return None;
        }
        let position = self.clip_position(href);
        let film = self.film.as_ref()?;
        if film.href != href {
            return None;
        }
        let slot = playback::frame_slot(position, film.interval, film.frames.len())?;
        Some(Arc::clone(&film.frames[slot].image))
    }

    /// True while `href` is reading and decoding frames.
    pub(super) fn is_loading(&self, href: &str) -> bool {
        self.loading.as_deref() == Some(href)
    }

    /// Width divided by height for the loaded frames of `href`.
    pub(super) fn film_ratio(&self, href: &str) -> Option<f32> {
        let film = self.film.as_ref()?;
        if film.href != href {
            return None;
        }
        let frame = film.frames.first()?;
        playback::jpeg_ratio(&frame.jpeg)
    }

    /// Play, pause, or resume `href`. Pause holds the place.
    pub(super) fn toggle_playback(
        &mut self,
        href: &str,
        kind: ClipKind,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        self.sync_picture_width(window);
        if self.is_loading(href) {
            self.cancel_load();
            return;
        }
        if self.playing_href() == Some(href) {
            self.pause_playback(window, cx);
            return;
        }
        let at = if self.hold.as_ref().is_some_and(|hold| hold.href == href) {
            let hold = self.hold.take().expect("hold");
            resume_point(&hold)
        } else {
            self.hold = None;
            Duration::ZERO
        };
        if self.playing_href().is_some() {
            self.end_playback(window, cx);
        }
        if self.film.as_ref().is_some_and(|film| film.href != href) {
            self.drop_film(window);
        }
        if kind == ClipKind::Video && self.film.as_ref().is_some_and(|film| film.href == href) {
            self.start_video_clock(href, at, cx);
            return;
        }
        self.start_clip(href, kind, at, cx);
    }

    pub(super) fn media_muted(&self) -> bool {
        self.media_muted
    }

    pub(super) fn toggle_mute(&mut self) {
        self.media_muted = !self.media_muted;
        if let Some(play) = &self.playback {
            play.set_muted(self.media_muted);
        }
    }

    pub(super) fn clip_position(&self, href: &str) -> Duration {
        if self.is_loading(href) {
            return Duration::ZERO;
        }
        if let Some((seek_href, at)) = &self.seek_preview
            && seek_href == href
        {
            return *at;
        }
        if let Some(play) = &self.playback
            && play.href() == href
        {
            return play.position();
        }
        if let Some(hold) = &self.hold
            && hold.href == href
        {
            return hold.position;
        }
        Duration::ZERO
    }

    pub(super) fn clip_duration(&self, href: &str) -> Duration {
        if let Some(play) = &self.playback
            && play.href() == href
            && !play.duration().is_zero()
        {
            return play.duration();
        }
        if let Some(hold) = &self.hold
            && hold.href == href
            && !hold.duration.is_zero()
        {
            return hold.duration;
        }
        match self.timings.get(href) {
            Some(TimingState::Ready(timing)) => timing.duration,
            _ => Duration::ZERO,
        }
    }

    pub(super) fn has_timing(&self, href: &str) -> bool {
        self.timings.contains_key(href)
    }

    pub(super) fn ensure_timing(&mut self, href: &str, cx: &mut Context<Root>) {
        if self.timings.contains_key(href) {
            return;
        }
        let Some(id) = self.open_note.clone() else {
            return;
        };
        let path = match self.vault.media_file(&id, href) {
            Ok(path) => path,
            Err(_) => {
                self.timings.insert(
                    href.to_string(),
                    TimingState::Ready(playback::ClipTiming::default()),
                );
                return;
            }
        };
        self.timings.insert(href.to_string(), TimingState::Pending);
        let href = href.to_string();
        let epoch = self.poster_epoch;
        cx.spawn(async move |this, cx| {
            let timing = cx
                .background_executor()
                .spawn(async move { playback::probe_timing(&path) })
                .await;
            this.update(cx, |root, cx| {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                if pane.poster_epoch != epoch {
                    return;
                }
                pane.timings.insert(href, TimingState::Ready(timing));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn begin_seek(&mut self, href: &str, ratio: f32) {
        self.place_seek(href, ratio, true);
    }

    pub(super) fn move_seek(&mut self, href: &str, ratio: f32) {
        if self.seek_drag {
            self.place_seek(href, ratio, true);
        }
    }

    pub(super) fn finish_seek(
        &mut self,
        href: &str,
        kind: ClipKind,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if !self.seek_drag {
            return;
        }
        let Some((seek_href, at)) = self.seek_preview.clone() else {
            self.seek_drag = false;
            return;
        };
        if seek_href != href {
            self.seek_drag = false;
            self.seek_preview = None;
            return;
        }
        self.sync_picture_width(window);
        self.seek_clip(href, kind, at, window, cx);
    }

    fn place_seek(&mut self, href: &str, ratio: f32, drag: bool) {
        let duration = self.clip_duration(href);
        if duration.is_zero() {
            return;
        }
        let at = duration.mul_f32(ratio.clamp(0.0, 1.0));
        self.seek_drag = drag;
        self.seek_preview = Some((href.to_string(), at));
    }

    fn seek_clip(
        &mut self,
        href: &str,
        kind: ClipKind,
        at: Duration,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let playing = self.playing_href() == Some(href);
        self.seek_drag = false;
        self.seek_preview = None;
        let duration = self.clip_duration(href);
        let at = if duration.is_zero() {
            at
        } else {
            at.min(duration)
        };
        if playing && self.film.as_ref().is_some_and(|film| film.href == href) {
            self.end_playback(window, cx);
            self.start_video_clock(href, at, cx);
            return;
        }
        if playing {
            self.end_playback(window, cx);
            self.start_clip(href, kind, at, cx);
            return;
        }
        self.hold = Some(ClipHold {
            href: href.to_string(),
            position: at,
            duration,
        });
        if self.film.as_ref().is_some_and(|film| film.href == href) {
            return;
        }
        self.grab_still(href, at, cx);
    }

    fn grab_still(&mut self, href: &str, at: Duration, cx: &mut Context<Root>) {
        let Some(id) = self.open_note.clone() else {
            return;
        };
        let path = match self.vault.media_file(&id, href) {
            Ok(path) => path,
            Err(_) => return,
        };
        let href = href.to_string();
        let epoch = self.poster_epoch;
        let width = self.picture_width;
        cx.spawn(async move |this, cx| {
            let bytes = cx
                .background_executor()
                .spawn(async move { playback::poster_jpeg(&path, at, width) })
                .await;
            this.update(cx, |root, cx| {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                if pane.poster_epoch != epoch {
                    return;
                }
                let Some(bytes) = bytes else {
                    return;
                };
                pane.posters.insert(
                    href,
                    PosterState::Ready(Arc::new(Image::from_bytes(ImageFormat::Jpeg, bytes))),
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn start_clip(&mut self, href: &str, kind: ClipKind, at: Duration, cx: &mut Context<Root>) {
        if kind == ClipKind::Video {
            self.begin_film(href, at, cx);
            return;
        }
        self.hold = None;
        self.playback_watch = self.playback_watch.wrapping_add(1);
        let watch = self.playback_watch;
        let Some(id) = self.open_note.clone() else {
            self.notice = Some("Open a note first".to_string());
            return;
        };
        let path = match self.vault.media_file(&id, href) {
            Ok(path) => path,
            Err(_) => {
                self.notice = Some("Could not open the media file".to_string());
                return;
            }
        };
        match playback::start(&path, href.to_string(), kind, at, self.media_muted) {
            Ok(started) => {
                let href = started.playback.href().to_string();
                self.timings
                    .insert(href.clone(), TimingState::Ready(started.playback.timing()));
                self.notice = started.notice;
                self.playback = Some(started.playback);
                self.watch_playback(href, watch, cx);
            }
            Err(err) => self.notice = Some(err),
        }
    }

    fn begin_film(&mut self, href: &str, at: Duration, cx: &mut Context<Root>) {
        self.cancel_load();
        self.playback_watch = self.playback_watch.wrapping_add(1);
        let watch = self.playback_watch;
        let Some(id) = self.open_note.clone() else {
            self.notice = Some("Open a note first".to_string());
            return;
        };
        let path = match self.vault.media_file(&id, href) {
            Ok(path) => path,
            Err(_) => {
                self.notice = Some("Could not open the media file".to_string());
                return;
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        self.load_stop = Some(Arc::clone(&stop));
        self.loading = Some(href.to_string());
        let href = href.to_string();
        let width = self.picture_width;
        let renderer = cx.svg_renderer();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move { playback::load_film(&path, width, renderer, &stop) })
                .await;
            this.update_in(cx, |root, window, cx| {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                if pane.playback_watch != watch {
                    if let Ok(film) = loaded {
                        drop_frames(film.frames, window);
                    }
                    return;
                }
                pane.loading = None;
                pane.load_stop = None;
                match loaded {
                    Ok(loaded) if loaded.frames.is_empty() => {
                        pane.notice = Some("Could not open the media file".to_string());
                    }
                    Ok(loaded) => {
                        let mut timing = loaded.timing;
                        timing.duration = loaded.duration;
                        pane.film = Some(Film {
                            href: href.clone(),
                            interval: playback::frame_interval(timing.fps),
                            timing,
                            frames: loaded.frames,
                        });
                        pane.start_video_clock(&href, at, cx);
                    }
                    Err(err) => pane.notice = Some(err),
                }
            })
            .ok();
        })
        .detach();
    }

    fn start_video_clock(&mut self, href: &str, at: Duration, cx: &mut Context<Root>) {
        let Some(timing) = self
            .film
            .as_ref()
            .filter(|film| film.href == href)
            .map(|film| film.timing)
        else {
            self.begin_film(href, at, cx);
            return;
        };
        self.hold = None;
        self.playback_watch = self.playback_watch.wrapping_add(1);
        let watch = self.playback_watch;
        let Some(id) = self.open_note.clone() else {
            self.notice = Some("Open a note first".to_string());
            return;
        };
        let path = match self.vault.media_file(&id, href) {
            Ok(path) => path,
            Err(_) => {
                self.notice = Some("Could not open the media file".to_string());
                return;
            }
        };
        match playback::start_video(&path, href.to_string(), at, self.media_muted, timing) {
            Ok(started) => {
                let href = started.playback.href().to_string();
                self.timings
                    .insert(href.clone(), TimingState::Ready(started.playback.timing()));
                self.notice = started.notice;
                self.playback = Some(started.playback);
                self.watch_playback(href, watch, cx);
            }
            Err(err) => self.notice = Some(err),
        }
    }

    fn watch_playback(&self, href: String, watch: u64, cx: &mut Context<Root>) {
        cx.spawn(async move |this, cx| {
            loop {
                let keep = this
                    .update_in(cx, |root, window, cx| {
                        let Some(pane) = root.pane.as_mut() else {
                            return false;
                        };
                        let keep = pane.poll_playback(&href, watch, window, cx);
                        cx.notify();
                        keep
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(8))
                    .await;
            }
        })
        .detach();
    }

    fn poll_playback(
        &mut self,
        href: &str,
        watch: u64,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        let finished = {
            let Some(play) = self.playback.as_mut() else {
                return false;
            };
            if self.playback_watch != watch || play.href() != href {
                return false;
            }
            play.finished()
        };
        if finished {
            self.pause_playback(window, cx);
            if let Some(hold) = &mut self.hold
                && !hold.duration.is_zero()
            {
                hold.position = hold.duration;
            }
            return false;
        }
        true
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
        if !self.voice_allowed() {
            self.notice = Some("Open a Speech note to record voice".to_string());
            return;
        }
        if !self.groq_ready() {
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
        let Some(key) = self.groq_key() else {
            self.capture = Capture::Voice(capture);
            self.notice = Some(MISSING_GROQ_KEY.to_string());
            return;
        };
        self.capture = Capture::Transcribing;
        let note_id = self.open_note.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { voice_to_text(capture, key) })
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
            self.notice = Some("Open a Sight note to record the screen".to_string());
            return;
        }
        let Some(id) = self.open_note.clone() else {
            self.notice = Some("Open a Sight note to record the screen".to_string());
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
                self.notice = Some(self.redact_notice(&err));
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
        self.require_idle()?;
        let text = self.vault.read(&id).map_err(|err| err.to_string())?;
        self.clear_note_rows();
        self.reveal(&id.parent());
        self.open_note = Some(id.clone());
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

    /// Load another note after the active one is gone. An empty vault opens the Archivist.
    fn settle_after_delete(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<(), String> {
        if self.take_next_note(&[], window, cx) {
            return Ok(());
        }
        self.clear_editor(window, cx);
        self.archivist.show();
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
        self.dismiss_media(window, cx);
        self.open_note = None;
        self.open_kind = NoteKind::Book;
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

    /// Show or hide the Archivist column.
    pub(crate) fn toggle_archivist(&mut self) {
        self.archivist.toggle();
    }

    /// Whether the Archivist column is open.
    pub(crate) fn archivist_open(&self) -> bool {
        self.archivist.is_open()
    }

    /// The Archivist session drawn in the dock panel.
    pub(crate) fn archivist(&self) -> &crate::archivist::Panel {
        &self.archivist
    }

    /// Send the draft to the Archivist. A busy turn or a missing key does nothing.
    ///
    /// Held rows are included even when the draft is empty.
    pub(crate) fn send_archivist(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if !self.archivist.is_open() || self.archivist.is_busy() || !self.groq_ready() {
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
        let Some(access) = self.archivist_access() else {
            return;
        };
        let vault = self.vault.clone();
        let epoch = self.epoch;
        self.archivist.launch(text, vault, epoch, access, cx);
    }

    fn press_note_row(&mut self, index: usize, shift: bool) {
        self.row_rewrite = None;
        self.row_gesture = rows::RowGesture::press(self.row_gesture.take(), index, shift);
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
        if !self.groq_ready() {
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
        let Some(id) = open else {
            if self.open_note.is_some() && self.capture_status().is_some() {
                self.notice =
                    Some("Stop the recording. The Archivist deleted this note.".to_string());
                return;
            }
            if !self.take_next_note(changes, window, cx) && self.open_note.is_some() {
                self.clear_editor(window, cx);
                self.archivist.show();
            }
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
        self.clear_note_rows();
        self.latest = next.clone();
        self.dirty = self.open_note.is_some() && next != self.saved_text;
        self.editor
            .update(cx, |state, cx| state.set_value(next, window, cx));
        self.dirty
    }

    fn apply_file(&mut self, text: &str, window: &mut Window, cx: &mut Context<Root>) {
        self.dismiss_media(window, cx);
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
        self.playback = None;
        if !self.dirty {
            return;
        }
        if let Some(id) = self.open_note.clone() {
            let file = encode_note(self.open_kind, &self.latest);
            let _ = self.vault.write(&id, &file);
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

/// Ask which display or window a new Sight note should record.
fn prompt_record_source(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) {
    let mut rows = vec![("Full display".to_string(), ScreenTarget::Display)];
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
        pane.notice = Some(WINDOW_LIST_ERROR.to_string());
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
                    .child(WINDOW_LIST_ERROR),
            );
        }
        for (index, (label, target)) in rows.iter().cloned().enumerate() {
            column = column.child(record_source_button(view.clone(), index, label, target));
        }
        dialog
            .title("Choose a screen")
            .w(px(420.))
            .child(column)
            .footer(
                Button::new("record-cancel")
                    .label("Cancel")
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
            .footer(dialog_footer("Cancel", "Create", ButtonVariant::Primary))
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

fn note_body(text: &str) -> (NoteKind, String) {
    let (kind, body) = decode_note(text);
    (kind, body.to_string())
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
        NoteKind::Book => "Book Runes (Bókrunar)",
        NoteKind::Speech => "Speech Runes (Málrunar)",
        NoteKind::Sight => "Sight Runes (Sjónrunar)",
        NoteKind::Deed => "Deed Runes (Verkrunar)",
        NoteKind::Research => "Research Runes (Rannsóknarrúnar)",
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
