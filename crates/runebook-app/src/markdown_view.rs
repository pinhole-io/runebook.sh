//! Markdown preview for a note row and for Archivist text.
//!
//! Block images, rules, footnotes, definitions, and `<details>` are drawn here.
//! A local image path must stay inside the vault. The resolver refuses a
//! symlink and a file larger than 8 MiB. A failure shows the alt text and
//! does not include a path.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::text::{
    InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    TextView, TextViewStyle, markdown_ast,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, ClickEvent, Image, ImageFormat, ImageSource, MouseButton, MouseDownEvent, MouseUpEvent,
    ObjectFit, SharedString, Window, div, img, px, rems,
};
use runebook::{NoteId, Vault};

const MAX_IMAGE: u64 = 8 * 1024 * 1024;

/// Vault used to resolve a relative image path.
#[derive(Clone)]
pub(crate) struct ImageRoots {
    pub vault: Vault,
    pub note: NoteId,
}

impl ImageRoots {
    fn local_path(&self, url: &str) -> Option<PathBuf> {
        let path = self.vault.media_file(&self.note, url).ok()?;
        let len = std::fs::metadata(&path).ok()?.len();
        (len > 0 && len <= MAX_IMAGE).then_some(path)
    }
}

/// Markdown view. `hide_anchors` drops definitions appended for reference lookup.
pub(crate) fn view(
    id: impl Into<gpui_kit::ElementId>,
    source: impl Into<SharedString>,
    images: Option<ImageRoots>,
    hide_anchors: bool,
) -> TextView {
    let shielded = shield_details(&source.into());
    let source = SharedString::from(crate::html_style::style_html(&shielded));
    let links = Arc::new(crate::anchors::link_urls(source.as_ref()));
    let images = Arc::new(images);
    let parser_links = Arc::clone(&links);
    TextView::markdown(id, source)
        .style(note_style())
        .markdown_block_parser(crate::mermaid::parse_block)
        .markdown_block_renderer("mermaid", crate::mermaid::render_block)
        .markdown_block_parser(move |node, cx| parse_extra(node, cx, hide_anchors, &parser_links))
        .markdown_block_renderer("md-rule", render_rule)
        .markdown_block_renderer("md-skip", |_, _, _| div())
        .markdown_block_renderer("md-link-def", render_link_def)
        .markdown_block_renderer("md-footnote", {
            let images = Arc::clone(&images);
            move |node, _, cx| render_footnote(node, &images, cx)
        })
        .markdown_block_renderer("md-details", {
            let images = Arc::clone(&images);
            move |node, _, cx| render_details(node, &images, cx)
        })
        .markdown_block_renderer("md-figure", {
            let images = Arc::clone(&images);
            move |node, _, cx| render_figure(node, &images, cx)
        })
        .markdown_block_renderer("md-deflist", {
            let images = Arc::clone(&images);
            move |node, _, cx| render_deflist(node, &images, cx)
        })
        .plugin(FootnoteMark)
        .plugin(HtmlInline)
        .plugin(ImageRef {
            links,
            images: Arc::clone(&images),
        })
}

/// One heading scale for every preview.
///
/// The scale and the paragraph gap come from `themes/runebook.json`.
/// Lower headings match the body.
fn note_style() -> TextViewStyle {
    let body = px(crate::theme::body_font_size());
    let spacing = crate::theme::note_spacing();
    let heading_1 = spacing.heading_1;
    let heading_2 = spacing.heading_2;
    let mut style = TextViewStyle::default();
    style.paragraph_gap = rems(spacing.paragraph_gap);
    style.heading_base_font_size = body;
    style.heading_font_size = Some(Arc::new(move |level, base| match level {
        1 => base * heading_1,
        2 => base * heading_2,
        _ => base,
    }));
    style
}

#[derive(Clone)]
struct Figure {
    url: String,
    alt: String,
}

struct Details {
    summary: String,
    body: String,
    starts_open: bool,
}

