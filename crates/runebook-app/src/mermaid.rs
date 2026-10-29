//! Render a `mermaid` code fence as an SVG image.
//!
//! The fence text is untrusted. `mmdc` receives a temp file, not a shell command.
//! A failure message never includes a filesystem path.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::component::text::{MarkdownNode, MarkdownParseContext, TextView, markdown_ast};
use gpui_kit::component::{ActiveTheme, ThemeMode};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Image, ImageFormat, SharedString, Window, div, img, px};

const MAX_SOURCE: usize = 32 * 1024;
const MAX_SVG: usize = 2 * 1024 * 1024;
const RENDER_LIMIT: Duration = Duration::from_secs(20);
const CONFIG: &str = r#"{"securityLevel":"strict","htmlLabels":false}"#;

enum Slot {
    Pending,
    Ready(Arc<Image>),
    Failed(SharedString),
}

enum Shown {
    Pending,
    Ready(Arc<Image>),
    Failed(SharedString),
}

/// Markdown view that draws `mermaid` fences as diagrams.
pub(crate) fn view(
    id: impl Into<gpui_kit::ElementId>,
    source: impl Into<SharedString>,
    cx: &App,
) -> TextView {
    crate::markdown_view::view(id, source, None, false, cx)
}

pub(crate) fn parse_block(
    node: &markdown_ast::Node,
    _: &MarkdownParseContext,
) -> Option<MarkdownNode> {
    let source = diagram_source(node)?;
    Some(
        MarkdownNode::new("mermaid", source.to_string())
            .text("Diagram")
            .markdown(format!("```mermaid\n{source}\n```")),
    )
}

fn diagram_source(node: &markdown_ast::Node) -> Option<&str> {
    let markdown_ast::Node::Code(code) = node else {
        return None;
    };
    let lang = code.lang.as_deref()?.trim();
    if !lang.eq_ignore_ascii_case("mermaid") {
        return None;
    }
    Some(code.value.as_str())
}

pub(crate) fn render_block(node: &MarkdownNode, _: &mut Window, cx: &mut App) -> AnyElement {
    let Some(source) = node.data::<String>() else {
        return note(crate::locale::text("mermaid.failed"));
    };
    let dark = cx.theme().mode == ThemeMode::Dark;
    match shown(cx, source, dark) {
        Shown::Pending => note(crate::locale::text("mermaid.pending")),
        Shown::Failed(message) => note(message),
        Shown::Ready(image) => div()
            .w_full()
            .min_w(px(0.))
            .child(img(image).w_full().max_h(px(420.)))
            .into_any_element(),
    }
}

fn note(message: impl Into<SharedString>) -> AnyElement {
    div().text_base().child(message.into()).into_any_element()
}

fn shown(cx: &mut App, source: &str, dark: bool) -> Shown {
    if source.len() > MAX_SOURCE {
        return Shown::Failed(SharedString::from(crate::locale::text("mermaid.large")));
    }
    if source.trim().is_empty() {
        return Shown::Failed(SharedString::from(crate::locale::text("mermaid.empty")));
    }
    let key = format!("{dark}\n{source}");
    {
        let mut slots = slots().lock().unwrap_or_else(|err| err.into_inner());
        match slots.get(&key) {
            Some(Slot::Pending) => return Shown::Pending,
            Some(Slot::Ready(image)) => return Shown::Ready(Arc::clone(image)),
            Some(Slot::Failed(message)) => return Shown::Failed(message.clone()),
            None => {
                if slots.len() >= 64 {
                    slots.clear();
                }
                slots.insert(key.clone(), Slot::Pending);
            }
        }
    }
    let source = source.to_string();
    cx.defer(move |cx| {
        cx.spawn(async move |cx| {
            let rendered = cx
                .background_executor()
                .spawn(async move { render_svg(&source, dark) })
                .await;
            cx.update(|cx| {
                let slot = match rendered {
                    Ok(bytes) => Slot::Ready(Arc::new(Image::from_bytes(ImageFormat::Svg, bytes))),
                    Err(message) => Slot::Failed(SharedString::from(message)),
                };
                slots()
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .insert(key, slot);
                cx.refresh_windows();
            });
        })
        .detach();
    });
    Shown::Pending
}

