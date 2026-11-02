//! Speech and frame review after a Sight recording stops.
//!
//! The microphone audio goes to Groq. The frames go to the Archivist model.
//! This module does not write a wav, and it does not keep the frames.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use gpui_kit::Context;
use runebook::NoteId;
use superglue::openai::{ChatMessage, ContentPart, ImageUrl, MessageContent};
use superglue::providers::ProviderId;
use superglue::{CallOptions, Client};

use super::{Capture, EditorPane, Root, SightVoice};
use crate::archivist::ModelAccess;
use crate::capture::{
    self, ScreenCapture, SpeechRequest, VoiceClip, VoiceOutcome, end_voice, transcribe_clip,
};
use crate::locale;
use crate::playback;

const REPLY_LIMIT: usize = 4_000;
const SPEECH_LIMIT: usize = 8_000;

const REVIEW_PROMPT: &str = "\
You describe a screen recording for a note.
The user message is either a set of frames or a timeline.
Write Markdown.
Do not wrap the reply in a code fence.
Do not invent names or events.
";

/// What the microphone produced when the screen capture stopped.
enum HeldAudio {
    Clip(VoiceClip),
    Empty,
    Failed(String),
    Skipped(SkippedVoice),
}

enum SkippedVoice {
    NoKey,
    Failed,
}

enum SpeechEnd {
    Text { text: String, truncated: bool },
    Empty,
    Failed(String),
}

enum Next {
    Stop,
    Transcribe,
    Frames,
}

enum ReviewEnd {
    Done(FrameReview),
    NoKey,
    NoFrames,
}

struct FrameReview {
    timeline: String,
    summary: String,
    error: Option<String>,
}

struct Reviewed {
    end: ReviewEnd,
    shortened: bool,
}

/// One stopped Sight recording, ready for speech and frame review.
pub(super) struct SightJob {
    pub(super) video: ScreenCapture,
    pub(super) voice: SightVoice,
    pub(super) path: PathBuf,
    pub(super) file_name: String,
    pub(super) note_id: Option<NoteId>,
    pub(super) speech: SpeechRequest,
    pub(super) groq: Option<String>,
    pub(super) access: Option<ModelAccess>,
}

/// Notice shown while the screen capture is still running.
pub(super) fn voice_notice(voice: &SightVoice) -> Option<String> {
    match voice {
        SightVoice::Live(_) => None,
        SightVoice::NoKey => Some(locale::text("notice.sight_no_groq").to_string()),
        SightVoice::Failed => Some(locale::text("notice.sight_no_mic").to_string()),
    }
}

