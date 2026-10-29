use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsutil::atomic_write;

/// Tool rounds used when a saved value is missing or below 1.
pub const DEFAULT_TOOL_ROUNDS: u32 = 250;

fn default_tool_rounds() -> u32 {
    DEFAULT_TOOL_ROUNDS
}

fn default_true() -> bool {
    true
}

/// Window language used when `locale` is missing or blank.
pub const DEFAULT_LOCALE: &str = "en";

fn default_locale() -> String {
    DEFAULT_LOCALE.to_string()
}

fn default_theme() -> String {
    "dark".to_string()
}

/// OpenAI-compatible host used when Settings and `OPENAI_BASE_URL` are empty.
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

/// Research model used when Settings and `OPENAI_MODEL` are empty.
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5.6-luna";

/// Archivist model used when Settings and `RUNEBOOK_MODEL` are empty.
pub const DEFAULT_ARCHIVIST_MODEL: &str = "openai:gpt-6-luna";

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
    /// Note id opened last. Used when [`Self::reopen_last_note`] is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_note: Option<String>,
    /// Open [`Self::last_note`] when that note is still in the vault.
    #[serde(default = "default_true")]
    pub reopen_last_note: bool,
    /// Input device name. Absent means the system default.
    #[serde(default)]
    pub microphone: Option<String>,
    /// Groq transcription model.
    #[serde(default)]
    pub speech_model: SpeechModel,
    /// Spoken language hint. Empty lets Groq detect the language.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub speech_language: String,
    /// Theme name. `dark` and `light` select the built-in themes.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Window language code. An unknown code uses English in the app.
    #[serde(default = "default_locale")]
    pub locale: String,
    /// Body and editor text size. Medium matches the theme file.
    #[serde(default)]
    pub text_size: TextSize,
    /// View used when a note opens.
    #[serde(default)]
    pub note_view: NoteViewChoice,
    /// Show line numbers in the source editor.
    #[serde(default = "default_true")]
    pub line_numbers: bool,
    /// Wrap long lines in the source editor.
    #[serde(default = "default_true")]
    pub soft_wrap: bool,
    /// When an edit is written.
    #[serde(default)]
    pub autosave: AutosavePace,
    /// New clips start muted.
    #[serde(default)]
    pub media_muted: bool,
    /// Saved dock layout, as JSON. Absent means the default split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dock_layout: Option<String>,
    /// Cap for Research search and read, and for the Archivist tool loop.
    #[serde(default = "default_tool_rounds")]
    pub tool_rounds: u32,
    /// Groq key. Empty uses `GROQ_API_KEY`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub groq_api_key: String,
    /// OpenAI-compatible key. Empty uses `OPENAI_API_KEY`, then the other gateway keys.
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
            last_note: None,
            reopen_last_note: true,
            microphone: None,
            speech_model: SpeechModel::default(),
            speech_language: String::new(),
            theme: default_theme(),
            locale: default_locale(),
            text_size: TextSize::default(),
            note_view: NoteViewChoice::default(),
            line_numbers: true,
            soft_wrap: true,
            autosave: AutosavePace::default(),
            media_muted: false,
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

/// Body and editor text size. Medium matches the theme file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    /// Two pixels below the theme file.
    Small,
    /// The size in the theme file.
    #[default]
    Medium,
    /// Four pixels above the theme file.
    Large,
}

/// When an edit is written to the note file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutosavePace {
    /// Write 200 ms after the last edit.
    Fast,
    /// Write 400 ms after the last edit.
    #[default]
    Normal,
    /// Write 1 second after the last edit.
    Slow,
    /// Write when the note closes or the window closes.
    Manual,
}

impl AutosavePace {
    /// Milliseconds before an edit is written.
    ///
    /// [`Self::Manual`] is `None`. The note is still written on leave and close.
    pub fn delay_ms(self) -> Option<u64> {
        match self {
            Self::Fast => Some(200),
            Self::Normal => Some(400),
            Self::Slow => Some(1_000),
            Self::Manual => None,
        }
    }
}

/// Groq model used for a Speech transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpeechModel {
    /// `whisper-large-v3-turbo`.
    #[default]
    Fast,
    /// `whisper-large-v3`.
    Accurate,
}

impl SpeechModel {
    /// Model name sent to Groq.
    pub fn api_name(self) -> &'static str {
        match self {
            Self::Fast => "whisper-large-v3-turbo",
            Self::Accurate => "whisper-large-v3",
        }
    }
}

impl AppConfig {
    /// Path of the user config file.
    pub fn path() -> PathBuf {
        config_home().join("runebook").join("config.toml")
    }

    /// Directory of user theme files.
    ///
    /// The path is `$XDG_CONFIG_HOME/runebook/themes`.
    /// When `XDG_CONFIG_HOME` is unset, the directory is `~/.config/runebook/themes`.
    pub fn themes_dir() -> PathBuf {
        config_home().join("runebook").join("themes")
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
        if config.locale.trim().is_empty() {
            config.locale = default_locale();
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
        assert_eq!(config.theme, "light");
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

    #[test]
    fn a_missing_preference_uses_the_current_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"light\"\n").unwrap();
        let config = AppConfig::load_from(&path).unwrap();
        assert!(config.reopen_last_note);
        assert!(config.last_note.is_none());
        assert!(config.line_numbers);
        assert!(config.soft_wrap);
        assert!(!config.media_muted);
        assert!(config.speech_language.is_empty());
        assert_eq!(config.locale, super::DEFAULT_LOCALE);
        assert_eq!(config.text_size, super::TextSize::Medium);
        assert_eq!(config.autosave, super::AutosavePace::Normal);
        assert_eq!(config.speech_model, super::SpeechModel::Fast);
        assert_eq!(config.autosave.delay_ms(), Some(400));
        assert_eq!(super::SpeechModel::Accurate.api_name(), "whisper-large-v3");
    }

    #[test]
    fn window_preferences_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let saved = AppConfig {
            last_note: Some("journal/today".to_string()),
            reopen_last_note: false,
            speech_model: super::SpeechModel::Accurate,
            speech_language: "en".to_string(),
            locale: "is".to_string(),
            text_size: super::TextSize::Large,
            line_numbers: false,
            soft_wrap: false,
            autosave: super::AutosavePace::Manual,
            media_muted: true,
            ..AppConfig::default()
        };
        saved.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded, saved);
        assert_eq!(loaded.autosave.delay_ms(), None);
        assert_eq!(loaded.locale, "is");
    }

    #[test]
    fn a_blank_locale_loads_as_english() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "locale = \"  \"\n").unwrap();
        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(config.locale, super::DEFAULT_LOCALE);
    }
}
