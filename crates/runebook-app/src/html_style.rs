//! Give HTML the same look as the markdown it stands in for.
//!
//! The kit styles a short list of tags. This pass rewrites the rest into
//! markdown the kit already draws, or into a private link the preview paints.
//! Fenced code, indented code, inline code, and comments stay literal.

pub(crate) const HTML_WIDGET: &str = "#rb-html-";

/// Rewrite HTML in `source` into markdown, outside code and comments.
pub(crate) fn style_html(source: &str) -> String {
    if source.is_empty() || indented_code(source) {
        return source.to_string();
    }
    lift_inline(&lift_blocks(source))
}

/// Text with tags removed and entities decoded.
pub(crate) fn visible_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars();
    while let Some(ch) = chars.next() {
        if ch == '<' {
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    let decoded = decode_entities(&out);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `Some` when `html` is one `<dl>` of terms and definitions.
pub(crate) fn def_list(html: &str) -> Option<Vec<(String, String)>> {
    if !outer_tag(html, "dl") {
        return None;
    }
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<dl")?;
    let tag_end = open + html[open..].find('>')?;
    let close = lower.rfind("</dl>")?;
    if close < tag_end {
        return None;
    }
    let inner = &html[tag_end + 1..close];
    let mut items = Vec::new();
    let mut term: Option<String> = None;
    let mut body = String::new();
    let mut mode = DefMode::Skip;
    let mut buf = String::new();
    let mut index = 0;
    while let Some(tag) = next_tag(inner, index) {
        buf.push_str(&inner[index..tag.start]);
        let name = tag.name.as_str();
        if name == "dt" || name == "dd" {
            finish_def(&mut mode, &mut buf, &mut term, &mut body, &mut items);
            mode = if tag.close {
                DefMode::Skip
            } else if name == "dt" {
                DefMode::Term
            } else {
                DefMode::Body
            };
            buf.clear();
        } else {
            buf.push_str(&inner[tag.start..tag.end]);
        }
        index = tag.end;
    }
    buf.push_str(&inner[index..]);
    finish_def(&mut mode, &mut buf, &mut term, &mut body, &mut items);
    flush_def(&mut term, &mut body, &mut items);
    if items.is_empty() { None } else { Some(items) }
}

/// `Some(url, caption)` when `html` is one `<figure>` with an image.
pub(crate) fn figure_html(html: &str) -> Option<(String, String)> {
    if !outer_tag(html, "figure") || html.to_ascii_lowercase().contains("<details") {
        return None;
    }
    let img = first_tag(html, "img")?;
    let src = attr_value(&img, "src")?;
    if src.is_empty() {
        return None;
    }
    let alt = attr_value(&img, "alt").unwrap_or_default();
    let caption = figcaption(html);
    let caption = if caption.is_empty() { alt } else { caption };
    Some((src, caption))
}

enum DefMode {
    Skip,
    Term,
    Body,
}

fn finish_def(
    mode: &mut DefMode,
    buf: &mut String,
    term: &mut Option<String>,
    body: &mut String,
    items: &mut Vec<(String, String)>,
) {
    match mode {
        DefMode::Term => {
            flush_def(term, body, items);
            let text = visible_text(buf);
            *term = Some(if text.is_empty() {
                "Term".to_string()
            } else {
                text
            });
        }
        DefMode::Body => {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(buf.trim());
        }
        DefMode::Skip => {}
    }
    buf.clear();
    *mode = DefMode::Skip;
}

fn flush_def(term: &mut Option<String>, body: &mut String, items: &mut Vec<(String, String)>) {
    let Some(term_text) = term.take() else {
        return;
    };
    items.push((term_text, body.trim().to_string()));
    body.clear();
}

fn figcaption(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(start) = lower.find("<figcaption") else {
        return String::new();
    };
    let Some(rel) = html[start..].find('>') else {
        return String::new();
    };
    let content = start + rel + 1;
    let Some(close) = lower[content..].find("</figcaption>") else {
        return String::new();
    };
    visible_text(&html[content..content + close])
}

fn first_tag(html: &str, name: &str) -> Option<String> {
    let mut index = 0;
    while let Some(tag) = next_tag(html, index) {
        if !tag.close && tag.name == name {
            return Some(html[tag.start..tag.end].to_string());
        }
        index = tag.end;
    }
    None
}

fn lift_blocks(source: &str) -> String {
    let lines = pieces(source);
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    let mut fence: Option<(char, usize)> = None;
    while index < lines.len() {
        let body = lines[index].body;
        if let Some((marker, count)) = fence {
            if closing_fence(body, marker, count) {
                fence = None;
            }
            out.push_str(lines[index].raw);
            index += 1;
            continue;
        }
        if let Some(open) = opening_fence(body) {
            fence = Some(open);
            out.push_str(lines[index].raw);
            index += 1;
            continue;
        }
        if pre_opens(body)
            && let Some(end) = pre_end(&lines, index)
        {
            let mut block = String::new();
            for line in &lines[index..=end] {
                block.push_str(line.raw);
            }
            if let Some(fenced) = pre_fence(&block) {
                out.push_str(&fenced);
                index = end + 1;
                continue;
            }
        }
        if hr_line(body) {
            out.push_str("---");
            if lines[index].raw.ends_with('\n') {
                out.push('\n');
            }
            index += 1;
            continue;
        }
        out.push_str(lines[index].raw);
        index += 1;
    }
    out
}

fn lift_inline(source: &str) -> String {
    let source = map_gaps(source, rename_tags);
    let source = map_gaps(&source, replace_quotes);
    let source = map_gaps(&source, replace_images);
    let source = map_gaps(&source, replace_inline_rules);
    let source = map_gaps(&source, |gap| {
        replace_pairs(gap, &["kbd", "sup", "sub", "small"], widget_link)
    });
    let source = map_gaps(&source, |gap| {
        replace_pairs(gap, &["code", "samp"], code_span)
    });
    map_gaps(&source, |gap| replace_pairs(gap, &["a"], anchor_link))
}

fn map_gaps(source: &str, mut edit: impl FnMut(&str) -> String) -> String {
    let protected = protected_ranges(source);
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    for (start, end) in protected {
        if cursor < start {
            out.push_str(&edit(&source[cursor..start]));
        }
        out.push_str(&source[start..end]);
        cursor = end;
    }
    if cursor < source.len() {
        out.push_str(&edit(&source[cursor..]));
    }
    out
}

fn protected_ranges(source: &str) -> Vec<(usize, usize)> {
    let mut ranges = fence_ranges(source);
    ranges.extend(comment_ranges(source, &ranges));
    ranges.extend(code_ranges(source, &ranges));
    ranges.sort_by_key(|range| range.0);
    ranges
}

fn covered(ranges: &[(usize, usize)], index: usize) -> bool {
    let pos = ranges.partition_point(|range| range.0 <= index);
    pos > 0 && index < ranges[pos - 1].1
}

fn fence_ranges(source: &str) -> Vec<(usize, usize)> {
    let lines = pieces(source);
    let mut ranges = Vec::new();
    let mut index = 0;
    let mut fence: Option<(char, usize, usize)> = None;
    while index < lines.len() {
        let body = lines[index].body;
        if let Some((marker, count, start)) = fence {
            if closing_fence(body, marker, count) {
                ranges.push((start, lines[index].end));
                fence = None;
            }
            index += 1;
            continue;
        }
        if let Some((marker, count)) = opening_fence(body) {
            fence = Some((marker, count, lines[index].start));
        }
        index += 1;
    }
    if let Some((_, _, start)) = fence {
        ranges.push((start, source.len()));
    }
    ranges
}

fn comment_ranges(source: &str, fences: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index + 4 <= bytes.len() {
        if covered(fences, index) {
            index += 1;
            continue;
        }
        if &bytes[index..index + 4] == b"<!--" {
            let start = index;
            index += 4;
            let mut end = None;
            while index + 3 <= bytes.len() {
                if &bytes[index..index + 3] == b"-->" {
                    end = Some(index + 3);
                    break;
                }
                index += 1;
            }
            let end = end.unwrap_or(source.len());
            ranges.push((start, end));
            index = end;
            continue;
        }
        index += 1;
    }
    ranges
}

fn code_ranges(source: &str, earlier: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if covered(earlier, index) || covered(&ranges, index) {
            index += 1;
            continue;
        }
        if bytes[index] != b'`' {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index] == b'`' {
            index += 1;
        }
        let count = index - start;
        let mut close = index;
        let mut found = None;
        while close < bytes.len() {
            if covered(earlier, close) {
                close += 1;
                continue;
            }
            if bytes[close] == b'`' {
                let run_start = close;
                while close < bytes.len() && bytes[close] == b'`' {
                    close += 1;
                }
                if close - run_start == count {
                    found = Some(close);
                    break;
                }
                continue;
            }
            close += 1;
        }
        if let Some(end) = found {
            ranges.push((start, end));
            index = end;
        }
    }
    ranges
}

fn rename_tags(source: &str) -> String {
    const RENAMES: &[(&str, &str)] = &[
        ("cite", "em"),
        ("var", "em"),
        ("dfn", "em"),
        ("abbr", "u"),
        ("ins", "u"),
    ];
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while let Some(tag) = next_tag(source, index) {
        out.push_str(&source[index..tag.start]);
        let replacement = RENAMES
            .iter()
            .find(|(name, _)| *name == tag.name)
            .map(|(_, next)| *next);
        if let Some(next) = replacement {
            let name_at = tag.start + 1 + usize::from(tag.close);
            out.push_str(&source[tag.start..name_at]);
            out.push_str(next);
            let name_end = name_at + tag.name.len();
            out.push_str(&source[name_end..tag.end]);
        } else {
            out.push_str(&source[tag.start..tag.end]);
        }
        index = tag.end;
    }
    out.push_str(&source[index..]);
    out
}

fn replace_quotes(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while let Some(tag) = next_tag(source, index) {
        out.push_str(&source[index..tag.start]);
        if tag.name == "q" {
            out.push(if tag.close { '”' } else { '“' });
        } else {
            out.push_str(&source[tag.start..tag.end]);
        }
        index = tag.end;
    }
    out.push_str(&source[index..]);
    out
}

fn replace_images(source: &str) -> String {
    replace_void(source, "img", |tag| {
        let src = attr_value(tag, "src")?;
        if src.is_empty() {
            return None;
        }
        let alt = attr_value(tag, "alt").unwrap_or_default();
        let dest = markdown_dest(&src)?;
        Some(format!("![{}]({dest})", escape_label(&alt)))
    })
}

fn replace_inline_rules(source: &str) -> String {
    replace_void(source, "hr", |_| Some("\n\n---\n\n".to_string()))
}

fn replace_void(source: &str, name: &str, mut edit: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while let Some(tag) = next_tag(source, index) {
        out.push_str(&source[index..tag.start]);
        let raw = &source[tag.start..tag.end];
        if !tag.close
            && tag.name == name
            && let Some(next) = edit(raw)
        {
            out.push_str(&next);
        } else {
            out.push_str(raw);
        }
        index = tag.end;
    }
    out.push_str(&source[index..]);
    out
}

fn replace_pairs(
    source: &str,
    names: &[&str],
    mut edit: impl FnMut(&str, &str) -> Option<String>,
) -> String {
    let mut source = source.to_string();
    let mut passes = 0;
    while passes < 4096 {
        let Some((start, end, next)) = take_pair(&source, names, &mut edit) else {
            break;
        };
        source.replace_range(start..end, &next);
        passes += 1;
    }
    source
}

fn take_pair(
    source: &str,
    names: &[&str],
    edit: &mut impl FnMut(&str, &str) -> Option<String>,
) -> Option<(usize, usize, String)> {
    let mut stack: Vec<OpenTag> = Vec::new();
    let mut index = 0;
    while let Some(tag) = next_tag(source, index) {
        let tracked = names.iter().any(|name| *name == tag.name);
        if tracked && !tag.close && !tag.void {
            stack.push(OpenTag {
                name: tag.name,
                start: tag.start,
                inner: tag.end,
            });
        } else if tracked
            && tag.close
            && let Some(pos) = stack.iter().rposition(|open| open.name == tag.name)
        {
            let open = stack.remove(pos);
            stack.truncate(pos);
            let raw_open = &source[open.start..open.inner];
            let inner = &source[open.inner..tag.start];
            if let Some(next) = edit(raw_open, inner) {
                return Some((open.start, tag.end, next));
            }
        }
        index = tag.end;
    }
    None
}

struct OpenTag {
    name: String,
    start: usize,
    inner: usize,
}

fn widget_link(open: &str, inner: &str) -> Option<String> {
    let name = tag_name_of(open)?;
    let label = visible_text(inner);
    if label.is_empty() {
        return None;
    }
    Some(format!("[{}]({HTML_WIDGET}{name})", escape_label(&label)))
}

fn code_span(_open: &str, inner: &str) -> Option<String> {
    if inner.contains('\n') {
        return None;
    }
    Some(inline_code(&decode_entities(inner)))
}

fn anchor_link(open: &str, inner: &str) -> Option<String> {
    let href = attr_value(open, "href")?;
    if href.is_empty() || inner.contains('[') || inner.contains(']') {
        return None;
    }
    let dest = markdown_dest(&href)?;
    let label = inner.trim();
    if label.is_empty() {
        return None;
    }
    Some(format!("[{label}]({dest})"))
}

fn inline_code(text: &str) -> String {
    let ticks = "`".repeat(longest_run(text, '`') + 1);
    if text.starts_with('`') || text.ends_with('`') || text.starts_with(' ') || text.ends_with(' ')
    {
        format!("{ticks} {text} {ticks}")
    } else {
        format!("{ticks}{text}{ticks}")
    }
}

fn escape_label(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace(']', "\\]")
        .replace('\n', " ")
}

fn markdown_dest(url: &str) -> Option<String> {
    if url
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '<' || ch == '>')
    {
        return None;
    }
    if url.contains('(') || url.contains(')') {
        Some(format!("<{url}>"))
    } else {
        Some(url.to_string())
    }
}