/// Stop the video and the microphone, then write the note.
pub(super) fn follow(job: SightJob, cx: &mut Context<Root>) {
    cx.spawn(async move |this, cx| {
        let SightJob {
            video,
            voice,
            path,
            file_name,
            note_id,
            speech,
            groq,
            access,
        } = job;
        let video_job = cx.background_executor().spawn(async move { video.stop() });
        let voice_job = cx
            .background_executor()
            .spawn(async move { take_audio(voice) });
        let video_end = video_job.await;
        let audio = voice_job.await;

        let Some(note_id) = note_id else {
            let _ = this.update(cx, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    idle(pane, &[locale::text("notice.recording_gone").to_string()]);
                }
                cx.notify();
            });
            return;
        };

        let mut notices = Vec::new();
        if let Err(err) = &video_end {
            notices.push(capture::notice(err));
        }
        let has_clip = matches!(audio, HeldAudio::Clip(_));
        if !has_clip {
            notices.extend(audio_notices(&audio));
        }
        let video_ok = video_end.is_ok();
        let id = note_id.clone();
        let stepped = this
            .update_in(cx, |root, window, cx| {
                let mut dirty = false;
                let stepped = 'step: {
                    let Some(pane) = root.pane.as_mut() else {
                        break 'step (Next::Stop, notices);
                    };
                    if video_ok {
                        let link = capture::markdown_link("screen recording", &file_name);
                        match pane.deliver_block(&id, &link, window, cx) {
                            Ok(wrote) => dirty = wrote,
                            Err(err) => {
                                notices.push(err);
                                idle(pane, &notices);
                                cx.notify();
                                break 'step (Next::Stop, notices);
                            }
                        }
                    }
                    if !has_clip && !video_ok {
                        idle(pane, &notices);
                        cx.notify();
                        break 'step (Next::Stop, notices);
                    }
                    pane.capture = if has_clip {
                        Capture::Transcribing
                    } else {
                        Capture::Reviewing
                    };
                    apply_notices(pane, &notices);
                    cx.notify();
                    let next = if has_clip {
                        Next::Transcribe
                    } else {
                        Next::Frames
                    };
                    (next, notices)
                };
                if dirty {
                    root.schedule_autosave(cx);
                }
                stepped
            })
            .ok()
            .unwrap_or((Next::Stop, Vec::new()));
        let (next, mut notices) = stepped;
        if matches!(next, Next::Stop) {
            return;
        }

        let speech_text = if matches!(next, Next::Transcribe) {
            let clip = match audio {
                HeldAudio::Clip(clip) => clip,
                _ => return,
            };
            let key = groq.unwrap_or_default();
            let outcome = cx
                .background_executor()
                .spawn(async move { transcribe_clip(clip, key, speech) })
                .await;
            let speech_end = speech_from(outcome);
            let speech_text = match &speech_end {
                SpeechEnd::Text { text, .. } => text.clone(),
                _ => String::new(),
            };
            let block = speech_section(&speech_end);
            notices.extend(speech_notices(&speech_end));
            let id = note_id.clone();
            let continued = this
                .update_in(cx, |root, window, cx| {
                    let mut dirty = false;
                    let stepped = 'step: {
                        let Some(pane) = root.pane.as_mut() else {
                            break 'step (false, notices);
                        };
                        if let Some(block) = block {
                            match pane.deliver_block(&id, &block, window, cx) {
                                Ok(wrote) => dirty = wrote,
                                Err(err) => {
                                    notices.push(err);
                                    idle(pane, &notices);
                                    cx.notify();
                                    break 'step (false, notices);
                                }
                            }
                        }
                        if video_ok {
                            pane.capture = Capture::Reviewing;
                            apply_notices(pane, &notices);
                            cx.notify();
                            (true, notices)
                        } else {
                            idle(pane, &notices);
                            cx.notify();
                            (false, notices)
                        }
                    };
                    if dirty {
                        root.schedule_autosave(cx);
                    }
                    stepped
                })
                .ok()
                .unwrap_or((false, Vec::new()));
            let (continued, next_notices) = continued;
            notices = next_notices;
            if !continued {
                return;
            }
            speech_text
        } else {
            drop(audio);
            drop(speech);
            drop(groq);
            String::new()
        };

        let reviewed = match access {
            None => {
                drop(path);
                drop(speech_text);
                Reviewed {
                    end: ReviewEnd::NoKey,
                    shortened: false,
                }
            }
            Some(access) => {
                let sampled = cx
                    .background_executor()
                    .spawn(async move { playback::sample_each_second(&path) })
                    .await;
                let shortened = sampled.shortened;
                let end = if sampled.frames.is_empty() {
                    drop(speech_text);
                    ReviewEnd::NoFrames
                } else {
                    let frames = sampled.frames;
                    match runtime()
                        .spawn(async move { describe(access, frames, speech_text).await })
                        .await
                    {
                        Ok(review) => ReviewEnd::Done(review),
                        Err(_) => ReviewEnd::Done(FrameReview {
                            timeline: String::new(),
                            summary: String::new(),
                            error: Some(locale::text("notice.review_failed").to_string()),
                        }),
                    }
                };
                Reviewed { end, shortened }
            }
        };

        let id = note_id;
        let _ = this.update_in(cx, |root, window, cx| {
            let mut dirty = false;
            {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                if reviewed.shortened {
                    notices.push(short_notice());
                }
                push_review_notices(pane, &mut notices, &reviewed.end);
                if let ReviewEnd::Done(review) = &reviewed.end {
                    let block = review_block(&review.timeline, &review.summary);
                    if !block.is_empty() {
                        match pane.deliver_block(&id, &block, window, cx) {
                            Ok(wrote) => dirty = wrote,
                            Err(err) => {
                                notices.push(err);
                                idle(pane, &notices);
                                cx.notify();
                                return;
                            }
                        }
                    }
                }
                idle(pane, &notices);
                cx.notify();
            }
            if dirty {
                root.schedule_autosave(cx);
            }
        });
    })
    .detach();
}

