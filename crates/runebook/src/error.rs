use std::path::PathBuf;

use crate::note::{FolderId, NoteId};

/// Failure from the vault, the link index, or app config.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `path` exists and is not a directory.
    #[error("vault path is not a directory: {path}")]
    NotADirectory { path: PathBuf },

    /// The directory has no `runebook.toml`.
    #[error("not a runebook vault (missing runebook.toml): {path}")]
    NotAVault { path: PathBuf },

    /// `runebook.toml` is already present.
    #[error("vault already exists at {path}")]
    AlreadyExists { path: PathBuf },

    /// The vault name is empty.
    #[error("vault name is empty")]
    InvalidVaultName,

    /// `runebook.toml` uses a schema this crate does not read.
    #[error("unsupported vault schema version {found} (expected {expected})")]
    UnsupportedSchema { found: u32, expected: u32 },

    /// `raw` is not a safe note or folder id.
    #[error("invalid note id: {raw}")]
    InvalidNoteId { raw: String },

    /// The note file is not in the vault.
    #[error("note not found: {id}")]
    NoteNotFound { id: NoteId },

    /// A note with this id already has a file.
    #[error("note already exists: {id}")]
    NoteExists { id: NoteId },

    /// The folder is missing, or the path is not a directory.
    #[error("folder not found: {id}")]
    FolderNotFound { id: FolderId },

    /// A folder with this id already exists.
    #[error("folder already exists: {id}")]
    FolderExists { id: FolderId },

    /// Rename or delete was asked to change the vault root.
    #[error("cannot change the vault root folder")]
    RootFolder,

    /// The folder holds a file that is not a note or a media file.
    #[error("folder contains a file that is not a note: {path}")]
    ForeignFile { path: PathBuf },

    /// The link does not name a video, audio, or image file.
    #[error("not a media file: {name}")]
    NotMedia { name: String },

    /// The media file is missing.
    #[error("media file not found: {path}")]
    MediaNotFound { path: PathBuf },

    /// The relative path leaves the vault root.
    #[error("path escapes the vault: {path}")]
    PathEscape { path: PathBuf },

    /// The path is a symlink. The vault does not follow symlinks.
    #[error("refusing to follow symlink: {path}")]
    Symlink { path: PathBuf },

    /// Note bytes are not UTF-8.
    #[error("note is not UTF-8: {id}")]
    NotUtf8 { id: NoteId },

    /// Filesystem failure. `path` is the file the call touched.
    #[error("failed to access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// `runebook.toml` or app config did not parse.
    #[error("invalid config at {path}: {source}")]
    Config {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// `runebook.toml` or app config did not serialize.
    #[error("failed to serialize config at {path}: {source}")]
    ConfigWrite {
        path: PathBuf,
        #[source]
        source: toml::ser::Error,
    },
}

/// Vault result.
pub type Result<T> = std::result::Result<T, Error>;