struct DefList {
    items: Vec<(String, String)>,
}

struct Footnote {
    id: String,
    body: String,
}

struct LinkDef {
    label: String,
    url: String,
}

fn parse_extra(
    node: &markdown_ast::Node,
    cx: &MarkdownParseContext<'_>,
    hide: bool,
    links: &HashMap<String, String>,
) -> Option<MarkdownNode> {
    if let Some(figure) = figure(node, links) {
        let alt = figure.alt.clone();
        return Some(MarkdownNode::new("md-figure", figure).text(alt));
    }
    match node {
        markdown_ast::Node::Code(code) => details_code(code),
        markdown_ast::Node::ThematicBreak(_) => Some(MarkdownNode::new("md-rule", ()).text("")),
        markdown_ast::Node::Html(html) => html_block(&html.value),
        markdown_ast::Node::FootnoteDefinition(def) => {
            if hide {
                return Some(MarkdownNode::new("md-skip", ()).text(""));
            }
            let raw = cx.node_source(node).unwrap_or("");
            let body = strip_footnote(raw);
            let id = def.identifier.clone();
            Some(
                MarkdownNode::new(
                    "md-footnote",
                    Footnote {
                        id: id.clone(),
                        body,
                    },
                )
                .text(id),
            )
        }
        markdown_ast::Node::Definition(def) => {
            if hide {
                return Some(MarkdownNode::new("md-skip", ()).text(""));
            }
            let label = def.identifier.clone();
            Some(
                MarkdownNode::new(
                    "md-link-def",
                    LinkDef {
                        label: label.clone(),
                        url: def.url.clone(),
                    },
                )
                .text(label),
            )
        }
        _ => None,
    }
}

fn details_code(code: &markdown_ast::Code) -> Option<MarkdownNode> {
    let lang = code.lang.as_deref()?.trim();
    if !lang.eq_ignore_ascii_case(DETAILS_FENCE) {
        return None;
    }
    let details = details_parts(&code.value)?;
    let summary = details.summary.clone();
    let markdown = code.value.clone();
    Some(
        MarkdownNode::new("md-details", details)
            .text(summary)
            .markdown(markdown),
    )
}

