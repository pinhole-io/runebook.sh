//! Vault tools for the Archivist.
//!
//! Each tool parses a note id or a folder id and calls [`Vault`].
//! A tool result never includes a filesystem path.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use runebook::{Error, FolderId, NoteId, TreeEntry, Vault};
use serde_json::{Value, json};
use superglue::Client;
use superglue::tools::{Tool, ToolContextPolicy, ToolInvokeError, ToolSpec};

use super::VaultChange;

const READ_CAP: usize = 32 * 1024;
const SEARCH_HITS: usize = 20;
const LIST_CAP: usize = 400;
const EXCERPT_CAP: usize = 120;

#[derive(Debug)]
struct ToolOutcome {
    value: Value,
    changes: Vec<VaultChange>,
}

type ToolFn = fn(&Vault, &Value) -> Result<ToolOutcome, String>;

struct Operation {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
    call: ToolFn,
}

fn operations() -> &'static [Operation] {
    &[
        Operation {
            name: "list_notes",
            description: "List the folders and notes in the vault. Ids are relative and have no .md suffix.",
            schema: empty_schema,
            call: list_notes,
        },
        Operation {
            name: "read_note",
            description: "Read one note. The id is a relative path without .md.",
            schema: || id_schema("id", "Note id, such as journal/2026-10-23"),
            call: read_note,
        },
        Operation {
            name: "open_note",
            description: "Show one note in the preview. The id is a relative path without .md.",
            schema: || id_schema("id", "Note id to show, such as journal/2026-10-23"),
            call: open_note,
        },
        Operation {
            name: "search_notes",
            description: "Find notes whose text contains the query. Each hit includes a short excerpt.",
            schema: || {
                schema(
                    json!({
                        "query": { "type": "string", "description": "Text to find in note bodies." }
                    }),
                    &["query"],
                )
            },
            call: search_notes,
        },
        Operation {
            name: "create_note",
            description: "Create a note and any missing parent folders. Fails when the note already exists.",
            schema: note_body_schema,
            call: create_note,
        },
        Operation {
            name: "write_note",
            description: "Replace the whole body of an existing note. Read the note first.",
            schema: note_body_schema,
            call: write_note,
        },
        Operation {
            name: "rename_note",
            description: "Rename a note. from and to are note ids.",
            schema: || pair_schema("from", "to", "Note id"),
            call: rename_note,
        },
        Operation {
            name: "delete_note",
            description: "Delete one note. Use this only when the user asked to delete that note.",
            schema: || id_schema("id", "Note id to delete."),
            call: delete_note,
        },
        Operation {
            name: "create_folder",
            description: "Create a folder and any missing parent folders.",
            schema: || id_schema("id", "Folder id, such as journal."),
            call: create_folder,
        },
        Operation {
            name: "rename_folder",
            description: "Rename a folder. from and to are folder ids.",
            schema: || pair_schema("from", "to", "Folder id"),
            call: rename_folder,
        },
        Operation {
            name: "delete_folder",
            description: "Delete a folder of notes and media files. A file that is not a note or a media file blocks the delete.",
            schema: || id_schema("id", "Folder id to delete."),
            call: delete_folder,
        },
    ]
}

