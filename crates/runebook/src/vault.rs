use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsutil::atomic_write;
use crate::index::VaultIndex;
use crate::kind::{self, NoteKind};
use crate::links::{WikiTarget, parse_wiki_links};
use crate::media::{self, is_media_path};
use crate::note::{FolderId, NoteId, validate_relative, vault_join};

const SCHEMA_VERSION: u32 = 1;
const VAULT_FILE: &str = "runebook.toml";

/// One folder of markdown notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vault {
    root: PathBuf,
    name: String,
}

/// Top-level file tree. The root folder itself is not an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultTree {
    /// Children of the vault root, folders first.
    pub children: Vec<TreeEntry>,
}

/// A folder or a note in [`VaultTree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeEntry {
    /// A directory under the vault.
    Folder {
        /// Folder id relative to the vault root.
        id: FolderId,
        /// Last path segment.
        name: String,
        /// Nested folders and notes.
        children: Vec<TreeEntry>,
    },
    /// A markdown note.
    Note {
        /// Note id.
        id: NoteId,
        /// File stem.
        name: String,
        /// Kind read from the file header. A missing header is Book.
        kind: NoteKind,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct VaultFile {
    schema_version: u32,
    name: String,
}

impl Vault {
    /// Create a vault at `path` and write `runebook.toml`.
    ///
    /// `path` may be missing. An existing `runebook.toml` is an error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AlreadyExists`], [`Error::NotADirectory`],
    /// [`Error::InvalidVaultName`], or [`Error::Io`].
    pub fn create(path: impl AsRef<Path>, name: impl AsRef<str>) -> Result<Self> {
        let path = path.as_ref();
        let name = name.as_ref().trim();
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(Error::InvalidVaultName);
        }
        if path.exists() && !path.is_dir() {
            return Err(Error::NotADirectory {
                path: path.to_path_buf(),
            });
        }
        let marker = path.join(VAULT_FILE);
        if marker.exists() {
            return Err(Error::AlreadyExists {
                path: path.to_path_buf(),
            });
        }
        fs::create_dir_all(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let file = VaultFile {
            schema_version: SCHEMA_VERSION,
            name: name.to_string(),
        };
        let text = toml::to_string(&file).map_err(|source| Error::ConfigWrite {
            path: marker.clone(),
            source,
        })?;
        atomic_write(&marker, &text)?;
        Ok(Self {
            root: path.to_path_buf(),
            name: name.to_string(),
        })
    }

    /// Open a vault that already has `runebook.toml`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotAVault`], [`Error::NotADirectory`],
    /// [`Error::UnsupportedSchema`], [`Error::InvalidVaultName`],
    /// [`Error::Config`], or [`Error::Io`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(Error::NotAVault {
                path: path.to_path_buf(),
            });
        }
        if !path.is_dir() {
            return Err(Error::NotADirectory {
                path: path.to_path_buf(),
            });
        }
        let marker = path.join(VAULT_FILE);
        if !marker.is_file() {
            return Err(Error::NotAVault {
                path: path.to_path_buf(),
            });
        }
        let text = fs::read_to_string(&marker).map_err(|source| Error::Io {
            path: marker.clone(),
            source,
        })?;
        let file: VaultFile = toml::from_str(&text).map_err(|source| Error::Config {
            path: marker,
            source,
        })?;
        if file.schema_version != SCHEMA_VERSION {
            return Err(Error::UnsupportedSchema {
                found: file.schema_version,
                expected: SCHEMA_VERSION,
            });
        }
        let name = file.name.trim();
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(Error::InvalidVaultName);
        }
        Ok(Self {
            root: path.to_path_buf(),
            name: name.to_string(),
        })
    }

    /// Absolute vault root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Name from `runebook.toml`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Scan notes and their wiki links.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] or [`Error::NotUtf8`] when a note cannot be read.
    pub fn notes(&self) -> Result<VaultIndex> {
        let tree = self.tree()?;
        let mut ids = Vec::new();
        collect_ids(&tree.children, &mut ids);
        let mut notes = BTreeMap::new();
        for id in ids {
            let text = self.read(&id)?;
            let (_, body) = kind::decode_note(&text);
            let targets = parse_wiki_links(body)
                .into_iter()
                .map(|link| link.target)
                .collect::<Vec<WikiTarget>>();
            notes.insert(id, targets);
        }
        Ok(VaultIndex::from_notes(notes))
    }

    /// List folders and notes. Hidden paths and symlinks are skipped.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when a directory cannot be read.
    /// Returns [`Error::InvalidNoteId`] when a file name is not a safe id.
    pub fn tree(&self) -> Result<VaultTree> {
        Ok(VaultTree {
            children: read_children(&self.root, &FolderId::root())?,
        })
    }

    /// Read a note as UTF-8.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoteNotFound`], [`Error::NotUtf8`], [`Error::Symlink`],
    /// or [`Error::Io`].
    pub fn read(&self, id: &NoteId) -> Result<String> {
        let path = self.note_path(id)?;
        require_note_file(&path, id)?;
        let bytes = fs::read(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        String::from_utf8(bytes).map_err(|_| Error::NotUtf8 { id: id.clone() })
    }

    /// Replace the body of an existing note.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoteNotFound`] when the note is missing.
    /// Returns [`Error::Symlink`] when the path is a symlink.
    pub fn write(&self, id: &NoteId, contents: &str) -> Result<()> {
        let path = self.note_path(id)?;
        require_note_file(&path, id)?;
        atomic_write(&path, contents)
    }

    /// Create a note and any missing parent folders.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoteExists`] when the file is already present.
    pub fn create_note(&self, id: &NoteId, contents: &str) -> Result<()> {
        let path = self.note_path(id)?;
        if fs::symlink_metadata(&path).is_ok() {
            return Err(Error::NoteExists { id: id.clone() });
        }
        atomic_write(&path, contents)
    }

    /// Rename a note. Parent folders for `to` are created.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoteNotFound`] or [`Error::NoteExists`].
    /// Returns [`Error::Symlink`] when the source is a symlink.
    pub fn rename_note(&self, from: &NoteId, to: &NoteId) -> Result<()> {
        let src = self.note_path(from)?;
        require_note_file(&src, from)?;
        if from == to {
            return Ok(());
        }
        let dest = self.note_path(to)?;
        if fs::symlink_metadata(&dest).is_ok() {
            return Err(Error::NoteExists { id: to.clone() });
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::rename(&src, &dest).map_err(|source| Error::Io { path: dest, source })
    }

    /// Delete one note file.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoteNotFound`] when the file is missing.
    /// Returns [`Error::Symlink`] when the path is a symlink.
    pub fn delete_note(&self, id: &NoteId) -> Result<()> {
        let path = self.note_path(id)?;
        require_note_file(&path, id)?;
        fs::remove_file(&path).map_err(|source| Error::Io { path, source })
    }

    /// Create a folder. Parent folders are created.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RootFolder`] for the root, or [`Error::FolderExists`].
    pub fn create_folder(&self, id: &FolderId) -> Result<()> {
        if id.is_root() {
            return Err(Error::RootFolder);
        }
        let path = self.folder_path(id)?;
        if fs::symlink_metadata(&path).is_ok() {
            return Err(Error::FolderExists { id: id.clone() });
        }
        fs::create_dir_all(&path).map_err(|source| Error::Io { path, source })
    }

    /// Rename a folder and the notes inside it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RootFolder`], [`Error::FolderNotFound`], or [`Error::FolderExists`].
    pub fn rename_folder(&self, from: &FolderId, to: &FolderId) -> Result<()> {
        if from.is_root() || to.is_root() {
            return Err(Error::RootFolder);
        }
        let src = self.folder_path(from)?;
        require_folder(&src, from)?;
        if from == to {
            return Ok(());
        }
        let dest = self.folder_path(to)?;
        if fs::symlink_metadata(&dest).is_ok() {
            return Err(Error::FolderExists { id: to.clone() });
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::rename(&src, &dest).map_err(|source| Error::Io { path: dest, source })
    }

    /// Path of a screen recording beside `id`.
    ///
    /// The file name is `{stem}-{stamp}.mp4`. `stamp` is one safe path segment.
    /// The returned path stays inside the vault root.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNoteId`] when `stamp` is empty, hidden, or has a
    /// slash. Returns [`Error::PathEscape`] when the path leaves the vault.
    pub fn recording_path(&self, id: &NoteId, stamp: &str) -> Result<PathBuf> {
        if stamp.contains('/') {
            return Err(Error::InvalidNoteId {
                raw: stamp.to_string(),
            });
        }
        validate_relative(stamp)?;
        let mut rel = id.parent().relative_path();
        rel.push(format!("{}-{stamp}.mp4", id.name()));
        let path = vault_join(&self.root, &rel)?;
        if !path.starts_with(&self.root) {
            return Err(Error::PathEscape { path });
        }
        Ok(path)
    }

    /// Resolve a media link in `note` to a file inside the vault.
    ///
    /// `href` is a relative path or a `file:` URL. The file must use a known
    /// video, audio, or image extension. The path cannot leave the vault, and
    /// a symlink is refused.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotMedia`], [`Error::MediaNotFound`], [`Error::PathEscape`],
    /// [`Error::Symlink`], or [`Error::Io`].
    pub fn media_file(&self, note: &NoteId, href: &str) -> Result<PathBuf> {
        media::resolve(&self.root, note, href)
    }

    /// Delete a folder of notes and media files.
    ///
    /// A known video, audio, or image file is removed with the folder. Any
    /// other non-note file stops the delete.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RootFolder`], [`Error::FolderNotFound`], or [`Error::ForeignFile`].
    pub fn delete_folder(&self, id: &FolderId) -> Result<()> {
        if id.is_root() {
            return Err(Error::RootFolder);
        }
        let path = self.folder_path(id)?;
        require_folder(&path, id)?;
        ensure_note_tree(&path)?;
        remove_note_tree(&path)
    }

    fn note_path(&self, id: &NoteId) -> Result<PathBuf> {
        vault_join(&self.root, &id.relative_path())
    }

    fn folder_path(&self, id: &FolderId) -> Result<PathBuf> {
        vault_join(&self.root, &id.relative_path())
    }
}

fn require_note_file(path: &Path, id: &NoteId) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Error::Symlink {
            path: path.to_path_buf(),
        }),
        Ok(meta) if meta.is_file() => Ok(()),
        Ok(_) => Err(Error::NoteNotFound { id: id.clone() }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(Error::NoteNotFound { id: id.clone() })
        }
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn require_folder(path: &Path, id: &FolderId) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Error::Symlink {
            path: path.to_path_buf(),
        }),
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(Error::FolderNotFound { id: id.clone() }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(Error::FolderNotFound { id: id.clone() })
        }
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn read_note_kind(path: &Path) -> Result<NoteKind> {
    let mut file = fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut buf = [0u8; 256];
    let read = file.read(&mut buf).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(kind::kind_from_prefix(&buf[..read]))
}

