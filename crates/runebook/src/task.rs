//! Checklist lines stored in any note.
//!
//! A task is one line. The due date and the done date use the markers from
//! the Obsidian Tasks plugin. The vault does not keep a second copy.

use std::fmt;

/// A calendar day written as `YYYY-MM-DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskDate {
    year: u16,
    month: u8,
    day: u8,
}

impl TaskDate {
    /// Parse `YYYY-MM-DD`. A day that is not on the calendar returns [`None`].
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 10 {
            return None;
        }
        let bytes = text.as_bytes();
        if bytes[4] != b'-' || bytes[7] != b'-' {
            return None;
        }
        let year = parse_u16(&bytes[0..4])?;
        let month = parse_u8(&bytes[5..7])?;
        let day = parse_u8(&bytes[8..10])?;
        Self::from_ymd(year, month, day)
    }

    /// Build a date. A day that is not on the calendar returns [`None`].
    pub fn from_ymd(year: u16, month: u8, day: u8) -> Option<Self> {
        if year == 0 || month == 0 || month > 12 || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }
}

impl fmt::Display for TaskDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// One checklist line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLine {
    /// `true` when the box is `[x]` or `[X]`.
    pub checked: bool,
    /// The words of the task. Date markers are not part of this text.
    pub text: String,
    /// Due date from `📅 YYYY-MM-DD`, when the date is real.
    pub due: Option<TaskDate>,
    /// Done date from `✅ YYYY-MM-DD`, when the date is real.
    pub done: Option<TaskDate>,
    indent: String,
    marker: String,
}

impl TaskLine {
    /// Text the preview shows: the description, then the due date.
    pub fn preview_text(&self) -> String {
        match &self.due {
            Some(due) if self.text.is_empty() => due.to_string(),
            Some(due) => format!("{} · {due}", self.text),
            None => self.text.clone(),
        }
    }

    fn render(&self) -> String {
        let box_mark = if self.checked { 'x' } else { ' ' };
        let mut line = format!("{}{} [{box_mark}]", self.indent, self.marker);
        if !self.text.is_empty() {
            line.push(' ');
            line.push_str(&self.text);
        }
        if let Some(due) = self.due {
            line.push_str(" 📅 ");
            line.push_str(&due.to_string());
        }
        if let Some(done) = self.done {
            line.push_str(" ✅ ");
            line.push_str(&done.to_string());
        }
        line
    }
}

/// A task found in one note body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRef {
    /// Zero-based line in the note body.
    pub line: u32,
    /// `true` when the box is checked.
    pub checked: bool,
    /// The words of the task.
    pub text: String,
    /// Due date, when the line has a real one.
    pub due: Option<TaskDate>,
    /// Done date, when the line has a real one.
    pub done: Option<TaskDate>,
}

/// Read a task line.
///
/// The marker is `-`, `*`, `+`, `1.`, or `1)`. The line may have a leading indent.
/// `[ ]` is open. `[x]` and `[X]` are done.
pub fn parse_task_line(line: &str) -> Option<TaskLine> {
    let indent_len = line
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(line.len());
    let (indent, rest) = line.split_at(indent_len);
    let (marker, rest) = take_marker(rest)?;
    let rest = rest.strip_prefix(" [")?;
    let mut state_chars = rest.chars();
    let state = state_chars.next()?;
    if !matches!(state, ' ' | 'x' | 'X') {
        return None;
    }
    let rest = state_chars.as_str().strip_prefix(']')?;
    let (text, due, done) = take_dates(rest);
    Some(TaskLine {
        checked: state != ' ',
        text,
        due,
        done,
        indent: indent.to_string(),
        marker: marker.to_string(),
    })
}

/// Flip one task line.
///
/// A check adds `✅` with `today` when the line has no done date.
/// An uncheck removes the done date. The due date stays.
/// A line that is not a task returns [`None`].
pub fn toggle_task_line(line: &str, today: TaskDate) -> Option<String> {
    let mut task = parse_task_line(line)?;
    task.checked = !task.checked;
    if task.checked {
        if task.done.is_none() {
            task.done = Some(today);
        }
    } else {
        task.done = None;
    }
    Some(task.render())
}

/// A new open task. `text` is stored as given.
pub fn new_task_line(text: &str, due: Option<TaskDate>) -> String {
    TaskLine {
        checked: false,
        text: text.to_string(),
        due,
        done: None,
        indent: String::new(),
        marker: "-".to_string(),
    }
    .render()
}

/// Replace the description and the due date. The box and the done date stay.
///
/// A line that is not a task returns [`None`].
pub fn rewrite_task(line: &str, text: &str, due: Option<TaskDate>) -> Option<String> {
    let mut task = parse_task_line(line)?;
    task.text = text.to_string();
    task.due = due;
    Some(task.render())
}

/// The logical text of body line `line`.
pub fn body_line(body: &str, line: u32) -> Option<&str> {
    source_lines(body)
        .into_iter()
        .nth(line as usize)
        .map(|row| row.logical)
}