fn pre_fence(block: &str) -> Option<String> {
    let lower = block.to_ascii_lowercase();
    let open = lower.find("<pre")?;
    let tag_end = open + block[open..].find('>')?;
    let close = lower.rfind("</pre>")?;
    if close < tag_end {
        return None;
    }
    let open_tag = &block[open..=tag_end];
    let raw_inner = &block[tag_end + 1..close];
    let lang = code_lang(open_tag, raw_inner);
    let inner = strip_code_wrap(raw_inner);
    let mut text = decode_entities(inner);
    if text.starts_with('\n') {
        text.remove(0);
    }
    if text.ends_with('\n') {
        text.pop();
    }
    Some(fenced(lang.as_deref(), &text))
}

fn strip_code_wrap(inner: &str) -> &str {
    let trimmed = inner.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("<code") {
        return inner;
    }
    let Some(rel) = trimmed.find('>') else {
        return inner;
    };
    let Some(close) = lower.rfind("</code>") else {
        return inner;
    };
    if close < rel {
        return inner;
    }
    let lead = inner.len() - inner.trim_start().len();
    &inner[lead + rel + 1..lead + close]
}

fn code_lang(open_pre: &str, inner: &str) -> Option<String> {
    attr_value(open_pre, "class")
        .as_deref()
        .and_then(language_class)
        .or_else(|| {
            let trimmed = inner.trim_start();
            let lower = trimmed.to_ascii_lowercase();
            if !lower.starts_with("<code") {
                return None;
            }
            let end = trimmed.find('>')?;
            language_class(&attr_value(&trimmed[..=end], "class")?)
        })
}

