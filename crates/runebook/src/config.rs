use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsutil::atomic_write;

/// Last-vault path, window preferences, and the saved dock layout.
///
/// The file is `$XDG_CONFIG_HOME/runebook/config.toml`.
/// When `XDG_CONFIG_HOME` is unset, the directory is `~/.config`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AppConfig {
    /// Vault opened on the last successful launch.
    #[serde(default)]
    pub last_vault: Option<PathBuf>,
    /// Input device name. Absent means the system default.
    #[serde(default)]
    pub microphone: Option<String>,
    /// Window theme.
    #[serde(default)]
    pub theme: ThemeChoice,
    /// View used when a note opens.
    #[serde(default)]
    pub note_view: NoteViewChoice,
    /// Saved dock layout, as JSON. Absent means the default split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dock_layout: Option<String>,
}

/// Light or dark window theme. Dark is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    /// Dark colors.
    #[default]
    Dark,
    /// Light colors.
    Light,
}

/// Which note surface opens first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NoteViewChoice {
    /// Rendered markdown.
    #[default]
    Preview,
    /// Markdown source.
    Source,
}

impl AppConfig {
    /// Path of the user config file.
    pub fn path() -> PathBuf {
        config_home().join("runebook").join("config.toml")
    }

    /// Load the user config. A missing file is the default config.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] when the file exists and is not valid TOML.
    /// Returns [`Error::Io`] when the file cannot be read.
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    /// Load config from `path`. A missing file is the default config.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] or [`Error::Io`].
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| Error::Config {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Write this config to [`Self::path`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConfigWrite`] or [`Error::Io`].
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    /// Write this config to `path`. Parent directories are created.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConfigWrite`] or [`Error::Io`].
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let text = toml::to_string(self).map_err(|source| Error::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;
        atomic_write(path, &text)
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn missing_dock_layout_loads_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"light\"\n").unwrap();
        let config = AppConfig::load_from(&path).unwrap();
        assert!(config.dock_layout.is_none());
        assert_eq!(config.theme, super::ThemeChoice::Light);
    }

    #[test]
    fn dock_layout_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let saved = AppConfig {
            dock_layout: Some("{\"version\":1}".to_string()),
            ..AppConfig::default()
        };
        saved.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded.dock_layout.as_deref(), Some("{\"version\":1}"));
    }
}

fn config_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home).join(".config"),
        _ => PathBuf::from(".config"),
    }
}
