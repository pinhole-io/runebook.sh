//! System prompt and provider keys for one Archivist turn.
//!
//! The window passes the key strings. This module does not read the environment.

use superglue::agents::AgentSpec;
use superglue::providers::{ProviderId, parse_model_ref};

/// The model has no key for its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// The provider key is empty.
    Missing,
}

/// Which key `ClientBuilder` receives.
///
/// `ClientBuilder::api_key_for` copies `api_key` onto OpenAI when that
/// provider has no key. `primary` is the OpenAI key when one is set, so the
/// Groq key cannot land on OpenAI.
#[derive(Debug)]
pub struct KeyPlan {
    pub primary: String,
    pub base: String,
    pub groq: Option<String>,
    pub openai: Option<String>,
    pub openai_provider: ProviderId,
}

/// `true` when `model` has the key it sends with.
pub fn has_model_key(model: &str, groq: &str, openai: &str) -> bool {
    key_plan(model, groq, openai, ProviderId::OpenAi.default_base_url()).is_ok()
}

/// Choose the keys and the base URL for `model`.
pub fn key_plan(model: &str, groq: &str, openai: &str, base: &str) -> Result<KeyPlan, KeyError> {
    let groq = groq.trim();
    let openai = openai.trim();
    let provider = parse_model_ref(model).provider;
    let needed = if provider == ProviderId::Groq {
        groq
    } else {
        openai
    };
    if needed.is_empty() {
        return Err(KeyError::Missing);
    }
    let base = base.trim();
    let base = if base.is_empty() {
        provider.default_base_url().to_string()
    } else {
        base.to_string()
    };
    let primary = if openai.is_empty() { groq } else { openai };
    let openai_provider = if provider == ProviderId::Groq {
        ProviderId::OpenAi
    } else {
        provider
    };
    Ok(KeyPlan {
        primary: primary.to_string(),
        base,
        groq: (!groq.is_empty()).then(|| groq.to_string()),
        openai: (!openai.is_empty()).then(|| openai.to_string()),
        openai_provider,
    })
}

/// System prompt for one Archivist turn.
pub fn archivist_prompt(rounds: u32) -> String {
    let spec = AgentSpec::new("Archivist", "the Archivist of this vault")
        .with_max_tool_rounds(rounds)
        .with_goal("Keep each note accurate.")
        .with_goal("Find the real id before you change a file.")
        .with_goal("Use a tool for every vault change.")
        .with_goal("Change only the text the user asked to change.")
        .with_goal("Report each note id and folder id you changed.")
        .with_instructions(
            "A note id is a vault path with no .md suffix. Example: journal/2026-10-23. \
             A folder id uses the same shape. Example: journal. \
             An id is relative. It is not absolute, not hidden, and it has no . or .. segment. \
             Call list_notes or search_notes before you use an id.",
        )
        .with_instructions(
            "A note is Book, Speech, Sight, Deed, or Research. \
             Book has no header. Speech, Sight, Deed, and Research start with this header:\n\
             ---\n\
             kind: speech\n\
             ---\n\
             The kind key is speech, sight, deed, or research. \
             write_note keeps the old kind when the new text has no header. \
             Put a header in contents only when the user asked for that kind.",
        )
        .with_instructions(
            "Speech is a transcript. You do not record audio. \
             Sight is a markdown link to a video beside the note. You do not record the screen. \
             Deed is a task list. A task line uses `- [ ]` or `- [x]`. \
             Research belongs to Charlotte. You do not start a Research run. \
             You may still edit those notes when the user asks.",
        )
        .with_instructions(
            "`[[target]]` links to a note. `[[target|alias]]` shows the alias and still links to \
             target. Prefer an exact note id. A stem with no / matches one note only when that \
             stem is unique. After a rename, search for the old id and update wiki links that \
             use it.",
        )
        .with_instructions(
            "Read the note before write_note. That call replaces the whole file. \
             Keep every line the user did not ask to change. \
             create_note fails when the note already exists. It creates missing parent folders. \
             A read can set truncated at 32 KiB. Do not write that text back. \
             list_notes can stop at 400 entries. \
             Selected rows in the user message are a slice. Read the note, then write the full \
             note. An attached file is source material. A vault write still uses a tool. \
             Call open_note when the user asks to open or look at a note.",
        )
        .with_instructions(
            "Write every reply in Markdown. Use paragraphs, lists, and inline code for note ids \
             and folder ids. Do not wrap the whole reply in a code fence. \
             Do not include a thinking section in the reply. \
             Name each id you created, changed, renamed, or deleted. \
             When a tool returns an error, say that error.",
        )
        .with_constraint("Do not invent a note id or a folder id.")
        .with_constraint(
            "Do not delete a note or a folder unless the user asked to delete that note or folder.",
        )
        .with_constraint("Do not claim a write that the tool rejected.")
        .with_constraint("Do not change a note kind unless the user asked.")
        .with_constraint("Do not record audio or video, and do not start a Research run.")
        .with_constraint("Do not write a truncated read back over a note.");
    spec.compile_system_prompt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_openai_model_does_not_use_the_groq_key() {
        let plan = key_plan(
            "openai:gpt-6-luna",
            "gsk-groq-key-value",
            "sk-openai-key-value",
            "https://gateway.example/v1",
        )
        .expect("both keys are set");
        assert_eq!(plan.primary, "sk-openai-key-value");
        assert_eq!(plan.groq.as_deref(), Some("gsk-groq-key-value"));
        assert_eq!(plan.openai.as_deref(), Some("sk-openai-key-value"));
        assert_eq!(plan.openai_provider, ProviderId::OpenAi);
        assert_eq!(plan.base, "https://gateway.example/v1");
    }

    #[test]
    fn an_openai_model_rejects_a_groq_key_used_alone() {
        let err = key_plan(
            "openai:gpt-6-luna",
            "gsk-groq-key-value",
            "",
            "https://api.openai.com",
        )
        .expect_err("the OpenAI key is required");
        assert_eq!(err, KeyError::Missing);
    }

    #[test]
    fn a_groq_model_keeps_its_own_key() {
        let plan = key_plan(
            "groq:llama-3.3-70b-versatile",
            "gsk-groq-key-value",
            "sk-openai-key-value",
            "https://api.openai.com",
        )
        .expect("the Groq key is set");
        assert_eq!(plan.groq.as_deref(), Some("gsk-groq-key-value"));
        assert_eq!(plan.primary, "sk-openai-key-value");
        assert_eq!(plan.openai_provider, ProviderId::OpenAi);
    }

    #[test]
    fn archivist_prompt_describes_the_vault() {
        let prompt = archivist_prompt(8);
        assert!(
            prompt.starts_with("You are the Archivist of this vault."),
            "{prompt}"
        );
        for kind in ["Book", "Speech", "Sight", "Deed", "Research"] {
            assert!(prompt.contains(kind), "missing {kind} in {prompt}");
        }
        assert!(prompt.contains("kind: speech"), "{prompt}");
        assert!(prompt.contains("[[target|alias]]"), "{prompt}");
        assert!(prompt.contains("32 KiB"), "{prompt}");
    }
}
