//! Speech note text for one HiDock file.
//!
//! The identity line is the duplicate key. The note id is a safe path.

use highdock_rs::SpokenLine;
use runebook::{NoteId, NoteKind, encode_note};

use crate::SpeechNote;

/// Skip a recording that is empty or already stored.
pub fn should_import(bytes: u32, id_exists: bool, identity_found: bool) -> bool {
    bytes > 0 && !id_exists && !identity_found
}

/// True when the body has speaker times and no spoken words.
pub fn body_needs_words(body: &str) -> bool {
    let mut saw_turn = false;
    for line in body.lines() {
        let line = line.trim().trim_start_matches("- ").trim();
        if line.is_empty() || line.starts_with("hidock-id:") {
            continue;
        }
        saw_turn = true;
        if let Some((_, rest)) = line.split_once(' ')
            && let Some((_, words)) = rest.split_once(' ')
            && !words.trim().is_empty()
        {
            return false;
        }
    }
    saw_turn
}

/// `hidock-id:<serial>:<file name>` on one line.
pub fn identity_line(serial: &str, file_name: &str) -> String {
    format!("hidock-id:{}:{}", one_line(serial), one_line(file_name))
}

/// `hidock/<serial>/<stem>`.
///
/// A slash in the serial or the file name becomes part of one path segment.
pub fn note_id(serial: &str, file_name: &str) -> runebook::Result<NoteId> {
    NoteId::parse(&format!(
        "hidock/{}/{}",
        safe_segment(serial),
        file_stem(file_name)
    ))
}

/// Speech markdown. The body starts with the identity line.
pub fn speech_markdown(
    serial: &str,
    file_name: &str,
    lines: &[SpokenLine],
) -> runebook::Result<String> {
    let _ = note_id(serial, file_name)?;
    let mut body = identity_line(serial, file_name);
    // A blank line, then a list. A plain newline is a space in the preview.
    body.push_str("\n\n");
    for line in lines {
        let text = one_line(&line.text);
        if text.is_empty() {
            body.push_str(&format!(
                "- {} {:.2}-{:.2}\n",
                line.speaker, line.start_secs, line.end_secs
            ));
        } else {
            body.push_str(&format!(
                "- {} {:.2}-{:.2} {text}\n",
                line.speaker, line.start_secs, line.end_secs
            ));
        }
    }
    Ok(encode_note(NoteKind::Speech, &body))
}

/// Build the note the caller writes.
pub fn speech_note(
    serial: &str,
    file_name: &str,
    lines: &[SpokenLine],
) -> runebook::Result<SpeechNote> {
    Ok(SpeechNote {
        id: note_id(serial, file_name)?,
        markdown: speech_markdown(serial, file_name, lines)?,
    })
}

fn one_line(raw: &str) -> String {
    raw.chars()
        .map(|ch| match ch {
            '\n' | '\r' => ' ',
            _ => ch,
        })
        .collect()
}

fn file_stem(file_name: &str) -> String {
    let flat = safe_segment(file_name);
    let stem = flat
        .rsplit_once('.')
        .map(|(stem, ext)| {
            if ext.eq_ignore_ascii_case("hda")
                || ext.eq_ignore_ascii_case("mp3")
                || ext.eq_ignore_ascii_case("wav")
                || ext.eq_ignore_ascii_case("ogg")
            {
                stem
            } else {
                flat.as_str()
            }
        })
        .unwrap_or(flat.as_str());
    if stem.is_empty() {
        safe_segment(&flat)
    } else {
        safe_segment(stem)
    }
}

fn safe_segment(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch == '/' || ch == '\\' || ch.is_control() {
            out.push('-');
        } else {
            out.push(ch);
        }
    }
    if out.is_empty() || out.starts_with('.') {
        let rest = out.trim_start_matches('.');
        if rest.is_empty() {
            return "device".to_string();
        }
        return format!("x{rest}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use runebook::decode_note;

    fn line(speaker: &str, text: &str) -> SpokenLine {
        SpokenLine {
            speaker: speaker.to_string(),
            start_secs: 0.0,
            end_secs: 1.25,
            text: text.to_string(),
        }
    }

    #[test]
    fn the_identity_line_and_the_speech_header_round_trip() {
        let markdown = speech_markdown("SER", "Rec.hda", &[line("SPEAKER_00", "hello")]).unwrap();
        let (kind, body) = decode_note(&markdown);
        assert_eq!(kind, NoteKind::Speech);
        assert!(body.starts_with("hidock-id:SER:Rec.hda\n"));
        assert!(body.contains("- SPEAKER_00 0.00-1.25 hello\n"));
        assert_eq!(identity_line("SER", "Rec.hda"), "hidock-id:SER:Rec.hda");
    }

    #[test]
    fn a_times_only_body_needs_words() {
        assert!(body_needs_words(
            "hidock-id:SER:Rec.hda\n\n- SPEAKER_00 2.05-2.34\n"
        ));
        assert!(!body_needs_words(
            "hidock-id:SER:Rec.hda\n\n- SPEAKER_00 2.05-2.34 hello\n"
        ));
    }

    #[test]
    fn a_known_identity_is_skipped() {
        assert!(!should_import(10, true, false));
        assert!(!should_import(10, false, true));
        assert!(should_import(10, false, false));
    }

    #[test]
    fn a_zero_byte_name_is_skipped() {
        assert!(!should_import(0, false, false));
    }

    #[test]
    fn a_slash_becomes_one_path_segment() {
        let id = note_id("a/b", "c/d.hda").unwrap();
        assert_eq!(id.as_str(), "hidock/a-b/c-d");
    }
}
