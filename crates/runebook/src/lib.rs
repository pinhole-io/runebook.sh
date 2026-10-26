//! Local markdown vault.
//!
//! A vault is a folder of `*.md` notes plus `runebook.toml`.
//! Wiki links and the file tree come from those files.
//! There is no sidecar database.
//! A note is Book, Speech, Sight, Deed, or Research. Book is a markdown file.
//! The other kinds start with a `kind` header.
//! Media files beside a note use a known video, audio, or image extension.
//! Folder delete removes those files.

mod config;
mod error;
mod fsutil;
mod index;
mod kind;
mod links;
mod media;
mod note;
mod vault;

pub use config::{
    AppConfig, DEFAULT_ARCHIVIST_MODEL, DEFAULT_OPENAI_BASE_URL, DEFAULT_OPENAI_MODEL,
    DEFAULT_TOOL_ROUNDS, NoteViewChoice, ThemeChoice,
};
pub use error::{Error, Result};
pub use index::VaultIndex;
pub use kind::{
    NoteKind, TaskLine, decode_note, encode_note, parse_task_line, preserve_kind, toggle_task_line,
};
pub use links::{
    ResolveResult, WikiLink, WikiTarget, note_target_from_url, parse_wiki_links, preview_markdown,
};
pub use media::is_external_url;
pub use note::{FolderId, NoteId};
pub use vault::{TreeEntry, Vault, VaultTree};