fn take_audio(voice: SightVoice) -> HeldAudio {
    match voice {
        SightVoice::NoKey => HeldAudio::Skipped(SkippedVoice::NoKey),
        SightVoice::Failed => HeldAudio::Skipped(SkippedVoice::Failed),
        SightVoice::Live(capture) => match end_voice(capture) {
            Ok(clip) => HeldAudio::Clip(clip),
            Err(capture::CaptureError::MicEmpty) => HeldAudio::Empty,
            Err(err) => HeldAudio::Failed(capture::notice(&err)),
        },
    }
}

fn speech_from(outcome: Result<VoiceOutcome, capture::CaptureError>) -> SpeechEnd {
    match outcome {
        Ok(outcome) if outcome.text.trim().is_empty() => SpeechEnd::Empty,
        Ok(outcome) => SpeechEnd::Text {
            text: outcome.text,
            truncated: outcome.truncated,
        },
        Err(err) => SpeechEnd::Failed(capture::notice(&err)),
    }
}

fn audio_notices(audio: &HeldAudio) -> Vec<String> {
    match audio {
        HeldAudio::Clip(_) => Vec::new(),
        HeldAudio::Empty => vec![locale::text("notice.no_speech").to_string()],
        HeldAudio::Failed(text) => vec![text.clone()],
        HeldAudio::Skipped(SkippedVoice::NoKey) => {
            vec![locale::text("notice.sight_no_groq").to_string()]
        }
        HeldAudio::Skipped(SkippedVoice::Failed) => {
            vec![locale::text("notice.sight_no_mic").to_string()]
        }
    }
}

fn speech_notices(speech: &SpeechEnd) -> Vec<String> {
    match speech {
        SpeechEnd::Text {
            truncated: true, ..
        } => vec![locale::text("notice.recording_short").to_string()],
        SpeechEnd::Text { .. } => Vec::new(),
        SpeechEnd::Empty => vec![locale::text("notice.no_speech").to_string()],
        SpeechEnd::Failed(text) => vec![text.clone()],
    }
}

fn speech_section(speech: &SpeechEnd) -> Option<String> {
    match speech {
        SpeechEnd::Text { text, .. } => section("sight.speech", text),
        _ => None,
    }
}

fn review_block(timeline: &str, summary: &str) -> String {
    let mut parts = Vec::new();
    if let Some(block) = section("sight.timeline", timeline) {
        parts.push(block);
    }
    if let Some(block) = section("sight.summary", summary) {
        parts.push(block);
    }
    parts.join("\n\n")
}

fn section(heading: &'static str, body: &str) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    Some(format!("## {}\n\n{body}", locale::text(heading)))
}

fn short_notice() -> String {
    let minutes = (playback::REVIEW_SECONDS / 60).to_string();
    locale::format("notice.review_short", &[("minutes", &minutes)])
}

fn push_review_notices(pane: &EditorPane, notices: &mut Vec<String>, end: &ReviewEnd) {
    match end {
        ReviewEnd::NoKey => notices.push(pane.archivist_key_message().to_string()),
        ReviewEnd::NoFrames => notices.push(locale::text("notice.review_failed").to_string()),
        ReviewEnd::Done(review) => {
            if let Some(err) = &review.error {
                notices.push(err.clone());
            } else if review.timeline.trim().is_empty() && review.summary.trim().is_empty() {
                notices.push(locale::text("notice.review_failed").to_string());
            }
        }
    }
}

fn idle(pane: &mut EditorPane, notices: &[String]) {
    pane.capture = Capture::Idle;
    pane.recording_note = None;
    apply_notices(pane, notices);
}