fn html_block(html: &str) -> Option<MarkdownNode> {
    if let Some(items) = crate::html_style::def_list(html) {
        let text = items
            .iter()
            .map(|(term, _)| term.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Some(MarkdownNode::new("md-deflist", DefList { items }).text(text));
    }
    if let Some((url, alt)) = crate::html_style::figure_html(html) {
        return Some(
            MarkdownNode::new(
                "md-figure",
                Figure {
                    url,
                    alt: alt.clone(),
                },
            )
            .text(alt),
        );
    }
    let details = details_parts(html)?;
    let summary = details.summary.clone();
    Some(MarkdownNode::new("md-details", details).text(summary))
}

fn figure(node: &markdown_ast::Node, links: &HashMap<String, String>) -> Option<Figure> {
    let markdown_ast::Node::Paragraph(paragraph) = node else {
        return None;
    };
    let [child] = paragraph.children.as_slice() else {
        return None;
    };
    match child {
        markdown_ast::Node::Image(image) => Some(Figure {
            url: image.url.clone(),
            alt: image.alt.clone(),
        }),
        markdown_ast::Node::ImageReference(image) => {
            let url = links.get(&image.identifier.to_ascii_lowercase())?.clone();
            Some(Figure {
                url,
                alt: image.alt.clone(),
            })
        }
        _ => None,
    }
}

fn render_rule(_: &MarkdownNode, _: &mut Window, cx: &mut App) -> gpui_kit::AnyElement {
    let border = cx.theme().border;
    div()
        .w_full()
        .py_2()
        .child(div().w_full().h(px(1.)).bg(border))
        .into_any_element()
}

fn render_link_def(node: &MarkdownNode, _: &mut Window, cx: &mut App) -> gpui_kit::AnyElement {
    let muted = cx.theme().muted_foreground;
    let text = node
        .data::<LinkDef>()
        .map(|def| format!("[{}]: {}", def.label, def.url))
        .unwrap_or_default();
    div()
        .text_base()
        .text_color(muted)
        .child(text)
        .into_any_element()
}

fn render_footnote(
    node: &MarkdownNode,
    images: &Arc<Option<ImageRoots>>,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let Some(note) = node.data::<Footnote>() else {
        return div().into_any_element();
    };
    let link = cx.theme().link;
    let id = note.id.clone();
    let body = note.body.clone();
    let key = text_key(&format!("{id}\n{body}"));
    let border = cx.theme().border;
    div()
        .w_full()
        .min_w(px(0.))
        .mt_2()
        .pt_2()
        .border_t_1()
        .border_color(border)
        .flex()
        .items_start()
        .gap_2()
        .child(div().text_base().text_color(link).child(id))
        .when(!body.trim().is_empty(), |row| {
            row.child(
                div().flex_1().min_w(px(0.)).child(
                    view(
                        SharedString::from(format!("md-fn-{key}")),
                        body,
                        images.as_ref().clone(),
                        false,
                    )
                    .selectable(false),
                ),
            )
        })
        .into_any_element()
}

fn render_details(
    node: &MarkdownNode,
    images: &Arc<Option<ImageRoots>>,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let Some(details) = node.data::<Details>() else {
        return div().into_any_element();
    };
    let key = text_key(&format!("{}\n{}", details.summary, details.body));
    let starts_open = details.starts_open;
    let open = {
        let state = details_state()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if state.shut.contains(&key) {
            false
        } else {
            starts_open || state.open.contains(&key)
        }
    };
    let summary = details.summary.clone();
    let body = details.body.clone();
    let marker = if open { "▾" } else { "▸" };
    let border = cx.theme().border;
    let fill = cx.theme().secondary;
    let ink = cx.theme().foreground;
    div()
        .w_full()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .id(SharedString::from(format!("md-summary-{key}")))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(fill)
                .cursor_pointer()
                .text_color(ink)
                .child(marker.to_string())
                .child(summary)
                .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                })
                .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| {
                    cx.stop_propagation();
                    let mut state = details_state()
                        .lock()
                        .unwrap_or_else(|err| err.into_inner());
                    if state.open.len() + state.shut.len() >= 128 {
                        state.open.clear();
                        state.shut.clear();
                    }
                    let shown = if state.shut.contains(&key) {
                        false
                    } else {
                        starts_open || state.open.contains(&key)
                    };
                    if shown {
                        state.open.remove(&key);
                        state.shut.insert(key);
                    } else {
                        state.shut.remove(&key);
                        state.open.insert(key);
                    }
                    cx.refresh_windows();
                }),
        )
        .when(open && !body.trim().is_empty(), |block| {
            block.child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(2.)).bg(border))
                    .child(
                        div().flex_1().min_w(px(0.)).child(
                            view(
                                SharedString::from(format!("md-details-{key}")),
                                body,
                                images.as_ref().clone(),
                                false,
                            )
                            .selectable(false),
                        ),
                    ),
            )
        })
        .into_any_element()
}

fn render_deflist(
    node: &MarkdownNode,
    images: &Arc<Option<ImageRoots>>,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let Some(list) = node.data::<DefList>() else {
        return div().into_any_element();
    };
    let ink = cx.theme().foreground;
    let mut column = div().w_full().min_w(px(0.)).flex().flex_col().gap_2();
    for (index, (term, body)) in list.items.iter().enumerate() {
        let term = term.clone();
        let body = body.clone();
        column = column.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_color(ink).child(term))
                .when(!body.trim().is_empty(), |item| {
                    item.child(
                        div().pl_4().child(
                            view(
                                SharedString::from(format!("md-def-{index}-{}", text_key(&body))),
                                body,
                                images.as_ref().clone(),
                                false,
                            )
                            .selectable(false),
                        ),
                    )
                }),
        );
    }
    column.into_any_element()
}

