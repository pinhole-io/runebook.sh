//! The note preview page.
//!
//! One native web view is created with the window. Later notes replace the
//! body through a script. Row actions come back as small JSON messages.
//! A custom protocol serves vault files. The page never receives a filesystem path.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use gpui_kit::component::{ActiveTheme, ThemeMode};
use gpui_kit::{AnyElement, App, AppContext, Context, Entity, Hsla, IntoElement, Window};
use gpui_webview::WebView;
use runebook::NoteId;
use runebook::Vault;

use crate::locale;
use crate::media::{ClipKind, MediaLink};

use super::EditorPane;
use super::rows::{heading_needs_room, split_rows};

const IPC_LIMIT: usize = 64 * 1024;
const QUEUE_LIMIT: usize = 64;
const FILE_LIMIT: u64 = 8 * 1024 * 1024;
const URL_LIMIT: usize = 2048;
const TEXT_LIMIT: usize = 8 * 1024;
const ROW_LIMIT: usize = 100_000;
const BLOB_LIMIT: usize = 64;
const PREVIEW_PUSH_LIMIT: u8 = 80;

/// The note was pushed and the page has not confirmed it.
pub(super) static PREVIEW_WAIT: AtomicBool = AtomicBool::new(false);
/// The page was off the screen and has just been shown again.
pub(super) static PREVIEW_RESHOWN: AtomicBool = AtomicBool::new(false);

/// One message from the preview page.
#[derive(Debug)]
pub(super) enum PageEvent {
    Press { index: usize, shift: bool },
    Drag { index: usize },
    Release,
    Copy,
    Add,
    Rewrite,
    Apply { text: String },
    Cancel,
    Toggle { index: usize },
    Open { url: String },
    /// The page script is ready for the note.
    Live,
    /// The page applied this stamp.
    Ack { stamp: u64 },
}

/// Read one page message. An unknown kind or an oversized body is `None`.
pub(super) fn parse_event(raw: &str) -> Option<PageEvent> {
    if raw.is_empty() || raw.len() > IPC_LIMIT {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let kind = value.get("kind")?.as_str()?;
    match kind {
        "press" => Some(PageEvent::Press {
            index: row_index(&value)?,
            shift: value
                .get("shift")
                .and_then(|item| item.as_bool())
                .unwrap_or(false),
        }),
        "drag" => Some(PageEvent::Drag {
            index: row_index(&value)?,
        }),
        "release" => Some(PageEvent::Release),
        "copy" => Some(PageEvent::Copy),
        "add" => Some(PageEvent::Add),
        "rewrite" => Some(PageEvent::Rewrite),
        "apply" => {
            let text = value.get("text")?.as_str()?;
            if text.len() > TEXT_LIMIT {
                return None;
            }
            Some(PageEvent::Apply {
                text: text.to_string(),
            })
        }
        "cancel" => Some(PageEvent::Cancel),
        "toggle" => Some(PageEvent::Toggle {
            index: row_index(&value)?,
        }),
        "open" => {
            let url = value.get("url")?.as_str()?.trim();
            if url.is_empty() || url.len() > URL_LIMIT {
                return None;
            }
            Some(PageEvent::Open {
                url: url.to_string(),
            })
        }
        "live" => Some(PageEvent::Live),
        "ack" => {
            let stamp = value.get("stamp")?.as_str()?;
            let stamp = stamp.parse().ok()?;
            Some(PageEvent::Ack { stamp })
        }
        _ => None,
    }
}

fn row_index(value: &serde_json::Value) -> Option<usize> {
    let index = value.get("index")?.as_u64()?;
    if index > ROW_LIMIT as u64 {
        return None;
    }
    usize::try_from(index).ok()
}

enum Blob {
    Href(String),
    Remote(String),
    Svg(Arc<Vec<u8>>),
}

enum Resolved {
    File(PathBuf),
    Remote(String),
    Svg(Arc<Vec<u8>>),
}

/// Vault files the page may request, keyed by an opaque token.
pub(super) struct MediaBook {
    vault: Option<Vault>,
    note: Option<NoteId>,
    blobs: HashMap<String, Blob>,
}

impl MediaBook {
    fn new() -> Self {
        Self {
            vault: None,
            note: None,
            blobs: HashMap::new(),
        }
    }

    fn reset(&mut self, vault: Vault, note: Option<NoteId>) {
        self.vault = Some(vault);
        self.note = note;
        self.blobs.clear();
    }

    fn insert(&mut self, blob: Blob) -> Option<String> {
        if self.blobs.len() >= BLOB_LIMIT {
            return None;
        }
        let token = format!("t{}", self.blobs.len());
        self.blobs.insert(token.clone(), blob);
        Some(token)
    }

    fn resolve(&self, token: &str) -> Option<Resolved> {
        match self.blobs.get(token)? {
            Blob::Svg(bytes) => Some(Resolved::Svg(Arc::clone(bytes))),
            Blob::Remote(url) => Some(Resolved::Remote(url.clone())),
            Blob::Href(href) => {
                let vault = self.vault.as_ref()?;
                let note = self.note.as_ref()?;
                let path = vault.media_file(note, href).ok()?;
                Some(Resolved::File(path))
            }
        }
    }
}

enum DiagramFace {
    Wait,
    Fail(String),
    Svg(Arc<Vec<u8>>),
}

struct Payload {
    script: String,
    stamp: u64,
}

/// Apply queued page messages, then push the note when it changed.
pub(crate) fn flush_preview(
    root: &mut crate::Root,
    window: &mut Window,
    cx: &mut Context<crate::Root>,
) {
    let events = root
        .pane
        .as_mut()
        .map(|pane| pane.take_preview_events())
        .unwrap_or_default();
    let mut dirty = false;
    for event in events {
        if let PageEvent::Open { url } = event {
            root.follow_link(&url, window, cx);
            continue;
        }
        if let Some(pane) = root.pane.as_mut() {
            dirty |= pane.handle_preview_event(event, window, cx);
        }
    }
    if dirty {
        root.schedule_autosave(cx);
    }
    if let Some(pane) = root.pane.as_mut() {
        pane.sync_preview(cx);
    }
}

/// Create the preview page and listen for its messages.
pub(super) fn install(window: &mut Window, cx: &mut Context<crate::Root>) -> PreviewHost {
    let book = Arc::new(Mutex::new(MediaBook::new()));
    let ready = Arc::new(AtomicBool::new(false));
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let view = open_view(window, cx, Arc::clone(&book), Arc::clone(&ready), tx);
    if view.is_some() {
        listen(rx, cx);
    }
    PreviewHost { view, book, ready }
}

/// The native page. On Wayland it is its own surface. On X11 it is a child window.
#[derive(Clone)]
pub(super) enum PreviewSurface {
    Child(Entity<WebView>),
    #[cfg(target_os = "linux")]
    Overlay(Entity<super::overlay::NoteOverlay>),
}

pub(super) struct PreviewHost {
    pub view: Option<PreviewSurface>,
    pub book: Arc<Mutex<MediaBook>>,
    pub ready: Arc<AtomicBool>,
}

pub(super) fn preview_element(surface: &PreviewSurface) -> AnyElement {
    match surface {
        PreviewSurface::Child(page) => page.clone().into_any_element(),
        #[cfg(target_os = "linux")]
        PreviewSurface::Overlay(page) => page.clone().into_any_element(),
    }
}

impl PreviewSurface {
    fn set_showing(&self, showing: bool, rewrite: bool, cx: &mut Context<crate::Root>) {
        #[cfg(not(target_os = "linux"))]
        let _ = rewrite;
        match self {
            Self::Child(view) => {
                view.update(cx, |webview, _| {
                    if showing {
                        webview.show();
                    } else {
                        webview.hide();
                    }
                });
            }
            #[cfg(target_os = "linux")]
            Self::Overlay(view) => {
                view.update(cx, |overlay, _| overlay.set_showing(showing, rewrite));
            }
        }
    }

    fn eval(&self, script: &str, cx: &mut Context<crate::Root>) -> bool {
        match self {
            Self::Child(view) => view.update(cx, |webview, _| {
                webview.raw().evaluate_script(script).is_ok()
            }),
            #[cfg(target_os = "linux")]
            Self::Overlay(view) => view.update(cx, |overlay, _| overlay.eval(script)),
        }
    }
}

fn listen(
    mut rx: futures::channel::mpsc::UnboundedReceiver<String>,
    cx: &mut Context<crate::Root>,
) {
    cx.spawn(async move |this, cx| {
        while let Some(raw) = rx.next().await {
            this.update(cx, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.push_page_raw(&raw);
                }
                cx.notify();
            })
            .ok();
        }
    })
    .detach();
}

