//! Note kind stored in the file.
//!
//! Book notes are the file body. Speech, Sight, and Deed notes start with a
//! short header. A file with no header, or a header this crate does not know,
//! is a Book note.

/// The four note kinds.
///
/// Book is the default. A file omits the header for Book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteKind {
    /// A markdown note. The file has no kind header.
    Book,
    /// A voice note. The file stores the transcript.
    Speech,
    /// A screen recording note. The video file sits beside the note.
    Sight,
    /// A task list. The body is markdown checkboxes.
    Deed,
}

/// One markdown task line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLine {
    /// `true` when the box is `[x]` or `[X]`.
    pub checked: bool,
    /// The line with the box removed, so a preview can draw its own box.
    pub plain: String,
    /// The same line with the box flipped.
    pub toggled: String,
}

const HEADER_LIMIT: usize = 256;

impl NoteKind {
    /// File key for Speech, Sight, and Deed. Book has no key.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Self::Book => None,
            Self::Speech => Some("speech"),
            Self::Sight => Some("sight"),
            Self::Deed => Some("deed"),
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "speech" => Some(Self::Speech),
            "sight" => Some(Self::Sight),
            "deed" => Some(Self::Deed),
            _ => None,
        }
    }
}

/// Split `text` into a kind and a body.
///
/// The header must sit in the first 256 bytes and use this shape:
///
/// ```text
/// ---
/// kind: speech
/// ---
/// ```
///
/// `kind` is `speech`, `sight`, or `deed`. Any other file is [`NoteKind::Book`]
/// and the body is `text`.
pub fn decode_note(text: &str) -> (NoteKind, &str) {
    let scan = byte_prefix(text, HEADER_LIMIT);
    match header_len(scan) {
        Some((kind, len)) => (kind, &text[len..]),
        None => (NoteKind::Book, text),
    }
}

/// Write `body` with the header for `kind`.
///
/// Book returns `body` unchanged.
pub fn encode_note(kind: NoteKind, body: &str) -> String {
    let Some(key) = kind.key() else {
        return body.to_string();
    };
    format!("---\nkind: {key}\n---\n{body}")
}

/// Keep the kind header from `previous` when `next` has none.
///
/// A Book note stays as `next` wrote it. A new text that already has a known
/// header stays as `next` wrote it.
pub fn preserve_kind(previous: &str, next: &str) -> String {
    let (old_kind, _) = decode_note(previous);
    if old_kind == NoteKind::Book {
        return next.to_string();
    }
    let (new_kind, _) = decode_note(next);
    if new_kind != NoteKind::Book {
        return next.to_string();
    }
    encode_note(old_kind, next)
}

/// Read a kind from the start of a file.
///
/// `bytes` may be longer than the header. Only the first 256 bytes are read.
pub(crate) fn kind_from_prefix(bytes: &[u8]) -> NoteKind {
    let end = utf8_prefix_len(bytes, HEADER_LIMIT);
    let Ok(text) = std::str::from_utf8(&bytes[..end]) else {
        return NoteKind::Book;
    };
    decode_note(text).0
}

/// Read a task line.
///
/// The line may use `-`, `*`, or `+`, and it may have a leading indent.
/// `[ ]` is open. `[x]` and `[X]` are done.
pub fn parse_task_line(line: &str) -> Option<TaskLine> {
    let indent_len = line
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(line.len());
    let (indent, rest) = line.split_at(indent_len);
    let mut chars = rest.chars();
    let marker = chars.next()?;
    if !matches!(marker, '-' | '*' | '+') {
        return None;
    }
    let rest = chars.as_str();
    let rest = rest.strip_prefix(" [")?;
    let mut state_chars = rest.chars();
    let state = state_chars.next()?;
    if !matches!(state, ' ' | 'x' | 'X') {
        return None;
    }
    let rest = state_chars.as_str().strip_prefix(']')?;
    let checked = state != ' ';
    let next = if checked { ' ' } else { 'x' };
    Some(TaskLine {
        checked,
        plain: format!("{indent}{marker}{rest}"),
        toggled: format!("{indent}{marker} [{next}]{rest}"),
    })
}

/// Flip one task line. A line that is not a task returns `None`.
pub fn toggle_task_line(line: &str) -> Option<String> {
    parse_task_line(line).map(|task| task.toggled)
}

