use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsutil::atomic_write;

/// Tool rounds used when a saved value is missing or below 1.
pub const DEFAULT_TOOL_ROUNDS: u32 = 6;

/// OpenAI-compatible host used when Settings and `OPENAI_BASE_URL` are empty.
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

/// Research model used when Settings and `OPENAI_MODEL` are empty.
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5.6-luna";

/// Archivist model used when Settings and `RUNEBOOK_MODEL` are empty.
pub const DEFAULT_ARCHIVIST_MODEL: &str = "openai:gpt-6-luna";

fn default_tool_rounds() -> u32 {
    DEFAULT_TOOL_ROUNDS
}

/// Last-vault path, window preferences, model settings, and the saved dock layout.
///
/// The file is `$XDG_CONFIG_HOME/runebook/config.toml`.
/// When `XDG_CONFIG_HOME` is unset, the directory is `~/.config`.
/// An empty model string means the matching environment variable, then the default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Cap for Research search and read, and for the Archivist tool loop.
    #[serde(default = "default_tool_rounds")]
    pub tool_rounds: u32,
    /// Groq key. Empty uses `GROQ_API_KEY`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub groq_api_key: String,
    /// OpenAI-compatible key. Empty uses `OPENAI_API_KEY`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub openai_api_key: String,
    /// OpenAI-compatible host. Empty uses `OPENAI_BASE_URL`, then [`DEFAULT_OPENAI_BASE_URL`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub openai_base_url: String,
    /// Research model. Empty uses `OPENAI_MODEL`, then [`DEFAULT_OPENAI_MODEL`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub openai_model: String,
    /// Archivist model. Empty uses `RUNEBOOK_MODEL`, then [`DEFAULT_ARCHIVIST_MODEL`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub archivist_model: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            last_vault: None,
            microphone: None,
            theme: ThemeChoice::default(),
            note_view: NoteViewChoice::default(),
            dock_layout: None,
            tool_rounds: DEFAULT_TOOL_ROUNDS,
            groq_api_key: String::new(),
            openai_api_key: String::new(),
            openai_base_url: String::new(),
            openai_model: String::new(),
            archivist_model: String::new(),
        }
    }
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
        let mut config: Self = toml::from_str(&text).map_err(|source| Error::Config {
            path: path.to_path_buf(),
            source,
        })?;
        if config.tool_rounds < 1 {
            config.tool_rounds = DEFAULT_TOOL_ROUNDS;
        }
        Ok(config)
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

    #[test]
    fn missing_model_fields_load_as_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"light\"\n").unwrap();
        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(config.tool_rounds, super::DEFAULT_TOOL_ROUNDS);
        assert!(config.groq_api_key.is_empty());
        assert!(config.openai_api_key.is_empty());
        assert!(config.openai_base_url.is_empty());
        assert!(config.openai_model.is_empty());
        assert!(config.archivist_model.is_empty());
    }

    #[test]
    fn model_fields_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let saved = AppConfig {
            tool_rounds: 8,
            groq_api_key: "groq-test-value".to_string(),
            openai_api_key: "openai-test-value".to_string(),
            openai_base_url: "https://example.test/v1".to_string(),
            openai_model: "test-model".to_string(),
            archivist_model: "openai:test-model".to_string(),
            ..AppConfig::default()
        };
        saved.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded.tool_rounds, 8);
        assert_eq!(loaded.groq_api_key, "groq-test-value");
        assert_eq!(loaded.openai_api_key, "openai-test-value");
        assert_eq!(loaded.openai_base_url, "https://example.test/v1");
        assert_eq!(loaded.openai_model, "test-model");
        assert_eq!(loaded.archivist_model, "openai:test-model");
    }

    #[test]
    fn tool_rounds_below_one_load_as_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "tool_rounds = 0\n").unwrap();
        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(config.tool_rounds, super::DEFAULT_TOOL_ROUNDS);
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
