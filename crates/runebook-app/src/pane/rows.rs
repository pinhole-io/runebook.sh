//! Selectable preview rows, and the text those rows send to the Archivist.
//!
//! A row is one Markdown block. That is a heading, a paragraph, a list item,
//! a quote, a table, a code block, an HTML block, a footnote, or a link
//! definition. A blank line separates rows. A block that owns a blank line
//! keeps that line.
//!
//! A plain click on the settled selection clears it.

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
    if let Some(kind) = html_block_start(body) {
        return html_block_end(lines, start, kind);
    }
    if is_math_open(body) {
        return math_end(lines, start);
    }
    if is_footnote_def(body) {
        return footnote_end(lines, start);
    }
    if is_link_def(body) {
        return link_def_end(lines, start);
    }
    if is_table(body) {
        return run_while(lines, start, is_table);
    }
    if is_heading(body) || is_hr(body) {
        return start;
    }
    if is_quote(body) {
        return quote_end(lines, start);
    }
    if let Some(indent) = list_indent(body) {
        return list_end(lines, start, indent);
    }
    if indented_code_start(body) {
        return indented_code_end(lines, start);
    }
    paragraph_end(lines, start)
}

/// A heading that follows body text or a table needs room above it.
///
/// The preview draws each block as its own row, so the gap lives on the row.
/// The first row stays flush. A heading that follows a heading stays tight.
pub(super) fn heading_needs_room(previous: Option<&str>, row: &str) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    row_is_heading(row) && !row_is_heading(previous)
}

fn row_is_heading(row: &str) -> bool {
    row.lines()
        .next()
        .is_some_and(|line| is_heading(line.trim_end_matches('\r')))
}

/// Left padding, in pixels, for a nested list item.
pub(super) fn list_pad(row: &str) -> f32 {
    let Some(line) = row.lines().next() else {
        return 0.;
    };
    match list_indent(line) {
        Some(indent) if indent > 0 => indent as f32 * 8.,
        _ => 0.,
    }
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
    let mut index = start + 1;
    while index < lines.len() {
        let body = lines[index].body;
        if body.trim().is_empty() {
            let Some(next) = next_content(lines, index) else {
                break;
            };
            let next_body = lines[next].body;
            if list_indent(next_body).is_some_and(|indent| indent <= marker_indent) {
                break;
            }
            if leading_indent(next_body) <= marker_indent {
                break;
            }
            end = next;
            index = next + 1;
            continue;
        }
        if starts_block(body) || leading_indent(body) <= marker_indent {
            break;
        }
        end = index;
        index += 1;
    }
    end
}

fn paragraph_end(lines: &[Line<'_>], start: usize) -> usize {
    let mut end = start;
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.body.trim().is_empty() {
            break;
        }
        if is_setext_underline(line.body) {
            return index;
        }
        if starts_block(line.body) {
            break;
        }
        end = index;
    }
    end
}