fn header_len(scan: &str) -> Option<(NoteKind, usize)> {
    let mut lines = scan.split_inclusive('\n');
    let first = lines.next()?;
    if line_text(first) != "---" || !first.ends_with('\n') {
        return None;
    }
    let second = lines.next()?;
    let key = line_text(second).strip_prefix("kind: ")?;
    let kind = NoteKind::from_key(key)?;
    if !second.ends_with('\n') {
        return None;
    }
    let third = lines.next()?;
    if line_text(third) != "---" || !third.ends_with('\n') {
        return None;
    }
    Some((kind, first.len() + second.len() + third.len()))
}

fn line_text(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

fn byte_prefix(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn utf8_prefix_len(bytes: &[u8], max: usize) -> usize {
    let mut end = bytes.len().min(max);
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_note_has_no_header() {
        let body = "# Day\n\nHello.\n";
        assert_eq!(encode_note(NoteKind::Book, body), body);
        assert_eq!(decode_note(body), (NoteKind::Book, body));
    }

    #[test]
    fn speech_sight_and_deed_round_trip() {
        let body = "# Talk\n";
        for kind in [NoteKind::Speech, NoteKind::Sight, NoteKind::Deed] {
            let file = encode_note(kind, body);
            assert_eq!(decode_note(&file), (kind, body));
            assert_eq!(kind_from_prefix(file.as_bytes()), kind);
        }
    }

    #[test]
    fn a_missing_header_is_a_book_note() {
        let text = "Just text.\n";
        assert_eq!(decode_note(text), (NoteKind::Book, text));
        assert_eq!(kind_from_prefix(text.as_bytes()), NoteKind::Book);
    }

    #[test]
    fn an_unknown_header_is_a_book_note() {
        let text = "---\nkind: rune\n---\n# Keep me\n";
        assert_eq!(decode_note(text), (NoteKind::Book, text));
        let named_book = "---\nkind: book\n---\n# Keep me\n";
        assert_eq!(decode_note(named_book), (NoteKind::Book, named_book));
    }

    #[test]
    fn a_header_past_the_scan_limit_is_a_book_note() {
        let mut text = String::from("---\n");
        text.push_str(&"x".repeat(HEADER_LIMIT));
        text.push_str("\nkind: speech\n---\nbody\n");
        assert_eq!(decode_note(&text).0, NoteKind::Book);
        assert_eq!(decode_note(&text).1, text.as_str());
    }

    #[test]
    fn crlf_header_decodes() {
        let text = "---\r\nkind: sight\r\n---\r\n# Clip\n";
        assert_eq!(decode_note(text), (NoteKind::Sight, "# Clip\n"));
    }

    #[test]
    fn preserve_kind_restores_a_missing_header() {
        let previous = encode_note(NoteKind::Speech, "Hello.\n");
        let next = preserve_kind(&previous, "Goodbye.\n");
        assert_eq!(decode_note(&next), (NoteKind::Speech, "Goodbye.\n"));
    }

    #[test]
    fn preserve_kind_keeps_a_header_the_new_text_already_has() {
        let previous = encode_note(NoteKind::Speech, "Hello.\n");
        let replacement = encode_note(NoteKind::Sight, "Clip.\n");
        assert_eq!(preserve_kind(&previous, &replacement), replacement);
        let book = "Plain.\n";
        assert_eq!(preserve_kind(book, "Next.\n"), "Next.\n");
    }

    #[test]
    fn task_line_toggle_covers_open_done_indent_and_a_normal_line() {
        assert_eq!(
            toggle_task_line("- [ ] wash"),
            Some("- [x] wash".to_string())
        );
        assert_eq!(
            toggle_task_line("- [x] wash"),
            Some("- [ ] wash".to_string())
        );
        assert_eq!(
            toggle_task_line("- [X] wash"),
            Some("- [ ] wash".to_string())
        );
        assert_eq!(
            toggle_task_line("  * [ ] wash"),
            Some("  * [x] wash".to_string())
        );
        assert_eq!(
            toggle_task_line("+ [x] wash"),
            Some("+ [ ] wash".to_string())
        );
        assert_eq!(toggle_task_line("# Not a task"), None);
        let open = parse_task_line("- [ ] wash").unwrap();
        assert!(!open.checked);
        assert_eq!(open.plain, "- wash");
        let done = parse_task_line("- [x] wash").unwrap();
        assert!(done.checked);
    }
}
