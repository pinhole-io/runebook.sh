//! Files parked on the Archivist composer.
//!
//! An image and a PDF go to the model as parts.
//! UTF-8 text stays in the user message.
//! A failure names the file and does not include the bytes.

use std::path::{Path, PathBuf};

use superglue::openai::{ChatMessage, ContentPart, MessageContent};

/// Most files one send accepts.
pub(crate) const MAX_FILES: usize = 8;
/// Largest file one send accepts.
pub(crate) const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// A path waiting for the next send.
pub(crate) struct ParkedFile {
    pub path: PathBuf,
    pub name: String,
}

/// Bytes read from one parked file.
pub(crate) struct LoadedFile {
    pub name: String,
    bytes: Vec<u8>,
}

enum Kind {
    Image,
    Pdf,
    Text,
}

/// The model message and the transcript line for one send.
pub(crate) fn prepare(
    instruction: &str,
    paths: &[PathBuf],
) -> Result<(String, ChatMessage), String> {
    let loaded = load_paths(paths)?;
    let names: Vec<String> = loaded.iter().map(|file| file.name.clone()).collect();
    let message = user_message(instruction, &loaded)?;
    Ok((display_line(instruction, &names), message))
}

/// Transcript text. File bytes stay out of this line.
pub(crate) fn display_line(instruction: &str, names: &[String]) -> String {
    let instruction = instruction.trim();
    if names.is_empty() {
        return instruction.to_string();
    }
    let list = names.join(", ");
    if instruction.is_empty() {
        format!("Files: {list}")
    } else {
        format!("{instruction}\n\nFiles: {list}")
    }
}

/// One user message. Images, then PDFs, then text files.
pub(crate) fn user_message(instruction: &str, files: &[LoadedFile]) -> Result<ChatMessage, String> {
    if files.len() > MAX_FILES {
        return Err("One send accepts at most 8 files.".to_string());
    }
    let mut images = Vec::new();
    let mut pdfs = Vec::new();
    let mut texts = Vec::new();
    for file in files {
        match classify(file)? {
            Kind::Image => images.push(file),
            Kind::Pdf => pdfs.push(file),
            Kind::Text => texts.push(file),
        }
    }
    let mut parts = Vec::new();
    let instruction = instruction.trim();
    if instruction.is_empty() {
        parts.push(ContentPart::Text {
            text: "Do not change a file until the user asks.".to_string(),
        });
    } else {
        parts.push(ContentPart::Text {
            text: instruction.to_string(),
        });
    }
    for file in images {
        parts.push(image_part(file)?);
    }
    for file in pdfs {
        parts.push(pdf_part(file)?);
    }
    for file in texts {
        let text = std::str::from_utf8(&file.bytes)
            .map_err(|_| format!("{} is not UTF-8 text.", file.name))?;
        parts.push(ContentPart::Text {
            text: format!("File `{}`.\n\n{text}", file.name),
        });
    }
    let mut message = ChatMessage::default();
    message.role = "user".to_string();
    message.content = Some(MessageContent::Parts(parts));
    Ok(message)
}

/// File name shown on the card and in errors.
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("file")
        .to_string()
}

fn load_paths(paths: &[PathBuf]) -> Result<Vec<LoadedFile>, String> {
    if paths.len() > MAX_FILES {
        return Err("One send accepts at most 8 files.".to_string());
    }
    let mut loaded = Vec::with_capacity(paths.len());
    for path in paths {
        loaded.push(load_one(path)?);
    }
    Ok(loaded)
}

fn load_one(path: &Path) -> Result<LoadedFile, String> {
    let name = file_name(path);
    if kind(&name).is_none() {
        return Err(format!("{name} is not a supported file."));
    }
    let meta = std::fs::metadata(path).map_err(|_| format!("Cannot read {name}."))?;
    if !meta.is_file() {
        return Err(format!("{name} is not a supported file."));
    }
    if meta.len() > MAX_BYTES {
        return Err(format!("{name} is larger than 8 MiB."));
    }
    let bytes = std::fs::read(path).map_err(|_| format!("Cannot read {name}."))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{name} is larger than 8 MiB."));
    }
    Ok(LoadedFile { name, bytes })
}

fn classify(file: &LoadedFile) -> Result<Kind, String> {
    let kind = kind(&file.name).ok_or_else(|| format!("{} is not a supported file.", file.name))?;
    if file.bytes.is_empty() {
        return Err(format!("{} is empty.", file.name));
    }
    if file.bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{} is larger than 8 MiB.", file.name));
    }
    Ok(kind)
}

fn kind(name: &str) -> Option<Kind> {
    let lower = name.to_ascii_lowercase();
    if superglue::images::is_image_filename(&lower) {
        Some(Kind::Image)
    } else if lower.ends_with(".pdf") {
        Some(Kind::Pdf)
    } else if lower.ends_with(".txt")
        || lower.ends_with(".md")
        || lower.ends_with(".csv")
        || lower.ends_with(".json")
        || lower.ends_with(".html")
    {
        Some(Kind::Text)
    } else {
        None
    }
}

