//! One HiDock pass for the open vault.
//!
//! The pass runs on a thread with its own Tokio runtime. It does not keep
//! the audio, and it does not delete the card file.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use runebook::Vault;
use runebook_hidock::{NoteStore, Report, Session, SpeechNote};
use superglue::openai::ChatMessage;
use superglue::providers::ProviderId;
use superglue::{CallOptions, Client};

use crate::archivist::ModelAccess;

/// Spoken text sent to the model for one quick note.
const PIECE_LIMIT: usize = 8_000;
/// Cleaned text stored in the note.
const REPLY_LIMIT: usize = 4_000;

const QUICK_PROMPT: &str = "\
You clean one spoken quick note.
The user message is the spoken pieces, in order.
Join them into the words the person said.
Drop filler sounds such as mm and um when they are not words.
Fix broken phrases.
Do not add facts.
Do not add a title.
Do not mention a speaker.
Write only the spoken text.
";

/// One speech roster, and one import at a time.
pub(crate) struct Gate {
    slot: Mutex<Option<Session>>,
    /// File name of the recording this pass is reading.
    file: Arc<Mutex<String>>,
}

/// Result of one attempt to take the roster.
pub(crate) enum Sync {
    /// Another import holds the recorder.
    Busy,
    /// The pass finished.
    Done(Outcome),
}

/// One import, plus how many stored quick notes were cleaned.
pub(crate) struct Outcome {
    pub(crate) report: Report,
    pub(crate) cleaned: usize,
}

impl Gate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            slot: Mutex::new(Some(Session::new())),
            file: Arc::new(Mutex::new(String::new())),
        })
    }

    /// File name shown while a recording is read. Empty when the pass is idle.
    pub(crate) fn current_file(&self) -> String {
        self.file
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    /// Run one import. A second caller gets [`Sync::Busy`] until this returns.
    pub(crate) fn run(
        &self,
        root: PathBuf,
        model: String,
        access: Option<ModelAccess>,
    ) -> Sync {
        let Some(mut session) = self.take() else {
            return Sync::Busy;
        };
        let file = Arc::clone(&self.file);
        let joined = std::thread::spawn(move || {
            let outcome = import_tick(&root, &model, access.as_ref(), &mut session, &file);
            (session, outcome)
        })
        .join();
        self.clear_file();
        match joined {
            Ok((session, outcome)) => {
                self.put(session);
                Sync::Done(outcome)
            }
            Err(_) => {
                self.put(Session::new());
                Sync::Done(stopped())
            }
        }
    }

    fn take(&self) -> Option<Session> {
        self.slot
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
    }

    fn put(&self, session: Session) {
        *self.slot.lock().unwrap_or_else(|err| err.into_inner()) = Some(session);
    }

    fn clear_file(&self) {
        *self.file.lock().unwrap_or_else(|err| err.into_inner()) = String::new();
    }
}

/// Import recordings that this vault does not already store.
pub(crate) fn import_tick(
    root: &Path,
    model: &str,
    access: Option<&ModelAccess>,
    session: &mut Session,
    file: &Mutex<String>,
) -> Outcome {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => return stopped_with(failed(err.to_string())),
    };
    runtime.block_on(import(root, model, access, session, file))
}

async fn import(
    root: &Path,
    model: &str,
    access: Option<&ModelAccess>,
    session: &mut Session,
    file: &Mutex<String>,
) -> Outcome {
    let vault = match Vault::open(root) {
        Ok(vault) => vault,
        Err(err) => return stopped_with(failed(err.to_string())),
    };
    let index = match vault.notes() {
        Ok(index) => index,
        Err(err) => return stopped_with(failed(err.to_string())),
    };
    let cleaned = polish_saved(&vault, &index, access, file).await;
    let trimmed = model.trim();
    let model_dir = if trimmed.is_empty() {
        None
    } else {
        Some(Path::new(trimmed))
    };
    let fill_words = model_dir.is_some();
    let report = runebook_hidock::pull(
        session,
        model_dir,
        |id, line| {
            if index.contains(id) {
                if fill_words && stored_needs_words(&vault, id) {
                    return false;
                }
                return true;
            }
            vault
                .search(line)
                .ok()
                .is_some_and(|found| !found.hits.is_empty())
        },
        |name| {
            *file.lock().unwrap_or_else(|err| err.into_inner()) = name.to_string();
        },
        QuickStore {
            vault: &vault,
            access,
        },
    )
    .await;
    Outcome { report, cleaned }
}

/// Sends a new quick note's pieces to the model before the first write.
struct QuickStore<'a> {
    vault: &'a Vault,
    access: Option<&'a ModelAccess>,
}

impl NoteStore for QuickStore<'_> {
    async fn store(&mut self, note: SpeechNote) -> Result<bool, String> {
        let note = present_quick(self.access, note).await;
        store_note(self.vault, &note)
    }
}

fn store_note(vault: &Vault, note: &runebook_hidock::SpeechNote) -> Result<bool, String> {
    match vault.create_note(&note.id, &note.markdown) {
        Ok(()) => Ok(true),
        Err(runebook::Error::NoteExists { .. }) if stored_needs_words(vault, &note.id) => vault
            .write(&note.id, &note.markdown)
            .map(|()| true)
            .map_err(|err| err.to_string()),
        Err(runebook::Error::NoteExists { .. }) => Ok(false),
        Err(err) => Err(err.to_string()),
    }
}