fn apply_notices(pane: &mut EditorPane, notices: &[String]) {
    let mut unique = Vec::new();
    for notice in notices {
        let notice = pane.redact_notice(notice.trim());
        if notice.is_empty() || unique.iter().any(|item: &String| item == &notice) {
            continue;
        }
        unique.push(notice);
    }
    pane.notice = if unique.is_empty() {
        None
    } else {
        Some(unique.join(locale::text("status.join")))
    };
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("runebook-sight")
            .enable_all()
            .build()
            .expect("sight runtime")
    })
}

async fn describe(access: ModelAccess, frames: Vec<Vec<u8>>, transcript: String) -> FrameReview {
    let client = match review_client(&access) {
        Ok(client) => client,
        Err(err) => {
            return FrameReview {
                timeline: String::new(),
                summary: String::new(),
                error: Some(err),
            };
        }
    };
    let mut lines = Vec::new();
    let mut error = None;
    for range in batch_ranges(frames.len()) {
        let start = range.start as u32;
        let message = frames_message(&frames[range], start);
        match ask(&client, message, &access).await {
            Ok(text) if !text.is_empty() => lines.push(text),
            Ok(_) => {}
            Err(err) => {
                error = Some(err);
                break;
            }
        }
    }
    let timeline = lines.join("\n");
    let summary = if timeline.is_empty() {
        String::new()
    } else {
        let message = ChatMessage::text("user", summary_prompt(&transcript, &timeline));
        match ask(&client, message, &access).await {
            Ok(text) => text,
            Err(err) => {
                if error.is_none() {
                    error = Some(err);
                }
                String::new()
            }
        }
    };
    FrameReview {
        timeline,
        summary,
        error,
    }
}

fn review_client(access: &ModelAccess) -> Result<Client, String> {
    let plan = runebook_agent::key_plan(
        &access.model,
        &access.groq_key,
        &access.openai_key,
        &access.openai_base_url,
    )
    .map_err(|_| crate::archivist::missing_model_key(&access.model).to_string())?;
    let mut builder = Client::builder()
        .api_key(plan.primary.clone())
        .base_url(plan.base.clone())
        .model(access.model.clone())
        .system_prompt(REVIEW_PROMPT)
        .max_tool_rounds(1)
        .reasoning_effort("low")
        .timeout(Duration::from_secs(120));
    if let Some(groq) = plan.groq.as_deref() {
        builder = builder.api_key_for(ProviderId::Groq, groq);
    }
    if let Some(openai) = plan.openai.as_deref() {
        builder = builder
            .api_key_for(plan.openai_provider, openai)
            .base_url_for(plan.openai_provider, plan.base.clone());
    }
    builder
        .build()
        .map_err(|err| redact_model(err.to_string(), access))
}

async fn ask(
    client: &Client,
    message: ChatMessage,
    access: &ModelAccess,
) -> Result<String, String> {
    let outcome = client
        .complete_messages(vec![message], CallOptions::default())
        .await
        .map_err(|err| redact_model(err.to_string(), access))?;
    Ok(bounded_reply(&outcome.content.unwrap_or_default()))
}

fn frames_message(frames: &[Vec<u8>], start: u32) -> ChatMessage {
    let first = second_label(start);
    let count = frames.len();
    let prompt = format!(
        "These frames are one second apart. The first frame is at {first}. There are {count} frames.\n\
         Write one Markdown list item for each frame.\n\
         Start each item with the timestamp, like `- {first}`.\n\
         Use one sentence for each item.\n\
         Describe only what is visible."
    );
    let mut parts = vec![ContentPart::Text { text: prompt }];
    for (offset, jpeg) in frames.iter().enumerate() {
        let label = second_label(start + offset as u32);
        parts.push(ContentPart::Text {
            text: format!("Frame at {label}."),
        });
        parts.push(ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: superglue::images::image_data_url(jpeg, "image/jpeg"),
                detail: None,
            },
        });
    }
    ChatMessage {
        role: "user".to_string(),
        content: Some(MessageContent::Parts(parts)),
        ..ChatMessage::default()
    }
}