fn language_class(class: &str) -> Option<String> {
    class.split_whitespace().find_map(|part| {
        let lang = part
            .strip_prefix("language-")
            .or_else(|| part.strip_prefix("lang-"))?;
        let lang = lang.trim();
        if lang.is_empty() || lang.contains(['`', ' ', '\n']) {
            None
        } else {
            Some(lang.to_string())
        }
    })
}

fn fenced(lang: Option<&str>, text: &str) -> String {
    let ticks = "`".repeat(longest_run(text, '`').max(2) + 1);
    let mut out = String::with_capacity(text.len() + ticks.len() * 2 + 8);
    out.push_str(&ticks);
    if let Some(lang) = lang {
        out.push_str(lang);
    }
    out.push('\n');
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&ticks);
    out.push('\n');
    out
}

struct Tag {
    start: usize,
    end: usize,
    close: bool,
    void: bool,
    name: String,
}

fn next_tag(source: &str, from: usize) -> Option<Tag> {
    let bytes = source.as_bytes();
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let start = index;
        let mut cursor = index + 1;
        if cursor < bytes.len() && bytes[cursor] == b'/' {
            cursor += 1;
        }
        if cursor >= bytes.len() || !bytes[cursor].is_ascii_alphabetic() {
            index += 1;
            continue;
        }
        let name_at = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_alphanumeric() {
            cursor += 1;
        }
        let name = source[name_at..cursor].to_ascii_lowercase();
        let close = bytes.get(start + 1) == Some(&b'/');
        let mut quoted = None;
        let mut void = false;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if let Some(quote) = quoted {
                if byte == quote {
                    quoted = None;
                }
                cursor += 1;
                continue;
            }
            if byte == b'"' || byte == b'\'' {
                quoted = Some(byte);
                cursor += 1;
                continue;
            }
            if byte == b'/' {
                void = true;
                cursor += 1;
                continue;
            }
            if byte == b'>' {
                cursor += 1;
                return Some(Tag {
                    start,
                    end: cursor,
                    close,
                    void: void && !close,
                    name,
                });
            }
            cursor += 1;
        }
        return None;
    }
    None
}