fn shell_builder(
    book: Arc<Mutex<MediaBook>>,
    ready: Arc<AtomicBool>,
    tx: futures::channel::mpsc::UnboundedSender<String>,
) -> wry::WebViewBuilder<'static> {
    let ipc_tx = tx.clone();
    let builder = wry::WebViewBuilder::new();
    // Linux loads the shell after the media scheme is marked local.
    #[cfg(not(target_os = "linux"))]
    let builder = builder.with_html(SHELL);
    builder
        .with_asynchronous_custom_protocol("rbmedia".into(), move |_id, request, responder| {
            let book = Arc::clone(&book);
            std::thread::spawn(move || {
                responder.respond(serve(&book, request.uri()));
            });
        })
        .with_ipc_handler(move |request| {
            let body = request.body();
            if body.len() > IPC_LIMIT {
                return;
            }
            let _ = ipc_tx.unbounded_send(body.clone());
        })
        .with_navigation_handler(|url| {
            let lower = url.to_ascii_lowercase();
            !(lower.starts_with("http:")
                || lower.starts_with("https:")
                || lower.starts_with("file:")
                || lower.starts_with("javascript:")
                || lower.starts_with("data:"))
        })
        .with_download_started_handler(|_, _| false)
        .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
        .with_on_page_load_handler(move |event, _url| {
            if matches!(event, wry::PageLoadEvent::Finished) {
                ready.store(true, Ordering::Release);
                let _ = tx.unbounded_send(String::new());
            }
        })
}

#[cfg(target_os = "linux")]
fn host_on_wayland(window: &Window) -> bool {
    // `Window::window_handle` is GPUI's own handle. The raw display handle is the trait.
    matches!(
        wry::raw_window_handle::HasWindowHandle::window_handle(window)
            .map(|handle| handle.as_raw()),
        Ok(wry::raw_window_handle::RawWindowHandle::Wayland(_))
    )
}

fn open_view(
    window: &mut Window,
    cx: &mut Context<crate::Root>,
    book: Arc<Mutex<MediaBook>>,
    ready: Arc<AtomicBool>,
    tx: futures::channel::mpsc::UnboundedSender<String>,
) -> Option<PreviewSurface> {
    let builder = shell_builder(book, ready, tx);
    #[cfg(target_os = "linux")]
    if host_on_wayland(window) {
        let dark = cx.theme().mode == ThemeMode::Dark;
        let opened = super::overlay::open(builder, dark, cx);
        if opened.is_none() {
            eprintln!("runebook: the note preview could not open");
        }
        return opened.map(PreviewSurface::Overlay);
    }
    let webview = match WebView::build(builder, window, cx) {
        Ok(webview) => webview,
        Err(_) => {
            eprintln!("runebook: the note preview could not open");
            return None;
        }
    };
    #[cfg(target_os = "linux")]
    allow_media_scheme(webview.raw());
    Some(PreviewSurface::Child(cx.new(|_| webview)))
}

