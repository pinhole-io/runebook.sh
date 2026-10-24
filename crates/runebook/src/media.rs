//! Media files that live beside notes.

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};
use crate::note::{NoteId, vault_join};

const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "webm", "mkv", "mov", "mp3", "wav", "ogg", "m4a", "flac", "png", "jpg", "jpeg", "gif",
    "webp",
];

/// `true` when `url` should open outside the vault.
///
/// Browser and mail links stay external. A relative path and a `file:` URL do not.
pub fn is_external_url(url: &str) -> bool {
    let url = url.trim();
    if url.starts_with("//") {
        return true;
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme == "mailto" {
        return true;
    }
    matches!(scheme.as_str(), "http" | "https") && rest.starts_with("//")
}

pub(crate) fn is_media_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            MEDIA_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(ext))
        })
}

pub(crate) fn resolve(root: &Path, note: &NoteId, href: &str) -> Result<PathBuf> {
    let href = href.trim();
    if href.is_empty() || is_external_url(href) {
        return Err(Error::NotMedia {
            name: href.to_string(),
        });
    }
    let without_fragment = href.split_once('#').map(|(path, _)| path).unwrap_or(href);
    let decoded = decode_percent(without_fragment).ok_or_else(|| Error::NotMedia {
        name: href.to_string(),
    })?;
    let path = if let Some(file_path) = file_url_path(&decoded) {
        absolute_in_vault(root, Path::new(&file_path))?
    } else if Path::new(&decoded).is_absolute() {
        absolute_in_vault(root, Path::new(&decoded))?
    } else {
        relative_in_vault(root, note, &decoded)?
    };
    if !is_media_path(&path) {
        return Err(Error::NotMedia {
            name: href.to_string(),
        });
    }
    ensure_real_file(root, &path)?;
    Ok(path)
}

fn relative_in_vault(root: &Path, note: &NoteId, href: &str) -> Result<PathBuf> {
    let mut rel = note.parent().relative_path();
    for component in Path::new(href).components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| Error::NotMedia {
                    name: href.to_string(),
                })?;
                if part.starts_with('.') || part.contains('\0') {
                    return Err(Error::NotMedia {
                        name: href.to_string(),
                    });
                }
                rel.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::PathEscape {
                    path: PathBuf::from(href),
                });
            }
        }
    }
    let path = vault_join(root, &rel)?;
    if !path.starts_with(root) {
        return Err(Error::PathEscape { path });
    }
    Ok(path)
}

fn absolute_in_vault(root: &Path, candidate: &Path) -> Result<PathBuf> {
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(Error::PathEscape {
            path: candidate.to_path_buf(),
        });
    }
    if !candidate.starts_with(root) {
        return Err(Error::PathEscape {
            path: candidate.to_path_buf(),
        });
    }
    Ok(candidate.to_path_buf())
}

fn ensure_real_file(root: &Path, path: &Path) -> Result<()> {
    let rel = path.strip_prefix(root).map_err(|_| Error::PathEscape {
        path: path.to_path_buf(),
    })?;
    let mut current = root.to_path_buf();
    if rel.as_os_str().is_empty() {
        return Err(Error::NotMedia {
            name: path.display().to_string(),
        });
    }
    let components: Vec<_> = rel.components().collect();
    for (index, component) in components.iter().enumerate() {
        let last = index + 1 == components.len();
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::Symlink { path: current });
            }
            Ok(meta) if last && !meta.is_file() => {
                return Err(Error::NotMedia {
                    name: path.display().to_string(),
                });
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::MediaNotFound {
                    path: path.to_path_buf(),
                });
            }
            Err(source) => {
                return Err(Error::Io {
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn file_url_path(href: &str) -> Option<String> {
    let rest = href.strip_prefix("file:")?;
    let rest = rest.strip_prefix("//localhost").unwrap_or(rest);
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

fn decode_percent(text: &str) -> Option<String> {
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
    use super::is_external_url;

    #[test]
    fn external_urls_stay_outside_the_vault() {
        assert!(is_external_url("https://example.com/a.mp4"));
        assert!(is_external_url("http://example.com"));
        assert!(is_external_url("mailto:a@example.com"));
        assert!(is_external_url("//example.com/a.png"));
        assert!(!is_external_url("day.mp4"));
        assert!(!is_external_url("file:///tmp/day.webm"));
        assert!(!is_external_url("clips/song.mp3"));
    }
}
