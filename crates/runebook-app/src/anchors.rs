//! Link and footnote definitions for one note.
//!
//! A preview row is parsed on its own. A reference in that row still needs
//! the definition from the rest of the note.

use std::collections::HashMap;

use runebook::preview_markdown;

/// Markdown for one preview row, plus whether appended definitions stay hidden.
pub(crate) struct Prepared {
    pub source: String,
    /// `true` when this row carries definitions only so references resolve.
    pub hide_anchors: bool,
    /// Definitions appended so a reference in this row can resolve.
    ///
    /// Empty when `hide_anchors` is false. The page omits this tail.
    pub suffix: String,
}

/// Build the markdown a preview row should parse.
pub(crate) fn prepare(note: &str, row: &str) -> Prepared {
    let anchors = collect(note);
    if is_literal(row) || is_anchor_row(row) {
        return Prepared {
            source: preview_markdown(row),
            hide_anchors: false,
            suffix: String::new(),
        };
    }
    let suffix = anchors.suffix(row);
    let mut source = preview_markdown(row);
    if suffix.is_empty() {
        return Prepared {
            source,
            hide_anchors: false,
            suffix: String::new(),
        };
    }
    let suffix = preview_markdown(&suffix);
    source.push_str("\n\n");
    source.push_str(&suffix);
    Prepared {
        source,
        hide_anchors: true,
        suffix,
    }
}

/// Destination URL for each link id in `source`.
pub(crate) fn link_urls(source: &str) -> HashMap<String, String> {
    collect(source)
        .links
        .into_iter()
        .map(|(id, (url, _))| (id, url))
        .collect()
}

struct Anchors {
    footnotes: HashMap<String, String>,
    links: HashMap<String, (String, String)>,
}

impl Anchors {
    fn suffix(&self, row: &str) -> String {
        let visible = mask_code(row);
        let mut parts = Vec::new();
        for id in self.links.keys() {
            if contains_link_ref(&visible, id)
                && let Some((_, raw)) = self.links.get(id)
            {
                parts.push(raw.trim_end().to_string());
            }
        }
        for id in footnote_refs(&visible) {
            if let Some(raw) = self.footnotes.get(&id) {
                parts.push(raw.trim_end().to_string());
            }
        }
        parts.join("\n\n")
    }
}

struct Piece<'a> {
    body: &'a str,
    raw: &'a str,
}

fn collect(source: &str) -> Anchors {
    let pieces = pieces(source);
    let mut footnotes = HashMap::new();
    let mut links = HashMap::new();
    let mut index = 0;
    let mut fence: Option<(char, usize)> = None;
    while index < pieces.len() {
        let body = pieces[index].body;
        if let Some((marker, count)) = fence {
            if closing_fence(body, marker, count) {
                fence = None;
            }
            index += 1;
            continue;
        }
        if let Some(open) = opening_fence(body) {
            fence = Some(open);
            index += 1;
            continue;
        }
        if let Some(label) = footnote_label(body) {
            let end = footnote_span(&pieces, index);
            footnotes.insert(label.to_ascii_lowercase(), join(&pieces[index..=end]));
            index = end + 1;
            continue;
        }
        if let Some(label) = link_label(body)
            && let Some(url) = link_url(body)
        {
            let end = link_span(&pieces, index);
            links.insert(
                label.to_ascii_lowercase(),
                (url, join(&pieces[index..=end])),
            );
            index = end + 1;
            continue;
        }
        index += 1;
    }
    Anchors { footnotes, links }
}

fn pieces(source: &str) -> Vec<Piece<'_>> {
    let mut start = 0;
    let mut pieces = Vec::new();
    for raw in source.split_inclusive('\n') {
        let end = start + raw.len();
        let body = raw.trim_end_matches(['\n', '\r']);
        pieces.push(Piece {
            body,
            raw: &source[start..end],
        });
        start = end;
    }
    pieces
}

