//! Research note text and the Charlotte investigation.
//!
//! The window draws the wizard, the cards, and the report.
//! This crate parses the note, asks Charlotte, and writes the plan and the findings.
//! It does not paint a window.
//!
//! The open Research note in this process starts a run.
//! The worst input is the question text and the model reply.
//! A failure is a [`ResearchError`]. It does not add an API key or a filesystem path.

mod error;
mod report;
mod run;

pub use charlotte::ResearchNode;
pub use error::ResearchError;
pub use report::{
    ResearchReport, parse_research_body, research_note_body, unique_research_id, upsert_plan,
};
pub use run::{
    InvestigateEnd, Investigation, chat_options, clarifying_questions, questions_from_reply,
    runtime, store_block, store_plan,
};