fn render_figure(
    node: &MarkdownNode,
    images: &Arc<Option<ImageRoots>>,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let Some(figure) = node.data::<Figure>() else {
        return div().into_any_element();
    };
    let alt = figure.alt.clone();
    let url = figure.url.clone();
    let source = figure_source(&url, images.as_ref());
    let muted = cx.theme().muted_foreground;
    let open_url = url.clone();
    let open_images = Arc::clone(images);
    div()
        .w_full()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap_1()
        .when_some(source, |block, source| {
            block.child(
                img(source)
                    .id(SharedString::from(format!("fig-{}", text_key(&url))))
                    .w_full()
                    .max_h(px(360.))
                    .object_fit(ObjectFit::Contain)
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(move |_: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        open_image(&open_url, open_images.as_ref(), cx);
                    }),
            )
        })
        .when(!alt.is_empty(), |block| {
            block.child(div().text_base().text_color(muted).child(alt))
        })
        .into_any_element()
}

fn figure_source(url: &str, images: &Option<ImageRoots>) -> Option<ImageSource> {
    if let Some(image) = data_image(url) {
        return Some(ImageSource::Image(Arc::new(image)));
    }
    if runebook::is_external_url(url) {
        return Some(url.to_string().into());
    }
    let path = images.as_ref()?.local_path(url)?;
    Some(path.as_path().into())
}

fn open_image(url: &str, images: &Option<ImageRoots>, cx: &mut App) {
    if runebook::is_external_url(url) {
        cx.open_url(url);
        return;
    }
    let Some(images) = images else {
        return;
    };
    let Ok(path) = images.vault.media_file(&images.note, url) else {
        return;
    };
    let _ = spawn_desktop_open(&path);
}

pub(crate) fn spawn_desktop_open(path: &std::path::Path) -> std::io::Result<std::process::Child> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
}

struct DetailState {
    open: HashSet<u64>,
    shut: HashSet<u64>,
}

fn details_state() -> &'static Mutex<DetailState> {
    static STATE: OnceLock<Mutex<DetailState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(DetailState {
            open: HashSet::new(),
            shut: HashSet::new(),
        })
    })
}

fn text_key(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

const DETAILS_FENCE: &str = "runebook-details";

/// CommonMark ends a `<details>` block at a blank line.
/// One fence keeps the element together so the summary can hide the body.
fn shield_details(source: &str) -> String {
    let lines = source_pieces(source);
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
        if details_opens(body)
            && let Some(end) = details_span(&lines, index)
        {
            let mut html = String::new();
            for line in &lines[index..=end] {
                html.push_str(line.raw);
            }
            out.push_str(&details_fence(&html));
            index = end + 1;
            continue;
        }
        out.push_str(lines[index].raw);
        index += 1;
    }
    out
}

struct SourcePiece<'a> {
    body: &'a str,
    raw: &'a str,
}

fn source_pieces(source: &str) -> Vec<SourcePiece<'_>> {
    let mut start = 0;
    let mut pieces = Vec::new();
    for raw in source.split_inclusive('\n') {
        let end = start + raw.len();
        let body = raw.trim_end_matches(['\n', '\r']);
        pieces.push(SourcePiece {
            body,
            raw: &source[start..end],
        });
        start = end;
    }
    pieces
}

fn details_span(lines: &[SourcePiece<'_>], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut fence: Option<(char, usize)> = None;
    let mut closed = false;
    for (index, line) in lines.iter().enumerate().skip(start) {
        if let Some((marker, count)) = fence {
            if closing_fence(line.body, marker, count) {
                fence = None;
            }
            continue;
        }
        if index != start
            && let Some(open) = opening_fence(line.body)
        {
            fence = Some(open);
            continue;
        }
        if has_details_close(line.body) {
            closed = true;
        }
        depth += details_delta(line.body);
        if closed && depth <= 0 {
            return Some(index);
        }
    }
    None
}

fn details_fence(html: &str) -> String {
    let fence = "`".repeat(backtick_run(html).max(2) + 1);
    let mut out = String::with_capacity(html.len() + fence.len() * 2 + DETAILS_FENCE.len() + 3);
    out.push_str(&fence);
    out.push_str(DETAILS_FENCE);
    out.push('\n');
    out.push_str(html);
    if !html.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&fence);
    out.push('\n');
    out
}