fn tag_name_of(open: &str) -> Option<String> {
    next_tag(open, 0).map(|tag| tag.name)
}

fn attr_value(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index] != b'<' {
        index += 1;
    }
    index += 1;
    if index < bytes.len() && bytes[index] == b'/' {
        index += 1;
    }
    while index < bytes.len() && bytes[index].is_ascii_alphanumeric() {
        index += 1;
    }
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] == b'>' || bytes[index] == b'/' {
            break;
        }
        let name_at = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_alphanumeric()
                || bytes[index] == b'-'
                || bytes[index] == b':')
        {
            index += 1;
        }
        let found = tag[name_at..index].eq_ignore_ascii_case(name);
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            if found {
                return Some(String::new());
            }
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let value = if bytes[index] == b'"' || bytes[index] == b'\'' {
            let quote = bytes[index];
            index += 1;
            let value_at = index;
            while index < bytes.len() && bytes[index] != quote {
                index += 1;
            }
            let value = &tag[value_at..index];
            if index < bytes.len() {
                index += 1;
            }
            value
        } else {
            let value_at = index;
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() && bytes[index] != b'>'
            {
                index += 1;
            }
            &tag[value_at..index]
        };
        if found {
            return Some(decode_entities(value));
        }
    }
    None
}

fn outer_tag(html: &str, name: &str) -> bool {
    let trimmed = html.trim_start();
    let Some(tag) = next_tag(trimmed, 0) else {
        return false;
    };
    tag.start == 0 && !tag.close && tag.name == name
}