fn collect_ids(entries: &[TreeEntry], ids: &mut Vec<NoteId>) {
    for entry in entries {
        match entry {
            TreeEntry::Note { id, .. } => ids.push(id.clone()),
            TreeEntry::Folder { children, .. } => collect_ids(children, ids),
        }
    }
}

fn read_children(root: &Path, folder: &FolderId) -> Result<Vec<TreeEntry>> {
    let dir = vault_join(root, &folder.relative_path())?;
    let read = fs::read_dir(&dir).map_err(|source| Error::Io {
        path: dir.clone(),
        source,
    })?;
    let mut entries = Vec::new();
    for item in read {
        let item = item.map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })?;
        let Some(name) = item.file_name().into_string().ok() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let file_type = item.file_type().map_err(|source| Error::Io {
            path: item.path(),
            source,
        })?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let id = folder.join_folder(&name)?;
            let children = read_children(root, &id)?;
            entries.push(TreeEntry::Folder { id, name, children });
        } else if file_type.is_file() {
            let Some(stem) = name.strip_suffix(".md") else {
                continue;
            };
            let id = folder.join_note(stem)?;
            let kind = read_note_kind(&item.path())?;
            entries.push(TreeEntry::Note {
                id,
                name: stem.to_string(),
                kind,
            });
        }
    }
    entries.sort_by(|left, right| {
        rank(left)
            .cmp(&rank(right))
            .then_with(|| sort_name(left).cmp(&sort_name(right)))
    });
    Ok(entries)
}

