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

/// Environment names for the Groq key. The first non-empty value wins.
pub(crate) const GROQ_KEY_ENVS: &[&str] = &["GROQ_API_KEY"];

/// Environment names for the OpenAI-compatible key. The first non-empty value wins.
pub(crate) const OPENAI_KEY_ENVS: &[&str] = &[
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "OPENROUTER_GATEWAY_KEY",
    "XAI_API_KEY",
    "RUNINFRA_GATEWAY_KEY",
    "VERCEL_GATEWAY_KEY",
];

/// First non-empty environment value from `names`.
pub(crate) fn first_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| env_var(name))
}

/// Saved key, or the first matching environment value.
pub(crate) fn field_secret(saved: &str, names: &[&str]) -> String {
    let saved = saved.trim();
    if !saved.is_empty() {
        return saved.to_string();
    }
    first_env(names).unwrap_or_default()
}

pub(crate) fn resolve_groq_key(saved: &str) -> Option<String> {
    let value = field_secret(saved, GROQ_KEY_ENVS);
    if value.is_empty() { None } else { Some(value) }
}

pub(crate) fn resolve_openai_key(saved: &str) -> Option<String> {
    let value = field_secret(saved, OPENAI_KEY_ENVS);
    if value.is_empty() { None } else { Some(value) }
}

/// Every key currently set in the environment.
pub(crate) fn env_secrets() -> Vec<String> {
    GROQ_KEY_ENVS
        .iter()
        .chain(OPENAI_KEY_ENVS.iter())
        .filter_map(|name| env_var(name))
        .collect()
}

/// First matching name in `values`. An empty value is skipped.
pub(crate) fn pick_env<'a>(names: &[&str], values: &[(&'a str, &'a str)]) -> Option<&'a str> {
    names.iter().find_map(|name| {
        values
            .iter()
            .find(|(key, value)| key == name && !value.trim().is_empty())
            .map(|(_, value)| value.trim())
    })
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
    use super::{OPENAI_KEY_ENVS, accept_tool_rounds, pick_env, redact_secrets, resolve_secret};

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
    fn an_empty_openai_key_uses_the_next_gateway_name() {
        let value = pick_env(
            OPENAI_KEY_ENVS,
            &[
                ("OPENAI_API_KEY", ""),
                ("OPENROUTER_API_KEY", "router-key-value"),
            ],
        );
        assert_eq!(value, Some("router-key-value"));
    }

    #[test]
    fn an_error_that_contains_a_saved_key_is_redacted() {
        let text = redact_secrets("request failed sk-saved-key-value", ["sk-saved-key-value"]);
        assert!(!text.contains("sk-saved-key-value"));
        assert!(text.contains("[redacted]"));
    }
}