fn backtick_run(text: &str) -> usize {
    let mut run = 0;
    let mut longest = 0;
    for ch in text.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

fn details_opens(line: &str) -> bool {
    let indent = leading_indent(line);
    if indent > 3 {
        return false;
    }
    let Some(rest) = line.get(indent..) else {
        return false;
    };
    let tag = "<details";
    let Some(head) = rest.get(..tag.len()) else {
        return false;
    };
    if !head.eq_ignore_ascii_case(tag) {
        return false;
    }
    let boundary = rest.as_bytes().get(tag.len()).copied().unwrap_or(b' ');
    !boundary.is_ascii_alphanumeric()
}

fn has_details_close(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let mut start = 0;
    while let Some(at) = lower[start..].find("</details") {
        let index = start + at;
        let after = index + "</details".len();
        let boundary = lower.as_bytes().get(after).copied().unwrap_or(b' ');
        if !boundary.is_ascii_alphanumeric() {
            return true;
        }
        start = index + 1;
    }
    false
}

fn details_delta(line: &str) -> i32 {
    let lower = line.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let tag = "details";
    let mut delta = 0i32;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let close = bytes.get(index + 1) == Some(&b'/');
        let name_at = if close { index + 2 } else { index + 1 };
        if lower.len() >= name_at + tag.len() && lower[name_at..].starts_with(tag) {
            let after = name_at + tag.len();
            let boundary = bytes.get(after).copied().unwrap_or(b' ');
            if !boundary.is_ascii_alphanumeric() {
                if close {
                    delta -= 1;
                } else if !void_tag(&lower[after..]) {
                    delta += 1;
                }
            }
        }
        index += 1;
    }
    delta
}

fn void_tag(after_name: &str) -> bool {
    after_name
        .find('>')
        .is_some_and(|end| after_name[..end].trim_end().ends_with('/'))
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

fn details_parts(html: &str) -> Option<Details> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<details")?;
    let end = lower.rfind("</details>")?;
    if end < open {
        return None;
    }
    let (summary, body_at) = if let Some(start) = lower.find("<summary") {
        let content = start + lower[start..].find('>')? + 1;
        let close = content + lower[content..].find("</summary>")?;
        let summary = crate::html_style::visible_text(&html[content..close]);
        let summary = if summary.is_empty() {
            "Details".to_string()
        } else {
            summary
        };
        (summary, close + "</summary>".len())
    } else {
        let after = open + lower[open..].find('>')? + 1;
        ("Details".to_string(), after)
    };
    if body_at > end {
        return None;
    }
    let open_tag_end = open + lower[open..].find('>')? + 1;
    let starts_open = html[open..open_tag_end].split_whitespace().any(|part| {
        part.trim_matches(|ch: char| matches!(ch, '<' | '>' | '/'))
            .split('=')
            .next()
            .unwrap_or("")
            .eq_ignore_ascii_case("open")
    });
    Some(Details {
        summary,
        body: html[body_at..end].trim().to_string(),
        starts_open,
    })
}

fn strip_footnote(raw: &str) -> String {
    let mut lines = raw.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let mut body = first
        .split_once("]:")
        .map(|(_, rest)| rest.trim_start().to_string())
        .unwrap_or_default();
    for line in lines {
        body.push('\n');
        if let Some(rest) = line.strip_prefix("    ") {
            body.push_str(rest);
        } else if let Some(rest) = line.strip_prefix('\t') {
            body.push_str(rest);
        } else {
            body.push_str(line);
        }
    }
    body
}

