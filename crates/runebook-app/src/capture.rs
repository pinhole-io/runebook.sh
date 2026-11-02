//! Map capture failures to window text.

pub(crate) use runebook_capture::{
    CaptureError, ScreenCapture, ScreenTarget, SpeechRequest, VoiceCapture, VoiceClip,
    VoiceOutcome, append_block, end_voice, list_input_devices, list_record_sources, markdown_link,
    transcribe_clip, utc_stamp_now, voice_to_text,
};

use crate::locale;

/// Sentence shown when the window list cannot be read.
pub(crate) fn window_list_error() -> &'static str {
    locale::text("capture.window_list")
}

/// Window sentence for one capture failure.
pub(crate) fn notice(err: &CaptureError) -> String {
    match err {
        CaptureError::MicStart => locale::text("mic.start").to_string(),
        CaptureError::MicThread => locale::text("mic.thread").to_string(),
        CaptureError::MicEmpty => locale::text("mic.empty").to_string(),
        CaptureError::GroqKey => locale::text("capture.groq_key").to_string(),
        CaptureError::MicNone => locale::text("mic.none").to_string(),
        CaptureError::MicList(err) => locale::format("mic.list", &[("err", err)]),
        CaptureError::MicMissing(name) => locale::format("mic.missing", &[("name", name)]),
        CaptureError::MicOpen(err) => locale::format("mic.open", &[("err", err)]),
        CaptureError::MicFormat => locale::text("mic.format").to_string(),
        CaptureError::MicStartDetail(err) => locale::format("mic.start_detail", &[("err", err)]),
        CaptureError::MicRate => locale::text("mic.rate").to_string(),
        CaptureError::MicEncode => locale::text("mic.encode").to_string(),
        CaptureError::GroqClient => locale::text("mic.groq_client").to_string(),
        CaptureError::GroqReach => locale::text("mic.groq_reach").to_string(),
        CaptureError::GroqFailed(status) => {
            locale::format("mic.groq_failed", &[("status", status)])
        }
        CaptureError::GroqUnreadable => locale::text("mic.groq_unreadable").to_string(),
        CaptureError::GroqUnexpected => locale::text("mic.groq_unexpected").to_string(),
        CaptureError::MacosDisplayOnly => locale::text("screen.macos").to_string(),
        CaptureError::WindowOnDisplay => locale::text("screen.window_display").to_string(),
        CaptureError::NoVideo => locale::text("screen.no_video").to_string(),
        CaptureError::X11Display => locale::text("screen.x11").to_string(),
        CaptureError::WindowList => locale::text("capture.window_list").to_string(),
        CaptureError::FfmpegMissing => locale::text("screen.ffmpeg_missing").to_string(),
        CaptureError::FfmpegStart(err) => locale::format("screen.ffmpeg_start", &[("err", err)]),
        CaptureError::WaylandStart => locale::text("screen.wayland_start").to_string(),
        CaptureError::WaylandMissing => locale::text("screen.wayland_missing").to_string(),
        CaptureError::WaylandAnswer => locale::text("screen.wayland_answer").to_string(),
        CaptureError::WaylandShare => locale::text("screen.wayland_share").to_string(),
        CaptureError::PixelFormat => locale::text("screen.pixel").to_string(),
        CaptureError::Frame => locale::text("screen.frame").to_string(),
        CaptureError::Copy => locale::text("screen.copy").to_string(),
        CaptureError::FfmpegFrames => locale::text("screen.ffmpeg_frames").to_string(),
        CaptureError::FfmpegStopped => locale::text("screen.ffmpeg_stopped").to_string(),
    }
}
