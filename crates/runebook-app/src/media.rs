//! A preview row whose only content is a vault video or audio link.

/// A video or audio file named by a preview row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipKind {
    Video,
    Audio,
}

/// A preview row whose only content is a media link.
pub(crate) struct MediaLink {
    pub label: String,
    pub href: String,
    pub kind: ClipKind,
}

/// Parse a row that is only `[label](file)` or `[label](<file>)`.
pub(crate) fn media_link(row: &str) -> Option<MediaLink> {
    let row = row.trim();
    let rest = row.strip_prefix('[')?;
    let (label, rest) = rest.split_once(']')?;
    let rest = rest.strip_prefix('(')?;
    let (href, rest) = split_href(rest)?;
    if !rest.is_empty() {
        return None;
    }
    let label = label.trim();
    let href = href.trim();
    if label.is_empty() || href.is_empty() || href.contains(['\n', '\r']) {
        return None;
    }
    let kind = clip_kind(href)?;
    Some(MediaLink {
        label: label.to_string(),
        href: href.to_string(),
        kind,
    })
}

fn split_href(rest: &str) -> Option<(&str, &str)> {
    if let Some(rest) = rest.strip_prefix('<') {
        let (href, rest) = rest.split_once('>')?;
        let rest = rest.strip_prefix(')')?;
        return Some((href, rest));
    }
    let (href, rest) = rest.split_once(')')?;
    Some((href, rest))
}

fn clip_kind(href: &str) -> Option<ClipKind> {
    let path = href.split(['?', '#']).next().unwrap_or(href);
    let (_, ext) = path.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "mp4" | "webm" | "mkv" | "mov" => Some(ClipKind::Video),
        "mp3" | "wav" | "ogg" | "m4a" | "flac" => Some(ClipKind::Audio),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_recording_link_is_video() {
        let link = media_link("[screen recording](day-1.mp4)").expect("link");
        assert_eq!(link.label, "screen recording");
        assert_eq!(link.href, "day-1.mp4");
        assert_eq!(link.kind, ClipKind::Video);
    }

    #[test]
    fn angle_bracket_link_is_video() {
        let link = media_link("[screen recording](<my day.mp4>)").expect("link");
        assert_eq!(link.href, "my day.mp4");
        assert_eq!(link.kind, ClipKind::Video);
    }

    #[test]
    fn mp3_link_is_audio() {
        let link = media_link("[lecture](talk.mp3)").expect("link");
        assert_eq!(link.kind, ClipKind::Audio);
    }

    #[test]
    fn a_normal_paragraph_is_not_a_clip() {
        assert!(media_link("A normal paragraph").is_none());
        assert!(media_link("See [screen recording](day-1.mp4) later").is_none());
        assert!(media_link("[shot](photo.png)").is_none());
    }
}
