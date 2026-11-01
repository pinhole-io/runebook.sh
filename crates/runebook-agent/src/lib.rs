//! Archivist tools, parked files, and the system prompt.
//!
//! The window draws the transcript. This crate writes the vault through tools.
//! A tool result has no filesystem path. A failure does not include an API key.

mod attach;
mod prompt;
mod tools;

use runebook::{FolderId, NoteId};

pub use attach::{AttachError, MAX_BYTES, MAX_FILES, ParkedFile, display_line, file_name, prepare};
pub use prompt::{KeyError, KeyPlan, archivist_prompt, has_model_key, key_plan};
pub use tools::register;

/// A vault change made by a tool during one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultChange {
    /// A note was written or created.
    Wrote(NoteId),
    /// A note moved from `from` to `to`.
    RenamedNote { from: NoteId, to: NoteId },
    /// A note was deleted.
    DeletedNote(NoteId),
    /// The user should see this note.
    Opened(NoteId),
    /// A folder was created.
    Folder(FolderId),
    /// A folder moved from `from` to `to`.
    RenamedFolder { from: FolderId, to: FolderId },
    /// A folder was deleted.
    DeletedFolder(FolderId),
}
