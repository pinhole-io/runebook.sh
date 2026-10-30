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

/// What to do with the speech-language field.
pub(crate) enum LanguageEdit {
    /// Store this value. Empty means Groq detects the language.
    Set(String),
    /// The user is still typing a two-letter code.
    Pending,
    /// The text is not a language code.
    Reject,
}

/// Accept a speech language. Empty and `auto` clear the hint.
///
/// A two-letter code is stored in lowercase. One letter waits.
pub(crate) fn accept_language(text: &str) -> LanguageEdit {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("auto") {
        return LanguageEdit::Set(String::new());
    }
    let lower = text.to_ascii_lowercase();
    if !lower.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return LanguageEdit::Reject;
    }
    match lower.len() {
        1 => LanguageEdit::Pending,
        2 => LanguageEdit::Set(lower),
        _ => LanguageEdit::Reject,
    }
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

/// How many model names the menu keeps.
const MODEL_LIST_LIMIT: usize = 200;

/// How many rows of a model list are read before the cap.
const MODEL_SCAN_LIMIT: usize = 1000;

/// Largest model name the menu accepts.
const MODEL_ID_LIMIT: usize = 128;

/// Body size cap for `GET /v1/models`. A larger body is dropped.
const MODEL_BODY_LIMIT: u64 = 1024 * 1024;

/// Result of one model-list request.
pub(crate) enum ListedModels {
    /// Names from the host, already capped.
    Ready(Vec<String>),
    /// The host did not return a model list.
    Failed,
}

/// `GET /v1/models` for `base`. A host that already ends in `/v1` is not doubled.
///
/// A URL with a user name, or a scheme other than `http` or `https`, is refused.
pub(crate) fn models_url(base: &str) -> Option<String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() || base.contains(['\n', '\r', ' ', '\t']) {
        return None;
    }
    let rest = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"))?;
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() || host.contains('@') {
        return None;
    }
    if base.ends_with("/v1") {
        Some(format!("{base}/models"))
    } else {
        Some(format!("{base}/v1/models"))
    }
}

/// Model names from an OpenAI-shaped `data` array.
///
/// Blank names and names with control characters are dropped. The list is sorted
/// and capped at [`MODEL_LIST_LIMIT`].
pub(crate) fn parse_model_ids(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let Some(rows) = value.get("data").and_then(|data| data.as_array()) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for row in rows.iter().take(MODEL_SCAN_LIMIT) {
        let Some(id) = row
            .get("id")
            .and_then(|id| id.as_str())
            .and_then(accept_model_id)
        else {
            continue;
        };
        if ids.iter().any(|item: &String| item == id) {
            continue;
        }
        ids.push(id.to_string());
    }
    ids.sort_by(|left, right| left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase()));
    ids.truncate(MODEL_LIST_LIMIT);
    ids
}

/// Menu rows: `default_label`, then `ids`, then any saved name the list omitted.
pub(crate) fn model_choices(default_label: &str, ids: &[String], saved: &[&str]) -> Vec<String> {
    let mut names = Vec::with_capacity(ids.len() + saved.len() + 1);
    names.push(default_label.to_string());
    for id in ids {
        if id != default_label && !names.iter().any(|item| item == id) {
            names.push(id.clone());
        }
    }
    for saved in saved {
        let saved = saved.trim();
        if saved.is_empty() || names.iter().any(|item| item == saved) {
            continue;
        }
        names.push(saved.to_string());
    }
    names
}

/// Load model names from `base`. An empty `key` sends no `Authorization` header.
///
/// A failure returns [`ListedModels::Failed`]. The error text is dropped so a key
/// cannot leak through a URL or a response body.
pub(crate) fn fetch_model_ids(base: &str, key: &str) -> ListedModels {
    let Some(url) = models_url(base) else {
        return ListedModels::Failed;
    };
    let Ok(client) = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
    else {
        return ListedModels::Failed;
    };
    let mut request = client.get(&url);
    let key = key.trim();
    if !key.is_empty() {
        request = request.bearer_auth(key);
    }
    let Ok(response) = request.send() else {
        return ListedModels::Failed;
    };
    if !response.status().is_success() {
        return ListedModels::Failed;
    }
    let mut body = Vec::new();
    let mut limited = std::io::Read::take(response, MODEL_BODY_LIMIT + 1);
    if std::io::Read::read_to_end(&mut limited, &mut body).is_err()
        || body.len() as u64 > MODEL_BODY_LIMIT
    {
        return ListedModels::Failed;
    }
    let Ok(text) = String::from_utf8(body) else {
        return ListedModels::Failed;
    };
    if !text.contains("\"data\"") {
        return ListedModels::Failed;
    }
    ListedModels::Ready(parse_model_ids(&text))
}

fn accept_model_id(id: &str) -> Option<&str> {
    let id = id.trim();
    if id.is_empty() || id.chars().count() > MODEL_ID_LIMIT {
        return None;
    }
    if id.chars().any(|ch| ch.is_control()) {
        return None;
    }
    Some(id)
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
    use super::{
        LanguageEdit, OPENAI_KEY_ENVS, accept_language, accept_tool_rounds, model_choices,
        models_url, parse_model_ids, pick_env, redact_secrets, resolve_secret,
    };

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
    fn a_speech_language_is_a_two_letter_code() {
        assert!(matches!(accept_language(""), LanguageEdit::Set(value) if value.is_empty()));
        assert!(matches!(accept_language("auto"), LanguageEdit::Set(value) if value.is_empty()));
        assert!(matches!(accept_language("e"), LanguageEdit::Pending));
        assert!(matches!(accept_language("EN"), LanguageEdit::Set(value) if value == "en"));
        assert!(matches!(accept_language("english"), LanguageEdit::Reject));
        assert!(matches!(accept_language("e1"), LanguageEdit::Reject));
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
    fn a_host_without_v1_gains_the_models_path() {
        assert_eq!(
            models_url("https://api.openai.com").as_deref(),
            Some("https://api.openai.com/v1/models")
        );
        assert_eq!(
            models_url("https://api.openai.com/v1/").as_deref(),
            Some("https://api.openai.com/v1/models")
        );
        assert_eq!(models_url("https://user:secret@api.openai.com"), None);
        assert_eq!(models_url("file:///tmp/models"), None);
    }

    #[test]
    fn model_ids_keep_the_name_and_drop_a_blank() {
        let body = r#"{"data":[{"id":"openai:gpt-6-luna"},{"id":" "},{"id":"groq:llama"}]}"#;
        assert_eq!(
            parse_model_ids(body),
            vec!["groq:llama".to_string(), "openai:gpt-6-luna".to_string()]
        );
    }

    #[test]
    fn a_saved_model_stays_when_the_list_omits_it() {
        let names = model_choices("Default", &["openai:a".to_string()], &["custom:b", ""]);
        assert_eq!(
            names,
            vec![
                "Default".to_string(),
                "openai:a".to_string(),
                "custom:b".to_string()
            ]
        );
    }

    #[test]
    fn an_error_that_contains_a_saved_key_is_redacted() {
        let text = redact_secrets("request failed sk-saved-key-value", ["sk-saved-key-value"]);
        assert!(!text.contains("sk-saved-key-value"));
        assert!(text.contains("[redacted]"));
    }
}
