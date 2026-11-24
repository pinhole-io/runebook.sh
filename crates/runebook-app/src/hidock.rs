//! One HiDock pass for the open vault.
//!
//! The pass runs on a thread with its own Tokio runtime. It does not keep
//! the audio, and it does not delete the card file.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use runebook::Vault;
use runebook_hidock::{Report, Session};

/// One speech roster, and one import at a time.
pub(crate) struct Gate {
    slot: Mutex<Option<Session>>,
    /// File name of the recording this pass is reading.
    file: Arc<Mutex<String>>,
}

/// Result of one attempt to take the roster.
pub(crate) enum Sync {
    /// Another import holds the recorder.
    Busy,
    /// The pass finished.
    Done(Report),
}

impl Gate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            slot: Mutex::new(Some(Session::new())),
            file: Arc::new(Mutex::new(String::new())),
        })
    }

    /// File name shown while a recording is read. Empty when the pass is idle.
    pub(crate) fn current_file(&self) -> String {
        self.file
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    /// Run one import. A second caller gets [`Sync::Busy`] until this returns.
    pub(crate) fn run(&self, root: PathBuf, model: String) -> Sync {
        let Some(mut session) = self.take() else {
            return Sync::Busy;
        };
        let file = Arc::clone(&self.file);
        let joined = std::thread::spawn(move || {
            let report = import_tick(&root, &model, &mut session, &file);
            (session, report)
        })
        .join();
        self.clear_file();
        match joined {
            Ok((session, report)) => {
                self.put(session);
                Sync::Done(report)
            }
            Err(_) => {
                self.put(Session::new());
                Sync::Done(stopped())
            }
        }
    }

    fn take(&self) -> Option<Session> {
        self.slot
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
    }

    fn put(&self, session: Session) {
        *self.slot.lock().unwrap_or_else(|err| err.into_inner()) = Some(session);
    }

    fn clear_file(&self) {
        *self.file.lock().unwrap_or_else(|err| err.into_inner()) = String::new();
    }
}

/// Import recordings that this vault does not already store.
pub(crate) fn import_tick(
    root: &Path,
    model: &str,
    session: &mut Session,
    file: &Mutex<String>,
) -> Report {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => return failed(err.to_string()),
    };
    runtime.block_on(import(root, model, session, file))
}

async fn import(root: &Path, model: &str, session: &mut Session, file: &Mutex<String>) -> Report {
    let vault = match Vault::open(root) {
        Ok(vault) => vault,
        Err(err) => return failed(err.to_string()),
    };
    let index = match vault.notes() {
        Ok(index) => index,
        Err(err) => return failed(err.to_string()),
    };
    let trimmed = model.trim();
    let model_dir = if trimmed.is_empty() {
        None
    } else {
        Some(Path::new(trimmed))
    };
    let fill_words = model_dir.is_some();
    runebook_hidock::pull(
        session,
        model_dir,
        |id, line| {
            if index.contains(id) {
                if fill_words && stored_needs_words(&vault, id) {
                    return false;
                }
                return true;
            }
            vault
                .search(line)
                .ok()
                .is_some_and(|found| !found.hits.is_empty())
        },
        |name| {
            *file.lock().unwrap_or_else(|err| err.into_inner()) = name.to_string();
        },
        |note| store_note(&vault, note),
    )
    .await
}

fn store_note(vault: &Vault, note: &runebook_hidock::SpeechNote) -> Result<bool, String> {
    match vault.create_note(&note.id, &note.markdown) {
        Ok(()) => Ok(true),
        Err(runebook::Error::NoteExists { .. }) if stored_needs_words(vault, &note.id) => vault
            .write(&note.id, &note.markdown)
            .map(|()| true)
            .map_err(|err| err.to_string()),
        Err(runebook::Error::NoteExists { .. }) => Ok(false),
        Err(err) => Err(err.to_string()),
    }
}

fn stored_needs_words(vault: &Vault, id: &runebook::NoteId) -> bool {
    let Ok(text) = vault.read(id) else {
        return false;
    };
    let (_, body) = runebook::decode_note(&text);
    runebook_hidock::body_needs_words(body)
}

fn failed(message: String) -> Report {
    Report {
        notes: Vec::new(),
        permission: false,
        message: Some(message),
        connected: false,
    }
}

fn stopped() -> Report {
    failed("the HiDock import stopped".to_string())
}