/// Let the page load `rbmedia` images.
///
/// WebKit blocks a custom scheme until it is local and CORS-enabled.
/// A local scheme is closed to the unique origin of `load_html`, so the
/// shell is loaded with this scheme as its base.
#[cfg(target_os = "linux")]
pub(super) fn allow_media_scheme(webview: &wry::WebView) {
    use webkit2gtk::{SecurityManagerExt, WebContextExt, WebViewExt};
    use wry::WebViewExtUnix;

    let native = webview.webview();
    if let Some(manager) = native
        .web_context()
        .and_then(|context| context.security_manager())
    {
        manager.register_uri_scheme_as_local("rbmedia");
        manager.register_uri_scheme_as_cors_enabled("rbmedia");
    }
    reload_shell(webview);
}

/// Load the shell again. The script then asks for the note.
#[cfg(target_os = "linux")]
pub(super) fn reload_shell(webview: &wry::WebView) {
    use webkit2gtk::WebViewExt;
    use wry::WebViewExtUnix;

    webview.webview().load_html(SHELL, Some(SHELL_URL));
}

impl EditorPane {
    pub(super) fn preview_entity(&self) -> Option<PreviewSurface> {
        self.preview_view.clone()
    }

    fn preview_showing(&self) -> bool {
        !self.settings_open
            && !self.research.wizard_open()
            && !self.research_note_open()
            && self.note_view == super::NoteView::Preview
    }

    pub(super) fn push_page_raw(&mut self, raw: &str) {
        if raw.is_empty() {
            // A finished load replaced the document. Push the note again.
            self.preview_stamp = 0;
            self.preview_sent = 0;
            self.preview_retry = true;
            self.preview_tries = 0;
            PREVIEW_WAIT.store(true, Ordering::Relaxed);
            return;
        }
        if self.preview_events.len() >= QUEUE_LIMIT {
            return;
        }
        if let Some(event) = parse_event(raw) {
            self.preview_events.push_back(event);
        }
    }

    pub(super) fn take_preview_events(&mut self) -> Vec<PageEvent> {
        self.preview_events.drain(..).collect()
    }

    /// Apply one page message. `true` when a task edit should wait for a save.
    pub(super) fn handle_preview_event(
        &mut self,
        event: PageEvent,
        window: &mut Window,
        cx: &mut Context<crate::Root>,
    ) -> bool {
        match event {
            PageEvent::Press { index, shift } => {
                self.arm_preview();
                self.row_rewrite = None;
                self.preview_rewrite = false;
                self.row_gesture =
                    super::rows::RowGesture::press(self.row_gesture.take(), index, shift);
                false
            }
            PageEvent::Drag { index } => {
                if let Some(gesture) = self.row_gesture.as_mut() {
                    gesture.drag_over(index);
                }
                false
            }
            PageEvent::Release => {
                if let Some(gesture) = self.row_gesture.as_mut() {
                    gesture.release();
                }
                false
            }
            PageEvent::Copy => {
                self.copy_selected_rows(cx);
                false
            }
            PageEvent::Add => {
                self.add_selected_rows_to_chat(cx);
                false
            }
            PageEvent::Rewrite => {
                if self.selected_note_text(cx).is_some() {
                    self.preview_rewrite = true;
                }
                false
            }
            PageEvent::Apply { text } => {
                self.rewrite_selection(&text, cx);
                false
            }
            PageEvent::Cancel => {
                self.arm_preview();
                self.clear_note_rows(cx);
                false
            }
            PageEvent::Toggle { index } => self.toggle_task(index, window, cx),
            PageEvent::Open { .. } => false,
            PageEvent::Live => {
                // The script can run after a push into an empty document.
                self.preview_stamp = 0;
                self.preview_sent = 0;
                self.preview_retry = true;
                self.preview_tries = 0;
                PREVIEW_WAIT.store(true, Ordering::Relaxed);
                false
            }
            PageEvent::Ack { stamp } => {
                if stamp == self.preview_sent {
                    self.preview_stamp = stamp;
                    self.preview_retry = false;
                    self.preview_tries = 0;
                    PREVIEW_WAIT.store(false, Ordering::Relaxed);
                }
                false
            }
        }
    }

    fn arm_preview(&mut self) {
        self.preview_retry = true;
        self.preview_tries = 0;
        PREVIEW_WAIT.store(true, Ordering::Relaxed);
    }

    /// The last push was not confirmed. `true` when the caller should draw again.
    pub(super) fn retry_preview(&mut self) -> bool {
        if !self.preview_showing() || !self.preview_ready.load(Ordering::Acquire) {
            return false;
        }
        if PREVIEW_RESHOWN.swap(false, Ordering::Relaxed) {
            // Settings, or another window, moved the page off the screen.
            // The old confirmation belongs to that hidden document.
            self.preview_stamp = 0;
            self.preview_sent = 0;
            self.preview_retry = true;
            self.preview_tries = 0;
            return true;
        }
        if self.preview_sent != 0 && self.preview_stamp == self.preview_sent {
            PREVIEW_WAIT.store(false, Ordering::Relaxed);
            return false;
        }
        if self.preview_tries >= PREVIEW_PUSH_LIMIT {
            PREVIEW_WAIT.store(false, Ordering::Relaxed);
            return false;
        }
        self.preview_tries = self.preview_tries.saturating_add(1);
        self.preview_retry = true;
        true
    }