fn image_part(file: &LoadedFile) -> Result<ContentPart, String> {
    let message = superglue::images::message_with_image_bytes(None, &file.name, &file.bytes);
    sole_part(message, &file.name)
}

fn pdf_part(file: &LoadedFile) -> Result<ContentPart, String> {
    let message = superglue::files::message_with_file_bytes(None, &file.name, &file.bytes);
    sole_part(message, &file.name)
}

fn sole_part(message: ChatMessage, name: &str) -> Result<ContentPart, String> {
    match message.content {
        Some(MessageContent::Parts(mut parts)) if parts.len() == 1 => {
            parts.pop().ok_or_else(|| format!("Cannot attach {name}."))
        }
        _ => Err(format!("Cannot attach {name}.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(name: &str, bytes: &[u8]) -> LoadedFile {
        LoadedFile {
            name: name.to_string(),
            bytes: bytes.to_vec(),
        }
    }

    fn parts(message: &ChatMessage) -> &[ContentPart] {
        match &message.content {
            Some(MessageContent::Parts(parts)) => parts,
            _ => panic!("the message has parts"),
        }
    }

    #[test]
    fn an_image_becomes_an_image_part() {
        let message = user_message("What is this?", &[loaded("pic.png", b"\x89PNG")]).expect("png");
        let parts = parts(&message);
        assert!(matches!(
            &parts[1],
            ContentPart::ImageUrl { image_url } if image_url.url.starts_with("data:image/png;base64,")
        ));
    }

    #[test]
    fn a_pdf_becomes_a_file_part() {
        let message = user_message("Read this.", &[loaded("note.pdf", b"%PDF-1.4")]).expect("pdf");
        let parts = parts(&message);
        match &parts[1] {
            ContentPart::File { file } => {
                let data = file.file_data.as_deref().expect("data");
                assert!(data.starts_with("data:application/pdf;base64,"));
                assert_eq!(file.filename.as_deref(), Some("note.pdf"));
            }
            _ => panic!("pdf part"),
        }
    }

    #[test]
    fn utf8_text_stays_text() {
        let message = user_message("Use this.", &[loaded("note.txt", b"hello")]).expect("text");
        let parts = parts(&message);
        match &parts[1] {
            ContentPart::Text { text } => {
                assert!(text.contains("note.txt"));
                assert!(text.contains("hello"));
            }
            _ => panic!("text part"),
        }
    }

    #[test]
    fn images_come_before_pdfs_and_text() {
        let message = user_message(
            "Sort these.",
            &[
                loaded("note.txt", b"hello"),
                loaded("note.pdf", b"%PDF"),
                loaded("pic.png", b"\x89PNG"),
            ],
        )
        .expect("files");
        let parts = parts(&message);
        assert!(matches!(parts[1], ContentPart::ImageUrl { .. }));
        assert!(matches!(parts[2], ContentPart::File { .. }));
        assert!(matches!(parts[3], ContentPart::Text { .. }));
    }

    #[test]
    fn a_large_file_fails() {
        let bytes = vec![0_u8; MAX_BYTES as usize + 1];
        let error = user_message("Look.", &[loaded("big.png", &bytes)]).expect_err("large");
        assert_eq!(error, "big.png is larger than 8 MiB.");
    }

    #[test]
    fn a_binary_file_fails() {
        let error = user_message("Look.", &[loaded("data.bin", b"secret-bytes")]).expect_err("bin");
        assert_eq!(error, "data.bin is not a supported file.");
        assert!(!error.contains("secret-bytes"));
    }

    #[test]
    fn text_that_is_not_utf8_fails() {
        let error = user_message("Look.", &[loaded("note.txt", b"\xff")]).expect_err("utf8");
        assert_eq!(error, "note.txt is not UTF-8 text.");
    }

    #[test]
    fn an_empty_instruction_tells_the_model_to_wait() {
        let message = user_message("", &[loaded("pic.png", b"\x89PNG")]).expect("wait");
        match &parts(&message)[0] {
            ContentPart::Text { text } => {
                assert_eq!(text, "Do not change a file until the user asks.");
            }
            _ => panic!("wait text"),
        }
    }

    #[test]
    fn the_display_line_names_the_file_and_skips_the_bytes() {
        let line = display_line("Summarize this.", &["note.txt".to_string()]);
        assert_eq!(line, "Summarize this.\n\nFiles: note.txt");
        assert!(!line.contains("hello"));
    }

    #[test]
    fn a_missing_file_names_the_file() {
        let error = match load_paths(&[PathBuf::from("/no/such/note.txt")]) {
            Ok(_) => panic!("a missing file must fail"),
            Err(error) => error,
        };
        assert_eq!(error, "Cannot read note.txt.");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_read() {
        let dir = std::env::temp_dir().join(format!("runebook-attach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let target = dir.join("note.txt");
        std::fs::write(&target, "hello").expect("write");
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        let loaded = load_paths(&[link]).expect("load");
        assert_eq!(loaded[0].bytes, b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
