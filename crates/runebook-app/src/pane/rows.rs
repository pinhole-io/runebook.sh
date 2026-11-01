//! Pointer gesture over preview rows, and the text those rows send to the Archivist.
//!
//! Row bounds come from `runebook_preview`. A plain click on the settled selection clears it.

pub(super) use runebook_preview::{heading_needs_room, list_pad, split_rows};

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
    /// Returns [`None`] when a plain click lands in the settled selection.
    /// That click clears it.
    /// Shift keeps the current anchor and opens the menu at once.
    /// A plain press on a new row waits for [`Self::release`].
    pub(super) fn press(current: Option<Self>, index: usize, shift: bool) -> Option<Self> {
        if shift {
            let anchor = current.map(|row| row.anchor).unwrap_or(index);
            return Some(Self {
                anchor,
                focus: index,
                dragging: false,
                menu: true,
            });
        }
        if current.is_some_and(|row| row.covers(index)) {
            return None;
        }
        Some(Self {
            anchor: index,
            focus: index,
            dragging: true,
            menu: false,
        })
    }

    /// The pointer is up, and `index` is inside the selection.
    fn covers(&self, index: usize) -> bool {
        if self.dragging {
            return false;
        }
        let lo = self.anchor.min(self.focus);
        let hi = self.anchor.max(self.focus);
        index >= lo && index <= hi
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

/// Selected markdown, plus the inclusive 1-based lines it occupies in `source`.
pub(super) fn selected_span(source: &str, gesture: &RowGesture) -> Option<(String, usize, usize)> {
    let rows = split_rows(source);
    let (lo, hi) = gesture.bounds(rows.len())?;
    let start = rows[lo].start;
    let end = rows[hi].end;
    let text = source.get(start..end)?;
    let text = usable_selection(text)?;
    let (start_line, end_line) = line_span(source, start, end);
    Some((text, start_line, end_line))
}

/// Inclusive 1-based line numbers for the byte range `start..end`.
///
/// `end` is exclusive. A trailing newline stays on the line it ends.
pub(super) fn line_span(source: &str, start: usize, end: usize) -> (usize, usize) {
    let start = char_floor(source, start);
    let end = char_floor(source, end).max(start);
    let start_line = line_at(source, start);
    if end == start {
        return (start_line, start_line);
    }
    let last = char_floor(source, end - 1);
    (start_line, line_at(source, last))
}

/// Label for one parked selection: `notes/day.md · 100-200`.
///
/// One line omits the end, so line 4 is `notes/day.md · 4`.
pub(super) fn selection_label(note_id: &str, start_line: usize, end_line: usize) -> String {
    let lines = if start_line == end_line {
        start_line.to_string()
    } else {
        format!("{start_line}-{end_line}")
    };
    format!("{note_id}.md · {lines}")
}

fn line_at(source: &str, offset: usize) -> usize {
    source[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn char_floor(source: &str, offset: usize) -> usize {
    let mut offset = offset.min(source.len());
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Selection text the menu can send. Surrounding newlines are dropped.
///
/// Whitespace alone is [`None`], so a caret or a blank drag does not open the menu.
pub(super) fn usable_selection(text: &str) -> Option<String> {
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

    #[test]
    fn range_keeps_the_blank_line_between_rows() {
        let source = "alpha\n\nbeta\n";
        let mut gesture = RowGesture::press(None, 0, false).expect("press");
        gesture.drag_over(1);
        let (selected, _, _) = selected_span(source, &gesture).expect("two rows");
        assert_eq!(selected, "alpha\n\nbeta");
    }

    #[test]
    fn one_row_omits_the_blank_line_around_it() {
        let source = "\n# Title\n\n";
        let gesture = RowGesture::press(None, 0, false).expect("press");
        let (text, _, _) = selected_span(source, &gesture).expect("title");
        assert_eq!(text, "# Title");
    }

    #[test]
    fn selected_span_names_the_lines_in_the_note() {
        let source = "alpha\n\nbeta\n";
        let mut gesture = RowGesture::press(None, 0, false).expect("press");
        gesture.drag_over(1);
        let (text, start, end) = selected_span(source, &gesture).expect("span");
        assert_eq!(text, "alpha\n\nbeta");
        assert_eq!((start, end), (1, 3));
    }

    #[test]
    fn a_middle_row_keeps_its_own_line() {
        let source = "\n# Title\n\n";
        let gesture = RowGesture::press(None, 0, false).expect("press");
        let (_, start, end) = selected_span(source, &gesture).expect("span");
        assert_eq!((start, end), (2, 2));
    }

    #[test]
    fn line_span_counts_a_partial_line() {
        let source = "one\ntwo\nthree\n";
        assert_eq!(line_span(source, 0, 4), (1, 1));
        assert_eq!(line_span(source, 4, 7), (2, 2));
        assert_eq!(line_span(source, 0, source.len()), (1, 3));
    }

    #[test]
    fn selection_label_names_the_file_and_the_lines() {
        assert_eq!(selection_label("test", 100, 200), "test.md · 100-200");
        assert_eq!(selection_label("notes/day", 4, 4), "notes/day.md · 4");
    }

    #[test]
    fn drag_extends_and_release_opens_the_menu() {
        let mut gesture = RowGesture::press(None, 2, false).expect("press");
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
        let first = RowGesture::press(None, 0, false).expect("press");
        let mut gesture = RowGesture::press(Some(first), 2, true).expect("shift");
        assert!(gesture.menu_open());
        assert!(!gesture.drag_over(3));
        assert_eq!(gesture.bounds(4), Some((0, 2)));
    }

    #[test]
    fn second_plain_click_on_the_selection_clears_it() {
        let mut gesture = RowGesture::press(None, 1, false).expect("press");
        gesture.release();
        assert!(RowGesture::press(Some(gesture), 1, false).is_none());
    }

    #[test]
    fn second_plain_click_inside_a_range_clears_it() {
        let mut gesture = RowGesture::press(None, 0, false).expect("press");
        gesture.drag_over(2);
        gesture.release();
        let settled = gesture.clone();
        assert!(RowGesture::press(Some(settled), 1, false).is_none());
        let shifted = RowGesture::press(Some(gesture), 3, true).expect("shift");
        assert_eq!(shifted.bounds(4), Some((0, 3)));
    }

    #[test]
    fn plain_click_on_another_row_starts_a_new_selection() {
        let mut gesture = RowGesture::press(None, 0, false).expect("press");
        gesture.release();
        let next = RowGesture::press(Some(gesture), 2, false).expect("next");
        assert_eq!(next.bounds(4), Some((2, 2)));
        assert!(!next.menu_open());
    }

    #[test]
    fn usable_selection_drops_surrounding_newlines() {
        assert_eq!(
            usable_selection("\n## Heading\n"),
            Some("## Heading".to_string())
        );
    }

    #[test]
    fn usable_selection_rejects_blank_text() {
        assert_eq!(usable_selection(""), None);
        assert_eq!(usable_selection("\n\n"), None);
        assert_eq!(usable_selection("  \t"), None);
    }

    #[test]
    fn stale_index_has_no_source() {
        let gesture = RowGesture::press(None, 4, false).expect("press");
        assert_eq!(gesture.bounds(2), None);
        assert!(selected_span("only\n", &gesture).is_none());
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
