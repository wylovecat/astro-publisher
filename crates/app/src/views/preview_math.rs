//! Formulas in the editor preview, drawn natively.
//!
//! The preview renders Markdown with TextView's Markdown parser, which also
//! hands `$$…$$` and `$…$` to a plugin. This one typesets the formula with
//! KaTeX (the same engine the published site uses), then draws the result as
//! GPUI elements through [`astro_publisher_core::math_dom`], which reads
//! KaTeX's layout back as a tree. A browser would be the alternative; this app
//! has no web view, and KaTeX's own markup is HTML and CSS that GPUI cannot
//! interpret.
//!
//! Approximations worth knowing about, all of them KaTeX's own doing rather
//! than this renderer: glyphs KaTeX ships as SVG (the radical sign, `\vec`'s
//! arrow) are drawn from Unicode or omitted, and vertical metrics come from
//! the type size rather than from KaTeX's pixel offsets.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use astro_publisher_core::math_dom::{
    ColumnAlign, MathDom, MathNode, TextStyle, is_empty, parse_formula,
};
use gpui::prelude::*;
use gpui::{AnyElement, App, FontWeight, IntoElement, Pixels, SharedString, Window, div, px};
use gpui_component::ActiveTheme;
use gpui_component::text::{
    InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    markdown_ast,
};

const INLINE_NAME: &str = "astro-publisher/inline-math";
const BLOCK_NAME: &str = "astro-publisher/display-math";

/// Typesets `$…$` (`display = false`) or `$$…$$` (`display = true`) for
/// TextView's Markdown parser.
pub struct MathPlugin {
    display: bool,
}

impl MathPlugin {
    /// Inline math: `$x$`.
    pub fn inline() -> Self {
        Self { display: false }
    }

    /// Display math: `$$…$$`.
    pub fn block() -> Self {
        Self { display: true }
    }
}

impl MarkdownPlugin for MathPlugin {
    fn is_block(&self) -> bool {
        self.display
    }

    fn name(&self) -> &str {
        if self.display {
            BLOCK_NAME
        } else {
            INLINE_NAME
        }
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let source = match node {
            markdown_ast::Node::Math(math) if self.display => &math.value,
            markdown_ast::Node::InlineMath(math) if !self.display => &math.value,
            _ => return None,
        };
        // A formula KaTeX cannot typeset returns `None`, which leaves TextView's
        // own fallback in place: the source stays visible rather than the
        // preview losing it.
        let dom = typeset(source, self.display)?;
        Some(
            MarkdownNode::new(self.name(), dom.clone())
                .text(dom.flat_text())
                .markdown(cx.node_source(node).unwrap_or(source)),
        )
    }

    fn render(&self, node: &MarkdownNode, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let dom = node.data::<MathDom>().cloned();
        let ink = Ink::for_block(cx);
        match dom {
            Some(dom) => render_formula(&dom, &ink).into_any_element(),
            // The node only exists when typesetting succeeded, so this is
            // unreachable in practice; showing the text keeps it harmless.
            None => node.as_text().to_string().into_any_element(),
        }
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let dom = node.data::<MathDom>()?.clone();
        let ink = Ink::for_inline(context, cx);
        Some(InlineElement::new(render_formula(&dom, &ink)))
    }
}

/// Typeset results by source, so a repeated formula costs nothing.
type TypesetCache = Mutex<HashMap<(String, bool), Option<MathDom>>>;

/// Typeset a formula, remembering the result.
///
/// TextView re-parses the document on every keystroke, and KaTeX runs a
/// JavaScript engine per formula; a post with a couple of hundred formulas
/// would otherwise pay for all of them again each time. The cache is keyed by
/// the source. A mutex rather than a thread-local because the parse may happen
/// off the UI thread.
fn typeset(source: &str, display: bool) -> Option<MathDom> {
    static CACHE: OnceLock<TypesetCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (source.to_owned(), display);

    if let Ok(cache) = cache.lock()
        && let Some(hit) = cache.get(&key)
    {
        return hit.clone();
    }
    let dom = parse_formula(source, display).ok();
    if let Ok(mut cache) = cache.lock() {
        // A post cannot hold more formulas than this, but the bound stops a
        // long editing session from growing the map without end.
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(key, dom.clone());
    }
    dom
}

/// The colours, fonts and type size a formula is drawn with.
struct Ink {
    color: gpui::Hsla,
    border: gpui::Hsla,
    font: SharedString,
    mono: SharedString,
    /// Size of one em, the unit every KaTeX offset is expressed in.
    em: Pixels,
}

