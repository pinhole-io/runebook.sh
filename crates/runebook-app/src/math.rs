//! Draw `$...$` and `$$...$$` as SVG.
//!
//! The formula is note text. The renderer stays in process and does not
//! run a shell. A formula longer than 8 KiB stays source text. A formula
//! the renderer cannot parse stays source text. The SVG is paths from the
//! embedded math font.

use std::sync::{Mutex, OnceLock};

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::text::{
    InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    markdown_ast,
};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Hsla, Image, ImageFormat, ObjectFit, Window, div, img, px};
use latex_rust::{Color, Dim, MathFont, MathStyle, SvgOptions, layout, parse, render_svg};

const MAX_FORMULA: usize = 8 * 1024;
const PT_PER_PX: f32 = 72.0 / 96.0;
const PX_PER_PT: f32 = 96.0 / 72.0;

struct Formula {
    latex: String,
    display: bool,
    source: String,
}

struct Picture {
    bytes: std::sync::Arc<Vec<u8>>,
    width: f32,
    height: f32,
    baseline: f32,
}

/// Display math (`$$...$$`).
pub(crate) struct MathBlock;

/// Inline math (`$...$`).
pub(crate) struct MathLine;

impl MarkdownPlugin for MathBlock {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "math-block"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::Math(math) = node else {
            return None;
        };
        formula(math.value.trim(), true, cx.node_source(node))
    }

    fn render(&self, node: &MarkdownNode, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let child = match node.data::<Formula>() {
            Some(formula) => match picture(formula, cx.theme().foreground, body_px()) {
                Some(drawn) => formula_image(&drawn).into_any_element(),
                None => fallback(formula).into_any_element(),
            },
            None => div().into_any_element(),
        };
        div().py_2().child(child)
    }
}

impl MarkdownPlugin for MathLine {
    fn name(&self) -> &str {
        "math-line"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::InlineMath(math) = node else {
            return None;
        };
        let latex = math.value.trim();
        if !looks_like_math(latex) {
            return None;
        }
        formula(latex, false, cx.node_source(node))
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let formula = node.data::<Formula>()?;
        let mut color = context.text_style().color;
        if color.a == 0.0 {
            color = cx.theme().foreground;
        }
        let drawn = picture(formula, color, context.font_size().as_f32())?;
        Some(InlineElement::new(formula_image(&drawn)).with_baseline(px(drawn.baseline)))
    }
}

fn formula(latex: &str, display: bool, source: Option<&str>) -> Option<MarkdownNode> {
    if latex.is_empty() || latex.len() > MAX_FORMULA {
        return None;
    }
    let source = source.unwrap_or(latex).to_string();
    Some(
        MarkdownNode::new(
            if display { "math-block" } else { "math-line" },
            Formula {
                latex: latex.to_string(),
                display,
                source: source.clone(),
            },
        )
        .text(source.clone())
        .markdown(source),
    )
}

fn fallback(formula: &Formula) -> AnyElement {
    div().child(formula.source.clone()).into_any_element()
}

fn formula_image(drawn: &Picture) -> impl IntoElement {
    img(std::sync::Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        drawn.bytes.as_ref().clone(),
    )))
    .w(px(drawn.width))
    .h(px(drawn.height))
    .object_fit(ObjectFit::Fill)
}

fn body_px() -> f32 {
    crate::theme::body_font_size()
}

/// Inline `$5 and $10` is a price, not a formula.
fn looks_like_math(latex: &str) -> bool {
    if latex.is_empty() || latex.len() > MAX_FORMULA {
        return false;
    }
    let special = latex.contains(['\\', '^', '_', '{', '}']);
    if latex.contains(char::is_whitespace) && !special {
        return false;
    }
    true
}

fn picture(formula: &Formula, color: Hsla, font_px: f32) -> Option<Picture> {
    if !(font_px.is_finite() && font_px > 0.0) {
        return None;
    }
    let (red, green, blue) = rgb(color);
    let key = format!(
        "{}|{red}|{green}|{blue}|{font_px:.2}|{}",
        formula.display, formula.latex
    );
    let mut slots = slots().lock().unwrap_or_else(|err| err.into_inner());
    if let Some(drawn) = slots.get(&key) {
        return Some(drawn.clone());
    }
    let drawn = render_formula(&formula.latex, formula.display, red, green, blue, font_px)?;
    if slots.len() >= 64 {
        slots.clear();
    }
    slots.insert(key, drawn.clone());
    Some(drawn)
}

fn render_formula(
    latex: &str,
    display: bool,
    red: u8,
    green: u8,
    blue: u8,
    font_px: f32,
) -> Option<Picture> {
    let font = math_font()?;
    let ast = parse(latex).ok()?;
    let style = if display {
        MathStyle::Display
    } else {
        MathStyle::Text
    };
    let tree = layout(&ast, font, style).ok()?;
    let em_pt = Dim::parse(&format!("{:.4}", font_px * PT_PER_PX));
    let mut options = SvgOptions::new();
    options.font_size_pt = em_pt.clone();
    options.color = Color::rgb(red, green, blue);
    options.display = display;
    let svg = render_svg(&tree, font, &options).ok()?;
    let width = dim_px(&tree.width, &em_pt);
    let above = dim_px(&tree.height, &em_pt);
    let below = dim_px(&tree.depth, &em_pt);
    let height = above + below;
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return None;
    }
    Some(Picture {
        bytes: std::sync::Arc::new(svg.into_bytes()),
        width,
        height,
        baseline: above,
    })
}

fn dim_px(em: &Dim, em_pt: &Dim) -> f32 {
    let pt = em * em_pt;
    f32::from_bits(pt.to_ieee32_bits()) * PX_PER_PT
}

fn rgb(color: Hsla) -> (u8, u8, u8) {
    let rgba = color.to_rgb();
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    (byte(rgba.r), byte(rgba.g), byte(rgba.b))
}

fn math_font() -> Option<&'static MathFont> {
    static FONT: OnceLock<Option<MathFont>> = OnceLock::new();
    FONT.get_or_init(|| MathFont::stix_two_math().ok()).as_ref()
}

fn slots() -> &'static Mutex<std::collections::HashMap<String, Picture>> {
    static SLOTS: OnceLock<Mutex<std::collections::HashMap<String, Picture>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

impl Clone for Picture {
    fn clone(&self) -> Self {
        Self {
            bytes: std::sync::Arc::clone(&self.bytes),
            width: self.width,
            height: self.height,
            baseline: self.baseline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{looks_like_math, render_formula};

    #[test]
    fn a_price_is_not_a_formula() {
        assert!(!looks_like_math("5 and 10"));
    }

    #[test]
    fn an_equation_is_a_formula() {
        assert!(looks_like_math(r"E = mc^2"));
        assert!(looks_like_math(r"\frac{n(n+1)}{2}"));
    }

    #[test]
    fn display_math_becomes_svg() {
        let picture = render_formula(
            r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}",
            true,
            220,
            220,
            220,
            16.0,
        )
        .expect("svg");
        let svg = String::from_utf8(picture.bytes.as_ref().clone()).expect("utf8");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("<path"));
        assert!(picture.width > 0.0);
        assert!(picture.height > picture.baseline);
    }

    #[test]
    fn inline_math_becomes_svg() {
        let picture = render_formula(r"E = mc^2", false, 220, 220, 220, 14.0).expect("svg");
        assert!(picture.width > 0.0);
        assert!(picture.baseline > 0.0);
    }
}