fn join(pieces: &[Piece<'_>]) -> String {
    let mut raw = String::new();
    for piece in pieces {
        raw.push_str(piece.raw);
    }
    raw
}

fn footnote_span(pieces: &[Piece<'_>], start: usize) -> usize {
    let mut end = start;
    let mut index = start + 1;
    while index < pieces.len() {
        let body = pieces[index].body;
        if body.trim().is_empty() {
            let Some(next) =
                (index + 1..pieces.len()).find(|at| !pieces[*at].body.trim().is_empty())
            else {
                break;
            };
            if leading_indent(pieces[next].body) < 4 {
                break;
            }
            end = next;
            index = next + 1;
            continue;
        }
        if leading_indent(body) < 4 {
            break;
        }
        end = index;
        index += 1;
    }
    end
}

fn link_span(pieces: &[Piece<'_>], start: usize) -> usize {
    let Some(line) = pieces.get(start + 1) else {
        return start;
    };
    if line.body.trim().is_empty() {
        return start;
    }
    let trimmed = line.body.trim();
    if trimmed.starts_with(['"', '\'', '(']) || leading_indent(line.body) >= 1 {
        return start + 1;
    }
    start
}

fn is_anchor_row(row: &str) -> bool {
    let Some(line) = row.lines().next() else {
        return false;
    };
    footnote_label(line).is_some() || link_label(line).is_some()
}

fn is_literal(row: &str) -> bool {
    let Some(line) = row.lines().next() else {
        return false;
    };
    if opening_fence(line).is_some() || line.trim() == "$$" {
        return true;
    }
    leading_indent(line) >= 4 && !line.trim().is_empty()
}

fn footnote_refs(row: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < row.len() {
        if row[index..].starts_with("[^")
            && let Some(label) = closed_footnote(&row[index..])
            && !found.contains(&label)
        {
            found.push(label);
        }
        index += row[index..].chars().next().map_or(1, char::len_utf8);
    }
    found
}

fn closed_footnote(slice: &str) -> Option<String> {
    let rest = slice.strip_prefix("[^")?;
    let (label, rest) = rest.split_once(']')?;
    if label.is_empty() || rest.starts_with(':') || label.contains(['[', ']', ' ', '\n']) {
        return None;
    }
    Some(label.to_ascii_lowercase())
}

fn contains_link_ref(row: &str, id: &str) -> bool {
    let needle = format!("[{id}]");
    let mut start = 0;
    while let Some(at) = row[start..].find(&needle) {
        let index = start + at;
        let after = index + needle.len();
        let next = row.as_bytes().get(after).copied();
        if !matches!(next, Some(b'(' | b':')) {
            return true;
        }
        start = after;
    }
    false
}

fn mask_code(row: &str) -> String {
    let mut out = String::with_capacity(row.len());
    let mut code = false;
    for ch in row.chars() {
        if ch == '`' {
            code = !code;
            out.push(' ');
            continue;
        }
        out.push(if code { ' ' } else { ch });
    }
    out
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

fn link_url(line: &str) -> Option<String> {
    let rest = line.split_once("]:")?.1.trim();
    if let Some(rest) = rest.strip_prefix('<') {
        let end = rest.find('>')?;
        let url = &rest[..end];
        return (!url.is_empty()).then(|| url.to_string());
    }
    if rest.is_empty() {
        return None;
    }
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let url = &rest[..end];
    (!url.is_empty()).then(|| url.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footnote_reference_keeps_the_definition() {
        let note = "See.[^note]\n\n[^note]: The note.\n";
        let prepared = prepare(note, "See.[^note]");
        assert!(prepared.hide_anchors);
        assert!(prepared.source.contains("See."));
        assert!(prepared.source.contains("[^note]: The note."));
    }

    #[test]
    fn footnote_definition_row_stays_alone() {
        let note = "See.[^note]\n\n[^note]: The note.\n";
        let prepared = prepare(note, "[^note]: The note.");
        assert!(!prepared.hide_anchors);
        assert_eq!(prepared.source.trim(), "[^note]: The note.");
    }

    #[test]
    fn link_reference_keeps_the_destination() {
        let note = "A [name][id].\n\n[id]: https://example.com\n";
        let prepared = prepare(note, "A [name][id].");
        assert!(prepared.hide_anchors);
        assert!(prepared.source.contains("[id]: https://example.com"));
    }

    #[test]
    fn code_fence_does_not_gain_a_definition() {
        let note = "```\n[^note]\n```\n\n[^note]: Secret.\n";
        let prepared = prepare(note, "```\n[^note]\n```");
        assert!(!prepared.source.contains("Secret"));
    }

    #[test]
    fn inline_code_does_not_count_as_a_reference() {
        let note = "Use `[^note]` here.\n\n[^note]: Secret.\n";
        let prepared = prepare(note, "Use `[^note]` here.");
        assert!(!prepared.source.contains("Secret"));
    }

    #[test]
    fn link_url_reads_angle_and_bare_destinations() {
        let urls = link_urls("[a]: <https://example.com/a>\n[b]: /b.png \"t\"\n");
        assert_eq!(
            urls.get("a").map(String::as_str),
            Some("https://example.com/a")
        );
        assert_eq!(urls.get("b").map(String::as_str), Some("/b.png"));
    }
}
