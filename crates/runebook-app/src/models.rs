//! Resolve Settings fields against the environment.
//!
//! An empty saved value uses the environment value. A saved value wins.
//! The status line and the note must not show a key.

use runebook::{
    AppConfig, DEFAULT_ARCHIVIST_MODEL, DEFAULT_OPENAI_BASE_URL, DEFAULT_OPENAI_MODEL,
    DEFAULT_TOOL_ROUNDS,
};

/// Longest saved key, host, or model name.
const VALUE_LIMIT: usize = 512;

/// Saved model settings. Empty strings mean the environment, then the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelSettings {
    pub(crate) tool_rounds: u32,
    pub(crate) groq_api_key: String,
    pub(crate) openai_api_key: String,
    pub(crate) openai_base_url: String,
    pub(crate) openai_model: String,
    pub(crate) archivist_model: String,
}

impl ModelSettings {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        Self {
            tool_rounds: if config.tool_rounds < 1 {
                DEFAULT_TOOL_ROUNDS
            } else {
                config.tool_rounds
            },
            groq_api_key: config.groq_api_key.clone(),
            openai_api_key: config.openai_api_key.clone(),
            openai_base_url: config.openai_base_url.clone(),
            openai_model: config.openai_model.clone(),
            archivist_model: config.archivist_model.clone(),
        }
    }
}

/// `Some` when `text` is within [`VALUE_LIMIT`].
pub(crate) fn bounded(text: &str) -> Option<&str> {
    let text = text.trim();
    if text.chars().count() > VALUE_LIMIT {
        None
    } else {
        Some(text)
    }
}

/// Parse a tool-round cap. A value below 1 is rejected.
pub(crate) fn accept_tool_rounds(text: &str) -> Option<u32> {
    let value = text.trim().parse::<u32>().ok()?;
    if value < 1 { None } else { Some(value) }
}

/// Saved text, then `env_value`, then `fallback`.
pub(crate) fn resolve_text(saved: &str, env_value: Option<&str>, fallback: &str) -> String {
    let saved = saved.trim();
    if !saved.is_empty() {
        return saved.to_string();
    }
    if let Some(value) = env_value.map(str::trim).filter(|value| !value.is_empty()) {
        return value.to_string();
    }
    fallback.to_string()
}

/// Saved key, then `env_value`. Empty means unset.
pub(crate) fn resolve_secret(saved: &str, env_value: Option<&str>) -> Option<String> {
    let value = resolve_text(saved, env_value, "");
    if value.is_empty() { None } else { Some(value) }
}

pub(crate) fn env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn resolve_openai_base(saved: &str) -> String {
    resolve_text(
        saved,
        env_var("OPENAI_BASE_URL").as_deref(),
        DEFAULT_OPENAI_BASE_URL,
    )
}

pub(crate) fn resolve_openai_model(saved: &str) -> String {
    resolve_text(
        saved,
        env_var("OPENAI_MODEL").as_deref(),
        DEFAULT_OPENAI_MODEL,
    )
}

pub(crate) fn resolve_archivist_model(saved: &str) -> String {
    resolve_text(
        saved,
        env_var("RUNEBOOK_MODEL").as_deref(),
        DEFAULT_ARCHIVIST_MODEL,
    )
}

pub(crate) fn resolve_groq_key(saved: &str) -> Option<String> {
    resolve_secret(saved, env_var("GROQ_API_KEY").as_deref())
}

pub(crate) fn resolve_openai_key(saved: &str) -> Option<String> {
    resolve_secret(saved, env_var("OPENAI_API_KEY").as_deref())
}

/// Replace each secret of 8 characters or more with `[redacted]`.
pub(crate) fn redact_secrets<'a>(text: &str, secrets: impl IntoIterator<Item = &'a str>) -> String {
    let mut text = text.to_string();
    for secret in secrets {
        let secret = secret.trim();
        if secret.len() >= 8 {
            text = text.replace(secret, "[redacted]");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{accept_tool_rounds, redact_secrets, resolve_secret};

    #[test]
    fn an_empty_saved_key_uses_the_environment_value() {
        let value = resolve_secret("", Some("env-key-value"));
        assert_eq!(value.as_deref(), Some("env-key-value"));
    }

    #[test]
    fn a_saved_key_wins_over_the_environment() {
        let value = resolve_secret("saved-key-value", Some("env-key-value"));
        assert_eq!(value.as_deref(), Some("saved-key-value"));
    }

    #[test]
    fn tool_rounds_below_one_are_rejected() {
        assert_eq!(accept_tool_rounds("0"), None);
        assert_eq!(accept_tool_rounds(""), None);
        assert_eq!(accept_tool_rounds("6"), Some(6));
    }

    #[test]
    fn an_error_that_contains_a_saved_key_is_redacted() {
        let text = redact_secrets("request failed sk-saved-key-value", ["sk-saved-key-value"]);
        assert!(!text.contains("sk-saved-key-value"));
        assert!(text.contains("[redacted]"));
    }
}