struct HtmlInline;

impl MarkdownPlugin for HtmlInline {
    fn name(&self) -> &str {
        "html-inline"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::Link(link) = node else {
            return None;
        };
        let kind = link.url.strip_prefix(crate::html_style::HTML_WIDGET)?;
        if !matches!(kind, "kbd" | "sup" | "sub" | "small") {
            return None;
        }
        let label = plain_text(node);
        if label.is_empty() {
            return None;
        }
        Some(MarkdownNode::new("html-inline", kind.to_string()).text(label))
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let kind = node.data::<String>()?;
        let text = node.as_text().to_string();
        let size = context.font_size();
        let element = match kind.as_str() {
            "kbd" => {
                let box_size = size * 0.85;
                div()
                    .px_1()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(3.))
                    .bg(cx.theme().secondary)
                    .text_size(box_size)
                    .line_height(box_size)
                    .child(text)
                    .into_any_element()
            }
            "sup" => {
                let box_size = size * 0.65;
                div()
                    .text_size(box_size)
                    .line_height(box_size)
                    .child(text)
                    .into_any_element()
            }
            "sub" => {
                let box_size = size * 0.65;
                div()
                    .text_size(box_size)
                    .line_height(box_size)
                    .child(text)
                    .into_any_element()
            }
            _ => {
                let box_size = size * 0.85;
                div()
                    .text_size(box_size)
                    .line_height(box_size)
                    .text_color(cx.theme().muted_foreground)
                    .child(text)
                    .into_any_element()
            }
        };
        let baseline = match kind.as_str() {
            "sup" => size,
            "sub" => size * 0.15,
            "small" => size * 0.75,
            _ => size * 0.8,
        };
        Some(InlineElement::new(element).with_baseline(baseline))
    }
}

fn plain_text(node: &markdown_ast::Node) -> String {
    match node {
        markdown_ast::Node::Text(text) => text.value.clone(),
        markdown_ast::Node::InlineCode(code) => code.value.clone(),
        other => other
            .children()
            .map(|children| children.iter().map(plain_text).collect())
            .unwrap_or_default(),
    }
}

struct FootnoteMark;

impl MarkdownPlugin for FootnoteMark {
    fn name(&self) -> &str {
        "fn-ref"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::FootnoteReference(note) = node else {
            return None;
        };
        let label = if note.identifier.is_empty() {
            note.label.clone().unwrap_or_default()
        } else {
            note.identifier.clone()
        };
        Some(MarkdownNode::new("fn-ref", ()).text(label))
    }

    fn render(
        &self,
        node: &MarkdownNode,
        _: &mut Window,
        cx: &mut App,
    ) -> impl gpui_kit::IntoElement {
        div()
            .text_xs()
            .text_color(cx.theme().link)
            .child(node.as_text().to_string())
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let size = context.font_size() * (2. / 3.);
        Some(
            InlineElement::new(
                div()
                    .text_size(size)
                    .line_height(size)
                    .text_color(cx.theme().link)
                    .child(node.as_text().to_string()),
            )
            .with_baseline(context.font_size()),
        )
    }
}

struct ImageRef {
    links: Arc<HashMap<String, String>>,
    images: Arc<Option<ImageRoots>>,
}

impl MarkdownPlugin for ImageRef {
    fn name(&self) -> &str {
        "img-ref"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        match node {
            markdown_ast::Node::Image(image) => {
                let alt = image.alt.clone();
                Some(
                    MarkdownNode::new(
                        "img-ref",
                        Figure {
                            url: image.url.clone(),
                            alt: alt.clone(),
                        },
                    )
                    .text(alt),
                )
            }
            markdown_ast::Node::ImageReference(image) => {
                let url = self
                    .links
                    .get(&image.identifier.to_ascii_lowercase())?
                    .clone();
                let alt = image.alt.clone();
                Some(
                    MarkdownNode::new(
                        "img-ref",
                        Figure {
                            url,
                            alt: alt.clone(),
                        },
                    )
                    .text(alt),
                )
            }
            _ => None,
        }
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        _: &InlineRenderContext,
        _: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        let figure = node.data::<Figure>()?;
        let source = figure_source(&figure.url, self.images.as_ref())?;
        Some(InlineElement::new(
            img(source)
                .id(SharedString::from(format!("in-{}", text_key(&figure.url))))
                .h(px(18.))
                .object_fit(ObjectFit::Contain),
        ))
    }
}

