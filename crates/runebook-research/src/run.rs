//! One Charlotte investigation the pane can start, cancel, and read.
//!
//! The pane passes the API key, the base URL, and the model name.
//! This module writes the plan and the findings through [`runebook::Vault`].
//! It does not paint the window.

use std::sync::{Arc, Mutex, OnceLock};

use charlotte::{ResearchNode, ask_clarifying_questions, run_research_graph};
use runebook::{NoteId, NoteKind, Vault, encode_note};
use superglue::cancel::CancellationToken;
use superglue::chat::ChatOptions;

use crate::error::ResearchError;
use crate::report::{parse_research_body, upsert_plan};

/// How one investigation ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvestigateEnd {
    /// The graph returned findings text.
    Findings(String),
    /// The pane cancelled the run.
    Stopped,
    /// The graph failed. The text is the host error.
    Failed(String),
}

/// A research run for one note.
///
/// The open Research note in this process starts the run.
/// The question text and the model reply are the untrusted input.
/// A failure is a [`ResearchError`] or [`InvestigateEnd::Failed`].
/// Neither value adds an API key or a filesystem path.
#[derive(Clone)]
pub struct Investigation {
    note: NoteId,
    query: String,
    answers: Vec<(String, String)>,
    phase: Arc<Mutex<ResearchNode>>,
    plan: Arc<Mutex<String>>,
    cancel: CancellationToken,
}

impl Investigation {
    /// Start a run for `note`. The graph has not been called yet.
    #[must_use]
    pub fn start(note: NoteId, query: String, answers: Vec<(String, String)>) -> Self {
        Self {
            note,
            query,
            answers,
            phase: Arc::new(Mutex::new(ResearchNode::Plan)),
            plan: Arc::new(Mutex::new(String::new())),
            cancel: CancellationToken::new(),
        }
    }

    /// The note this run writes.
    #[must_use]
    pub fn note(&self) -> &NoteId {
        &self.note
    }

    /// The question.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Clarifying answers. Each pair is a question and an answer.
    #[must_use]
    pub fn answers(&self) -> &[(String, String)] {
        &self.answers
    }

    /// Ask the graph to stop.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Token passed into the model call.
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Sidebar label for the current graph node.
    #[must_use]
    pub fn phase_label(&self) -> String {
        self.phase
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .phase()
            .to_string()
    }

    /// Plan text from the latest graph update.
    #[must_use]
    pub fn plan_text(&self) -> String {
        self.plan
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    /// Record the graph node the pane shows.
    pub fn set_phase(&self, node: ResearchNode) {
        *self.phase.lock().unwrap_or_else(|err| err.into_inner()) = node;
    }

    /// Build chat settings for this run.
    ///
    /// # Errors
    ///
    /// Returns [`ResearchError::MissingKey`] when `api_key` is empty.
    /// Returns [`ResearchError::Chat`] when the host rejects the settings.
    pub fn prepare(
        &self,
        api_key: &str,
        base_url: &str,
        model: &str,
        tool_rounds: u32,
    ) -> Result<ChatOptions, ResearchError> {
        chat_options(api_key, base_url, model, tool_rounds, Some(self.token()))
    }

    /// Run the graph. Updates [`phase_label`] and [`plan_text`] while it runs.
    pub async fn run(&self, options: ChatOptions, tool_rounds: u32) -> InvestigateEnd {
        let cancel = self.cancel.clone();
        let end = run_research_graph(
            &self.query,
            &self.answers,
            cancel.clone(),
            options,
            tool_rounds,
            {
                let phase = Arc::clone(&self.phase);
                move |node| {
                    *phase.lock().unwrap_or_else(|err| err.into_inner()) = node;
                }
            },
            {
                let plan = Arc::clone(&self.plan);
                move |text| {
                    *plan.lock().unwrap_or_else(|err| err.into_inner()) = text.to_string();
                }
            },
        )
        .await;
        match end {
            Ok(_) if cancel.is_cancelled() => InvestigateEnd::Stopped,
            Ok(text) => InvestigateEnd::Findings(text),
            Err(_) if cancel.is_cancelled() => InvestigateEnd::Stopped,
            Err(err) => InvestigateEnd::Failed(err.to_string()),
        }
    }
}

/// Chat settings for a research call.
///
/// An empty key returns [`ResearchError::MissingKey`].
/// This function does not put the key into the error text.
///
/// # Errors
///
/// Returns [`ResearchError::MissingKey`] when `api_key` is empty.
/// Returns [`ResearchError::Chat`] when the host rejects the settings.
pub fn chat_options(
    api_key: &str,
    base_url: &str,
    model: &str,
    tool_rounds: u32,
    cancel: Option<CancellationToken>,
) -> Result<ChatOptions, ResearchError> {
    if api_key.trim().is_empty() {
        return Err(ResearchError::MissingKey);
    }
    charlotte::research_chat_options_from(api_key, base_url, model, tool_rounds, cancel)
        .map_err(|err| ResearchError::Chat(err.to_string()))
}

/// Ask the clarifying questions for one query.
///
/// # Errors
///
/// Returns [`ResearchError::Chat`] when the model call fails.
pub async fn clarifying_questions(
    query: &str,
    options: &ChatOptions,
) -> Result<String, ResearchError> {
    ask_clarifying_questions(query, options)
        .await
        .map_err(|err| ResearchError::Chat(err.to_string()))
}

/// Questions parsed from a model reply. A bad payload is an empty list.
#[must_use]
pub fn questions_from_reply(text: &str) -> Vec<String> {
    charlotte::parse_clarifying_questions(text)
}

/// Write the plan into the note when it changed.
///
/// Returns the written file so the open editor can refresh.
/// An empty plan, or a plan that is already in the note, writes nothing.
///
/// # Errors
///
/// Returns [`ResearchError::Write`] when the vault rejects the file.
/// The error has no path.
pub fn store_plan(
    vault: &Vault,
    id: &NoteId,
    kind: NoteKind,
    body: &str,
    plan: &str,
) -> Result<Option<String>, ResearchError> {
    let plan = plan.trim();
    if plan.is_empty() || parse_research_body(body).plan == plan {
        return Ok(None);
    }
    let file = encode_note(kind, &upsert_plan(body, plan));
    vault.write(id, &file).map_err(|_| ResearchError::Write)?;
    Ok(Some(file))
}

/// Append `block` to the note and write it.
///
/// Returns the written file so the open editor can refresh.
///
/// # Errors
///
/// Returns [`ResearchError::Write`] when the vault rejects the file.
/// The error has no path.
pub fn store_block(
    vault: &Vault,
    id: &NoteId,
    kind: NoteKind,
    body: &str,
    block: &str,
) -> Result<String, ResearchError> {
    let mut next = body.to_string();
    if !next.ends_with('\n') {
        next.push('\n');
    }
    next.push('\n');
    next.push_str(block.trim());
    next.push('\n');
    let file = encode_note(kind, &next);
    vault.write(id, &file).map_err(|_| ResearchError::Write)?;
    Ok(file)
}

/// Runtime that awaits Charlotte. The pane does not start a second one.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_name("runebook-research")
            .enable_all()
            .build()
            .expect("research runtime")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_key_is_missing() {
        let err = chat_options("", "https://api.openai.com", "gpt-test", 1, None).unwrap_err();
        assert!(matches!(err, ResearchError::MissingKey));
        let text = err.to_string();
        assert!(!text.contains("sk-"));
        assert!(!text.contains("api.openai.com"));
    }

    #[test]
    fn a_bad_question_payload_is_empty() {
        assert!(questions_from_reply("nope").is_empty());
    }
}