/// Register every vault tool on `client`.
///
/// The client register path fails the turn when a tool returns an error.
/// A rejected id is a JSON result so the model can continue.
pub(super) async fn register(
    client: &Client,
    vault: Vault,
    changes: Arc<Mutex<Vec<VaultChange>>>,
) -> Result<(), String> {
    let gate = Arc::new(Mutex::new(()));
    for op in operations() {
        let tool = VaultTool {
            spec: ToolSpec::new(op.name, (op.schema)()).with_description(op.description),
            vault: vault.clone(),
            changes: Arc::clone(&changes),
            gate: Arc::clone(&gate),
            call: op.call,
        };
        client
            .register_tool(Arc::new(tool))
            .await
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

struct VaultTool {
    spec: ToolSpec,
    vault: Vault,
    changes: Arc<Mutex<Vec<VaultChange>>>,
    gate: Arc<Mutex<()>>,
    call: ToolFn,
}

#[async_trait]
impl Tool for VaultTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn context_policy(&self) -> ToolContextPolicy {
        ToolContextPolicy::Inline
    }

    async fn call(&self, arguments: Value) -> Result<Value, ToolInvokeError> {
        let _gate = self.gate.lock().unwrap_or_else(|err| err.into_inner());
        match (self.call)(&self.vault, &arguments) {
            Ok(ToolOutcome { value, changes }) => {
                self.changes
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .extend(changes);
                Ok(value)
            }
            Err(message) => Ok(json!({ "ok": false, "error": message })),
        }
    }
}

fn list_notes(vault: &Vault, _: &Value) -> Result<ToolOutcome, String> {
    let tree = vault.tree().map_err(tool_error)?;
    let mut left = LIST_CAP;
    let mut children = Vec::new();
    let truncated = push_entries(&tree.children, &mut left, &mut children);
    Ok(ToolOutcome {
        value: json!({ "children": children, "truncated": truncated }),
        changes: Vec::new(),
    })
}

fn read_note(vault: &Vault, args: &Value) -> Result<ToolOutcome, String> {
    let id = note_id(args, "id")?;
    let text = vault.read(&id).map_err(tool_error)?;
    let (content, truncated) = clip_bytes(&text, READ_CAP);
    Ok(ToolOutcome {
        value: json!({ "id": id.as_str(), "content": content, "truncated": truncated }),
        changes: Vec::new(),
    })
}

fn open_note(vault: &Vault, args: &Value) -> Result<ToolOutcome, String> {
    let id = note_id(args, "id")?;
    vault.read(&id).map_err(tool_error)?;
    Ok(ToolOutcome {
        value: json!({ "ok": true, "id": id.as_str() }),
        changes: vec![VaultChange::Opened(id)],
    })
}

fn search_notes(vault: &Vault, args: &Value) -> Result<ToolOutcome, String> {
    let query = arg_str(args, "query")?.trim();
    if query.is_empty() {
        return Err("query is empty".to_string());
    }
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    let mut truncated = false;
    for id in note_ids(vault)? {
        let text = vault.read(&id).map_err(tool_error)?;
        if !text.to_lowercase().contains(&needle) {
            continue;
        }
        if hits.len() == SEARCH_HITS {
            truncated = true;
            break;
        }
        hits.push(json!({
            "id": id.as_str(),
            "excerpt": excerpt(&text, &needle),
        }));
    }
    Ok(ToolOutcome {
        value: json!({ "query": query, "hits": hits, "truncated": truncated }),
        changes: Vec::new(),
    })
}

fn create_note(vault: &Vault, args: &Value) -> Result<ToolOutcome, String> {
    let id = note_id(args, "id")?;
    let contents = optional_str(args, "contents")?;
    vault.create_note(&id, contents).map_err(tool_error)?;
    Ok(ToolOutcome {
        value: json!({ "ok": true, "id": id.as_str() }),
        changes: vec![VaultChange::Wrote(id)],
    })
}

fn write_note(vault: &Vault, args: &Value) -> Result<ToolOutcome, String> {
    let id = note_id(args, "id")?;
    let contents = arg_str(args, "contents")?;
    vault.write(&id, contents).map_err(tool_error)?;
    Ok(ToolOutcome {
        value: json!({ "ok": true, "id": id.as_str() }),
        changes: vec![VaultChange::Wrote(id)],
    })
}

fn rename_note(vault: &Vault, args: &Value) -> Result<ToolOutcome, String> {
    let from = note_id(args, "from")?;
    let to = note_id(args, "to")?;
    vault.rename_note(&from, &to).map_err(tool_error)?;
    Ok(ToolOutcome {
        value: json!({ "ok": true, "from": from.as_str(), "to": to.as_str() }),
        changes: vec![VaultChange::RenamedNote { from, to }],
    })
}

fn delete_note(vault: &Vault, args: &Value) -> Result<ToolOutcome, String> {
    let id = note_id(args, "id")?;
    vault.delete_note(&id).map_err(tool_error)?;
    Ok(ToolOutcome {
        value: json!({ "ok": true, "id": id.as_str() }),
        changes: vec![VaultChange::DeletedNote(id)],
    })
}

fn create_folder(vault: &Vault, args: &Value) -> Result<ToolOutcome, String> {
    let id = folder_id(args, "id")?;
    vault.create_folder(&id).map_err(tool_error)?;
    Ok(ToolOutcome {
        value: json!({ "ok": true, "id": id.as_str() }),
        changes: vec![VaultChange::Folder(id)],
    })
}

fn rename_folder(vault: &Vault, args: &Value) -> Result<ToolOutcome, String> {
    let from = folder_id(args, "from")?;
    let to = folder_id(args, "to")?;
    vault.rename_folder(&from, &to).map_err(tool_error)?;
    Ok(ToolOutcome {
        value: json!({ "ok": true, "from": from.as_str(), "to": to.as_str() }),
        changes: vec![VaultChange::RenamedFolder { from, to }],
    })
}

fn delete_folder(vault: &Vault, args: &Value) -> Result<ToolOutcome, String> {
    let id = folder_id(args, "id")?;
    vault.delete_folder(&id).map_err(tool_error)?;
    Ok(ToolOutcome {
        value: json!({ "ok": true, "id": id.as_str() }),
        changes: vec![VaultChange::DeletedFolder(id)],
    })
}

fn note_ids(vault: &Vault) -> Result<Vec<NoteId>, String> {
    let tree = vault.tree().map_err(tool_error)?;
    let mut ids = Vec::new();
    collect_notes(&tree.children, &mut ids);
    Ok(ids)
}

fn collect_notes(entries: &[TreeEntry], ids: &mut Vec<NoteId>) {
    for entry in entries {
        match entry {
            TreeEntry::Note { id, .. } => ids.push(id.clone()),
            TreeEntry::Folder { children, .. } => collect_notes(children, ids),
        }
    }
}

/// Returns true when the walk stopped at [`LIST_CAP`].
fn push_entries(entries: &[TreeEntry], left: &mut usize, out: &mut Vec<Value>) -> bool {
    for entry in entries {
        if *left == 0 {
            return true;
        }
        match entry {
            TreeEntry::Note { id, name } => {
                *left -= 1;
                out.push(json!({ "kind": "note", "id": id.as_str(), "name": name }));
            }
            TreeEntry::Folder { id, name, children } => {
                *left -= 1;
                let mut nested = Vec::new();
                let truncated = push_entries(children, left, &mut nested);
                out.push(json!({
                    "kind": "folder",
                    "id": id.as_str(),
                    "name": name,
                    "children": nested,
                }));
                if truncated {
                    return true;
                }
            }
        }
    }
    false
}

fn excerpt(text: &str, needle: &str) -> String {
    for line in text.lines() {
        if line.to_lowercase().contains(needle) {
            return clip_chars(line, EXCERPT_CAP);
        }
    }
    clip_chars(text, EXCERPT_CAP)
}

fn clip_chars(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let end = text
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    format!("{}…", &text[..end])
}

fn clip_bytes(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}…", &text[..end]), true)
}