fn summary_prompt(transcript: &str, timeline: &str) -> String {
    let speech = if transcript.trim().is_empty() {
        "No speech.".to_string()
    } else {
        clip_chars(transcript.trim(), SPEECH_LIMIT)
    };
    let timeline = clip_chars(timeline.trim(), REPLY_LIMIT * 8);
    format!(
        "Write one short paragraph about this screen recording.\n\
         Use the speech and the timeline.\n\
         Do not add events that are not in them.\n\n\
         Speech:\n{speech}\n\n\
         Timeline:\n{timeline}"
    )
}

fn batch_ranges(count: usize) -> Vec<std::ops::Range<usize>> {
    let size = runebook_agent::MAX_FILES.max(1);
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < count {
        let end = (start + size).min(count);
        ranges.push(start..end);
        start = end;
    }
    ranges
}

fn second_label(second: u32) -> String {
    if second >= 3_600 {
        let hours = second / 3_600;
        let minutes = (second % 3_600) / 60;
        let secs = second % 60;
        return format!("{hours}:{minutes:02}:{secs:02}");
    }
    format!("{}:{:02}", second / 60, second % 60)
}

fn bounded_reply(text: &str) -> String {
    clip_chars(text.trim(), REPLY_LIMIT)
}

fn clip_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let end = text
        .char_indices()
        .nth(limit)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    text[..end].trim_end().to_string()
}

fn redact_model(text: String, access: &ModelAccess) -> String {
    let mut secrets = crate::models::env_secrets();
    for key in [&access.groq_key, &access.openai_key] {
        let key = key.trim();
        if !key.is_empty() {
            secrets.push(key.to_string());
        }
    }
    let text = crate::models::redact_secrets(&text, secrets.iter().map(String::as_str));
    clip_chars(&text, 400)
}

#[cfg(test)]
mod tests {
    use super::{
        HeldAudio, SpeechEnd, audio_notices, batch_ranges, review_block, second_label, section,
        speech_notices, speech_section,
    };

    #[test]
    fn frames_split_into_batches_of_eight() {
        assert_eq!(runebook_agent::MAX_FILES, 8);
        assert!(batch_ranges(0).is_empty());
        assert_eq!(batch_ranges(8), vec![0..8]);
        assert_eq!(batch_ranges(9), vec![0..8, 8..9]);
        let all = batch_ranges(180);
        assert_eq!(all.len(), 23);
        assert_eq!(all[0], 0..8);
        assert_eq!(all[22], 176..180);
    }

    #[test]
    fn second_labels_use_a_clock() {
        assert_eq!(second_label(0), "0:00");
        assert_eq!(second_label(61), "1:01");
        assert_eq!(second_label(3_600), "1:00:00");
    }

    #[test]
    fn speech_and_review_sections_keep_their_headings() {
        let speech = speech_section(&SpeechEnd::Text {
            text: "  hello  ".to_string(),
            truncated: false,
        })
        .expect("speech");
        assert_eq!(speech, "## Speech\n\nhello");
        assert!(speech_section(&SpeechEnd::Empty).is_none());
        let review = review_block("- 0:00 The editor is open.", "A short edit.");
        assert!(review.starts_with("## Timeline\n\n- 0:00 The editor is open."));
        assert!(review.contains("## Summary\n\nA short edit."));
        assert!(review_block("", "").is_empty());
        assert_eq!(
            review_block("- 0:00 The editor is open.", ""),
            "## Timeline\n\n- 0:00 The editor is open."
        );
        assert!(section("sight.speech", "   ").is_none());
    }

    #[test]
    fn an_empty_microphone_uses_the_no_speech_notice() {
        assert_eq!(
            audio_notices(&HeldAudio::Empty),
            vec!["The recording had no speech".to_string()]
        );
        assert_eq!(
            speech_notices(&SpeechEnd::Text {
                text: "hello".to_string(),
                truncated: true,
            }),
            vec!["The recording was shortened to fit the transcription limit".to_string()]
        );
    }
}
