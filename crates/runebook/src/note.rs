use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

/// Relative note path without the `.md` suffix.
///
/// Segments use `/`. `journal/2026-10-23` is the file `journal/2026-10-23.md`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteId(String);

impl NoteId {
    /// Parse `raw` as a note id.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNoteId`] when `raw` is empty, absolute, hidden,
    /// or contains `.` or `..` segments.
    pub fn parse(raw: &str) -> Result<Self> {
        validate_relative(raw)?;
        Ok(Self(raw.to_string()))
    }

    /// Borrow the id text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Last path segment. This is the file stem.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Parent folder. The root folder is empty.
    pub fn parent(&self) -> FolderId {
        match self.0.rsplit_once('/') {
            Some((parent, _)) => FolderId(parent.to_string()),
            None => FolderId::root(),
        }
    }

    /// Vault-relative file path, including `.md`.
    pub fn relative_path(&self) -> PathBuf {
        let mut path = PathBuf::new();
        let mut parts = self.0.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_some() {
                path.push(part);
            } else {
                path.push(format!("{part}.md"));
            }
        }
        path
    }
}

impl std::fmt::Display for NoteId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Relative folder path inside the vault. An empty id is the vault root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FolderId(String);

impl FolderId {
    /// The vault root.
    pub fn root() -> Self {
        Self(String::new())
    }

    /// `true` when this id is the vault root.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Parse `raw` as a folder id. An empty string is the root.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNoteId`] for an unsafe relative path.
    pub fn parse(raw: &str) -> Result<Self> {
        if raw.is_empty() {
            return Ok(Self::root());
        }
        validate_relative(raw)?;
        Ok(Self(raw.to_string()))
    }

    /// Borrow the id text. The root borrows `""`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Last path segment. The root returns `""`.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or("")
    }

    /// Parent folder. The root has no parent.
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        match self.0.rsplit_once('/') {
            Some((parent, _)) => Some(Self(parent.to_string())),
            None => Some(Self::root()),
        }
    }

    /// Note id for `stem` inside this folder.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNoteId`] when `stem` is not one safe segment.
    pub fn join_note(&self, stem: &str) -> Result<NoteId> {
        if stem.contains('/') {
            return Err(Error::InvalidNoteId {
                raw: stem.to_string(),
            });
        }
        if self.is_root() {
            NoteId::parse(stem)
        } else {
            NoteId::parse(&format!("{}/{stem}", self.0))
        }
    }

    /// Child folder named `name`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNoteId`] when `name` is not one safe segment.
    pub fn join_folder(&self, name: &str) -> Result<Self> {
        if name.contains('/') {
            return Err(Error::InvalidNoteId {
                raw: name.to_string(),
            });
        }
        if self.is_root() {
            Self::parse(name)
        } else {
            Self::parse(&format!("{}/{name}", self.0))
        }
    }

    /// Vault-relative directory path. The root is an empty path.
    pub fn relative_path(&self) -> PathBuf {
        let mut path = PathBuf::new();
        if self.is_root() {
            return path;
        }
        for segment in self.0.split('/') {
            path.push(segment);
        }
        path
    }
}

impl std::fmt::Display for FolderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_root() {
            f.write_str("/")
        } else {
            f.write_str(&self.0)
        }
    }
}

/// Reject empty, absolute, hidden, and parent segments.
pub(crate) fn validate_relative(raw: &str) -> Result<()> {
    if raw.is_empty()
        || raw.starts_with('/')
        || raw.ends_with('/')
        || raw.contains('\\')
        || raw.contains('\0')
    {
        return Err(Error::InvalidNoteId {
            raw: raw.to_string(),
        });
    }
    for segment in raw.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.starts_with('.')
            || segment.chars().any(char::is_control)
        {
            return Err(Error::InvalidNoteId {
                raw: raw.to_string(),
            });
        }
    }
    Ok(())
}

/// Join `rel` onto `root` and refuse parent or absolute components.
pub(crate) fn vault_join(root: &Path, rel: &Path) -> Result<PathBuf> {
    if rel.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(Error::PathEscape {
            path: rel.to_path_buf(),
        });
    }
    Ok(root.join(rel))
}