    pub(super) fn sync_preview(&mut self, cx: &mut Context<crate::Root>) {
        let Some(view) = self.preview_view.clone() else {
            return;
        };
        let showing = self.preview_showing();
        view.set_showing(showing, self.preview_rewrite, cx);
        if !showing || !self.preview_ready.load(Ordering::Acquire) {
            if !showing {
                self.preview_stamp = 0;
                self.preview_sent = 0;
                self.preview_retry = true;
                self.preview_tries = 0;
                PREVIEW_WAIT.store(false, Ordering::Relaxed);
            }
            return;
        }
        if PREVIEW_RESHOWN.swap(false, Ordering::Relaxed) {
            self.preview_stamp = 0;
            self.preview_sent = 0;
            self.preview_retry = true;
            self.preview_tries = 0;
        }
        let payload = self.preview_payload(cx);
        if payload.stamp == self.preview_stamp && !self.preview_retry {
            PREVIEW_WAIT.store(false, Ordering::Relaxed);
            return;
        }
        if payload.stamp != self.preview_sent {
            self.preview_tries = 0;
        }
        if payload.stamp == self.preview_sent && !self.preview_retry {
            return;
        }
        if self.preview_tries >= PREVIEW_PUSH_LIMIT {
            return;
        }
        self.preview_retry = false;
        if view.eval(&payload.script, cx) {
            self.preview_sent = payload.stamp;
            PREVIEW_WAIT.store(true, Ordering::Relaxed);
        }
    }

    fn preview_payload(&mut self, cx: &mut Context<crate::Root>) -> Payload {
        let mut book = match self.preview_book.lock() {
            Ok(book) => book,
            Err(err) => err.into_inner(),
        };
        book.reset(self.vault.clone(), self.open_note.clone());
        let dark = cx.theme().mode == ThemeMode::Dark;
        let html =
            blocks(
                &self.latest,
                self.media_muted,
                &mut book,
                |source| match crate::mermaid::diagram(cx, source, dark) {
                    crate::mermaid::Diagram::Pending => DiagramFace::Wait,
                    crate::mermaid::Diagram::Failed(message) => {
                        DiagramFace::Fail(message.to_string())
                    }
                    crate::mermaid::Diagram::Svg(bytes) => DiagramFace::Svg(bytes),
                },
            );
        drop(book);
        let css = theme_css(cx);
        let rows = split_rows(&self.latest);
        let bounds = self
            .row_gesture
            .as_ref()
            .and_then(|gesture| gesture.bounds(rows.len()));
        let menu = self
            .row_gesture
            .as_ref()
            .is_some_and(|gesture| gesture.menu_open());
        let html_hash = hash_one(&html);
        let stamp = hash_parts(&[
            &html_hash.to_string(),
            &css,
            &bounds.map(|(lo, _)| lo).unwrap_or(usize::MAX).to_string(),
            &bounds.map(|(_, hi)| hi).unwrap_or(usize::MAX).to_string(),
            if menu { "menu" } else { "" },
            if self.preview_rewrite { "rewrite" } else { "" },
            if self.media_muted { "muted" } else { "" },
        ]);
        let body = serde_json::json!({
            "html": html,
            "hash": html_hash.to_string(),
            "css": css,
            "lo": bounds.map(|(lo, _)| lo),
            "hi": bounds.map(|(_, hi)| hi),
            "menu": menu,
            "rewrite": self.preview_rewrite,
            "muted": self.media_muted,
            "stamp": stamp.to_string(),
            "labels": {
                "copy": locale::text("preview.copy"),
                "add": locale::text("preview.add"),
                "rewrite": locale::text("preview.rewrite"),
                "apply": locale::text("preview.apply"),
                "placeholder": locale::text("preview.rewrite_placeholder"),
            },
        });
        Payload {
            script: format!("window.runebook&&window.runebook.apply({})", js_arg(&body)),
            stamp,
        }
    }
}