fn stored_needs_words(vault: &Vault, id: &runebook::NoteId) -> bool {
    let Ok(text) = vault.read(id) else {
        return false;
    };
    let (_, body) = runebook::decode_note(&text);
    runebook_hidock::body_needs_words(body)
}

/// Rewrite stored quick notes that still show speaker lines.
///
/// The words are already in the file. This does not read the recorder.
/// A full recording is left as speaker lines.
async fn polish_saved(
    vault: &Vault,
    index: &runebook::VaultIndex,
    access: Option<&ModelAccess>,
    file: &Mutex<String>,
) -> usize {
    let mut cleaned = 0;
    for id in index.ids() {
        if !runebook_hidock::is_quick_id(id.as_str()) {
            continue;
        }
        let Ok(text) = vault.read(id) else {
            continue;
        };
        let (kind, body) = runebook::decode_note(&text);
        if kind != runebook::NoteKind::Speech {
            continue;
        }
        let Some(pieces) = runebook_hidock::quick_pieces(body) else {
            continue;
        };
        let Some(identity) = runebook_hidock::identity_in(body) else {
            continue;
        };
        let stem = id.as_str().rsplit('/').next().unwrap_or(id.as_str());
        *file.lock().unwrap_or_else(|err| err.into_inner()) = stem.to_string();
        let Some(prose) = clean_pieces(access, &pieces).await else {
            continue;
        };
        let markdown = runebook_hidock::quick_speech(identity, &prose);
        if vault.write(id, &markdown).is_ok() {
            cleaned += 1;
        }
    }
    cleaned
}

async fn present_quick(access: Option<&ModelAccess>, note: SpeechNote) -> SpeechNote {
    if !runebook_hidock::is_quick_id(note.id.as_str()) {
        return note;
    }
    let (_, body) = runebook::decode_note(&note.markdown);
    let Some(pieces) = runebook_hidock::quick_pieces(body) else {
        return note;
    };
    let Some(identity) = runebook_hidock::identity_in(body).map(str::to_string) else {
        return note;
    };
    let Some(prose) = clean_pieces(access, &pieces).await else {
        return note;
    };
    SpeechNote {
        id: note.id,
        markdown: runebook_hidock::quick_speech(&identity, &prose),
    }
}

/// Clean the pieces with the Archivist model.
///
/// No key, or a failed call, joins the pieces. The note still has no speaker
/// label. The call sends text only. It does not send audio or a key.
async fn clean_pieces(access: Option<&ModelAccess>, pieces: &str) -> Option<String> {
    let pieces = clip_chars(pieces, PIECE_LIMIT);
    if pieces.trim().is_empty() {
        return None;
    }
    if let Some(access) = access
        && let Some(text) = ask_model(access, &pieces).await
        && !text.is_empty()
    {
        return Some(text);
    }
    let joined = pieces.split_whitespace().collect::<Vec<_>>().join(" ");
    (!joined.is_empty()).then_some(joined)
}

async fn ask_model(access: &ModelAccess, pieces: &str) -> Option<String> {
    let client = quick_client(access).ok()?;
    let message = ChatMessage::text("user", pieces.to_string());
    let outcome = client
        .complete_messages(vec![message], CallOptions::default())
        .await
        .ok()?;
    let text = plain_reply(&outcome.content.unwrap_or_default());
    let text = clip_chars(&text, REPLY_LIMIT);
    (!text.is_empty()).then_some(text)
}

fn quick_client(access: &ModelAccess) -> Result<Client, ()> {
    let plan = runebook_agent::key_plan(
        &access.model,
        &access.groq_key,
        &access.openai_key,
        &access.openai_base_url,
    )
    .map_err(|_| ())?;
    let mut builder = Client::builder()
        .api_key(plan.primary)
        .base_url(plan.base.clone())
        .model(access.model.clone())
        .system_prompt(QUICK_PROMPT)
        .max_tool_rounds(1)
        .reasoning_effort("low")
        .timeout(Duration::from_secs(60));
    if let Some(groq) = plan.groq.as_deref() {
        builder = builder.api_key_for(ProviderId::Groq, groq);
    }
    if let Some(openai) = plan.openai.as_deref() {
        builder = builder
            .api_key_for(plan.openai_provider, openai)
            .base_url_for(plan.openai_provider, plan.base);
    }
    builder.build().map_err(|_| ())
}

fn plain_reply(text: &str) -> String {
    let text = strip_fence(text.trim());
    text.split_whitespace()
        .filter(|word| !word.starts_with("hidock-id:"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let rest = rest.trim_start_matches(|ch: char| !ch.is_whitespace());
    let rest = rest.trim_start();
    rest.trim_end().trim_end_matches("```").trim()
}

fn clip_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let end = text
        .char_indices()
        .nth(limit)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    text[..end].trim_end().to_string()
}

fn stopped_with(report: Report) -> Outcome {
    Outcome { report, cleaned: 0 }
}

fn failed(message: String) -> Report {
    Report {
        notes: Vec::new(),
        permission: false,
        message: Some(message),
        connected: false,
    }
}

fn stopped() -> Outcome {
    stopped_with(failed("the HiDock import stopped".to_string()))
}
