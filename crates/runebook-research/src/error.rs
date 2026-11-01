//! Failures from a research run.
//!
//! A variant names the failure. It does not carry a window sentence or an API key.
//! The window maps the variant to text and redacts that text before it is shown.

/// One research failure.
#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    /// The OpenAI key is missing.
    #[error("missing model key")]
    MissingKey,
    /// One hundred note ids for this question are already in the folder.
    #[error("research folder is full")]
    FolderFull,
    /// The vault rejected the note id. The text is the vault error.
    #[error("{0}")]
    Name(String),
    /// The vault did not accept the note write.
    #[error("could not write the research note")]
    Write,
    /// The model call failed. The text is the host error.
    #[error("{0}")]
    Chat(String),
    /// The clarifying-question task did not finish.
    #[error("stopped")]
    Stopped,
}