fn data_image(url: &str) -> Option<Image> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime = meta.split(';').next()?.trim();
    let format = ImageFormat::from_mime_type(mime)?;
    let bytes = if meta.split(';').any(|part| part.trim() == "base64") {
        decode_base64(data)?
    } else {
        percent_decode(data)?
    };
    if bytes.is_empty() || bytes.len() > MAX_IMAGE as usize {
        return None;
    }
    Some(Image::from_bytes(format, bytes))
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let (Some(a), Some(b)) = (value(chunk[0]), value(chunk[1])) else {
            return None;
        };
        let pad = chunk.iter().filter(|byte| **byte == b'=').count();
        if pad > 2 {
            return None;
        }
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            let c = value(chunk[2])?;
            out.push((b << 4) | (c >> 2));
            if chunk[3] != b'=' {
                let d = value(chunk[3])?;
                out.push((c << 6) | d);
            }
        }
    }
    Some(out)
}

fn percent_decode(text: &str) -> Option<Vec<u8>> {
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
        } else if bytes[index] == b'+' {
            out.push(b' ');
            index += 1;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Some(out)
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
    fn details_keeps_the_summary_and_the_body() {
        let html = "<details>\n<summary>Click to expand</summary>\n\nBody\n\n</details>";
        let details = details_parts(html).expect("details");
        assert_eq!(details.summary, "Click to expand");
        assert_eq!(details.body, "Body");
    }

    #[test]
    fn shielded_details_keep_the_summary_and_the_sentence() {
        let source = "\
<details>
<summary>Click to expand</summary>

Some Markdown renderers allow inline HTML, but support varies.

</details>

After.
";
        let shielded = shield_details(source);
        assert!(shielded.ends_with("After.\n"));
        let details = details_parts(&fence_html(&shielded)).expect("details");
        assert_eq!(details.summary, "Click to expand");
        assert_eq!(
            details.body,
            "Some Markdown renderers allow inline HTML, but support varies."
        );
    }

    #[test]
    fn code_fence_keeps_a_details_tag() {
        let source = "```\n<details>\n</details>\n```\n";
        assert_eq!(shield_details(source), source);
    }

    #[test]
    fn a_curly_apostrophe_stays_prose() {
        let source = "Someone’s notes stay prose.\n";
        assert_eq!(shield_details(source), source);
    }

    #[test]
    fn unclosed_details_stays_literal() {
        let source = "<details>\n<summary>Click to expand</summary>\n\nStill open.\n";
        assert_eq!(shield_details(source), source);
    }

    fn fence_html(source: &str) -> String {
        let ticks = source.chars().take_while(|ch| *ch == '`').count();
        let rest = source[ticks..]
            .strip_prefix("runebook-details\n")
            .expect("info");
        let close = format!("\n{}", "`".repeat(ticks));
        let end = rest.rfind(&close).expect("close");
        rest[..end].to_string()
    }

    #[test]
    fn footnote_body_drops_the_marker_and_the_indent() {
        let body = strip_footnote("[^note]: First\n    second\n");
        assert_eq!(body, "First\nsecond");
    }

    #[test]
    fn data_image_decodes_png_base64() {
        let image = data_image("data:image/png;base64,iVBORw0KGgo=").expect("png");
        assert_eq!(image.format(), ImageFormat::Png);
        assert_eq!(image.bytes(), b"\x89PNG\r\n\x1a\n");
    }
}
