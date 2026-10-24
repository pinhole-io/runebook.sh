//! Selectable preview rows, and the text those rows send to the Archivist.
//!
//! A row is one Markdown block: a heading, a paragraph, a list item, a quote,
//! a table, or a fenced code block. A blank line separates rows.

/// Byte range of one row in the note source. `end` includes the row's last newline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SourceRow {
    pub start: usize,
    pub end: usize,
}

/// Pointer gesture over preview rows.
///
/// The menu stays closed while the pointer is down, so a drag can cross rows first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RowGesture {
    anchor: usize,
    focus: usize,
    dragging: bool,
    menu: bool,
}

impl RowGesture {
    /// Start a gesture at `index`.
    ///
    /// Shift keeps the current anchor and opens the menu at once.
    /// A plain press waits for [`Self::release`].
    pub(super) fn press(current: Option<Self>, index: usize, shift: bool) -> Self {
        if shift {
            let anchor = current.map(|row| row.anchor).unwrap_or(index);
            return Self {
                anchor,
                focus: index,
                dragging: false,
                menu: true,
            };
        }
        Self {
            anchor: index,
            focus: index,
            dragging: true,
            menu: false,
        }
    }

    /// Move the focus while the pointer is down. Returns whether the range changed.
    pub(super) fn drag_over(&mut self, index: usize) -> bool {
        if !self.dragging || self.focus == index {
            return false;
        }
        self.focus = index;
        true
    }

    /// Open the menu when a drag ends.
    pub(super) fn release(&mut self) {
        if self.dragging {
            self.dragging = false;
            self.menu = true;
        }
    }

    pub(super) fn menu_open(&self) -> bool {
        self.menu && !self.dragging
    }

    /// Inclusive row indexes, when both ends still exist.
    pub(super) fn bounds(&self, len: usize) -> Option<(usize, usize)> {
        if len == 0 || self.anchor >= len || self.focus >= len {
            return None;
        }
        Some((self.anchor.min(self.focus), self.anchor.max(self.focus)))
    }
}

/// Split `source` into selectable rows.
pub(super) fn split_rows(source: &str) -> Vec<SourceRow> {
    let lines = source_lines(source);
    let mut rows = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if lines[index].body.trim().is_empty() {
            index += 1;
            continue;
        }
        let end = row_end(&lines, index);
        rows.push(SourceRow {
            start: lines[index].start,
            end: lines[end].end,
        });
        index = end + 1;
    }
    rows
}

/// Markdown source for the gesture, without surrounding newlines.
pub(super) fn selected_source(source: &str, gesture: &RowGesture) -> Option<String> {
    let rows = split_rows(source);
    let (lo, hi) = gesture.bounds(rows.len())?;
    let text = source.get(rows[lo].start..rows[hi].end)?;
    let text = text.trim_matches(['\n', '\r']);
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// User message that gives the Archivist the selected rows.
///
/// `items` are `(note id, markdown)`. An empty draft still produces a message
/// when at least one item has text, and that message tells the Archivist to wait.
pub(super) fn context_message<'a>(
    items: impl IntoIterator<Item = (&'a str, &'a str)>,
    user: &str,
) -> Option<String> {
    let user = user.trim();
    let wait = if user.is_empty() {
        "Do not change a file until the user asks.\n\n"
    } else {
        ""
    };
    let mut blocks = Vec::new();
    for (note_id, source) in items {
        let source = source.trim_matches(['\n', '\r']);
        if source.trim().is_empty() {
            continue;
        }
        blocks.push(format!(
            "Selected rows from note `{note_id}`.\n\
             These rows may need an update or a correction.\n\
             {wait}{}",
            fenced(source)
        ));
    }
    if blocks.is_empty() && user.is_empty() {
        return None;
    }
    if user.is_empty() {
        return Some(blocks.join("\n\n"));
    }
    if blocks.is_empty() {
        return Some(user.to_string());
    }
    Some(format!("{}\n\n{user}", blocks.join("\n\n")))
}

/// User message that asks the Archivist to rewrite one selection.
pub(super) fn rewrite_message(note_id: &str, source: &str, change: &str) -> Option<String> {
    let change = change.trim();
    let source = source.trim_matches(['\n', '\r']);
    if change.is_empty() || source.trim().is_empty() {
        return None;
    }
    Some(format!(
        "Rewrite the selected rows in note `{note_id}`.\n\
         Read that note first.\n\
         Call write_note with the full note.\n\
         Change only the selected rows.\n\
         Keep every other line the same.\n\
         \n\
         Selected rows:\n\
         \n\
         {}\n\
         \n\
         Change: {change}",
        fenced(source)
    ))
}

struct Line<'a> {
    start: usize,
    end: usize,
    body: &'a str,
}

fn source_lines(source: &str) -> Vec<Line<'_>> {
    let mut start = 0;
    let mut lines = Vec::new();
    if source.is_empty() {
        return lines;
    }
    for piece in source.split_inclusive('\n') {
        let end = start + piece.len();
        let body = piece.strip_suffix('\n').unwrap_or(piece);
        let body = body.strip_suffix('\r').unwrap_or(body);
        lines.push(Line { start, end, body });
        start = end;
    }
    lines
}

