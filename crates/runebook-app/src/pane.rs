//! Vault workspace: file tree, source editor, markdown preview, and row selection.

mod chat;
mod disk;
mod edit;
mod keys_page;
mod kind_dialog;
mod names;
mod prefs;
mod preview;
mod record;
mod research;
mod rows;
mod settings;
mod tasks;
mod tree;
mod view;

use chat::*;
use disk::*;
use edit::*;
use prefs::*;
use record::*;

pub(crate) use disk::{present_scan, scan_vault};

pub(crate) use preview::queue_clips;
pub(crate) use tree::files;
pub(crate) use view::{note, status_bar};

use rows::{
    context_message, line_span, rewrite_message, selected_span, selection_label, split_rows,
    usable_selection,
};

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
    App, AppContext, ClickEvent, Context, Entity, FocusHandle, Focusable, Image, ImageFormat,
    PathPromptOptions, RenderImage, SharedString, Subscription, Window, div, px,
};
use runebook::{
    AppConfig, AutosavePace, Error, FolderId, NoteId, NoteKind, NoteViewChoice, SearchHit,
    SpeechModel, TaskDate, TextSize, Vault, VaultIndex, VaultStamp, VaultTree, decode_note,
    encode_note, toggle_task_line,
};

use crate::Root;
use crate::capture::{
    ScreenCapture, ScreenTarget, SpeechRequest, VoiceCapture, append_block, list_input_devices,
    list_record_sources, markdown_link, utc_stamp_now, voice_to_text, window_list_error,
};
use crate::locale;
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
    stamp: VaultStamp,
    open_note: Option<NoteId>,
    open_kind: NoteKind,
    tree_selection: TreeSelection,
    collapsed: HashSet<FolderId>,
    note_view: NoteView,
    default_note_view: NoteView,
    settings_open: bool,
    tasks_open: bool,
    tasks_show_done: bool,
    key_fields: Option<keys_page::KeyFields>,
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
    /// Focus for the preview, so Ctrl+C reaches the row selection.
    preview_focus: FocusHandle,
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
    /// Note that owns the live voice or screen capture.
    recording_note: Option<NoteId>,
    prompt: Option<Prompt>,
    rename_blur: Option<Subscription>,
    search: Option<Entity<InputState>>,
    search_hits: Vec<SearchHit>,
    search_truncated: bool,
    _search_watch: Option<Subscription>,
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
        let preview_focus = cx.focus_handle();
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
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder(locale::text("search.placeholder")));
        let search_watch = cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(pane) = this.pane.as_mut()
            {
                pane.refresh_search(cx);
                cx.notify();
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
            preview_focus,
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
            tasks_open: false,
            tasks_show_done: false,
            key_fields: None,
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
            playback: None,
            playback_watch: 0,
            hold: None,
            media_muted: prefs.media_muted,
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
            recording_note: None,
            prompt: None,
            rename_blur: None,
            search: Some(search),
            search_hits: Vec::new(),
            search_truncated: false,
            _search_watch: Some(search_watch),
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

    pub(super) fn root(&self) -> &Path {
        self.vault.root()
    }

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
            self.notice = Some(locale::text("media.open_note").to_string());
            return;
        };
        let path = match self.vault.media_file(&id, href) {
            Ok(path) => path,
            Err(_) => {
                self.notice = Some(locale::text("media.open_failed").to_string());
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
            self.notice = Some(locale::text("media.open_note").to_string());
            return;
        };
        let path = match self.vault.media_file(&id, href) {
            Ok(path) => path,
            Err(_) => {
                self.notice = Some(locale::text("media.open_failed").to_string());
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
                        pane.notice = Some(locale::text("media.open_failed").to_string());
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
            self.notice = Some(locale::text("media.open_note").to_string());
            return;
        };
        let path = match self.vault.media_file(&id, href) {
            Ok(path) => path,
            Err(_) => {
                self.notice = Some(locale::text("media.open_failed").to_string());
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
}

fn next_epoch() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
