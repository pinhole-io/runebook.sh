//! Failures from microphone capture and screen recording.
//!
//! A variant names the failure. It does not carry a window sentence or an API key.
//! The window maps the variant to text.

/// One capture failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// The microphone thread did not start.
    MicStart,
    /// The microphone thread panicked.
    MicThread,
    /// The recording has no samples.
    MicEmpty,
    /// Groq has no API key.
    GroqKey,
    /// The host has no default input.
    MicNone,
    /// Listing input devices failed. The string is the host error.
    MicList(String),
    /// The named input device is missing.
    MicMissing(String),
    /// Opening the input stream failed. The string is the host error.
    MicOpen(String),
    /// The input sample format is not supported.
    MicFormat,
    /// Starting the input stream failed. The string is the host error.
    MicStartDetail(String),
    /// The sample rate is zero.
    MicRate,
    /// WAV encoding failed.
    MicEncode,
    /// The Groq client could not be built.
    GroqClient,
    /// The Groq request did not complete.
    GroqReach,
    /// Groq rejected the upload. The string is the HTTP status.
    GroqFailed(String),
    /// The Groq body was not text.
    GroqUnreadable,
    /// The Groq body had no transcript.
    GroqUnexpected,
    /// macOS records the full display only.
    MacosDisplayOnly,
    /// A window region needs a display session that can record it.
    WindowOnDisplay,
    /// The recording file is missing or empty.
    NoVideo,
    /// X11 has no `DISPLAY` value.
    X11Display,
    /// The window list could not be read.
    WindowList,
    /// `ffmpeg` is not installed.
    FfmpegMissing,
    /// `ffmpeg` did not start. The string is the process error.
    FfmpegStart(String),
    /// The Wayland recording thread did not start.
    WaylandStart,
    /// The Wayland display is missing.
    WaylandMissing,
    /// The compositor did not answer.
    WaylandAnswer,
    /// The compositor did not share a frame.
    WaylandShare,
    /// The frame pixel format is not supported.
    PixelFormat,
    /// A frame buffer could not be created.
    Frame,
    /// The frame copy failed.
    Copy,
    /// `ffmpeg` was not ready for a frame.
    FfmpegFrames,
    /// `ffmpeg` stopped reading frames.
    FfmpegStopped,
}
