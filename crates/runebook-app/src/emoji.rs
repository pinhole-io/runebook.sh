//! Replace a gemoji shortcode with its character.
//!
//! A shortcode inside a fence or inline code stays literal.

/// Replace `:name:` in prose. Fenced code and inline code stay literal.
pub(crate) fn expand(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut fence: Option<(char, usize)> = None;
    for line in source.split_inclusive('\n') {
        if let Some((marker, len)) = fence {
            out.push_str(line);
            if fence_close(line, marker, len) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = fence_open(line) {
            fence = Some(open);
            out.push_str(line);
            continue;
        }
        expand_line(line, &mut out);
    }
    out
}

fn expand_line(line: &str, out: &mut String) {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            let ticks = run_len(bytes, index, b'`');
            if let Some(end) = find_closer(bytes, index + ticks, ticks) {
                out.push_str(&line[index..end]);
                index = end;
                continue;
            }
        }
        if bytes[index] == b':'
            && let Some((emoji, len)) = shortcode_at(&line[index..])
        {
            out.push_str(emoji);
            index += len;
            continue;
        }
        let ch = line[index..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        index += ch.len_utf8();
    }
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

fn fence_open(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    let body = trimmed.trim_start_matches(' ');
    let indent = trimmed.len() - body.len();
    if indent > 3 {
        return None;
    }
    let marker = body.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let len = body.chars().take_while(|ch| *ch == marker).count();
    if len < 3 {
        return None;
    }
    let info = body[len..].trim();
    if marker == '`' && info.contains('`') {
        return None;
    }
    Some((marker, len))
}

fn fence_close(line: &str, marker: char, len: usize) -> bool {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    let body = trimmed.trim_start_matches(' ');
    let indent = trimmed.len() - body.len();
    if indent > 3 {
        return false;
    }
    let run = body.chars().take_while(|ch| *ch == marker).count();
    run >= len && body[run..].trim().is_empty()
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

#[cfg(test)]
mod tests {
    use super::expand;

    #[test]
    fn a_rocket_shortcode_becomes_the_character() {
        let text = expand("smile: :rocket:");
        assert!(text.contains('🚀'));
        assert!(!text.contains(":rocket:"));
    }

    #[test]
    fn a_fenced_block_keeps_a_shortcode_literal() {
        let source = "```\n:rocket:\n```\n";
        assert_eq!(expand(source), source);
    }

    #[test]
    fn inline_code_keeps_a_shortcode_literal() {
        let source = "see `:rocket:` here";
        assert_eq!(expand(source), source);
    }

    #[test]
    fn a_shortcode_after_a_fence_still_expands() {
        let text = expand("```\n:rocket:\n```\n:smile:");
        assert!(text.contains(":rocket:"));
        assert!(text.contains('😄'));
        assert!(!text.contains(":smile:"));
    }
}