impl Ink {
    fn for_block(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            color: theme.foreground,
            border: theme.foreground,
            font: theme.font_family.clone(),
            mono: theme.mono_font_family.clone(),
            em: theme.font_size,
        }
    }

    fn for_inline(context: &InlineRenderContext, cx: &mut App) -> Self {
        Self {
            color: context.text_style().color,
            font: context.text_style().font_family.clone(),
            ..Self::for_block(cx)
        }
    }

    /// The type size as a number, so an offset can be a fraction of an em.
    fn em_value(&self) -> f32 {
        f32::from(self.em)
    }

    /// One em at a given size, clamped so a broken scale cannot vanish.
    fn em(&self, scale: f32) -> Pixels {
        px((self.em_value() * scale.clamp(0.2, 4.0)).max(1.0))
    }
}

/// A whole formula: a centred line for `$$…$$`, an inline box for `$…$`.
fn render_formula(dom: &MathDom, ink: &Ink) -> AnyElement {
    let body = render(&dom.root, ink, 1.0);
    if dom.display {
        div()
            .flex()
            .flex_col()
            .w_full()
            .items_center()
            .py_2()
            .child(body)
            .into_any_element()
    } else {
        div()
            .flex()
            .flex_row()
            .items_baseline()
            .child(body)
            .into_any_element()
    }
}

/// Draw one node. `scale` is the font size relative to the formula's own.
fn render(node: &MathNode, ink: &Ink, scale: f32) -> AnyElement {
    match node {
        MathNode::Row(children) => row(children, ink, scale),
        MathNode::Text(text) => {
            let color = match text.color {
                Some([red, green, blue]) => gpui::Rgba {
                    r: red as f32 / 255.0,
                    g: green as f32 / 255.0,
                    b: blue as f32 / 255.0,
                    a: 1.0,
                }
                .into(),
                None => ink.color,
            };
            div()
                .text_size(ink.em(text.scale * scale))
                .text_color(color)
                .font_family(if text.style == TextStyle::Mono {
                    ink.mono.clone()
                } else {
                    ink.font.clone()
                })
                .when(text.style == TextStyle::Italic, |this| this.italic())
                .when(text.style == TextStyle::Bold, |this| {
                    this.font_weight(FontWeight::BOLD)
                })
                .child(text.text.clone())
                .into_any_element()
        }
        MathNode::Space(width) => div().w(ink.em(width * scale)).into_any_element(),
        MathNode::Frac {
            numerator,
            denominator,
        } => {
            // Numerator, rule, denominator, each centred on the widest piece.
            let half = scale * 0.88;
            div()
                .flex()
                .flex_col()
                .items_stretch()
                .px(px(2.0))
                .child(centre(numerator, ink, half))
                .child(rule(ink))
                .child(centre(denominator, ink, half))
                .into_any_element()
        }
        MathNode::Scripts { base, sup, sub } => {
            stacked(base, sup.as_deref(), sub.as_deref(), ink, scale)
        }
        MathNode::OverUnder { base, over, under } => {
            stacked(base, over.as_deref(), under.as_deref(), ink, scale)
        }
        MathNode::Accent { base, accent } => {
            // Stacked rather than overlapped: the mark sits centred above the
            // base without needing either one's measured width.
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(if is_empty(accent) {
                    div().into_any_element()
                } else {
                    render(accent, ink, scale * 0.8)
                })
                .child(render(base, ink, scale))
                .into_any_element()
        }
        MathNode::Rule { body, above } => {
            let body = centre(body, ink, scale);
            if *above {
                div()
                    .flex()
                    .flex_col()
                    .items_stretch()
                    .child(rule(ink))
                    .child(body)
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .items_stretch()
                    .child(body)
                    .child(rule(ink))
                    .into_any_element()
            }
        }
        MathNode::Framed { body } => div()
            .border_1()
            .border_color(ink.border)
            .rounded(px(3.0))
            .px(px(4.0))
            .py(px(2.0))
            .mx(px(2.0))
            .child(render(body, ink, scale))
            .into_any_element(),
        MathNode::Radical { body, index } => {
            // The radical sign stands in for the SVG glyph KaTeX ships; `Div` is not
            // `Clone`, so the covered part is built by this closure.
            let overline = || {
                div()
                    .flex()
                    .flex_col()
                    .items_stretch()
                    .child(rule(ink))
                    .child(centre(body, ink, scale))
            };
            let radical = div()
                .flex()
                .flex_row()
                .items_stretch()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .text_size(ink.em(scale * 1.1))
                        .text_color(ink.color)
                        .font_family(ink.font.clone())
                        .child("√"),
                )
                .child(overline())
                .into_any_element();
            let Some(index) = index else {
                return radical;
            };
            div()
                .flex()
                .flex_row()
                .items_start()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .pr(px(1.0))
                        .mr(px(-3.0))
                        .child(render(index, ink, scale * 0.6)),
                )
                .child(overline())
                .into_any_element()
        }
        MathNode::Table { rows, align } => {
            // Laid out by column rather than by row: cells of one column then
            // line up, which is the whole point of `array` and `cases`.
            let height = rows.iter().map(Vec::len).max().unwrap_or(0);
            div()
                .flex()
                .flex_row()
                .items_start()
                .py(px(2.0))
                .children((0..height).map(|column| {
                    let kind = align.get(column).copied().unwrap_or(ColumnAlign::Center);
                    div()
                        .flex()
                        .flex_col()
                        .px(px(3.0))
                        .when(kind == ColumnAlign::Center, |this| this.items_center())
                        .when(kind == ColumnAlign::Right, |this| this.items_end())
                        .children((0..height).map(|row| {
                            let cell = rows
                                .get(row)
                                .and_then(|row| row.get(column))
                                .cloned()
                                .unwrap_or(MathNode::Row(Vec::new()));
                            render(&cell, ink, scale)
                        }))
                }))
                .into_any_element()
        }
    }
}

