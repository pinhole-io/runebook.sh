//! Definition lists, math, and emoji for one preview row.
//!
//! The markdown compiler has no definition list and no gemoji map.
//! Math leaves the compiler as a code span. This pass finishes those three.

use latex2mathml::DisplayStyle;

const INLINE_CLASS: &str = "language-math math-inline";
const DISPLAY_CLASS: &str = "language-math math-display";

/// Replace `:shortcode:` with Unicode. Fenced code and inline code stay literal.
pub(super) fn expand_emoji(source: &str) -> String {
    if fenced_row(source) {
        return source.to_string();
    }
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            let ticks = run_len(bytes, index, b'`');
            if let Some(end) = find_closer(bytes, index + ticks, ticks) {
                out.push_str(&source[index..end]);
                index = end;
                continue;
            }
        }
        if bytes[index] == b':'
            && let Some((emoji, len)) = shortcode_at(&source[index..])
        {
            out.push_str(emoji);
            index += len;
            continue;
        }
        let ch = source[index..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// `Some` HTML when `source` is one or more terms, each with a `: ` definition.
///
/// `compile` turns the text of a term or a definition into HTML.
pub(super) fn definition_list(
    source: &str,
    mut compile: impl FnMut(&str) -> String,
) -> Option<String> {
    let items = definition_items(source)?;
    let mut html = String::from("<dl>");
    for item in items {
        html.push_str("<dt>");
        html.push_str(&piece(&item.term.join("\n"), &mut compile));
        html.push_str("</dt>");
        for def in item.defs {
            html.push_str("<dd>");
            html.push_str(&piece(&def, &mut compile));
            html.push_str("</dd>");
        }
    }
    html.push_str("</dl>");
    Some(html)
}

/// Replace math code spans with MathML. A formula that cannot convert stays code.
pub(super) fn render_math(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut index = 0;
    while let Some(hit) = next_math(html, index) {
        out.push_str(&html[index..hit.from]);
        match mathml(&html[hit.body_start..hit.body_end], hit.display) {
            Some(mathml) => out.push_str(&mathml),
            None => out.push_str(&html[hit.from..hit.to]),
        }
        index = hit.to;
    }
    out.push_str(&html[index..]);
    out
}

struct Item {
    term: Vec<String>,
    defs: Vec<String>,
}

struct MathHit {
    from: usize,
    to: usize,
    body_start: usize,
    body_end: usize,
    display: bool,
}

fn definition_items(source: &str) -> Option<Vec<Item>> {
    let mut items = Vec::new();
    let mut term = Vec::new();
    let mut defs = Vec::new();
    let mut saw_def = false;
    for line in source.lines() {
        if line.trim().is_empty() {
            return None;
        }
        if let Some(text) = definition_text(line) {
            if term.is_empty() {
                return None;
            }
            saw_def = true;
            defs.push(text);
            continue;
        }
        if saw_def {
            items.push(Item { term, defs });
            term = Vec::new();
            defs = Vec::new();
            saw_def = false;
        }
        term.push(line.trim().to_string());
    }
    if !saw_def {
        return None;
    }
    items.push(Item { term, defs });
    Some(items)
}

fn definition_text(line: &str) -> Option<String> {
    let trimmed = line.trim_start_matches(' ');
    let indent = line.len() - trimmed.len();
    if indent > 3 {
        return None;
    }
    let rest = trimmed.strip_prefix(':')?;
    if !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let text = rest.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn piece(source: &str, compile: &mut impl FnMut(&str) -> String) -> String {
    let html = compile(source);
    let trimmed = html.trim();
    let inner = trimmed
        .strip_prefix("<p>")
        .and_then(|rest| rest.strip_suffix("</p>"))
        .unwrap_or(trimmed);
    crate::html_style::sanitize_fragment(inner)
}

fn fenced_row(source: &str) -> bool {
    let Some(line) = source.lines().next() else {
        return false;
    };
    let trimmed = line.trim_start_matches(' ');
    let indent = line.len() - trimmed.len();
    if indent > 3 {
        return false;
    }
    let Some(marker) = trimmed.chars().next() else {
        return false;
    };
    if marker != '`' && marker != '~' {
        return false;
    }
    trimmed.chars().take_while(|ch| *ch == marker).count() >= 3
}

fn shortcode_at(text: &str) -> Option<(&'static str, usize)> {
    let rest = text.strip_prefix(':')?;
    let end = rest.find(':')?;
    let name = &rest[..end];
    if name.is_empty() || name.len() > 64 {
        return None;
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '+' | '-'))
    {
        return None;
    }
    let emoji = emojis::get_by_shortcode(name)?;
    Some((emoji.as_str(), name.len() + 2))
}

fn run_len(bytes: &[u8], index: usize, byte: u8) -> usize {
    bytes[index..]
        .iter()
        .take_while(|value| **value == byte)
        .count()
}

fn find_closer(bytes: &[u8], from: usize, ticks: usize) -> Option<usize> {
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            let run = run_len(bytes, index, b'`');
            if run == ticks {
                return Some(index + ticks);
            }
            index += run;
            continue;
        }
        index += 1;
    }
    None
}