fn note_id(args: &Value, key: &str) -> Result<NoteId, String> {
    NoteId::parse(arg_str(args, key)?.trim()).map_err(tool_error)
}

fn folder_id(args: &Value, key: &str) -> Result<FolderId, String> {
    FolderId::parse(arg_str(args, key)?.trim()).map_err(tool_error)
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    match args.get(key) {
        Some(Value::String(text)) => Ok(text.as_str()),
        Some(_) => Err(format!("{key} must be a string")),
        None => Err(format!("missing {key}")),
    }
}

fn optional_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(text)) => Ok(text.as_str()),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn tool_error(err: Error) -> String {
    match err {
        Error::InvalidNoteId { raw } => {
            format!("invalid note id: {}", clip_chars(&raw, 80))
        }
        Error::Io { .. }
        | Error::PathEscape { .. }
        | Error::ForeignFile { .. }
        | Error::Symlink { .. }
        | Error::NotADirectory { .. }
        | Error::NotAVault { .. }
        | Error::AlreadyExists { .. }
        | Error::MediaNotFound { .. }
        | Error::Config { .. }
        | Error::ConfigWrite { .. } => vault_failure(&err),
        other => other.to_string(),
    }
}

/// A message for errors whose display text includes a filesystem path.
fn vault_failure(err: &Error) -> String {
    match err {
        Error::Io { .. } => "could not access a file in the vault".to_string(),
        Error::PathEscape { .. } => "path escapes the vault".to_string(),
        Error::ForeignFile { .. } => "folder contains a file that is not a note".to_string(),
        Error::Symlink { .. } => "refusing to follow a symlink".to_string(),
        Error::MediaNotFound { .. } => "media file not found".to_string(),
        Error::NotADirectory { .. } | Error::NotAVault { .. } | Error::AlreadyExists { .. } => {
            "vault path is not available".to_string()
        }
        Error::Config { .. } | Error::ConfigWrite { .. } => "vault config is invalid".to_string(),
        _ => "could not change the vault".to_string(),
    }
}