fn blocks(
    note: &str,
    muted: bool,
    book: &mut MediaBook,
    mut diagram: impl FnMut(&str) -> DiagramFace,
) -> String {
    let slices: Vec<String> = split_rows(note)
        .into_iter()
        .map(|row| note[row.start..row.end].to_string())
        .collect();
    let mut html = String::new();
    for (index, slice) in slices.iter().enumerate() {
        let room = heading_needs_room(
            index.checked_sub(1).map(|prev| slices[prev].as_str()),
            slice,
        );
        let class = if room { "row room" } else { "row" };
        html.push_str(&format!(r#"<section class="{class}" data-row="{index}">"#));
        html.push_str(&row_body(note, slice, muted, book, &mut diagram));
        html.push_str("</section>");
    }
    html
}

fn row_body(
    note: &str,
    slice: &str,
    muted: bool,
    book: &mut MediaBook,
    diagram: &mut impl FnMut(&str) -> DiagramFace,
) -> String {
    if let Some(link) = crate::media::media_link(slice) {
        return media_html(&link, muted, book);
    }
    if let Some(source) = mermaid_source(slice) {
        return diagram_html(&source, book, diagram);
    }
    let prepared = crate::anchors::prepare(note, slice);
    let visible = if prepared.hide_anchors {
        prepared
            .source
            .trim_end()
            .strip_suffix(prepared.suffix.trim_end())
            .unwrap_or(prepared.source.as_str())
            .trim_end()
    } else {
        prepared.source.as_str()
    };
    let visible = super::rich::expand_emoji(visible);
    if let Some(html) = super::rich::definition_list(&visible, compile_markdown) {
        let html = super::rich::render_math(&html);
        return crate::html_style::bind_images(&html, |src| bind_src(src, book));
    }
    let markdown = if prepared.suffix.is_empty() {
        visible
    } else {
        format!("{visible}\n\n{}", prepared.suffix)
    };
    let rendered = compile_markdown(&markdown);
    let rendered = if prepared.suffix.is_empty() {
        rendered
    } else {
        let extra = compile_markdown(&prepared.suffix);
        rendered
            .trim_end()
            .strip_suffix(extra.trim())
            .unwrap_or(rendered.as_str())
            .trim()
            .to_string()
    };
    let safe = crate::html_style::sanitize_fragment(&rendered);
    let safe = super::rich::render_math(&safe);
    crate::html_style::bind_images(&safe, |src| bind_src(src, book))
}

fn bind_src(src: &str, book: &mut MediaBook) -> Option<String> {
    let blob = if is_remote(src) {
        Blob::Remote(src.to_string())
    } else {
        Blob::Href(src.to_string())
    };
    book.insert(blob).map(|token| token_url(&token))
}

fn media_html(link: &MediaLink, muted: bool, book: &mut MediaBook) -> String {
    let Some(token) = book.insert(Blob::Href(link.href.clone())) else {
        return format!("<p>{}</p>", escape(&link.label));
    };
    let tag = match link.kind {
        ClipKind::Video => "video",
        ClipKind::Audio => "audio",
    };
    let mute = if muted { " muted" } else { "" };
    format!(
        r#"<{tag} controls src="{}"{mute}></{tag}>"#,
        token_url(&token)
    )
}

fn diagram_html(
    source: &str,
    book: &mut MediaBook,
    diagram: &mut impl FnMut(&str) -> DiagramFace,
) -> String {
    match diagram(source) {
        DiagramFace::Wait => format!("<p>{}</p>", escape(locale::text("mermaid.pending"))),
        DiagramFace::Fail(message) => format!("<p>{}</p>", escape(&message)),
        DiagramFace::Svg(bytes) => {
            let Some(token) = book.insert(Blob::Svg(bytes)) else {
                return format!("<p>{}</p>", escape(locale::text("mermaid.failed")));
            };
            format!(
                r#"<img src="{}" alt="{}">"#,
                token_url(&token),
                escape(locale::text("mermaid.pending"))
            )
        }
    }
}

fn mermaid_source(row: &str) -> Option<String> {
    let trimmed = row.trim();
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|ch| *ch == marker).count();
    if count < 3 {
        return None;
    }
    let rest = trimmed[count..].trim_start();
    let (lang, body) = rest.split_once('\n')?;
    if !lang.trim().eq_ignore_ascii_case("mermaid") {
        return None;
    }
    let mut lines = Vec::new();
    for line in body.lines() {
        let fence = line.trim();
        if fence.len() >= count && fence.chars().all(|ch| ch == marker) {
            return Some(lines.join("\n"));
        }
        lines.push(line);
    }
    None
}

fn compile_markdown(source: &str) -> String {
    let mut options = markdown::Options::gfm();
    options.compile.allow_dangerous_html = true;
    options.compile.allow_dangerous_protocol = true;
    // The tag filter would turn a script tag into visible text. The sanitizer
    // drops the element, so the filter stays off.
    options.compile.gfm_tagfilter = false;
    options.compile.gfm_task_list_item_checkable = true;
    options.parse.constructs.math_text = true;
    options.parse.constructs.math_flow = true;
    markdown::to_html_with_options(source, &options).unwrap_or_default()
}

fn theme_css(cx: &App) -> String {
    let theme = cx.theme();
    let spacing = crate::theme::note_spacing();
    let body = crate::theme::body_font_size();
    let heading = syntax_color(cx, "title").unwrap_or(theme.foreground);
    let code = syntax_color(cx, "text.literal")
        .or_else(|| syntax_color(cx, "string"))
        .unwrap_or(theme.foreground);
    format!(
        ":root{{--bg:{bg};--fg:{fg};--selection:{selection};--heading:{heading};--code:{code};--panel:{panel};--border:{border};--font:{font},sans-serif;--mono:{mono},monospace;--body:{body}px;--h1:{h1};--h2:{h2};--gap:{gap};--before:{before};--radius:{radius}px;}}",
        bg = css_color(theme.background),
        fg = css_color(theme.foreground),
        selection = css_color(theme.selection),
        heading = css_color(heading),
        code = css_color(code),
        panel = css_color(theme.secondary),
        border = css_color(theme.border),
        font = css_family(theme.font_family.as_ref()),
        mono = css_family(theme.mono_font_family.as_ref()),
        body = body,
        h1 = spacing.heading_1,
        h2 = spacing.heading_2,
        gap = spacing.paragraph_gap,
        before = spacing.heading_before,
        radius = theme.radius.as_f32(),
    )
}

fn syntax_color(cx: &App, name: &str) -> Option<Hsla> {
    cx.theme()
        .highlight_theme
        .style
        .syntax
        .style(name)
        .and_then(|style| style.color)
}

fn css_color(color: Hsla) -> String {
    let rgb = color.to_rgb();
    format!(
        "rgba({},{},{},{})",
        (rgb.r * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb.g * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb.b * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb.a * 1000.0).round() / 1000.0
    )
}

fn css_family(name: &str) -> String {
    let mut out = String::from("\"");
    for ch in name.chars() {
        match ch {
            '\\' | '"' => {
                out.push('\\');
                out.push(ch);
            }
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn token_url(token: &str) -> String {
    format!("rbmedia://local/{token}")
}

fn is_remote(url: &str) -> bool {
    let url = url.trim();
    url.starts_with("https://") || url.starts_with("http://")
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

fn js_arg(value: &serde_json::Value) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "{}".to_string())
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn hash_one(text: &str) -> u64 {
    hash_parts(&[text])
}

fn hash_parts(parts: &[&str]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for part in parts {
        part.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(target_os = "linux")]
const SHELL_URL: &str = "rbmedia://local/shell";

fn is_shell_request(uri: &wry::http::Uri) -> bool {
    matches!(uri.path(), "/shell" | "/shell/")
}

fn serve(book: &Mutex<MediaBook>, uri: &wry::http::Uri) -> wry::http::Response<Cow<'static, [u8]>> {
    if is_shell_request(uri) {
        return bytes_response("text/html; charset=utf-8", SHELL.as_bytes().to_vec());
    }
    let token = uri
        .path()
        .rsplit('/')
        .find(|part| !part.is_empty())
        .map(str::to_string);
    let resolved = token.and_then(|token| {
        let book = match book.lock() {
            Ok(book) => book,
            Err(err) => err.into_inner(),
        };
        book.resolve(&token)
    });
    let Some(resolved) = resolved else {
        return empty(404);
    };
    match resolved {
        Resolved::Svg(bytes) => bytes_response("image/svg+xml", bytes.as_ref().clone()),
        Resolved::File(path) => match read_capped(&path) {
            Some(bytes) => bytes_response(mime_for(&path), bytes),
            None => empty(404),
        },
        Resolved::Remote(url) => match fetch_remote(&url) {
            Some((bytes, mime)) => bytes_response(mime, bytes),
            None => empty(404),
        },
    }
}

fn read_capped(path: &std::path::Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > FILE_LIMIT {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    (bytes.len() as u64 <= FILE_LIMIT && !bytes.is_empty()).then_some(bytes)
}

fn fetch_remote(url: &str) -> Option<(Vec<u8>, &'static str)> {
    if !is_remote(url) {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(2))
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let mut response = client.get(url).send().ok()?;
    if !response.status().is_success() || !is_remote(response.url().as_str()) {
        return None;
    }
    if response
        .content_length()
        .is_some_and(|len| len > FILE_LIMIT)
    {
        return None;
    }
    let mime = mime_header(response.headers().get(wry::http::header::CONTENT_TYPE));
    let mut bytes = Vec::new();
    std::io::Read::take(&mut response, FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.is_empty() || bytes.len() as u64 > FILE_LIMIT {
        return None;
    }
    Some((bytes, mime))
}

fn mime_header(value: Option<&wry::http::HeaderValue>) -> &'static str {
    let Some(value) = value else {
        return "application/octet-stream";
    };
    let Ok(text) = value.to_str() else {
        return "application/octet-stream";
    };
    let kind = text.split(';').next().unwrap_or("").trim();
    match kind {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/svg+xml" => {
            // The match returns a copy of a static string through the arms below.
            if kind == "image/png" {
                "image/png"
            } else if kind == "image/jpeg" {
                "image/jpeg"
            } else if kind == "image/gif" {
                "image/gif"
            } else if kind == "image/webp" {
                "image/webp"
            } else {
                "image/svg+xml"
            }
        }
        _ => "application/octet-stream",
    }
}

fn mime_for(path: &std::path::Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        _ => "application/octet-stream",
    }
}

fn bytes_response(mime: &str, bytes: Vec<u8>) -> wry::http::Response<Cow<'static, [u8]>> {
    let len = bytes.len().to_string();
    wry::http::Response::builder()
        .status(200)
        .header(wry::http::header::CONTENT_TYPE, mime)
        .header(wry::http::header::CONTENT_LENGTH, len)
        .header(wry::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Cow::Owned(bytes))
        .unwrap_or_else(|_| empty(500))
}

fn empty(status: u16) -> wry::http::Response<Cow<'static, [u8]>> {
    wry::http::Response::builder()
        .status(status)
        .body(Cow::Owned(Vec::new()))
        .unwrap_or_else(|_| wry::http::Response::new(Cow::Owned(Vec::new())))
}

const SHELL: &str = r##"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style id="theme"></style>
<style>
html{height:100%}
html,body{margin:0;background:var(--bg,#111);color:var(--fg,#eee)}
body{min-height:100%;font-family:var(--font,sans-serif);font-size:var(--body,14px)}
#note{padding:16px}
section.row{border-radius:var(--radius,6px);padding:2px 8px;cursor:pointer}
section.row.selected{background:var(--selection,#345)}
section.row.room{margin-top:calc(var(--before,1) * 1rem)}
h1,h2,h3,h4,h5,h6{color:var(--heading,inherit);font-weight:600;margin:0.4em 0}
h1{font-size:calc(var(--body,14px) * var(--h1,1.4))}
h2{font-size:calc(var(--body,14px) * var(--h2,1.2))}
p,ul,ol,pre,blockquote,table,dl{margin:calc(var(--gap,1) * 0.4rem) 0}
section.row:has(> ul),section.row:has(> ol){padding-top:0;padding-bottom:0}
section.row > ul,section.row > ol{margin-top:0;margin-bottom:0}
a{color:var(--heading,inherit)}
blockquote{margin-left:0;margin-right:0;padding:0.2em 0 0.2em 0.8em;border-left:3px solid var(--border,#444)}
hr{border:0;border-top:1px solid var(--border,#444);margin:calc(var(--gap,1) * 0.8rem) 0}
table{border-collapse:collapse;width:100%}
th,td{border:1px solid var(--border,#444);padding:0.4em 0.7em;vertical-align:top}
th{background:var(--panel,transparent);font-weight:600}
pre,code,kbd{font-family:var(--mono,monospace);color:var(--code,inherit)}
:not(pre)>code{background:var(--panel,transparent);padding:0.1em 0.35em;border-radius:var(--radius,6px)}
pre{background:var(--panel,transparent);border-radius:var(--radius,6px);padding:8px;overflow:auto}
pre code{background:transparent;padding:0}
dt{font-weight:600}
dd{margin:0.2em 0 0.6em 1.2em}
.footnotes{font-size:0.92em}
.sr-only{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}
li:has(> input[type=checkbox]){list-style:none}
.math-display{display:block;text-align:center;overflow-x:auto}
img,video,audio{max-width:100%}
video{background:var(--panel,transparent)}
#menu{margin:8px 0;display:flex;flex-wrap:wrap;gap:8px;padding:8px;background:var(--panel,#222);border:1px solid var(--border,#444);border-radius:var(--radius,6px)}
#menu[hidden]{display:none}
#menu button,#menu input{font:inherit;color:inherit;background:var(--bg,#111);border:1px solid var(--border,#444);border-radius:var(--radius,6px);padding:4px 8px}
#rewrite{min-width:16rem}
</style>
</head>
<body>
<article id="note"></article>
<div id="menu" hidden></div>
<script>
(function () {
  const note = document.getElementById("note");
  const menu = document.getElementById("menu");
  const theme = document.getElementById("theme");
  let hash = "";
  let down = false;
  let rewriteOpen = false;
  function post(message) {
    if (window.ipc) window.ipc.postMessage(JSON.stringify(message));
  }
  function esc(text) {
    return String(text).replace(/[&<>"]/g, function (ch) {
      return {"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;"}[ch];
    });
  }
  function rowAt(event) {
    const el = document.elementFromPoint(event.clientX, event.clientY);
    const section = el && el.closest("section[data-row]");
    return section ? Number(section.dataset.row) : null;
  }
  function interactive(event) {
    return event.target.closest("a, input, video, audio, button, #menu");
  }
  note.addEventListener("pointerdown", function (event) {
    if (interactive(event)) return;
    const index = rowAt(event);
    if (index == null) {
      post({kind:"cancel"});
      return;
    }
    down = true;
    note.setPointerCapture(event.pointerId);
    post({kind:"press", index:index, shift:event.shiftKey});
  });
  note.addEventListener("pointermove", function (event) {
    if (!down) return;
    const index = rowAt(event);
    if (index == null) return;
    post({kind:"drag", index:index});
  });
  note.addEventListener("pointerup", function () {
    if (!down) return;
    down = false;
    post({kind:"release"});
  });
  note.addEventListener("click", function (event) {
    const link = event.target.closest("a[href]");
    if (link) {
      event.preventDefault();
      post({kind:"open", url:link.getAttribute("href")});
      return;
    }
    const image = event.target.closest("img[data-open]");
    if (image) {
      event.preventDefault();
      post({kind:"open", url:image.getAttribute("data-open")});
      return;
    }
    const box = event.target.closest("input[type=checkbox]");
    if (box) {
      event.preventDefault();
      const section = box.closest("section[data-row]");
      if (section) post({kind:"toggle", index:Number(section.dataset.row)});
    }
  });
  document.addEventListener("keydown", function (event) {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "c") {
      if (event.target.closest("input, textarea")) return;
      event.preventDefault();
      post({kind:"copy"});
    }
  });
  menu.addEventListener("click", function (event) {
    const act = event.target.closest("[data-act]");
    if (!act) return;
    event.preventDefault();
    if (act.dataset.act === "apply") {
      const field = document.getElementById("rewrite");
      post({kind:"apply", text:field ? field.value : ""});
      return;
    }
    post({kind:act.dataset.act});
  });
  menu.addEventListener("keydown", function (event) {
    if (event.target.id !== "rewrite") return;
    if (event.key === "Enter") {
      event.preventDefault();
      post({kind:"apply", text:event.target.value});
    } else if (event.key === "Escape") {
      event.preventDefault();
      post({kind:"cancel"});
    }
  });
  function parkMenu() {
    if (menu.parentNode !== document.body) document.body.appendChild(menu);
  }
  function menuHtml(payload) {
    const labels = payload.labels;
    let html = '<button type="button" data-act="copy">' + esc(labels.copy) + '</button>'
      + '<button type="button" data-act="add">' + esc(labels.add) + '</button>'
      + '<button type="button" data-act="rewrite">' + esc(labels.rewrite) + '</button>';
    if (payload.rewrite) {
      html += '<input id="rewrite" type="text" placeholder="' + esc(labels.placeholder) + '">'
        + '<button type="button" data-act="apply">' + esc(labels.apply) + '</button>';
    }
    return html;
  }
  window.runebook = {
    apply: function (payload) {
      theme.textContent = payload.css;
      if (payload.hash !== hash) {
        const top = note.scrollTop;
        parkMenu();
        note.innerHTML = payload.html;
        hash = payload.hash;
        note.scrollTop = top;
      }
      note.querySelectorAll("section[data-row]").forEach(function (section) {
        const index = Number(section.dataset.row);
        const on = payload.lo != null && index >= payload.lo && index <= payload.hi;
        section.classList.toggle("selected", on);
      });
      document.querySelectorAll("video, audio").forEach(function (el) {
        el.muted = !!payload.muted;
      });
      if (!payload.menu) {
        menu.hidden = true;
        menu.innerHTML = "";
        rewriteOpen = false;
        parkMenu();
      } else {
        const row = note.querySelector('section[data-row="' + payload.hi + '"]');
        if (row) row.after(menu);
        else parkMenu();
        menu.hidden = false;
        const keep = payload.rewrite && rewriteOpen && document.getElementById("rewrite");
        if (!keep) {
          menu.innerHTML = menuHtml(payload);
          rewriteOpen = !!payload.rewrite;
          const field = document.getElementById("rewrite");
          if (field) field.focus();
        }
      }
      post({kind:"ack", stamp: payload.stamp});
    }
  };
  post({kind:"live"});
})();
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    fn html_of(note: &str) -> (String, MediaBook) {
        let mut book = MediaBook::new();
        let html = blocks(note, true, &mut book, |_| DiagramFace::Wait);
        (html, book)
    }

    #[test]
    fn rows_keep_their_indexes() {
        let (html, _) = html_of("# Title\n\nA paragraph.\n");
        assert!(html.contains("data-row=\"0\""));
        assert!(html.contains("data-row=\"1\""));
        assert!(html.contains("<h1>"));
    }

    #[test]
    fn a_task_row_is_a_checkbox() {
        let (html, _) = html_of("- [ ] Buy milk\n");
        assert!(html.contains("data-row=\"0\""));
        assert!(html.contains("type=\"checkbox\""));
        assert!(!html.to_ascii_lowercase().contains("disabled"));
    }

    #[test]
    fn a_video_row_uses_a_token() {
        let (html, book) = html_of("[clip](day.mp4)\n");
        assert!(html.contains("<video"));
        assert!(html.contains("rbmedia://local/t0"));
        assert!(html.contains("muted"));
        assert!(!html.contains("day.mp4"));
        assert!(matches!(book.blobs.get("t0"), Some(Blob::Href(href)) if href == "day.mp4"));
    }

    #[test]
    fn a_definition_row_is_a_list() {
        let (html, _) = html_of("Markdown\n: A lightweight markup language.\n");
        assert!(html.contains("<dt>"));
        assert!(html.contains("<dd>"));
        assert!(html.contains("lightweight"));
    }

    #[test]
    fn inline_math_becomes_mathml() {
        let (html, _) = html_of("Energy $E = mc^2$ here.\n");
        assert!(html.contains("<math"));
        let (display, _) = html_of("$$\nE = mc^2\n$$\n");
        assert!(display.contains("math-display"));
        assert!(display.contains("<math"));
    }

    #[test]
    fn a_footnote_keeps_its_hidden_label() {
        let (html, _) = html_of("See[^1].\n\n[^1]: Note text.\n");
        assert!(html.contains("sr-only"));
        assert!(html.contains("user-content-fn"));
    }

    #[test]
    fn a_math_tag_in_a_note_does_not_survive() {
        let (html, _) = html_of("<math><mi>x</mi></math>\n");
        assert!(!html.to_ascii_lowercase().contains("<math"));
    }

    #[test]
    fn a_rocket_shortcode_becomes_the_character() {
        let (html, _) = html_of("Go :rocket: now.\n");
        assert!(html.contains('🚀'));
        assert!(!html.contains(":rocket:"));
        let (fenced, _) = html_of("```\n:rocket:\n```\n");
        assert!(fenced.contains(":rocket:"));
        assert!(!fenced.contains('🚀'));
    }

    #[test]
    fn a_script_in_a_note_does_not_survive() {
        let (html, _) = html_of("<script>alert(1)</script>\n");
        assert!(!html.to_ascii_lowercase().contains("<script"));
        assert!(!html.contains("alert(1)"));
    }

    #[test]
    fn a_live_message_asks_for_the_note() {
        assert!(matches!(parse_event(r#"{"kind":"live"}"#), Some(PageEvent::Live)));
    }

    #[test]
    fn an_ack_keeps_the_stamp_string() {
        match parse_event(r#"{"kind":"ack","stamp":"42"}"#) {
            Some(PageEvent::Ack { stamp }) => assert_eq!(stamp, 42),
            other => panic!("ack was {other:?}"),
        }
    }

    #[test]
    fn an_unknown_page_message_is_ignored() {
        assert!(parse_event(r#"{"kind":"nope"}"#).is_none());
        assert!(parse_event("").is_none());
        let huge = format!(r#"{{"kind":"open","url":"{}"}}"#, "a".repeat(3000));
        assert!(parse_event(&huge).is_none());
    }

    #[test]
    fn the_menu_follows_the_last_selected_row() {
        assert!(SHELL.contains("row.after(menu)"));
        assert!(SHELL.contains("parkMenu"));
        assert!(SHELL.contains("document.body.appendChild(menu)"));
        assert!(!SHELL.contains("position:sticky"));
    }

    #[test]
    fn an_image_response_names_its_length() {
        let response = bytes_response("image/png", vec![1, 2, 3, 4]);
        let headers = response.headers();
        assert_eq!(
            header_text(headers, wry::http::header::CONTENT_TYPE),
            Some("image/png")
        );
        assert_eq!(
            header_text(headers, wry::http::header::CONTENT_LENGTH),
            Some("4")
        );
        assert_eq!(
            header_text(headers, wry::http::header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some("*")
        );
    }

    #[test]
    fn the_shell_request_is_the_page() {
        let book = std::sync::Mutex::new(MediaBook::new());
        let uri = wry::http::Uri::from_static("rbmedia://local/shell");
        let response = serve(&book, &uri);
        assert_eq!(response.status(), 200);
        assert!(response.body().starts_with(b"<!DOCTYPE html>"));
    }

    fn header_text(
        headers: &wry::http::HeaderMap,
        name: wry::http::header::HeaderName,
    ) -> Option<&str> {
        headers.get(name).and_then(|value| value.to_str().ok())
    }

    #[test]
    fn a_press_message_keeps_the_row() {
        match parse_event(r#"{"kind":"press","index":2,"shift":true}"#) {
            Some(PageEvent::Press { index, shift }) => {
                assert_eq!(index, 2);
                assert!(shift);
            }
            other => panic!("expected a press, got {other:?}"),
        }
    }
}
