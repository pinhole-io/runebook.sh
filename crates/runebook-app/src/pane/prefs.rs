//! Settings and model actions for the open vault.

use super::*;

impl EditorPane {
    /// Open or close settings in the note area.
    ///
    /// Opening settings builds the theme list, the microphone list, and the model list.
    pub(super) fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.show_themes(window, cx);
            self.begin_microphone_load(cx);
            self.schedule_model_list_load(Duration::ZERO, cx);
            self.prepare_keys(window, cx);
        } else {
            self.close_keys();
        }
    }

    /// Fill the theme menu from the catalog. A theme added while Settings is open
    /// shows up the next time Settings opens.
    pub(super) fn show_themes(&mut self, window: &mut Window, cx: &mut Context<Root>) {
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
    pub(super) fn schedule_model_list_load(&mut self, delay: Duration, cx: &mut Context<Root>) {
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

    pub(super) fn show_model_selects(
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

    pub(super) fn model_select(
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
    pub(super) fn apply_listed_model(&mut self, field: ModelField, name: String) {
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
    pub(super) fn apply_theme(&mut self, name: String, cx: &mut Context<Root>) {
        let Some(loaded) = crate::theme::catalog().by_name(&name).cloned() else {
            return;
        };
        let size = self.text_size;
        crate::theme::apply(&loaded, cx);
        self.choose_theme(name);
        crate::theme::apply_text_size(size, cx);
    }

    pub(super) fn begin_microphone_load(&mut self, cx: &mut Context<Root>) {
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

    pub(super) fn show_microphones(
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

    pub(super) fn apply_microphone(&mut self, value: Option<String>) {
        let name = match value.as_deref() {
            Some(name) if name == locale::text("mic.system") => None,
            None => None,
            Some(name) => Some(name.to_string()),
        };
        self.microphone = name.clone();
        self.store_setting(|config| config.microphone = name);
    }

    /// Save the window language and refresh text that was captured earlier.
    pub(super) fn choose_locale(
        &mut self,
        code: &str,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
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

    pub(super) fn refresh_placeholders(&mut self, window: &mut Window, cx: &mut Context<Root>) {
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
    pub(super) fn choose_theme(&mut self, name: String) {
        self.theme = name.clone();
        self.store_setting(move |config| config.theme = name);
    }

    /// Save the text size. The caller already applied it to the window.
    pub(super) fn choose_text_size(&mut self, size: TextSize) {
        self.text_size = size;
        self.store_setting(move |config| config.text_size = size);
    }

    /// Show or hide source line numbers, and save that choice.
    pub(super) fn choose_line_numbers(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if self.line_numbers == on {
            return;
        }
        self.line_numbers = on;
        self.editor
            .update(cx, |state, cx| state.set_line_number(on, window, cx));
        self.store_setting(move |config| config.line_numbers = on);
    }

    /// Wrap or scroll long source lines, and save that choice.
    pub(super) fn choose_soft_wrap(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if self.soft_wrap == on {
            return;
        }
        self.soft_wrap = on;
        self.editor
            .update(cx, |state, cx| state.set_soft_wrap(on, window, cx));
        self.store_setting(move |config| config.soft_wrap = on);
    }

    /// Save when edits are written. Manual waits for leave or close.
    pub(super) fn choose_autosave(&mut self, pace: AutosavePace) {
        self.autosave = pace;
        self.store_setting(move |config| config.autosave = pace);
    }

    /// Delay before the next autosave. Manual is `None`.
    pub(crate) fn autosave_delay(&self) -> Option<Duration> {
        self.autosave.delay_ms().map(Duration::from_millis)
    }

    /// Save the Groq transcription model.
    pub(super) fn choose_speech_model(&mut self, model: SpeechModel) {
        self.speech_model = model;
        self.store_setting(move |config| config.speech_model = model);
    }

    /// Start new clips muted, and apply that to the clip that is playing.
    pub(super) fn choose_start_muted(&mut self, on: bool) {
        self.start_muted = on;
        self.media_muted = on;
        if let Some(play) = &self.playback {
            play.set_muted(on);
        }
        self.store_setting(move |config| config.media_muted = on);
    }

    /// Save whether the next launch opens the last note.
    pub(super) fn choose_reopen_last_note(&mut self, on: bool) {
        self.reopen_last_note = on;
        self.store_setting(move |config| config.reopen_last_note = on);
        if on && let Some(id) = self.open_note.clone() {
            self.remember_note(&id);
        }
    }

    /// Remember `id` for the next launch.
    pub(super) fn remember_note(&mut self, id: &NoteId) {
        let saved = id.to_string();
        if self.remembered_note.as_deref() == Some(saved.as_str()) {
            return;
        }
        self.remembered_note = Some(saved.clone());
        self.store_setting(move |config| config.last_note = Some(saved));
    }

    /// Save the default note view and show that view on the open note.
    pub(super) fn choose_note_view(
        &mut self,
        view: NoteView,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        self.default_note_view = view;
        self.show_note_view(view, window, cx);
        let choice = note_view_choice(view);
        self.store_setting(|config| config.note_view = choice);
    }

    pub(super) fn show_note_view(
        &mut self,
        view: NoteView,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if self.research_note_open() {
            self.note_view = NoteView::Preview;
            return;
        }
        self.note_view = view;
        if view == NoteView::Source {
            self.editor.focus_handle(cx).focus(window, cx);
        }
    }

    pub(super) fn store_setting(&mut self, change: impl FnOnce(&mut AppConfig)) {
        if let Err(err) = save_setting(change) {
            self.notice = Some(self.redact_notice(&err));
        }
    }

    pub(super) fn redact_notice(&self, text: &str) -> String {
        let mut secrets = vec![
            self.models.groq_api_key.clone(),
            self.models.openai_api_key.clone(),
        ];
        secrets.extend(models::env_secrets());
        models::redact_secrets(text, secrets.iter().map(String::as_str))
    }

    pub(super) fn saved_secrets(&self) -> Vec<String> {
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

    pub(crate) fn archivist_ready(&self) -> bool {
        self.archivist_access().is_some()
    }

    pub(crate) fn archivist_key_message(&self) -> &'static str {
        crate::archivist::missing_model_key(&models::resolve_archivist_model(
            &self.models.archivist_model,
        ))
    }

    pub(super) fn archivist_access(&self) -> Option<crate::archivist::ModelAccess> {
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

    pub(super) fn groq_key(&self) -> Option<String> {
        models::resolve_groq_key(&self.models.groq_api_key)
    }

    pub(super) fn apply_model_field(&mut self, field: ModelField, cx: &mut Context<Root>) {
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

    pub(super) fn apply_speech_language(&mut self, text: &str) {
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

    pub(super) fn store_models(&mut self) {
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
}

pub(super) fn model_input(
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

pub(super) fn watch_model(
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

pub(super) fn saved_microphone(name: Option<String>) -> Option<String> {
    name.filter(|value| !value.trim().is_empty())
}

pub(super) fn note_view_from(choice: NoteViewChoice) -> NoteView {
    match choice {
        NoteViewChoice::Preview => NoteView::Preview,
        NoteViewChoice::Source => NoteView::Source,
    }
}

pub(super) fn note_view_choice(view: NoteView) -> NoteViewChoice {
    match view {
        NoteView::Preview => NoteViewChoice::Preview,
        NoteView::Source => NoteViewChoice::Source,
    }
}