fn next_math(html: &str, from: usize) -> Option<MathHit> {
    let inline = find_code(html, from, INLINE_CLASS, false);
    let display = find_code(html, from, DISPLAY_CLASS, true);
    match (inline, display) {
        (Some(inline), Some(display)) if display.from < inline.from => Some(display),
        (Some(inline), _) => Some(inline),
        (_, display) => display,
    }
}

fn find_code(html: &str, from: usize, class: &str, display: bool) -> Option<MathHit> {
    let marker = format!("<code class=\"{class}\">");
    let start = html[from..].find(&marker)? + from;
    let body_start = start + marker.len();
    let body_end = html[body_start..].find("</code>")? + body_start;
    let mut from_tag = start;
    let mut to = body_end + "</code>".len();
    if display {
        let prefix = html[..start].trim_end();
        if let Some(pre) = prefix.strip_suffix("<pre>") {
            from_tag = pre.len();
            if html[to..].starts_with("</pre>") {
                to += "</pre>".len();
            }
        }
    }
    Some(MathHit {
        from: from_tag,
        to,
        body_start,
        body_end,
        display,
    })
}

fn mathml(body: &str, display: bool) -> Option<String> {
    let latex = unescape_code(body.trim());
    if latex.is_empty() {
        return None;
    }
    let style = if display {
        DisplayStyle::Block
    } else {
        DisplayStyle::Inline
    };
    let mathml = latex2mathml::latex_to_mathml(&latex, style).ok()?;
    if mathml.to_ascii_lowercase().contains("<script") {
        return None;
    }
    if display {
        Some(format!("<div class=\"math-display\">{mathml}</div>"))
    } else {
        Some(mathml)
    }
}

fn unescape_code(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_row_becomes_a_term_and_a_definition() {
        let html = definition_list("Markdown\n: A lightweight markup language.\n", |text| {
            format!("<p>{text}</p>")
        })
        .expect("list");
        assert!(html.contains("<dt>"));
        assert!(html.contains("<dd>"));
        assert!(html.contains("Markdown"));
        assert!(html.contains("lightweight markup language"));
    }

    #[test]
    fn a_sentence_with_a_colon_stays_a_sentence() {
        assert!(
            definition_list("See: this stays a sentence.\n", |text| text.to_string()).is_none()
        );
    }

    #[test]
    fn a_rocket_shortcode_becomes_the_character() {
        let text = expand_emoji("Go :rocket: now.");
        assert!(text.contains('🚀'));
        assert!(!text.contains(":rocket:"));
    }

    #[test]
    fn a_fenced_block_keeps_a_shortcode_literal() {
        let source = "```\n:rocket:\n```\n";
        assert_eq!(expand_emoji(source), source);
    }
}
