use std::ops::Range;

use crate::note::NoteId;

/// Text inside `[[...]]` before an optional alias.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WikiTarget(String);

impl WikiTarget {
    /// Build a target from already trimmed text.
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Borrow the target text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WikiTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One wiki link and the byte range of its `[[...]]` source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiLink {
    /// Link target, without the alias.
    pub target: WikiTarget,
    /// Text after `|`, when the link has one.
    pub alias: Option<String>,
    /// Byte range of the full `[[...]]` span in the source.
    pub span: Range<usize>,
}

/// How [`crate::VaultIndex::resolve`] classified a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveResult {
    /// The target is an existing note id.
    Exact(NoteId),
    /// The target has no `/` and matches one file stem.
    UniqueStem(NoteId),
    /// Nothing in the vault matches.
    Missing,
    /// The stem matches more than one note, and there is no exact id.
    Ambiguous(Vec<NoteId>),
}

/// Find wiki links outside fenced and inline code.
pub fn parse_wiki_links(src: &str) -> Vec<WikiLink> {
    let mut links = Vec::new();
    let mut offset = 0;
    let mut fence: Option<(char, usize)> = None;
    for line in src.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        if let Some((ch, len)) = fence {
            if is_closing_fence(body, ch, len) {
                fence = None;
            }
        } else if let Some(open) = opening_fence(body) {
            fence = Some(open);
        } else {
            parse_line(body, offset, &mut links);
        }
        offset += line.len();
    }
    links
}

/// Replace wiki links with markdown links that use `runebook://note/...`.
pub fn preview_markdown(src: &str) -> String {
    let links = parse_wiki_links(src);
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0;
    for link in links {
        out.push_str(&src[cursor..link.span.start]);
        let label = link
            .alias
            .clone()
            .filter(|alias| !alias.is_empty())
            .unwrap_or_else(|| link.target.as_str().to_string());
        out.push('[');
        push_link_label(&mut out, &label);
        out.push_str("](");
        out.push_str(&wiki_url(&link.target));
        out.push(')');
        cursor = link.span.end;
    }
    out.push_str(&src[cursor..]);
    out
}

/// Read a `runebook://note/...` URL back into a wiki target.
pub fn note_target_from_url(url: &str) -> Option<WikiTarget> {
    let rest = url.strip_prefix("runebook://note/")?;
    let decoded = decode_path(rest)?;
    if decoded.is_empty() {
        return None;
    }
    Some(WikiTarget::new(decoded))
}

fn wiki_url(target: &WikiTarget) -> String {
    format!("runebook://note/{}", encode_path(target.as_str()))
}

fn parse_line(line: &str, line_start: usize, links: &mut Vec<WikiLink>) {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut in_code = false;
    while index < bytes.len() {
        let current = bytes[index];
        if current == b'`' {
            in_code = !in_code;
            index += 1;
            continue;
        }
        if !in_code && current == b'\\' {
            index += 1;
            if index < bytes.len() {
                index += next_len(line, index);
            }
            continue;
        }
        if !in_code
            && current == b'['
            && bytes.get(index + 1) == Some(&b'[')
            && let Some((link, next)) = parse_one(&line[index..], line_start + index)
        {
            links.push(link);
            index = next - line_start;
            continue;
        }
        index += next_len(line, index);
    }
}

fn parse_one(slice: &str, start: usize) -> Option<(WikiLink, usize)> {
    let body = slice.strip_prefix("[[")?;
    let end = body.find("]]")?;
    let inner = &body[..end];
    if inner.contains('\n') {
        return None;
    }
    let (target_raw, alias_raw) = match inner.split_once('|') {
        Some((target, alias)) => (target, Some(alias)),
        None => (inner, None),
    };
    let target = target_raw.trim();
    if target.is_empty() || target.contains(']') {
        return None;
    }
    let alias = alias_raw.map(str::trim).filter(|alias| !alias.is_empty());
    let consumed = 2 + end + 2;
    Some((
        WikiLink {
            target: WikiTarget::new(target),
            alias: alias.map(str::to_string),
            span: start..start + consumed,
        },
        start + consumed,
    ))
}

fn opening_fence(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim();
    let mut chars = trimmed.chars();
    let ch = chars.next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let mut len = 1;
    for next in chars.by_ref() {
        if next == ch {
            len += 1;
        } else {
            break;
        }
    }
    if len < 3 {
        return None;
    }
    let rest: String = trimmed.chars().skip(len).collect();
    if ch == '`' && rest.contains('`') {
        return None;
    }
    Some((ch, len))
}

fn is_closing_fence(line: &str, ch: char, min: usize) -> bool {
    let trimmed = line.trim();
    let len = trimmed.chars().count();
    len >= min && trimmed.chars().all(|current| current == ch)
}

fn next_len(text: &str, index: usize) -> usize {
    text.get(index..)
        .and_then(|rest| rest.chars().next())
        .map_or(1, char::len_utf8)
}

fn push_link_label(out: &mut String, label: &str) {
    for ch in label.chars() {
        if ch == '[' || ch == ']' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
}

fn encode_path(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn decode_path(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = from_hex(bytes[index + 1])?;
            let low = from_hex(bytes[index + 2])?;
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_alias_and_plain_targets() {
        let src = "See [[inbox]] and [[journal/day|Today]].";
        let links = parse_wiki_links(src);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target.as_str(), "inbox");
        assert_eq!(links[0].alias, None);
        assert_eq!(links[1].target.as_str(), "journal/day");
        assert_eq!(links[1].alias.as_deref(), Some("Today"));
        assert_eq!(&src[links[0].span.clone()], "[[inbox]]");
        assert_eq!(&src[links[1].span.clone()], "[[journal/day|Today]]");
    }

    #[test]
    fn parse_skips_fences_inline_code_and_escapes() {
        let src = "Use `[[skip]]` and \\[[skip]] too.\n```\n[[skip]]\n```\n[[keep]]\n";
        let links = parse_wiki_links(src);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target.as_str(), "keep");
    }

    #[test]
    fn preview_rewrites_alias_and_spaces() {
        let preview = preview_markdown("Go to [[my note|Mine]].");
        assert_eq!(preview, "Go to [Mine](runebook://note/my%20note).");
        assert_eq!(
            note_target_from_url("runebook://note/my%20note")
                .unwrap()
                .as_str(),
            "my note"
        );
    }
}