/// Replace body line `line` and keep that line's newline.
pub fn replace_line(body: &str, line: u32, next: &str) -> Option<String> {
    let lines = source_lines(body);
    let current = lines.get(line as usize)?;
    let mut out = String::with_capacity(body.len() + next.len());
    out.push_str(&body[..current.start]);
    out.push_str(next);
    out.push_str(current.newline);
    out.push_str(&body[current.end..]);
    Some(out)
}

/// Flip body line `line`. A line that is not a task returns [`None`].
pub fn toggle_body_line(body: &str, line: u32, today: TaskDate) -> Option<String> {
    let current = body_line(body, line)?;
    let toggled = toggle_task_line(current, today)?;
    replace_line(body, line, &toggled)
}

/// Put `task_line` on its own line at byte `at`.
///
/// `at` is clamped to `body`. A cursor in the middle of a line splits that line.
pub fn insert_task_at(body: &str, at: usize, task_line: &str) -> String {
    let at = floor_char(body, at.min(body.len()));
    let task_line = task_line.trim_end_matches(['\n', '\r']);
    let before = &body[..at];
    let after = &body[at..];
    let mut out = String::with_capacity(body.len() + task_line.len() + 2);
    out.push_str(before);
    if !before.is_empty() && !before.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(task_line);
    if after.is_empty() || !after.starts_with('\n') {
        out.push('\n');
    }
    out.push_str(after);
    out
}

/// Tasks in `body`. A line inside a fenced code block is skipped.
pub fn collect_tasks(body: &str) -> Vec<TaskRef> {
    let mut tasks = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (index, row) in source_lines(body).into_iter().enumerate() {
        if let Some((marker, count)) = fence {
            if closing_fence(row.logical, marker, count) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = opening_fence(row.logical) {
            fence = Some(open);
            continue;
        }
        let Some(task) = parse_task_line(row.logical) else {
            continue;
        };
        tasks.push(TaskRef {
            line: index as u32,
            checked: task.checked,
            text: task.text,
            due: task.due,
            done: task.done,
        });
    }
    tasks
}

struct SourceLine<'a> {
    logical: &'a str,
    start: usize,
    end: usize,
    newline: &'a str,
}

fn source_lines(body: &str) -> Vec<SourceLine<'_>> {
    let mut lines = Vec::new();
    if body.is_empty() {
        return lines;
    }
    let mut start = 0;
    for piece in body.split_inclusive('\n') {
        let end = start + piece.len();
        let logical = piece.strip_suffix('\n').unwrap_or(piece);
        let logical = logical.strip_suffix('\r').unwrap_or(logical);
        let newline = &piece[logical.len()..];
        lines.push(SourceLine {
            logical,
            start,
            end,
            newline,
        });
        start = end;
    }
    lines
}

fn take_marker(rest: &str) -> Option<(&str, &str)> {
    let mut chars = rest.chars();
    let first = chars.next()?;
    if matches!(first, '-' | '*' | '+') {
        let len = first.len_utf8();
        return Some((&rest[..len], &rest[len..]));
    }
    if !first.is_ascii_digit() {
        return None;
    }
    let digits = rest
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let after = &rest[digits..];
    let punct = after.chars().next()?;
    if punct != '.' && punct != ')' {
        return None;
    }
    let len = digits + punct.len_utf8();
    Some((&rest[..len], &rest[len..]))
}

fn take_dates(rest: &str) -> (String, Option<TaskDate>, Option<TaskDate>) {
    let mut due = None;
    let mut done = None;
    let mut kept = String::new();
    let mut index = 0;
    while let Some(ch) = rest[index..].chars().next() {
        let len = ch.len_utf8();
        if matches!(ch, '📅' | '✅')
            && let Some((date, end)) = date_after(rest, index + len)
        {
            if ch == '📅' && due.is_none() {
                due = Some(date);
            }
            if ch == '✅' && done.is_none() {
                done = Some(date);
            }
            index = end;
            continue;
        }
        kept.push(ch);
        index += len;
    }
    (collapse_space(&kept), due, done)
}

fn date_after(text: &str, mut index: usize) -> Option<(TaskDate, usize)> {
    while index < text.len() {
        let ch = text[index..].chars().next()?;
        if ch != ' ' && ch != '\t' {
            break;
        }
        index += ch.len_utf8();
    }
    let end = index.checked_add(10)?;
    if end > text.len() {
        return None;
    }
    let date = TaskDate::parse(&text[index..end])?;
    let boundary = end == text.len()
        || text[end..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_whitespace());
    if !boundary {
        return None;
    }
    Some((date, end))
}

fn collapse_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn floor_char(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn parse_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 || bytes.iter().any(|byte| !byte.is_ascii_digit()) {
        return None;
    }
    let mut value = 0u16;
    for byte in bytes {
        value = value.checked_mul(10)?.checked_add(u16::from(byte - b'0'))?;
    }
    Some(value)
}