fn decode_entities(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'&'
            && let Some((ch, next)) = entity(text, index)
        {
            out.push(ch);
            index = next;
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

fn entity(text: &str, start: usize) -> Option<(char, usize)> {
    let rest = text.get(start..)?;
    let end = rest.find(';')?;
    if end < 3 || end > 12 {
        return None;
    }
    let body = &rest[1..end];
    let next = start + end + 1;
    if let Some(number) = body.strip_prefix('#') {
        let code = if let Some(hex) = number.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            number.parse().ok()?
        };
        return Some((char::from_u32(code)?, next));
    }
    let ch = match body.to_ascii_lowercase().as_str() {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        _ => return None,
    };
    Some((ch, next))
}

struct SourcePiece<'a> {
    start: usize,
    end: usize,
    body: &'a str,
    raw: &'a str,
}

fn pieces(source: &str) -> Vec<SourcePiece<'_>> {
    let mut start = 0;
    let mut lines = Vec::new();
    for raw in source.split_inclusive('\n') {
        let end = start + raw.len();
        let body = raw.trim_end_matches(['\n', '\r']);
        lines.push(SourcePiece {
            start,
            end,
            body,
            raw: &source[start..end],
        });
        start = end;
    }
    lines
}

fn indented_code(source: &str) -> bool {
    let mut any = false;
    for line in source.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line
            .bytes()
            .take_while(|byte| *byte == b' ' || *byte == b'\t')
            .count();
        if indent < 4 {
            return false;
        }
        any = true;
    }
    any
}

fn pre_opens(line: &str) -> bool {
    let indent = leading_indent(line);
    if indent > 3 {
        return false;
    }
    let rest = &line[indent..];
    starts_tag(rest, "pre")
}

