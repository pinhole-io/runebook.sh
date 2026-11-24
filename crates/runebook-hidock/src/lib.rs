//! Import one HiDock card into speech notes.
//!
//! This crate does not open a vault and does not write a file.
//! The caller checks for a known note, then writes the markdown.
//! Audio stays in memory for the transcription and is then dropped.
//! The card file is not deleted.

mod note;

use std::path::Path;

use highdock_rs::{Device, SpokenLine, TranscriptEngine, decode_recording, separate_speakers};

/// `decode_recording` refuses a larger buffer.
const DECODE_INPUT_LIMIT: u64 = 256 * 1024 * 1024;

pub use note::{body_needs_words, identity_line, note_id, should_import, speech_markdown};

/// One speech note ready to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechNote {
    /// `hidock/<serial>/<stem>`.
    pub id: runebook::NoteId,
    /// Speech header, the identity line, then the speaker lines.
    pub markdown: String,
}

/// Notes from one pass, plus a notice the caller may show once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Recordings that were not already in the vault.
    pub notes: Vec<SpeechNote>,
    /// The USB node rejected the open.
    pub permission: bool,
    /// A failure that does not include audio or a voice vector.
    pub message: Option<String>,
    /// A recorder answered the open.
    pub connected: bool,
}

impl Report {
    fn quiet() -> Self {
        Self {
            notes: Vec::new(),
            permission: false,
            message: None,
            connected: false,
        }
    }

    fn failed(err: impl std::fmt::Display) -> Self {
        let message = err.to_string();
        Self {
            notes: Vec::new(),
            permission: is_permission(&message),
            message: Some(message),
            connected: false,
        }
    }
}

/// Parakeet engine kept for one app session.
///
/// An empty model path does not load it. The roster stays in this value.
/// This crate does not write the roster.
pub struct Session {
    model: Option<std::path::PathBuf>,
    engine: Option<TranscriptEngine>,
}

impl Session {
    /// No model loaded.
    pub fn new() -> Self {
        Self {
            model: None,
            engine: None,
        }
    }

    fn engine(&mut self, dir: &Path) -> Result<&mut TranscriptEngine, highdock_rs::Error> {
        if self.model.as_deref() != Some(dir) {
            self.engine = None;
            self.model = None;
        }
        if self.engine.is_none() {
            self.engine = Some(TranscriptEngine::open(dir)?);
            self.model = Some(dir.to_path_buf());
        }
        self.engine.as_mut().ok_or_else(|| {
            highdock_rs::Error::Transcript("the speech model did not stay loaded".into())
        })
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// Download recordings that `known` has not seen.
///
/// `known` receives the note id and the identity line. Return true to skip
/// that file. `progress` receives the file name before that file is read.
/// `store` writes one note as soon as its words are ready. Return `Ok(true)`
/// when the note was saved. `model_dir` fills words and keeps speaker ids on
/// `session`. An empty `model_dir` writes times and a per-file speaker id.
/// No connected recorder returns an empty report.
pub async fn pull<F, P, W>(
    session: &mut Session,
    model_dir: Option<&Path>,
    mut known: F,
    mut progress: P,
    mut store: W,
) -> Report
where
    F: FnMut(&runebook::NoteId, &str) -> bool,
    P: FnMut(&str),
    W: FnMut(&SpeechNote) -> Result<bool, String>,
{
    let mut device = match Device::open_first().await {
        Ok(device) => device,
        Err(highdock_rs::Error::NotFound) => return Report::quiet(),
        Err(err) => return Report::failed(err),
    };
    let serial = match device.device_info().await {
        Ok(info) => info.serial,
        Err(err) => return Report::failed(err),
    };
    let recordings = match device.list_recordings().await {
        Ok(recordings) => recordings,
        Err(err) => return Report::failed(err),
    };
    let mut notes = Vec::new();
    let mut message = None;
    for recording in &recordings {
        let id = match note_id(&serial, &recording.name) {
            Ok(id) => id,
            Err(err) => {
                message = Some(err.to_string());
                continue;
            }
        };
        let line = identity_line(&serial, &recording.name);
        let seen = recording.bytes > 0 && known(&id, &line);
        if !should_import(recording.bytes, seen, false) {
            continue;
        }
        progress(&recording.name);
        let mut bytes = Vec::new();
        if let Err(err) = device
            .download(recording, |chunk| {
                let next = bytes.len() as u64 + chunk.len() as u64;
                if next > DECODE_INPUT_LIMIT {
                    return Err(highdock_rs::Error::RecordingTooLarge {
                        bytes: next,
                        limit: DECODE_INPUT_LIMIT,
                    });
                }
                bytes.extend_from_slice(chunk);
                Ok(())
            })
            .await
        {
            let too_large = matches!(err, highdock_rs::Error::RecordingTooLarge { .. });
            message = Some(err.to_string());
            if too_large {
                continue;
            }
            break;
        }
        let speech = match decode_recording(&bytes) {
            Ok(speech) => speech,
            Err(err) => {
                message = Some(err.to_string());
                continue;
            }
        };
        drop(bytes);
        let lines = match lines_for(session, model_dir, &speech) {
            Ok(lines) => lines,
            Err(err) => {
                message = Some(err.to_string());
                break;
            }
        };
        match note::speech_note(&serial, &recording.name, &lines) {
            Ok(note) => match store(&note) {
                Ok(true) => notes.push(note),
                Ok(false) => {}
                Err(err) => message = Some(err),
            },
            Err(err) => message = Some(err.to_string()),
        }
    }
    Report {
        notes,
        permission: message.as_deref().is_some_and(is_permission),
        message,
        connected: true,
    }
}

fn lines_for(
    session: &mut Session,
    model_dir: Option<&Path>,
    speech: &highdock_rs::Speech,
) -> Result<Vec<SpokenLine>, highdock_rs::Error> {
    match model_dir {
        Some(dir) => session.engine(dir)?.transcribe(speech),
        None => separate_speakers(speech),
    }
}

fn is_permission(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("permission denied") || message.contains("access denied")
}