fn quote_end(lines: &[Line<'_>], start: usize) -> usize {
    let mut end = start;
    let mut index = start + 1;
    while index < lines.len() {
        if lines[index].body.trim().is_empty() {
            let Some(next) = next_content(lines, index) else {
                break;
            };
            if !is_quote(lines[next].body) {
                break;
            }
            end = next;
            index = next + 1;
            continue;
        }
        if !is_quote(lines[index].body) {
            break;
        }
        end = index;
        index += 1;
    }
    end
}

fn next_content(lines: &[Line<'_>], blank: usize) -> Option<usize> {
    (blank + 1..lines.len()).find(|index| !lines[*index].body.trim().is_empty())
}

fn starts_block(body: &str) -> bool {
    opening_fence(body).is_some()
        || is_table(body)
        || is_heading(body)
        || is_hr(body)
        || is_quote(body)
        || list_indent(body).is_some()
        || html_interrupts(body)
        || is_footnote_def(body)
        || is_link_def(body)
        || is_math_open(body)
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

fn is_setext_underline(line: &str) -> bool {
    let indent = leading_indent(line);
    if indent > 3 {
        return false;
    }
    let rest = line[indent..].trim_end();
    let Some(marker) = rest.chars().next() else {
        return false;
    };
    (marker == '=' || marker == '-') && rest.chars().all(|ch| ch == marker)
}

fn indented_code_start(line: &str) -> bool {
    leading_indent(line) >= 4 && !line.trim().is_empty() && list_indent(line).is_none()
}

fn indented_code_end(lines: &[Line<'_>], start: usize) -> usize {
    let mut end = start;
    let mut index = start + 1;
    while index < lines.len() {
        if lines[index].body.trim().is_empty() {
            let Some(next) = next_content(lines, index) else {
                break;
            };
            if !indented_code_start(lines[next].body) {
                break;
            }
            end = next;
            index = next + 1;
            continue;
        }
        if !indented_code_start(lines[index].body) {
            break;
        }
        end = index;
        index += 1;
    }
    end
}

fn is_math_open(line: &str) -> bool {
    let indent = leading_indent(line);
    indent <= 3 && line[indent..].trim() == "$$"
}

fn math_end(lines: &[Line<'_>], start: usize) -> usize {
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if is_math_open(line.body) {
            return index;
        }
    }
    lines.len() - 1
}

fn is_footnote_def(line: &str) -> bool {
    footnote_label(line).is_some()
}

fn footnote_label(line: &str) -> Option<&str> {
    let indent = leading_indent(line);
    if indent > 3 {
        return None;
    }
    let rest = line[indent..].strip_prefix("[^")?;
    let (label, _) = rest.split_once("]:")?;
    if label.is_empty() || label.contains(['[', ']', '^', ' ']) {
        return None;
    }
    Some(label)
}

fn footnote_end(lines: &[Line<'_>], start: usize) -> usize {
    let mut end = start;
    let mut index = start + 1;
    while index < lines.len() {
        let body = lines[index].body;
        if body.trim().is_empty() {
            let Some(next) = next_content(lines, index) else {
                break;
            };
            if leading_indent(lines[next].body) < 4 || starts_block(lines[next].body) {
                break;
            }
            end = next;
            index = next + 1;
            continue;
        }
        if leading_indent(body) < 4 || starts_block(body) {
            break;
        }
        end = index;
        index += 1;
    }
    end
}

fn is_link_def(line: &str) -> bool {
    link_label(line).is_some()
}

fn link_label(line: &str) -> Option<&str> {
    let indent = leading_indent(line);
    if indent > 3 {
        return None;
    }
    let rest = line[indent..].strip_prefix('[')?;
    if rest.starts_with('^') {
        return None;
    }
    let (label, _) = rest.split_once("]:")?;
    if label.is_empty() || label.contains(['[', ']']) {
        return None;
    }
    Some(label)
}

fn link_def_end(lines: &[Line<'_>], start: usize) -> usize {
    let Some(line) = lines.get(start + 1) else {
        return start;
    };
    if line.body.trim().is_empty() || starts_block(line.body) {
        return start;
    }
    let trimmed = line.body.trim();
    if trimmed.starts_with(['"', '\'', '(']) || leading_indent(line.body) >= 1 {
        return start + 1;
    }
    start
}

fn html_interrupts(line: &str) -> bool {
    html_block_start(line).is_some_and(HtmlKind::interrupts)
}

fn html_block_end(lines: &[Line<'_>], start: usize, kind: HtmlKind) -> usize {
    match kind {
        HtmlKind::UntilClose(tag) => until_close(lines, start, tag),
        HtmlKind::Comment => until_token(lines, start, "-->"),
        HtmlKind::Instruction => until_token(lines, start, "?>"),
        HtmlKind::Declaration => until_token(lines, start, ">"),
        HtmlKind::Cdata => until_token(lines, start, "]]>"),
        HtmlKind::Container(tag) => container_end(lines, start, &tag),
        HtmlKind::UntilBlank { .. } => until_blank(lines, start),
    }
}

fn until_blank(lines: &[Line<'_>], start: usize) -> usize {
    let mut end = start;
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.body.trim().is_empty() {
            break;
        }
        end = index;
    }
    end
}

fn until_token(lines: &[Line<'_>], start: usize, token: &str) -> usize {
    for (index, line) in lines.iter().enumerate().skip(start) {
        if line.body.contains(token) {
            return index;
        }
    }
    lines.len() - 1
}

fn until_close(lines: &[Line<'_>], start: usize, tag: &str) -> usize {
    let needle = format!("</{tag}");
    for (index, line) in lines.iter().enumerate().skip(start) {
        if !line.body.to_ascii_lowercase().contains(&needle) {
            continue;
        }
        if index == start && open_delta(line.body, tag) > 0 {
            continue;
        }
        return index;
    }
    lines.len() - 1
}

fn container_end(lines: &[Line<'_>], start: usize, tag: &str) -> usize {
    let mut depth = 0i32;
    for (index, line) in lines.iter().enumerate().skip(start) {
        depth += open_delta(line.body, tag);
        if depth <= 0 {
            return index;
        }
    }
    lines.len() - 1
}

fn open_delta(line: &str, tag: &str) -> i32 {
    let lower = line.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut delta = 0i32;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let close = bytes.get(index + 1) == Some(&b'/');
        let name_at = if close { index + 2 } else { index + 1 };
        if lower[name_at..].starts_with(tag) {
            let after = name_at + tag.len();
            let boundary = bytes.get(after).copied().unwrap_or(b' ');
            if !boundary.is_ascii_alphanumeric() {
                if close {
                    delta -= 1;
                } else if !void_tag(&lower[after..]) {
                    delta += 1;
                }
            }
        }
        index += 1;
    }
    delta
}

fn void_tag(after_name: &str) -> bool {
    after_name
        .find('>')
        .is_some_and(|end| after_name[..end].trim_end().ends_with('/'))
}

enum HtmlKind {
    UntilClose(&'static str),
    Comment,
    Instruction,
    Declaration,
    Cdata,
    Container(String),
    UntilBlank { interrupt: bool },
}

impl HtmlKind {
    fn interrupts(self) -> bool {
        match self {
            Self::UntilBlank { interrupt } => interrupt,
            _ => true,
        }
    }
}

fn html_block_start(line: &str) -> Option<HtmlKind> {
    let indent = leading_indent(line);
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    if rest.starts_with("<!--") {
        return Some(HtmlKind::Comment);
    }
    if rest.starts_with("<?") {
        return Some(HtmlKind::Instruction);
    }
    if rest.starts_with("<![CDATA[") {
        return Some(HtmlKind::Cdata);
    }
    if rest.starts_with("<!") {
        return Some(HtmlKind::Declaration);
    }
    let (close, name) = tag_name(rest)?;
    if matches!(name.as_str(), "script" | "pre" | "style" | "textarea") && !close {
        return Some(HtmlKind::UntilClose(match name.as_str() {
            "script" => "script",
            "pre" => "pre",
            "style" => "style",
            _ => "textarea",
        }));
    }
    if BLOCK_TAGS.contains(&name.as_str()) && !close {
        if SPAN_TAGS.contains(&name.as_str()) {
            return Some(HtmlKind::Container(name));
        }
        return Some(HtmlKind::UntilBlank { interrupt: true });
    }
    if rest.contains('>') {
        return Some(HtmlKind::UntilBlank { interrupt: false });
    }
    None
}

fn tag_name(rest: &str) -> Option<(bool, String)> {
    let inner = rest.strip_prefix('<')?;
    let close = inner.starts_with('/');
    let inner = inner.strip_prefix('/').unwrap_or(inner);
    if !inner.starts_with(|ch: char| ch.is_ascii_alphabetic()) {
        return None;
    }
    let name_len = inner
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric())
        .count();
    let name = inner[..name_len].to_ascii_lowercase();
    let after = inner[name_len..].chars().next().unwrap_or(' ');
    if !(after.is_ascii_whitespace() || after == '>' || after == '/') {
        return None;
    }
    Some((close, name))
}

const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "legend",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];

const SPAN_TAGS: &[&str] = &[
    "article",
    "aside",
    "blockquote",
    "details",
    "dialog",
    "div",
    "dl",
    "fieldset",
    "figure",
    "footer",
    "form",
    "header",
    "main",
    "nav",
    "section",
    "table",
];

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
    fn a_heading_after_a_table_needs_room() {
        let source = "| A | B |\n| --- | --- |\n\n## Next\n";
        let rows = row_texts(source);
        assert!(heading_needs_room(Some(rows[0]), rows[1]));
    }

    #[test]
    fn a_heading_after_text_needs_room() {
        assert!(heading_needs_room(Some("A paragraph."), "## Next"));
    }

    #[test]
    fn the_first_heading_stays_flush() {
        assert!(!heading_needs_room(None, "# Title"));
    }

    #[test]
    fn a_heading_after_a_heading_stays_close() {
        assert!(!heading_needs_room(Some("# One"), "## Two"));
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
        let mut gesture = RowGesture::press(None, 0, false).expect("press");
        gesture.drag_over(1);
        let selected = selected_source(source, &gesture).expect("two rows");
        assert_eq!(selected, "alpha\n\nbeta");
    }

    #[test]
    fn one_row_omits_the_blank_line_around_it() {
        let source = "\n# Title\n\n";
        let gesture = RowGesture::press(None, 0, false).expect("press");
        assert_eq!(
            selected_source(source, &gesture).as_deref(),
            Some("# Title")
        );
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
    fn stale_index_has_no_source() {
        let gesture = RowGesture::press(None, 4, false).expect("press");
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

    #[test]
    fn setext_heading_stays_with_its_text() {
        let source = "Title\n===\n\n---\n";
        assert_eq!(row_texts(source), vec!["Title\n===", "---"]);
    }

    #[test]
    fn dash_after_a_paragraph_is_a_setext_heading() {
        let source = "Section\n---\n\nNext\n";
        assert_eq!(row_texts(source), vec!["Section\n---", "Next"]);
    }

    #[test]
    fn indented_code_keeps_an_inner_blank_line() {
        let source = "    one\n\n    two\n\nAfter\n";
        assert_eq!(row_texts(source), vec!["    one\n\n    two", "After"]);
    }

    #[test]
    fn quote_keeps_a_blank_line_between_paragraphs() {
        let source = "> one\n\n> two\n\nplain\n";
        assert_eq!(row_texts(source), vec!["> one\n\n> two", "plain"]);
    }

    #[test]
    fn loose_list_continuation_stays_with_the_item() {
        let source = "- one\n\n  continued\n- two\n";
        assert_eq!(row_texts(source), vec!["- one\n\n  continued", "- two"]);
    }

    #[test]
    fn details_block_keeps_the_blank_line() {
        let source =
            "<details>\n<summary>Click to expand</summary>\n\nBody\n\n</details>\n\nAfter\n";
        assert_eq!(
            row_texts(source),
            vec![
                "<details>\n<summary>Click to expand</summary>\n\nBody\n\n</details>",
                "After"
            ]
        );
    }

    #[test]
    fn math_fence_stays_one_row() {
        let source = "$$\ne = mc^2\n$$\n\nAfter\n";
        assert_eq!(row_texts(source), vec!["$$\ne = mc^2\n$$", "After"]);
    }

    #[test]
    fn footnote_definition_is_its_own_row() {
        let source = "See.[^note]\n\n[^note]: The note.\n    more\n\nNext\n";
        assert_eq!(
            row_texts(source),
            vec!["See.[^note]", "[^note]: The note.\n    more", "Next"]
        );
    }

    #[test]
    fn link_definition_keeps_a_title_line() {
        let source = "[id]: https://example.com\n  \"Title\"\n\nAfter\n";
        assert_eq!(
            row_texts(source),
            vec!["[id]: https://example.com\n  \"Title\"", "After"]
        );
    }

    #[test]
    fn nested_list_pad_follows_the_marker() {
        assert_eq!(list_pad("- top\n"), 0.);
        assert_eq!(list_pad("  - nested\n"), 16.);
    }
}
