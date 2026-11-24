//! Import one HiDock card into speech notes.
//!
//! This crate does not open a vault and does not write a file.
//! The caller checks for a known note, then writes the markdown.
//! Audio stays in memory for the transcription and is then dropped.
//! The card file is not deleted.

mod note;

use std::path::Path;
use std::thread;

use highdock_rs::{Device, Speech, decode_recording, separate_speakers};

/// `decode_recording` refuses a larger buffer.
const DECODE_INPUT_LIMIT: u64 = 256 * 1024 * 1024;

pub use note::{
    body_needs_words, identity_in, identity_line, is_quick_file, is_quick_id, note_id,
    quick_pieces, quick_speech, should_import, speech_markdown,
};

/// Writes one speech note. The import awaits this before it reads the next file.
pub trait NoteStore {
    /// `Ok(true)` when the note was saved.
    fn store(
        &mut self,
        note: SpeechNote,
    ) -> impl std::future::Future<Output = Result<bool, String>>;
}

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
/// `store` writes one note as soon as its words are ready. `Ok(true)` means
/// the note was saved. `model_dir` fills words and keeps speaker ids on
/// `session`. An empty `model_dir` writes times and a per-file speaker id.
/// No connected recorder returns an empty report.
pub async fn pull<F, P, S>(
    session: &mut Session,
    model_dir: Option<&Path>,
    mut known: F,
    mut progress: P,
    mut store: S,
) -> Report
where
    F: FnMut(&runebook::NoteId, &str) -> bool,
    P: FnMut(&str),
    S: NoteStore,
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
    let mut cursor = 0usize;
    let width = model_dir.map(|_| highdock_rs::transcription_width(recordings.len()));
    if let Some(dir) = model_dir {
        if let Err(err) = session.engine(dir) {
            return Report::failed(err);
        }
    }
    let mut batch = take_files(
        &mut device,
        &recordings,
        &serial,
        &mut cursor,
        width.unwrap_or(1),
        &mut known,
        &mut progress,
    )
    .await;
    while !batch.files.is_empty() {
        let files = std::mem::take(&mut batch.files);
        let stop = batch.stop;
        if batch.message.is_some() {
            message = batch.message.take();
        }
        let names: Vec<String> = files.iter().map(|file| file.name.clone()).collect();
        let speeches: Vec<Speech> = files.into_iter().map(|file| file.speech).collect();
        let transcribed = if model_dir.is_some() {
            let engine = session.engine.take();
            let worker = thread::spawn(move || {
                let Some(mut engine) = engine else {
                    return (
                        None,
                        Err(highdock_rs::Error::Transcript(
                            "the speech model did not stay loaded".into(),
                        )),
                    );
                };
                let lines = engine.transcribe_batch(&speeches);
                (Some(engine), lines)
            });
            if !stop {
                batch = take_files(
                    &mut device,
                    &recordings,
                    &serial,
                    &mut cursor,
                    width.unwrap_or(1),
                    &mut known,
                    &mut progress,
                )
                .await;
            }
            match worker.join() {
                Ok((engine, lines)) => {
                    session.engine = engine;
                    if session.engine.is_none() {
                        session.model = None;
                    }
                    lines
                }
                Err(_) => {
                    session.engine = None;
                    session.model = None;
                    message = Some("the transcription stopped".to_string());
                    break;
                }
            }
        } else {
            Ok(speeches
                .iter()
                .map(|speech| separate_speakers(speech))
                .collect())
        };
        let transcribed = match transcribed {
            Ok(lines) => lines,
            Err(err) => {
                message = Some(err.to_string());
                break;
            }
        };
        for (name, lines) in names.into_iter().zip(transcribed) {
            let lines = match lines {
                Ok(lines) => lines,
                Err(err) => {
                    message = Some(err.to_string());
                    continue;
                }
            };
            match note::speech_note(&serial, &name, &lines) {
                Ok(note) => match store.store(note.clone()).await {
                    Ok(true) => notes.push(note),
                    Ok(false) => {}
                    Err(err) => message = Some(err),
                },
                Err(err) => message = Some(err.to_string()),
            }
        }
        if stop {
            break;
        }
        if model_dir.is_none() {
            batch = take_files(
                &mut device,
                &recordings,
                &serial,
                &mut cursor,
                1,
                &mut known,
                &mut progress,
            )
            .await;
        }
    }
    if message.is_none() {
        message = batch.message;
    }
    Report {
        notes,
        permission: message.as_deref().is_some_and(is_permission),
        message,
        connected: true,
    }
}

fn is_permission(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("permission denied") || message.contains("access denied")
}

struct Decoded {
    name: String,
    speech: Speech,
}

struct Taken {
    files: Vec<Decoded>,
    message: Option<String>,
    stop: bool,
}

async fn take_files<T, F, P>(
    device: &mut Device<T>,
    recordings: &[highdock_rs::Recording],
    serial: &str,
    cursor: &mut usize,
    limit: usize,
    known: &mut F,
    progress: &mut P,
) -> Taken
where
    T: highdock_rs::Transport,
    F: FnMut(&runebook::NoteId, &str) -> bool,
    P: FnMut(&str),
{
    let mut taken = Taken {
        files: Vec::new(),
        message: None,
        stop: false,
    };
    while taken.files.len() < limit && *cursor < recordings.len() {
        let recording = &recordings[*cursor];
        *cursor += 1;
        let id = match note_id(serial, &recording.name) {
            Ok(id) => id,
            Err(err) => {
                taken.message = Some(err.to_string());
                continue;
            }
        };
        let line = identity_line(serial, &recording.name);
        let seen = recording.bytes > 0 && known(&id, &line);
        if !note::should_import(recording.bytes, seen, false) {
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
            taken.message = Some(err.to_string());
            if too_large {
                continue;
            }
            taken.stop = true;
            break;
        }
        match decode_recording(&bytes) {
            Ok(speech) => taken.files.push(Decoded {
                name: recording.name.clone(),
                speech,
            }),
            Err(err) => taken.message = Some(err.to_string()),
        }
    }
    taken
}
