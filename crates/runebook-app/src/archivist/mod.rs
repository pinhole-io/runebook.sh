//! Archivist chat. Superglue runs the tool loop. The vault crate writes the files.

mod attach;
mod column;
mod tools;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui_kit::component::input::TextareaState;
use gpui_kit::{AppContext, Context, Entity, ScrollHandle, SharedString, Window};
use runebook::{FolderId, NoteId, Vault};
use superglue::agents::AgentSpec;
use superglue::openai::ChatMessage;
use superglue::providers::{ProviderId, parse_model_ref};
use superglue::{CallOptions, Client, ClientConversation};

pub(crate) use column::column;

use crate::Root;
use crate::locale;

/// Notice when the Archivist model has no key.
pub(crate) fn missing_model_key(model: &str) -> &'static str {
    if parse_model_ref(model).provider == ProviderId::Groq {
        locale::text("capture.groq_key")
    } else {
        locale::text("capture.openai_key")
    }
}

/// Keys, model, and tool-round cap for one Archivist session.
///
/// `groq_key` is only the Groq provider key. `openai_key` is the
/// OpenAI-compatible key. An empty string means that key is unset.
pub(crate) struct ModelAccess {
    pub(crate) groq_key: String,
    pub(crate) openai_key: String,
    pub(crate) openai_base_url: String,
    pub(crate) model: String,
    pub(crate) tool_rounds: u32,
}

/// A vault change made by a tool during one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VaultChange {
    Wrote(NoteId),
    RenamedNote { from: NoteId, to: NoteId },
    DeletedNote(NoteId),
    Opened(NoteId),
    Folder(FolderId),
    RenamedFolder { from: FolderId, to: FolderId },
    DeletedFolder(FolderId),
}

enum Line {
    User(String),
    Archivist {
        thinking: SharedString,
        body: SharedString,
    },
    Error(String),
}