fn empty_schema() -> Value {
    schema(json!({}), &[])
}

fn id_schema(key: &str, description: &str) -> Value {
    schema(
        json!({ key: { "type": "string", "description": description } }),
        &[key],
    )
}

fn note_body_schema() -> Value {
    schema(
        json!({
            "id": { "type": "string", "description": "Note id, such as journal/2026-10-23." },
            "contents": { "type": "string", "description": "Full markdown body." }
        }),
        &["id", "contents"],
    )
}

fn pair_schema(from: &str, to: &str, kind: &str) -> Value {
    schema(
        json!({
            from: { "type": "string", "description": format!("Current {kind}.") },
            to: { "type": "string", "description": format!("New {kind}.") }
        }),
        &[from, to],
    )
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    struct TempVault {
        path: PathBuf,
        vault: Vault,
    }

    impl TempVault {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("runebook-archivist-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            let vault = Vault::create(&path, "Notes").unwrap();
            Self { path, vault }
        }
    }

    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn rejects_a_note_id_that_leaves_the_vault() {
        let temp = TempVault::new();
        let err = read_note(&temp.vault, &json!({ "id": "../secret" })).unwrap_err();
        assert!(err.contains("invalid note id"), "{err}");
        assert!(!err.contains(temp.path.to_string_lossy().as_ref()));
        assert!(!temp.path.join("secret.md").exists());
    }

    #[test]
    fn create_write_search_and_delete_a_note() {
        let temp = TempVault::new();
        let vault = &temp.vault;
        create_note(
            vault,
            &json!({ "id": "journal/day", "contents": "The red lantern." }),
        )
        .unwrap();
        write_note(
            vault,
            &json!({ "id": "journal/day", "contents": "The blue lantern." }),
        )
        .unwrap();

        let found = search_notes(vault, &json!({ "query": "Blue Lantern" })).unwrap();
        let value = found.value;
        assert_eq!(value["hits"][0]["id"], "journal/day");
        assert!(
            value["hits"][0]["excerpt"]
                .as_str()
                .unwrap()
                .contains("blue")
        );

        let listed = list_notes(vault, &json!({})).unwrap().value;
        let listed_text = listed.to_string();
        assert!(listed_text.contains("journal/day"), "{listed_text}");
        assert!(!listed_text.contains(temp.path.to_string_lossy().as_ref()));

        delete_note(vault, &json!({ "id": "journal/day" })).unwrap();
        let missing = read_note(vault, &json!({ "id": "journal/day" })).unwrap_err();
        assert!(missing.contains("note not found"), "{missing}");
        assert!(!missing.contains(temp.path.to_string_lossy().as_ref()));
    }

    #[test]
    fn open_note_selects_an_existing_note() {
        let temp = TempVault::new();
        let vault = &temp.vault;
        create_note(
            vault,
            &json!({ "id": "journal/day", "contents": "The blue lantern." }),
        )
        .unwrap();
        let opened = open_note(vault, &json!({ "id": "journal/day" })).unwrap();
        let text = opened.value.to_string();
        assert_eq!(opened.value["ok"], true);
        assert_eq!(opened.value["id"], "journal/day");
        assert_eq!(
            opened.changes,
            vec![VaultChange::Opened(NoteId::parse("journal/day").unwrap())]
        );
        assert!(!text.contains(temp.path.to_string_lossy().as_ref()));

        let missing = open_note(vault, &json!({ "id": "missing" })).unwrap_err();
        assert!(!missing.contains(temp.path.to_string_lossy().as_ref()));
    }
}
