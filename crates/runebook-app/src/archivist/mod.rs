//! Archivist chat. Superglue runs the tool loop. The vault crate writes the files.

mod column;
mod tools;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui_kit::component::input::InputState;
use gpui_kit::{AppContext, Context, Entity, SharedString, Window};
use runebook::{FolderId, NoteId, Vault};
use superglue::agents::AgentSpec;
use superglue::providers::ProviderId;
use superglue::{CallOptions, Client, ClientConversation};

pub(crate) use column::column;

use crate::Root;

/// Notice shown when the Groq key is unset.
pub(crate) const MISSING_KEY: &str = "Set the Groq key in Settings.";

/// Key, model, and tool-round cap for one Archivist session.
pub(crate) struct ModelAccess {
    pub(crate) key: String,
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
            error: Some("The Archivist stopped.".to_string()),
            changes: Vec::new(),
        }
    }
}

/// Transcript and the superglue session for one vault.
pub(crate) struct Panel {
    open: bool,
    busy: bool,
    lines: Vec<Line>,
    live: LiveText,
    live_rendered: RenderedMarkdown,
    draft: Entity<InputState>,
    session: Option<Session>,
    drop_session: bool,
    turn: u64,
    /// Rows parked by Add to chat. The next send includes them.
    held: Vec<HeldRows>,
}

/// Rows the user added to the chat and has not sent yet.
struct HeldRows {
    note_id: String,
    source: String,
}

impl Panel {
    pub(crate) fn new(open: bool, window: &mut Window, cx: &mut Context<Root>) -> Self {
        let draft = cx.new(|cx| InputState::new(window, cx).placeholder("Ask the Archivist"));
        Self {
            open,
            busy: false,
            lines: Vec::new(),
            live: LiveText::default(),
            live_rendered: RenderedMarkdown::default(),
            draft,
            session: None,
            drop_session: false,
            turn: 0,
            held: Vec::new(),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Open the column. Add to chat and Rewrite use this.
    pub(crate) fn show(&mut self) {
        self.open = true;
    }

    pub(crate) fn has_held(&self) -> bool {
        !self.held.is_empty()
    }

    /// Remember rows until the next send. An empty body is ignored.
    pub(crate) fn hold_rows(&mut self, note_id: String, source: String) {
        let source = source.trim_matches(['\n', '\r']).to_string();
        if source.trim().is_empty() {
            return;
        }
        if self
            .held
            .iter()
            .any(|row| row.note_id == note_id && row.source == source)
        {
            return;
        }
        self.held.push(HeldRows { note_id, source });
        self.open = true;
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

    pub(crate) fn take_held(&mut self) -> Vec<(String, String)> {
        self.held
            .drain(..)
            .map(|row| (row.note_id, row.source))
            .collect()
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.busy
    }

    /// Draft text with surrounding whitespace removed.
    pub(crate) fn draft_text(&self, cx: &Context<Root>) -> String {
        self.draft.read(cx).value().trim().to_string()
    }

    /// Clear the draft after a send is accepted.
    pub(crate) fn clear_draft(&self, window: &mut Window, cx: &mut Context<Root>) {
        self.draft
            .update(cx, |state, cx| state.set_value(String::new(), window, cx));
    }

    /// Take the session and mark this turn busy.
    fn begin(&mut self, text: String) -> (Option<Session>, u64) {
        self.busy = true;
        self.live = LiveText::default();
        self.live_rendered = RenderedMarkdown::default();
        self.lines.push(Line::User(text));
        self.turn = self.turn.wrapping_add(1);
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
        let (session, turn) = self.begin(text.clone());
        let draft = Arc::new(Mutex::new(LiveText::default()));
        let watch = Arc::clone(&draft);
        cx.spawn(async move |this, cx| {
            let task =
                runtime().spawn(async move { run_turn(session, vault, text, draft, access).await });
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
                body: SharedString::from("The Archivist returned no text."),
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
        done.changes
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
    text: String,
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
    session.conversation.push_user(text);
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
        Err(err) => (live.body, Some(redact(err.to_string(), &access.key))),
    };
    Turn {
        session: Some(session),
        thinking: live.thinking,
        body,
        error,
        changes,
    }
}

async fn open_session(vault: Vault, access: &ModelAccess) -> Result<Session, String> {
    let key = access.key.trim();
    if key.is_empty() {
        return Err(MISSING_KEY.to_string());
    }
    let rounds = if access.tool_rounds < 1 {
        6
    } else {
        access.tool_rounds
    };
    let prompt = archivist_prompt(rounds);
    let client = Client::builder()
        .api_key(key.to_string())
        .model(access.model.clone())
        .system_prompt(prompt)
        .max_tool_rounds(rounds)
        // Chat completions hide this summary: gpt-6-luna rejects tools unless effort is none.
        // The OpenAI turn uses the Responses stream, which keeps tools and returns the summary.
        .reasoning_effort("low")
        .api_key_for(ProviderId::Groq, key.to_string())
        .build()
        .map_err(|err| redact(err.to_string(), key))?;
    let changes = Arc::new(Mutex::new(Vec::new()));
    tools::register(&client, vault, Arc::clone(&changes))
        .await
        .map_err(|err| redact(err, key))?;
    Ok(Session {
        conversation: client.conversation(),
        changes,
    })
}

fn archivist_prompt(rounds: u32) -> String {
    let spec = AgentSpec::new("Archivist", "the Archivist of this vault")
        .with_max_tool_rounds(rounds)
        .with_goal("Keep the notes accurate.")
        .with_goal("Use tools for every file change.")
        .with_goal("Say which note ids and folder ids changed.")
        .with_instructions(
            "Read a note before you replace it. write_note replaces the whole file. \
             Note ids are vault-relative paths without the .md suffix. \
             Example: journal/2026-10-23.",
        )
        .with_instructions(
            "Call open_note to show a note in the preview when the user asks to open or look at \
             that note. Do not invent the id.",
        )
        .with_instructions(
            "Write every reply in Markdown. Use paragraphs, lists, and inline code for note ids \
             and folder ids. Do not wrap the whole reply in a code fence. \
             Do not include a thinking section in the reply.",
        )
        .with_constraint("Do not invent note ids or folder ids.")
        .with_constraint(
            "Do not delete a note or folder unless the user asked to delete that note or folder.",
        )
        .with_constraint("Do not claim a write that the tool rejected.");
    spec.compile_system_prompt()
}

fn redact(text: String, key: &str) -> String {
    let env_key = std::env::var("GROQ_API_KEY").unwrap_or_default();
    let text = crate::models::redact_secrets(&text, [key, env_key.trim()]);
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