fn row_end(lines: &[Line<'_>], start: usize) -> usize {
    let body = lines[start].body;
    if let Some((marker, count)) = opening_fence(body) {
        return close_fence(lines, start, marker, count);
    }
    if is_table(body) {
        return run_while(lines, start, is_table);
    }
    if is_heading(body) || is_hr(body) {
        return start;
    }
    if is_quote(body) {
        return run_while(lines, start, is_quote);
    }
    if let Some(indent) = list_indent(body) {
        return list_end(lines, start, indent);
    }
    let mut end = start;
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.body.trim().is_empty() || starts_block(line.body) {
            break;
        }
        end = index;
    }
    end
}

fn close_fence(lines: &[Line<'_>], start: usize, marker: char, count: usize) -> usize {
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if closing_fence(line.body, marker, count) {
            return index;
        }
    }
    lines.len() - 1
}

fn run_while(lines: &[Line<'_>], start: usize, keep: fn(&str) -> bool) -> usize {
    let mut end = start;
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.body.trim().is_empty() || !keep(line.body) {
            break;
        }
        end = index;
    }
    end
}

fn list_end(lines: &[Line<'_>], start: usize, marker_indent: usize) -> usize {
    let mut end = start;
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.body.trim().is_empty() || starts_block(line.body) {
            break;
        }
        if leading_indent(line.body) <= marker_indent {
            break;
        }
        end = index;
    }
    end
}

fn starts_block(body: &str) -> bool {
    opening_fence(body).is_some()
        || is_table(body)
        || is_heading(body)
        || is_hr(body)
        || is_quote(body)
        || list_indent(body).is_some()
}

fn leading_indent(line: &str) -> usize {
    line.chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .count()
}