/// Markdown source kept as one allocation so the text view can parse it once.
#[derive(Clone, Default)]
struct RenderedMarkdown {
    thinking: SharedString,
    body: SharedString,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct LiveText {
    thinking: String,
    body: String,
}

struct Session {
    conversation: ClientConversation,
    changes: Arc<Mutex<Vec<VaultChange>>>,
}

pub(crate) struct Turn {
    session: Option<Session>,
    thinking: String,
    body: String,
    error: Option<String>,
    changes: Vec<VaultChange>,
}

impl Turn {
    /// A turn whose task ended before it could return a session.
    fn stopped() -> Self {
        Self {
            session: None,
            thinking: String::new(),
            body: String::new(),
            error: Some(locale::text("archivist.stopped").to_string()),
            changes: Vec::new(),
        }
    }
}

/// Transcript and the superglue session for one vault.
pub(crate) struct Panel {
    busy: bool,
    lines: Vec<Line>,
    live: LiveText,
    live_rendered: RenderedMarkdown,
    draft: Entity<TextareaState>,
    log_scroll: ScrollHandle,
    session: Option<Session>,
    drop_session: bool,
    turn: u64,
    /// Rows parked by Add to chat. The next send includes them.
    held: Vec<HeldRows>,
    /// Files parked by Add file. The next send includes them.
    files: Vec<attach::ParkedFile>,
    /// `true` while parked files are read, before the model turn starts.
    reading: bool,
}

/// Rows the user added to the chat and has not sent yet.
struct HeldRows {
    note_id: String,
    source: String,
    /// File name and line range shown in the column, such as `test.md · 100-200`.
    label: String,
}

impl Panel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Root>) -> Self {
        let draft = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(locale::text("archivist.placeholder"))
                .submit_on_enter(true)
                .auto_grow(1, 8)
        });
        Self {
            busy: false,
            lines: Vec::new(),
            live: LiveText::default(),
            live_rendered: RenderedMarkdown::default(),
            draft,
            log_scroll: ScrollHandle::new(),
            session: None,
            drop_session: false,
            turn: 0,
            held: Vec::new(),
            files: Vec::new(),
            reading: false,
        }
    }

    pub(crate) fn has_held(&self) -> bool {
        !self.held.is_empty()
    }

    /// Remember rows until the next send. An empty body is ignored.
    ///
    /// `label` is the file name and the line range. The same lines replace
    /// the parked text, so a later edit of that range stays current.
    pub(crate) fn hold_rows(&mut self, note_id: String, source: String, label: String) {
        let source = source.trim_matches(['\n', '\r']).to_string();
        if source.trim().is_empty() {
            return;
        }
        if let Some(row) = self
            .held
            .iter_mut()
            .find(|row| row.note_id == note_id && row.label == label)
        {
            row.source = source;
            return;
        }
        self.held.push(HeldRows {
            note_id,
            source,
            label,
        });
    }

    pub(crate) fn dismiss_held(&mut self, index: usize) {
        if index < self.held.len() {
            self.held.remove(index);
        }
    }

    /// Drop parked rows for one note after a rewrite of that note.
    pub(crate) fn drop_held_note(&mut self, note_id: &str) {
        self.held.retain(|row| row.note_id != note_id);
    }

    pub(crate) fn take_held(&mut self) -> Vec<(String, String, String)> {
        self.held
            .drain(..)
            .map(|row| (row.note_id, row.source, row.label))
            .collect()
    }

    /// Copy parked rows. The send keeps them until the file read succeeds.
    pub(crate) fn held_pairs(&self) -> Vec<(String, String)> {
        self.held
            .iter()
            .map(|row| (row.note_id.clone(), row.source.clone()))
            .collect()
    }

    /// Drop the rows that this send already copied.
    pub(crate) fn drop_sent_held(&mut self, sent: &[(String, String)]) {
        for (note_id, source) in sent {
            if let Some(index) = self
                .held
                .iter()
                .position(|row| row.note_id == *note_id && row.source == *source)
            {
                self.held.remove(index);
            }
        }
    }

    pub(crate) fn has_files(&self) -> bool {
        !self.files.is_empty()
    }

    /// Park files for the next send. More than 8 files adds none of them.
    pub(crate) fn hold_files(&mut self, paths: Vec<PathBuf>) -> Result<(), String> {
        let mut added = Vec::new();
        for path in paths {
            if self.files.iter().any(|file| file.path == path) {
                continue;
            }
            let name = attach::file_name(&path);
            added.push(attach::ParkedFile { path, name });
        }
        if self.files.len() + added.len() > attach::MAX_FILES {
            return Err(locale::text("archivist.max_files").to_string());
        }
        if added.is_empty() {
            return Ok(());
        }
        self.files.extend(added);
        Ok(())
    }

    pub(crate) fn dismiss_file(&mut self, index: usize) {
        if self.busy || index >= self.files.len() {
            return;
        }
        self.files.remove(index);
    }

    pub(crate) fn file_paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }

    /// Drop the paths that this send already copied.
    pub(crate) fn drop_sent_files(&mut self, paths: &[PathBuf]) {
        self.files.retain(|file| !paths.contains(&file.path));
    }

    /// Show an error and leave parked files in place.
    pub(crate) fn note_error(&mut self, error: String) {
        self.lines.push(Line::Error(error));
        self.follow_log();
    }

    /// Mark the panel busy while files are read.
    pub(crate) fn begin_read(&mut self) -> bool {
        if self.busy {
            return false;
        }
        self.busy = true;
        self.reading = true;
        true
    }

    /// The file read failed. Parked files stay.
    pub(crate) fn fail_read(&mut self, error: String) {
        if !self.reading {
            return;
        }
        self.reading = false;
        self.busy = false;
        self.lines.push(Line::Error(error));
        self.follow_log();
    }

    /// The vault changed before the file read finished.
    pub(crate) fn cancel_read(&mut self) {
        if !self.reading {
            return;
        }
        self.reading = false;
        self.busy = false;
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.busy
    }

    /// Draft text with surrounding whitespace removed.
    pub(crate) fn draft_text(&self, cx: &Context<Root>) -> String {
        self.draft.read(cx).value().trim().to_string()
    }

    /// Set the draft placeholder from the active locale.
    pub(crate) fn set_draft_placeholder(&self, window: &mut Window, cx: &mut Context<Root>) {
        let label = locale::text("archivist.placeholder");
        self.draft
            .update(cx, |state, cx| state.set_placeholder(label, window, cx));
    }

    /// Clear the draft after a send is accepted.
    pub(crate) fn clear_draft(&self, window: &mut Window, cx: &mut Context<Root>) {
        self.draft
            .update(cx, |state, cx| state.set_value(String::new(), window, cx));
    }

    /// Clear the draft when it is still the text that was sent.
    pub(crate) fn clear_draft_if(&self, sent: &str, window: &mut Window, cx: &mut Context<Root>) {
        if self.draft_text(cx) == sent {
            self.clear_draft(window, cx);
        }
    }

    /// Take the session and mark this turn busy.
    fn begin(&mut self, text: String) -> (Option<Session>, u64) {
        self.reading = false;
        self.busy = true;
        self.live = LiveText::default();
        self.live_rendered = RenderedMarkdown::default();
        self.lines.push(Line::User(text));
        self.turn = self.turn.wrapping_add(1);
        self.log_scroll.scroll_to_bottom();
        (self.session.take(), self.turn)
    }

    /// Run one turn on the Archivist runtime and apply vault changes on `epoch`.
    /// Drop the saved session so the next turn reads Settings again.
    pub(crate) fn end_session(&mut self) {
        if self.busy {
            self.drop_session = true;
        }
        self.session = None;
    }

    pub(crate) fn launch(
        &mut self,
        text: String,
        vault: Vault,
        epoch: u64,
        access: ModelAccess,
        cx: &mut Context<Root>,
    ) {
        let message = ChatMessage::text("user", text.clone());
        self.launch_message(text, message, vault, epoch, access, cx);
    }

    /// Read parked files, then start the model turn.
    ///
    /// A bad file leaves the parked files in place and shows the error.
    pub(crate) fn launch_files(
        &mut self,
        instruction: String,
        draft: String,
        held: Vec<(String, String)>,
        paths: Vec<PathBuf>,
        vault: Vault,
        epoch: u64,
        access: ModelAccess,
        cx: &mut Context<Root>,
    ) {
        if !self.reading {
            return;
        }
        let sent_paths = paths.clone();
        cx.spawn(async move |this, cx| {
            let built = runtime()
                .spawn_blocking(move || attach::prepare(&instruction, &paths))
                .await;
            let built = match built {
                Ok(result) => result,
                Err(_) => Err(locale::text("archivist.read_failed").to_string()),
            };
            this.update_in(cx, |root, window, cx| {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                if !pane.same_epoch(epoch) {
                    pane.cancel_archivist_read();
                    cx.notify();
                    return;
                }
                match built {
                    Err(err) => pane.fail_archivist_read(err),
                    Ok((display, message)) => pane.accept_archivist_files(
                        display,
                        message,
                        &held,
                        &sent_paths,
                        &draft,
                        vault,
                        epoch,
                        access,
                        window,
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn launch_message(
        &mut self,
        display: String,
        message: ChatMessage,
        vault: Vault,
        epoch: u64,
        access: ModelAccess,
        cx: &mut Context<Root>,
    ) {
        let (session, turn) = self.begin(display);
        let draft = Arc::new(Mutex::new(LiveText::default()));
        let watch = Arc::clone(&draft);
        cx.spawn(async move |this, cx| {
            let task = runtime()
                .spawn(async move { run_turn(session, vault, message, draft, access).await });
            loop {
                let pending = !task.is_finished();
                let snap = live_snapshot(&watch);
                this.update_in(cx, |root, _, cx| {
                    let Some(pane) = root.pane.as_mut() else {
                        return;
                    };
                    if pane.same_epoch(epoch) && pane.show_archivist_live(turn, snap) {
                        cx.notify();
                    }
                })
                .ok();
                if !pending {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            let done = match task.await {
                Ok(done) => done,
                Err(_) => Turn::stopped(),
            };
            this.update_in(cx, |root, window, cx| {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                if !pane.same_epoch(epoch) {
                    return;
                }
                pane.finish_archivist(turn, done, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Replace the in-progress reply for this turn. Returns whether the text changed.
    pub(crate) fn show_live(&mut self, turn: u64, next: LiveText) -> bool {
        if turn != self.turn || !self.busy || next == self.live {
            return false;
        }
        self.live_rendered = RenderedMarkdown {
            thinking: SharedString::from(next.thinking.as_str()),
            body: SharedString::from(next.body.as_str()),
        };
        self.live = next;
        self.follow_log();
        true
    }

    /// Store the reply. Returns the vault changes from this turn.
    ///
    /// A stale `turn` leaves the panel unchanged.
    pub(crate) fn finish(&mut self, turn: u64, done: Turn) -> Vec<VaultChange> {
        if turn != self.turn {
            return Vec::new();
        }
        self.busy = false;
        self.live = LiveText::default();
        self.live_rendered = RenderedMarkdown::default();
        self.session = if self.drop_session {
            self.drop_session = false;
            None
        } else {
            done.session
        };
        let empty = done.thinking.trim().is_empty() && done.body.trim().is_empty();
        if done.error.is_none() && empty {
            self.lines.push(Line::Archivist {
                thinking: SharedString::default(),
                body: SharedString::from(locale::text("archivist.no_text")),
            });
        } else if !empty {
            self.lines.push(Line::Archivist {
                thinking: SharedString::from(done.thinking),
                body: SharedString::from(done.body),
            });
        }
        if let Some(err) = done.error {
            self.lines.push(Line::Error(err));
        }
        self.follow_log();
        done.changes
    }
}

impl Panel {
    /// Keep the latest text in view while the log is already at the bottom.
    fn follow_log(&self) {
        if log_follows(
            self.log_scroll.offset().y.as_f32(),
            self.log_scroll.max_offset().y.as_f32(),
        ) {
            self.log_scroll.scroll_to_bottom();
        }
    }
}

/// `true` when the viewport is at, or near, the bottom of the log.
fn log_follows(offset_y: f32, max_y: f32) -> bool {
    offset_y + max_y < 48.
}

#[cfg(test)]
mod tests {
    use super::{archivist_prompt, key_plan, log_follows, missing_model_key};
    use superglue::providers::ProviderId;

    #[test]
    fn the_log_follows_while_it_is_near_the_bottom() {
        assert!(log_follows(0., 0.));
        assert!(log_follows(-200., 200.));
        assert!(log_follows(-170., 200.));
        assert!(!log_follows(-100., 200.));
    }

    #[test]
    fn an_openai_model_does_not_use_the_groq_key() {
        let plan = key_plan(
            "openai:gpt-6-luna",
            "gsk-groq-key-value",
            "sk-openai-key-value",
            "https://gateway.example/v1",
        )
        .expect("both keys are set");
        assert_eq!(plan.primary, "sk-openai-key-value");
        assert_eq!(plan.groq.as_deref(), Some("gsk-groq-key-value"));
        assert_eq!(plan.openai.as_deref(), Some("sk-openai-key-value"));
        assert_eq!(plan.openai_provider, ProviderId::OpenAi);
        assert_eq!(plan.base, "https://gateway.example/v1");
    }

    #[test]
    fn an_openai_model_rejects_a_groq_key_used_alone() {
        let err = key_plan(
            "openai:gpt-6-luna",
            "gsk-groq-key-value",
            "",
            "https://api.openai.com",
        )
        .expect_err("the OpenAI key is required");
        assert_eq!(err, missing_model_key("openai:gpt-6-luna"));
        assert_eq!(err, "Set the OpenAI key in Settings.");
    }

    #[test]
    fn a_groq_model_keeps_its_own_key() {
        let plan = key_plan(
            "groq:llama-3.3-70b-versatile",
            "gsk-groq-key-value",
            "sk-openai-key-value",
            "https://api.openai.com",
        )
        .expect("the Groq key is set");
        assert_eq!(plan.groq.as_deref(), Some("gsk-groq-key-value"));
        assert_eq!(plan.primary, "sk-openai-key-value");
        assert_eq!(plan.openai_provider, ProviderId::OpenAi);
    }

    #[test]
    fn archivist_prompt_describes_the_vault() {
        let prompt = archivist_prompt(8);
        assert!(
            prompt.starts_with("You are the Archivist of this vault."),
            "{prompt}"
        );
        for kind in ["Book", "Speech", "Sight", "Deed", "Research"] {
            assert!(prompt.contains(kind), "missing {kind} in {prompt}");
        }
        assert!(prompt.contains("kind: speech"), "{prompt}");
        assert!(prompt.contains("[[target|alias]]"), "{prompt}");
        assert!(prompt.contains("32 KiB"), "{prompt}");
    }
}

fn live_snapshot(draft: &Mutex<LiveText>) -> LiveText {
    draft.lock().unwrap_or_else(|err| err.into_inner()).clone()
}

fn push_live(draft: &Mutex<LiveText>, thinking: bool, delta: String) {
    let mut live = draft.lock().unwrap_or_else(|err| err.into_inner());
    if thinking {
        live.thinking.push_str(&delta);
    } else {
        live.body.push_str(&delta);
    }
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_name("runebook-archivist")
            .enable_all()
            .build()
            .expect("archivist runtime")
    })
}

async fn run_turn(
    session: Option<Session>,
    vault: Vault,
    message: ChatMessage,
    draft: Arc<Mutex<LiveText>>,
    access: ModelAccess,
) -> Turn {
    let mut session = match session {
        Some(session) => session,
        None => match open_session(vault, &access).await {
            Ok(session) => session,
            Err(err) => {
                return Turn {
                    session: None,
                    thinking: String::new(),
                    body: String::new(),
                    error: Some(err),
                    changes: Vec::new(),
                };
            }
        },
    };
    session.conversation.push_user_message(message);
    let sink = Arc::clone(&draft);
    let result = session
        .conversation
        .stream(
            CallOptions::default(),
            {
                let sink = Arc::clone(&sink);
                move |delta| push_live(&sink, false, delta)
            },
            {
                let sink = Arc::clone(&sink);
                move |delta| push_live(&sink, true, delta)
            },
        )
        .await;
    let changes = std::mem::take(
        &mut *session
            .changes
            .lock()
            .unwrap_or_else(|err| err.into_inner()),
    );
    let live = std::mem::take(&mut *draft.lock().unwrap_or_else(|err| err.into_inner()));
    let (body, error) = match result {
        Ok(outcome) => {
            let body = if live.body.is_empty() {
                outcome.content
            } else {
                live.body
            };
            (body, None)
        }
        Err(err) => (
            live.body,
            Some(redact(
                err.to_string(),
                &[&access.groq_key, &access.openai_key],
            )),
        ),
    };
    Turn {
        session: Some(session),
        thinking: live.thinking,
        body,
        error,
        changes,
    }
}

/// Which key `ClientBuilder` receives.
///
/// `ClientBuilder::api_key_for` copies `api_key` onto OpenAI when that
/// provider has no key. `primary` is the OpenAI key when one is set, so the
/// Groq key cannot land on OpenAI.
#[derive(Debug)]
struct KeyPlan {
    primary: String,
    base: String,
    groq: Option<String>,
    openai: Option<String>,
    openai_provider: ProviderId,
}

/// `true` when `model` has the key it sends with.
pub(crate) fn has_model_key(model: &str, groq: &str, openai: &str) -> bool {
    key_plan(model, groq, openai, ProviderId::OpenAi.default_base_url()).is_ok()
}

fn key_plan(model: &str, groq: &str, openai: &str, base: &str) -> Result<KeyPlan, String> {
    let groq = groq.trim();
    let openai = openai.trim();
    let provider = parse_model_ref(model).provider;
    let needed = if provider == ProviderId::Groq {
        groq
    } else {
        openai
    };
    if needed.is_empty() {
        return Err(missing_model_key(model).to_string());
    }
    let base = base.trim();
    let base = if base.is_empty() {
        provider.default_base_url().to_string()
    } else {
        base.to_string()
    };
    let primary = if openai.is_empty() { groq } else { openai };
    let openai_provider = if provider == ProviderId::Groq {
        ProviderId::OpenAi
    } else {
        provider
    };
    Ok(KeyPlan {
        primary: primary.to_string(),
        base,
        groq: (!groq.is_empty()).then(|| groq.to_string()),
        openai: (!openai.is_empty()).then(|| openai.to_string()),
        openai_provider,
    })
}

async fn open_session(vault: Vault, access: &ModelAccess) -> Result<Session, String> {
    let plan = key_plan(
        &access.model,
        &access.groq_key,
        &access.openai_key,
        &access.openai_base_url,
    )?;
    let rounds = if access.tool_rounds < 1 {
        runebook::DEFAULT_TOOL_ROUNDS
    } else {
        access.tool_rounds
    };
    let prompt = archivist_prompt(rounds);
    let mut builder = Client::builder()
        .api_key(plan.primary.clone())
        .base_url(plan.base.clone())
        .model(access.model.clone())
        .system_prompt(prompt)
        .max_tool_rounds(rounds)
        // Chat completions hide this summary: gpt-6-luna rejects tools unless effort is none.
        // The OpenAI turn uses the Responses stream, which keeps tools and returns the summary.
        .reasoning_effort("low");
    if let Some(groq) = plan.groq.as_deref() {
        builder = builder.api_key_for(ProviderId::Groq, groq);
    }
    if let Some(openai) = plan.openai.as_deref() {
        builder = builder
            .api_key_for(plan.openai_provider, openai)
            .base_url_for(plan.openai_provider, plan.base.clone());
    }
    let secrets = [
        access.groq_key.as_str(),
        access.openai_key.as_str(),
        plan.primary.as_str(),
    ];
    let client = builder
        .build()
        .map_err(|err| redact(err.to_string(), &secrets))?;
    let changes = Arc::new(Mutex::new(Vec::new()));
    tools::register(&client, vault, Arc::clone(&changes))
        .await
        .map_err(|err| redact(err, &secrets))?;
    Ok(Session {
        conversation: client.conversation(),
        changes,
    })
}

fn archivist_prompt(rounds: u32) -> String {
    let spec = AgentSpec::new("Archivist", "the Archivist of this vault")
        .with_max_tool_rounds(rounds)
        .with_goal("Keep each note accurate.")
        .with_goal("Find the real id before you change a file.")
        .with_goal("Use a tool for every vault change.")
        .with_goal("Change only the text the user asked to change.")
        .with_goal("Report each note id and folder id you changed.")
        .with_instructions(
            "A note id is a vault path with no .md suffix. Example: journal/2026-10-23. \
             A folder id uses the same shape. Example: journal. \
             An id is relative. It is not absolute, not hidden, and it has no . or .. segment. \
             Call list_notes or search_notes before you use an id.",
        )
        .with_instructions(
            "A note is Book, Speech, Sight, Deed, or Research. \
             Book has no header. Speech, Sight, Deed, and Research start with this header:\n\
             ---\n\
             kind: speech\n\
             ---\n\
             The kind key is speech, sight, deed, or research. \
             write_note keeps the old kind when the new text has no header. \
             Put a header in contents only when the user asked for that kind.",
        )
        .with_instructions(
            "Speech is a transcript. You do not record audio. \
             Sight is a markdown link to a video beside the note. You do not record the screen. \
             Deed is a task list. A task line uses `- [ ]` or `- [x]`. \
             Research belongs to Charlotte. You do not start a Research run. \
             You may still edit those notes when the user asks.",
        )
        .with_instructions(
            "`[[target]]` links to a note. `[[target|alias]]` shows the alias and still links to \
             target. Prefer an exact note id. A stem with no / matches one note only when that \
             stem is unique. After a rename, search for the old id and update wiki links that \
             use it.",
        )
        .with_instructions(
            "Read the note before write_note. That call replaces the whole file. \
             Keep every line the user did not ask to change. \
             create_note fails when the note already exists. It creates missing parent folders. \
             A read can set truncated at 32 KiB. Do not write that text back. \
             list_notes can stop at 400 entries. \
             Selected rows in the user message are a slice. Read the note, then write the full \
             note. An attached file is source material. A vault write still uses a tool. \
             Call open_note when the user asks to open or look at a note.",
        )
        .with_instructions(
            "Write every reply in Markdown. Use paragraphs, lists, and inline code for note ids \
             and folder ids. Do not wrap the whole reply in a code fence. \
             Do not include a thinking section in the reply. \
             Name each id you created, changed, renamed, or deleted. \
             When a tool returns an error, say that error.",
        )
        .with_constraint("Do not invent a note id or a folder id.")
        .with_constraint(
            "Do not delete a note or a folder unless the user asked to delete that note or folder.",
        )
        .with_constraint("Do not claim a write that the tool rejected.")
        .with_constraint("Do not change a note kind unless the user asked.")
        .with_constraint("Do not record audio or video, and do not start a Research run.")
        .with_constraint("Do not write a truncated read back over a note.");
    spec.compile_system_prompt()
}

fn redact(text: String, keys: &[&str]) -> String {
    let mut secrets = crate::models::env_secrets();
    for key in keys {
        let key = key.trim();
        if !key.is_empty() {
            secrets.push(key.to_string());
        }
    }
    let text = crate::models::redact_secrets(&text, secrets.iter().map(String::as_str));
    const MAX: usize = 400;
    if text.chars().count() <= MAX {
        return text;
    }
    let end = text
        .char_indices()
        .nth(MAX)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    format!("{}…", &text[..end])
}
