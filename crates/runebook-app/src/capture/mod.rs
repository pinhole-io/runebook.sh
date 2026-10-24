//! Microphone transcription and screen recording for the open note.

mod screen;
mod voice;

pub(crate) use screen::{ScreenCapture, ScreenTarget, WINDOW_LIST_ERROR, list_record_sources};
pub(crate) use voice::{VoiceCapture, VoiceOutcome, list_input_devices, voice_to_text};

use std::time::{SystemTime, UNIX_EPOCH};

/// Append `block` after `body`, with a blank line between them.
pub(crate) fn append_block(body: &str, block: &str) -> String {
    if body.is_empty() {
        return format!("{block}\n");
    }
    if body.ends_with("\n\n") {
        format!("{body}{block}\n")
    } else if body.ends_with('\n') {
        format!("{body}\n{block}\n")
    } else {
        format!("{body}\n\n{block}\n")
    }
}

/// Markdown link. Angle brackets keep spaces and parentheses in the path.
pub(crate) fn markdown_link(label: &str, file_name: &str) -> String {
    if file_name.contains([' ', '(', ')']) {
        format!("[{label}](<{file_name}>)")
    } else {
        format!("[{label}]({file_name})")
    }
}

/// UTC time as `YYYYMMDDTHHMMSSZ`.
pub(crate) fn utc_stamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let tod = unix_secs % 86_400;
    let hour = tod / 3_600;
    let minute = (tod % 3_600) / 60;
    let second = tod % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

pub(crate) fn utc_stamp_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    utc_stamp(secs)
}

/// Howard Hinnant's civil-from-days. `days` is days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

/// `true` when `GROQ_API_KEY` is set to a non-empty value.
///
/// The key itself is not returned.
pub(crate) fn groq_key_is_set() -> bool {
    std::env::var("GROQ_API_KEY").is_ok_and(|key| !key.trim().is_empty())
}

pub(crate) const MISSING_GROQ_KEY: &str = "Set GROQ_API_KEY to transcribe voice notes";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_block_separates_existing_text() {
        assert_eq!(append_block("", "hello"), "hello\n");
        assert_eq!(append_block("note", "hello"), "note\n\nhello\n");
        assert_eq!(append_block("note\n", "hello"), "note\n\nhello\n");
        assert_eq!(append_block("note\n\n", "hello"), "note\n\nhello\n");
    }

    #[test]
    fn markdown_link_quotes_spaces() {
        assert_eq!(
            markdown_link("screen recording", "day-1.mp4"),
            "[screen recording](day-1.mp4)"
        );
        assert_eq!(
            markdown_link("screen recording", "my day.mp4"),
            "[screen recording](<my day.mp4>)"
        );
    }

    #[test]
    fn utc_stamp_formats_known_instants() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(86_400), "19700102T000000Z");
        assert_eq!(utc_stamp(1_700_000_000), "20231114T221320Z");
    }
}