/// Symbols side by side, aligned on their baselines.
fn row(children: &[MathNode], ink: &Ink, scale: f32) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_baseline()
        .children(children.iter().map(|child| render(child, ink, scale)))
        .into_any_element()
}

/// One piece centred inside a stretch, which is how KaTeX keeps the halves of a
/// fraction and the cells of a table on one axis.
fn centre(node: &MathNode, ink: &Ink, scale: f32) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .justify_center()
        .child(render(node, ink, scale))
        .into_any_element()
}

/// A horizontal rule one pixel high, full width.
fn rule(ink: &Ink) -> AnyElement {
    div()
        .flex()
        .flex_shrink_0()
        .h(px(1.0))
        .bg(ink.color)
        .my(px(1.0))
        .into_any_element()
}

/// Size of a script relative to its base, the factor KaTeX uses for `\scriptstyle`.
const SCRIPT_SCALE: f32 = 0.72;

/// A base with scripts painted out of flow, anchored just past its right edge.
///
/// Stacking them in an ordinary column is the obvious layout and it is wrong
/// here: the column is taller than the base, so whatever baseline the row picks
/// up from it drags the rest of the formula down with it. An out-of-flow anchor
/// keeps the base as the row's only in-flow child, so the line keeps its
/// baseline, while the anchor reserves the width the scripts need.
fn stacked(
    base: &MathNode,
    first: Option<&MathNode>,
    second: Option<&MathNode>,
    ink: &Ink,
    scale: f32,
) -> AnyElement {
    let em = ink.em_value() * scale;
    // A rough width for the scripts: half an em per character is close enough
    // for the digits and short labels scripts are made of, and it only decides
    // how far the line advances.
    let width = [first, second]
        .into_iter()
        .flatten()
        .map(|node| script_width(node, ink, scale))
        .fold(0.0f32, f32::max);

    let mut anchor = div()
        .relative()
        .w(px(width))
        .h(px(0.0))
        // Room for both scripts, so the line box holds them instead of clipping.
        .pt(px(em * 0.55))
        .pb(px(em * 0.45));
    if let Some(node) = first {
        anchor = anchor.child(div().absolute().left_0().top(px(-em * 0.62)).child(render(
            node,
            ink,
            scale * SCRIPT_SCALE,
        )));
    }
    if let Some(node) = second {
        anchor = anchor.child(div().absolute().left_0().top(px(em * 0.48)).child(render(
            node,
            ink,
            scale * SCRIPT_SCALE,
        )));
    }

    div()
        .flex()
        .flex_row()
        .items_baseline()
        .child(render(base, ink, scale))
        .child(anchor)
        .into_any_element()
}

/// An estimate of how wide a script draws, in pixels.
fn script_width(node: &MathNode, ink: &Ink, scale: f32) -> f32 {
    let size = f32::from(ink.em(scale * SCRIPT_SCALE));
    match node {
        MathNode::Text(text) => text.text.chars().count() as f32 * size * 0.5,
        MathNode::Space(width) => f32::from(ink.em(width * scale)),
        MathNode::Row(children) => children
            .iter()
            .map(|child| script_width(child, ink, scale))
            .sum(),
        _ => size,
    }
}