fn slots() -> &'static Mutex<std::collections::HashMap<String, Slot>> {
    static SLOTS: OnceLock<Mutex<std::collections::HashMap<String, Slot>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn render_svg(source: &str, dark: bool) -> Result<Vec<u8>, String> {
    let temp = TempDir::create()?;
    let input = temp.path.join("diagram.mmd");
    let output = temp.path.join("diagram.svg");
    let config = temp.path.join("config.json");
    std::fs::write(&input, source)
        .map_err(|_| crate::locale::text("mermaid.failed").to_string())?;
    std::fs::write(&config, CONFIG)
        .map_err(|_| crate::locale::text("mermaid.failed").to_string())?;
    let puppeteer = temp.path.join("puppeteer.json");
    let theme = if dark { "dark" } else { "default" };
    let mut command = Command::new("mmdc");
    command
        .current_dir(&temp.path)
        .arg("-q")
        .arg("-t")
        .arg(theme)
        .arg("-b")
        .arg("transparent")
        .arg("-c")
        .arg(&config);
    if let Some(browser) = system_browser() {
        let puppeteer_json = format!(r#"{{"executablePath":"{browser}"}}"#);
        std::fs::write(&puppeteer, puppeteer_json)
            .map_err(|_| crate::locale::text("mermaid.failed").to_string())?;
        command.arg("-p").arg(&puppeteer);
    }
    let mut child = command
        .arg("-i")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| {
            if err.kind() == ErrorKind::NotFound {
                crate::locale::text("mermaid.install").to_string()
            } else {
                crate::locale::text("mermaid.start").to_string()
            }
        })?;
    let status = wait_for(&mut child, RENDER_LIMIT)?;
    if !status.success() {
        return Err(crate::locale::text("mermaid.failed").to_string());
    }
    let bytes =
        std::fs::read(&output).map_err(|_| crate::locale::text("mermaid.failed").to_string())?;
    if bytes.len() > MAX_SVG || !looks_like_svg(&bytes) {
        return Err(crate::locale::text("mermaid.failed").to_string());
    }
    Ok(bytes)
}

fn wait_for(child: &mut std::process::Child, limit: Duration) -> Result<ExitStatus, String> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(crate::locale::text("mermaid.timeout").to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(_) => return Err(crate::locale::text("mermaid.failed").to_string()),
        }
    }
}

fn system_browser() -> Option<&'static str> {
    const CANDIDATES: [&str; 5] = [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
    ];
    CANDIDATES
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    let text = String::from_utf8_lossy(head);
    let trimmed = text.trim_start().to_ascii_lowercase();
    if trimmed.contains("<html") {
        return false;
    }
    trimmed.starts_with("<svg") || trimmed.starts_with("<?xml")
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn create() -> Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("runebook-mermaid-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&path)
            .map_err(|_| crate::locale::text("mermaid.failed").to_string())?;
        Ok(Self { path })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(lang: Option<&str>, value: &str) -> markdown_ast::Node {
        markdown_ast::Node::Code(markdown_ast::Code {
            value: value.to_string(),
            position: None,
            lang: lang.map(str::to_string),
            meta: None,
        })
    }

    #[test]
    fn mermaid_fence_keeps_the_diagram_source() {
        let node = code(Some("Mermaid"), "graph TD\nA-->B");
        assert_eq!(diagram_source(&node), Some("graph TD\nA-->B"));
    }

    #[test]
    fn other_fences_stay_code() {
        let node = code(Some("rust"), "fn main() {}");
        assert_eq!(diagram_source(&node), None);
    }

    #[test]
    fn svg_check_accepts_svg_and_rejects_html() {
        assert!(looks_like_svg(br#"<?xml version="1.0"?><svg></svg>"#));
        assert!(looks_like_svg(
            b"  <svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"
        ));
        assert!(!looks_like_svg(b"<html><svg></svg></html>"));
        assert!(!looks_like_svg(b"not a diagram"));
    }

    #[test]
    fn system_browser_is_a_known_path() {
        let Some(path) = system_browser() else {
            return;
        };
        assert!(path.starts_with("/usr/bin/") || path.starts_with("/snap/bin/"));
    }
}