fn parse_u8(bytes: &[u8]) -> Option<u8> {
    if bytes.len() != 2 || bytes.iter().any(|byte| !byte.is_ascii_digit()) {
        return None;
    }
    Some((bytes[0] - b'0') * 10 + (bytes[1] - b'0'))
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap(year: u16) -> bool {
    let year = u32::from(year);
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn opening_fence(line: &str) -> Option<(char, usize)> {
    let indent = line
        .bytes()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
        .count();
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
    let after = &rest[marker.len_utf8() * count..];
    if marker == '`' && after.contains('`') {
        return None;
    }
    Some((marker, count))
}

fn closing_fence(line: &str, marker: char, count: usize) -> bool {
    let indent = line
        .bytes()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
        .count();
    if indent > 3 {
        return false;
    }
    let rest = line[indent..].trim_end();
    !rest.is_empty() && rest.chars().all(|ch| ch == marker) && rest.chars().count() >= count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> TaskDate {
        TaskDate::parse(text).expect("test date")
    }

    #[test]
    fn task_line_toggle_covers_open_done_indent_and_a_normal_line() {
        let today = day("2026-11-01");
        assert_eq!(
            toggle_task_line("- [ ] wash", today),
            Some("- [x] wash ✅ 2026-11-01".to_string())
        );
        assert_eq!(
            toggle_task_line("- [x] wash ✅ 2026-11-01", today),
            Some("- [ ] wash".to_string())
        );
        assert_eq!(
            toggle_task_line("- [X] wash", today),
            Some("- [ ] wash".to_string())
        );
        assert_eq!(
            toggle_task_line("  * [ ] wash", today),
            Some("  * [x] wash ✅ 2026-11-01".to_string())
        );
        assert_eq!(
            toggle_task_line("+ [x] wash ✅ 2026-11-01", today),
            Some("+ [ ] wash".to_string())
        );
        assert_eq!(toggle_task_line("# Not a task", today), None);
        let open = parse_task_line("- [ ] wash").unwrap();
        assert!(!open.checked);
        assert_eq!(open.text, "wash");
        let done = parse_task_line("- [x] wash").unwrap();
        assert!(done.checked);
    }

    #[test]
    fn toggle_keeps_the_due_date_and_writes_the_done_date() {
        let today = day("2026-11-01");
        assert_eq!(
            toggle_task_line("- [ ] wash 📅 2026-11-02", today),
            Some("- [x] wash 📅 2026-11-02 ✅ 2026-11-01".to_string())
        );
        assert_eq!(
            toggle_task_line("- [x] wash 📅 2026-11-02 ✅ 2026-11-01", today),
            Some("- [ ] wash 📅 2026-11-02".to_string())
        );
    }

    #[test]
    fn a_bad_date_stays_in_the_description() {
        let task = parse_task_line("- [ ] wash 📅 2026-13-01").unwrap();
        assert_eq!(task.due, None);
        assert_eq!(task.text, "wash 📅 2026-13-01");
        assert!(TaskDate::parse("2023-02-29").is_none());
        assert!(TaskDate::parse("1900-02-29").is_none());
        assert_eq!(
            TaskDate::parse("2000-02-29").unwrap().to_string(),
            "2000-02-29"
        );
        assert!(TaskDate::parse("0000-01-01").is_none());
    }

    #[test]
    fn numbered_markers_are_tasks() {
        let today = day("2026-11-01");
        assert_eq!(
            toggle_task_line("1. [ ] first", today),
            Some("1. [x] first ✅ 2026-11-01".to_string())
        );
        assert_eq!(
            toggle_task_line("2) [x] second ✅ 2026-11-01", today),
            Some("2) [ ] second".to_string())
        );
    }

    #[test]
    fn rewrite_changes_the_words_and_the_due_date() {
        let line = "- [x] wash 📅 2026-11-02 ✅ 2026-11-01";
        let next = rewrite_task(line, "fold", Some(day("2026-12-01"))).unwrap();
        assert_eq!(next, "- [x] fold 📅 2026-12-01 ✅ 2026-11-01");
    }

    #[test]
    fn insert_puts_a_task_on_its_own_line() {
        let line = new_task_line("wash", Some(day("2026-11-02")));
        assert_eq!(line, "- [ ] wash 📅 2026-11-02");
        assert_eq!(
            insert_task_at("Hello", 5, &line),
            "Hello\n- [ ] wash 📅 2026-11-02\n"
        );
        assert_eq!(
            insert_task_at("# Day\n\n", 7, &line),
            "# Day\n\n- [ ] wash 📅 2026-11-02\n"
        );
        assert_eq!(
            insert_task_at("Hello", 2, "- [ ] wash"),
            "He\n- [ ] wash\nllo"
        );
    }

    #[test]
    fn a_fence_hides_a_task_line() {
        let body = "```\n- [ ] hidden\n```\n- [ ] shown\n";
        let tasks = collect_tasks(body);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].line, 3);
        assert_eq!(tasks[0].text, "shown");
        assert!(!tasks[0].checked);
    }

    #[test]
    fn toggle_body_line_flips_one_line() {
        let today = day("2026-11-01");
        let body = "# Day\n\n- [ ] wash\n";
        let next = toggle_body_line(body, 2, today).unwrap();
        assert_eq!(next, "# Day\n\n- [x] wash ✅ 2026-11-01\n");
        assert!(toggle_body_line(body, 0, today).is_none());
    }
}