fn rank(entry: &TreeEntry) -> u8 {
    match entry {
        TreeEntry::Folder { .. } => 0,
        TreeEntry::Note { .. } => 1,
    }
}

fn sort_name(entry: &TreeEntry) -> String {
    match entry {
        TreeEntry::Folder { name, .. } | TreeEntry::Note { name, .. } => name.to_lowercase(),
    }
}

fn ensure_note_tree(dir: &Path) -> Result<()> {
    let read = fs::read_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for item in read {
        let item = item.map_err(|source| Error::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = item.path();
        let file_type = item.file_type().map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_symlink() || (!file_type.is_dir() && !is_kept_file(&path)) {
            return Err(Error::ForeignFile { path });
        }
        if file_type.is_dir() {
            ensure_note_tree(&path)?;
        }
    }
    Ok(())
}

fn is_note_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "md")
}

fn is_kept_file(path: &Path) -> bool {
    is_note_file(path) || is_media_path(path)
}

fn remove_note_tree(dir: &Path) -> Result<()> {
    let read = fs::read_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for item in read {
        let item = item.map_err(|source| Error::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = item.path();
        let file_type = item.file_type().map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            remove_note_tree(&path)?;
        } else {
            fs::remove_file(&path).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
        }
    }
    fs::remove_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })
}