fn opening_fence(line: &str) -> Option<(char, usize)> {
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

fn closing_fence(line: &str, marker: char, count: usize) -> bool {
    let indent = leading_indent(line);
    if indent > 3 {
        return false;
    }
    let rest = line[indent..].trim_end();
    !rest.is_empty() && rest.chars().all(|ch| ch == marker) && rest.chars().count() >= count
}

fn is_heading(line: &str) -> bool {
    let indent = leading_indent(line);
    if indent > 3 {
        return false;
    }
    let rest = &line[indent..];
    let hashes = rest.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    match rest.as_bytes().get(hashes) {
        None | Some(b' ' | b'\t') => true,
        Some(_) => false,
    }
}

fn is_hr(line: &str) -> bool {
    let indent = leading_indent(line);
    if indent > 3 {
        return false;
    }
    let rest = line[indent..].trim();
    if rest.len() < 3 {
        return false;
    }
    let Some(marker) = rest.chars().next() else {
        return false;
    };
    if marker != '-' && marker != '*' && marker != '_' {
        return false;
    }
    let marks = rest.chars().filter(|ch| *ch == marker).count();
    marks >= 3
        && rest
            .chars()
            .all(|ch| ch == marker || ch == ' ' || ch == '\t')
}

fn is_quote(line: &str) -> bool {
    let indent = leading_indent(line);
    indent <= 3 && line[indent..].starts_with('>')
}

fn is_table(line: &str) -> bool {
    let rest = line.trim_start();
    rest.starts_with('|') && rest.matches('|').count() >= 2
}

fn list_indent(line: &str) -> Option<usize> {
    let indent = leading_indent(line);
    let rest = &line[indent..];
    let bytes = rest.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    if matches!(bytes[0], b'-' | b'+' | b'*') {
        return matches!(bytes.get(1), Some(b' ' | b'\t')).then_some(indent);
    }
    let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let marker = *rest.as_bytes().get(digits)?;
    if marker != b'.' && marker != b')' {
        return None;
    }
    matches!(rest.as_bytes().get(digits + 1), Some(b' ' | b'\t')).then_some(indent)
}

fn fenced(source: &str) -> String {
    let fence = markdown_fence(source);
    format!("{fence}markdown\n{source}\n{fence}")
}

fn markdown_fence(source: &str) -> String {
    let mut run = 0;
    let mut longest = 0;
    for ch in source.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_texts(source: &str) -> Vec<&str> {
        split_rows(source)
            .into_iter()
            .map(|row| source[row.start..row.end].trim_matches(['\n', '\r']))
            .collect()
    }

    #[test]
    fn empty_note_has_no_rows() {
        assert!(split_rows("").is_empty());
        assert!(split_rows("\n\n").is_empty());
    }

    #[test]
    fn heading_paragraph_and_list_are_separate_rows() {
        let source = "# Test Series\n\nA small hierarchy.\n\n- alpha one\n- beta two\n";
        assert_eq!(
            row_texts(source),
            vec![
                "# Test Series",
                "A small hierarchy.",
                "- alpha one",
                "- beta two"
            ]
        );
    }

    #[test]
    fn wrapped_paragraph_stays_one_row() {
        let source = "alpha line\nstill alpha\n\n# Next\n";
        assert_eq!(row_texts(source), vec!["alpha line\nstill alpha", "# Next"]);
    }

    #[test]
    fn list_continuation_stays_with_the_item() {
        let source = "- one\n  continued\n- two\n";
        assert_eq!(row_texts(source), vec!["- one\n  continued", "- two"]);
    }

    #[test]
    fn nested_list_is_its_own_row() {
        let source = "- one\n  - nested\n";
        assert_eq!(row_texts(source), vec!["- one", "  - nested"]);
    }

    #[test]
    fn fence_keeps_inner_blank_line() {
        let source = "```mermaid\ngraph TD\n\nA-->B\n```\n\nAfter\n";
        assert_eq!(
            row_texts(source),
            vec!["```mermaid\ngraph TD\n\nA-->B\n```", "After"]
        );
    }

    #[test]
    fn unclosed_fence_runs_to_the_end() {
        let source = "```\nstill code\n\n# not a heading\n";
        assert_eq!(
            row_texts(source),
            vec!["```\nstill code\n\n# not a heading"]
        );
    }

    #[test]
    fn table_is_one_row() {
        let source = "| a | b |\n| --- | --- |\n| 1 | 2 |\n\nAfter\n";
        assert_eq!(
            row_texts(source),
            vec!["| a | b |\n| --- | --- |\n| 1 | 2 |", "After"]
        );
    }

    #[test]
    fn quote_lines_group() {
        let source = "> one\n> two\n\nplain\n";
        assert_eq!(row_texts(source), vec!["> one\n> two", "plain"]);
    }

    #[test]
    fn range_keeps_the_blank_line_between_rows() {
        let source = "alpha\n\nbeta\n";
        let mut gesture = RowGesture::press(None, 0, false);
        gesture.drag_over(1);
        let selected = selected_source(source, &gesture).expect("two rows");
        assert_eq!(selected, "alpha\n\nbeta");
    }

    #[test]
    fn one_row_omits_the_blank_line_around_it() {
        let source = "\n# Title\n\n";
        let gesture = RowGesture::press(None, 0, false);
        assert_eq!(
            selected_source(source, &gesture).as_deref(),
            Some("# Title")
        );
    }

    #[test]
    fn drag_extends_and_release_opens_the_menu() {
        let mut gesture = RowGesture::press(None, 2, false);
        assert!(!gesture.menu_open());
        assert!(gesture.drag_over(0));
        assert_eq!(gesture.bounds(4), Some((0, 2)));
        gesture.release();
        assert!(gesture.menu_open());
        assert!(!gesture.drag_over(3));
        assert_eq!(gesture.bounds(4), Some((0, 2)));
    }

    #[test]
    fn shift_press_keeps_the_anchor_and_opens_the_menu() {
        let first = RowGesture::press(None, 0, false);
        let mut gesture = RowGesture::press(Some(first), 2, true);
        assert!(gesture.menu_open());
        assert!(!gesture.drag_over(3));
        assert_eq!(gesture.bounds(4), Some((0, 2)));
    }

    #[test]
    fn stale_index_has_no_source() {
        let gesture = RowGesture::press(None, 4, false);
        assert_eq!(gesture.bounds(2), None);
        assert!(selected_source("only\n", &gesture).is_none());
    }

    #[test]
    fn context_message_names_the_note_and_waits() {
        let message = context_message([("test-series/README", "- alpha")], "").expect("context");
        assert!(message.contains("note `test-series/README`"));
        assert!(message.contains("may need an update or a correction"));
        assert!(message.contains("Do not change a file until the user asks."));
        assert!(message.contains("- alpha"));
    }

    #[test]
    fn context_message_appends_the_user_text() {
        let message = context_message([("note", "row")], "Fix the name.").expect("both");
        assert!(message.contains("row"));
        assert!(!message.contains("Do not change a file"));
        assert!(message.ends_with("Fix the name."));
    }

    #[test]
    fn context_message_keeps_a_plain_draft() {
        assert_eq!(context_message([], "Hello").as_deref(), Some("Hello"));
        assert!(context_message([], "  ").is_none());
    }

    #[test]
    fn fence_grows_past_backticks_in_the_row() {
        let message = context_message([("note", "```\ncode\n```")], "").expect("fence");
        assert!(message.contains("````markdown\n```\ncode\n```\n````"));
    }

    #[test]
    fn rewrite_message_limits_the_edit_to_the_rows() {
        let message =
            rewrite_message("journal/day", "- alpha", "Make it shorter.").expect("rewrite");
        assert!(message.contains("note `journal/day`"));
        assert!(message.contains("Call write_note with the full note."));
        assert!(message.contains("Change only the selected rows."));
        assert!(message.contains("- alpha"));
        assert!(message.ends_with("Change: Make it shorter."));
    }

    #[test]
    fn rewrite_message_rejects_an_empty_change() {
        assert!(rewrite_message("note", "- alpha", "  ").is_none());
        assert!(rewrite_message("note", "  ", "Change it.").is_none());
    }
}
