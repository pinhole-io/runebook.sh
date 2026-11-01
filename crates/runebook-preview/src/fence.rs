//! A Markdown fence at the start of a line.
//!
//! The indent count is a byte index. It counts spaces and tabs only.
//! A tab-indented fence is still a fence. The emoji shortcode scanner
//! does not use this module. That scanner ignores a tab indent.

/// Leading spaces and tabs, as a byte index into `line`.
#[must_use]
pub fn leading_indent(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
        .count()
}

/// Opening fence on `line`. The pair is the marker and its length.
///
/// A fence needs at least three backticks or tildes. Indent above three
/// spaces is not a fence. A backtick fence cannot contain a backtick in
/// the info string.
#[must_use]
pub fn opening_fence(line: &str) -> Option<(char, usize)> {
    let indent = leading_indent(line);
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let marker = rest.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let count = rest.chars().take_while(|ch| *ch == marker).count();
    if count < 3 {
        return None;
    }
    let after = &rest[count..];
    if marker == '`' && after.contains('`') {
        return None;
    }
    Some((marker, count))
}

/// `true` when `line` closes a fence opened with `marker` of length `count`.
#[must_use]
pub fn closing_fence(line: &str, marker: char, count: usize) -> bool {
    let indent = leading_indent(line);
    if indent > 3 {
        return false;
    }
    let rest = line[indent..].trim_end();
    !rest.is_empty() && rest.chars().all(|ch| ch == marker) && rest.chars().count() >= count
}
