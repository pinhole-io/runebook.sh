//! Local markdown vault.
//!
//! A vault is a folder of `*.md` notes plus `runebook.toml`.
//! Wiki links and the file tree come from those files.
//! There is no sidecar database.
//! Media files beside a note use a known video, audio, or image extension.
//! Folder delete removes those files.

mod config;
mod error;
mod fsutil;
mod index;
mod links;
mod media;
mod note;
mod vault;

pub use config::{AppConfig, NoteViewChoice, ThemeChoice};
pub use error::{Error, Result};
pub use index::VaultIndex;
pub use links::{
    ResolveResult, WikiLink, WikiTarget, note_target_from_url, parse_wiki_links, preview_markdown,
};
pub use media::is_external_url;
pub use note::{FolderId, NoteId};
pub use vault::{TreeEntry, Vault, VaultTree};
