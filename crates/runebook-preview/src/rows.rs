//! Markdown rows in a note.
//!
//! A row is one Markdown block. That is a heading, a paragraph, a list item,
//! a quote, a table, a code block, an HTML block, a footnote, or a link
//! definition. A blank line separates rows. A block that owns a blank line
//! keeps that line.

/// Byte range of one row in the note source. `end` includes the row's last newline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRow {
    pub start: usize,
    pub end: usize,
}

/// Split `source` into selectable rows.
pub fn split_rows(source: &str) -> Vec<SourceRow> {
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
pub fn heading_needs_room(previous: Option<&str>, row: &str) -> bool {
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
pub fn list_pad(row: &str) -> f32 {
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