fn pre_end(lines: &[SourcePiece<'_>], start: usize) -> Option<usize> {
    for (index, line) in lines.iter().enumerate().skip(start) {
        if has_close(line.body, "pre") && (index != start || close_after_open(line.body, "pre")) {
            return Some(index);
        }
    }
    None
}

fn close_after_open(line: &str, name: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let open = format!("<{name}");
    let close = format!("</{name}");
    let Some(at) = lower.find(&open) else {
        return false;
    };
    lower[at + open.len()..].contains(&close)
}

fn hr_line(line: &str) -> bool {
    let indent = leading_indent(line);
    if indent > 3 {
        return false;
    }
    let rest = line[indent..].trim();
    let Some(tag) = next_tag(rest, 0) else {
        return false;
    };
    tag.start == 0 && tag.end == rest.len() && tag.name == "hr" && !tag.close
}

fn has_close(line: &str, name: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let needle = format!("</{name}");
    let mut start = 0;
    while let Some(at) = lower[start..].find(&needle) {
        let index = start + at;
        let after = index + needle.len();
        let boundary = lower.as_bytes().get(after).copied().unwrap_or(b' ');
        if !boundary.is_ascii_alphanumeric() {
            return true;
        }
        start = index + 1;
    }
    false
}

fn starts_tag(text: &str, name: &str) -> bool {
    let tag = format!("<{name}");
    if text.len() < tag.len() || !text[..tag.len()].eq_ignore_ascii_case(&tag) {
        return false;
    }
    let boundary = text.as_bytes().get(tag.len()).copied().unwrap_or(b' ');
    !boundary.is_ascii_alphanumeric()
}

fn leading_indent(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
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

fn longest_run(text: &str, needle: char) -> usize {
    let mut run = 0;
    let mut longest = 0;
    for ch in text.chars() {
        if ch == needle {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kbd_becomes_a_private_link() {
        assert_eq!(
            style_html("Press <kbd>Ctrl</kbd> now.\n"),
            "Press [Ctrl](#rb-html-kbd) now.\n"
        );
    }

    #[test]
    fn a_bracket_in_a_key_stays_inside_the_label() {
        assert_eq!(style_html("<kbd>a]b</kbd>"), "[a\\]b](#rb-html-kbd)");
    }

    #[test]
    fn inline_code_tag_becomes_a_code_span() {
        assert_eq!(style_html("<code>a*b</code>"), "`a*b`");
    }

    #[test]
    fn pre_becomes_a_fenced_block() {
        let source = "<pre><code class=\"language-py\">\nprint(1)\n</code></pre>\n";
        assert_eq!(style_html(source), "```py\nprint(1)\n```\n");
    }

    #[test]
    fn entities_in_pre_become_characters() {
        let source = "<pre>&lt;tag&gt;</pre>\n";
        assert_eq!(style_html(source), "```\n<tag>\n```\n");
    }

    #[test]
    fn a_rule_tag_becomes_a_thematic_break() {
        assert_eq!(style_html("<hr/>\n"), "---\n");
        assert_eq!(style_html("A<hr>B"), "A\n\n---\n\nB");
    }

    #[test]
    fn cite_and_quote_use_italic_and_quotes() {
        assert_eq!(style_html("<cite>Book</cite>"), "<em>Book</em>");
        assert_eq!(style_html("<q>hi</q>"), "“hi”");
    }

    #[test]
    fn a_link_and_an_image_become_markdown() {
        assert_eq!(
            style_html("<a href=\"https://example.com\">Ex</a>"),
            "[Ex](https://example.com)"
        );
        assert_eq!(style_html("<img src=\"/a.png\" alt=\"A\">"), "![A](/a.png)");
    }

    #[test]
    fn code_keeps_the_tags_inside_it() {
        let fenced = "```\n<kbd>Ctrl</kbd>\n```\n";
        assert_eq!(style_html(fenced), fenced);
        let inline = "`<kbd>Ctrl</kbd>`";
        assert_eq!(style_html(inline), inline);
        let indented = "    <kbd>Ctrl</kbd>\n";
        assert_eq!(style_html(indented), indented);
        let comment = "<!-- <kbd>x</kbd> -->";
        assert_eq!(style_html(comment), comment);
    }

    #[test]
    fn a_definition_list_keeps_each_term() {
        let html = "<dl><dt>Term</dt><dd>Means <em>this</em>.</dd></dl>";
        let items = def_list(html).expect("list");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "Term");
        assert_eq!(items[0].1, "Means <em>this</em>.");
    }

    #[test]
    fn a_figure_uses_the_caption() {
        let html =
            "<figure><img src=\"/a.png\" alt=\"Alt\"><figcaption>Caption</figcaption></figure>";
        assert_eq!(
            figure_html(html),
            Some(("/a.png".to_string(), "Caption".to_string()))
        );
    }
}
