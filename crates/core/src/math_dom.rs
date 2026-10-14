//! KaTeX's HTML, read back as a layout tree.
//!
//! [`crate::preview::render_formula`] produces exactly the markup the published
//! site renders in a browser, but the desktop app has no CSS engine: handing
//! that markup to a text engine shows the MathML source and a squashed layout,
//! which is what the editor preview used to do. KaTeX's vertical lists are
//! however a complete description of the layout — each entry is one piece of a
//! fraction, radical, frame, accent, or script stack — so this module reads
//! them back into an explicit tree the app can lay out natively.
//!
//! Only the visual half of the markup is read. The `.katex-mathml` copy (which
//! carries the original TeX in an `<annotation>`) is skipped, as are the SVG
//! glyphs KaTeX uses for radicals and stretchy delimiters: the app draws those
//! from Unicode and box geometry instead.

use html5ever::ParseOpts;
use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

/// A typeset formula, ready to be laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct MathDom {
    /// Display formulas (`$$…$$`) sit on their own centred line.
    pub display: bool,
    pub root: MathNode,
}

/// One primitive of a formula.
#[derive(Debug, Clone, PartialEq)]
pub enum MathNode {
    /// Symbols laid out left to right.
    Row(Vec<MathNode>),
    /// `\frac`: a numerator above a rule, above a denominator.
    Frac {
        numerator: Box<MathNode>,
        denominator: Box<MathNode>,
    },
    /// A base carrying scripts: `x²`, `aᵢ`, `aᵢⁿ`.
    Scripts {
        base: Box<MathNode>,
        sup: Option<Box<MathNode>>,
        sub: Option<Box<MathNode>>,
    },
    /// Scripts stacked above and below a wide operator: `\sum_{i}^{n}`.
    OverUnder {
        base: Box<MathNode>,
        over: Option<Box<MathNode>>,
        under: Option<Box<MathNode>>,
    },
    /// `\hat`, `\vec`: a mark directly above the base.
    Accent {
        base: Box<MathNode>,
        accent: Box<MathNode>,
    },
    /// `\overline` / `\underline`: the body with a rule beside it.
    Rule { body: Box<MathNode>, above: bool },
    /// `\boxed`: the body inside a frame.
    Framed { body: Box<MathNode> },
    /// `\sqrt`, with the optional index of `\sqrt[n]{…}`.
    Radical {
        body: Box<MathNode>,
        index: Option<Box<MathNode>>,
    },
    /// `array`, `matrix`, `cases`: rows of cells, top row first.
    Table {
        rows: Vec<Vec<MathNode>>,
        align: Vec<ColumnAlign>,
    },
    /// A run of symbols sharing one style.
    Text(MathText),
    /// Horizontal space, in multiples of the base font size.
    Space(f32),
}

/// A run of glyphs sharing one style.
#[derive(Debug, Clone, PartialEq)]
pub struct MathText {
    pub text: String,
    /// Font size relative to the surrounding text.
    pub scale: f32,
    pub style: TextStyle,
    /// `\color{…}` as an RGB triple.
    pub color: Option<[u8; 3]>,
}

/// How a run of glyphs is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextStyle {
    /// Variables, set slanted.
    Italic,
    /// Digits, operators, `\mathrm`, `\text`.
    Upright,
    /// `\mathbf`.
    Bold,
    /// `\mathtt`.
    Mono,
}

/// Column alignment of a table cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnAlign {
    Left,
    Center,
    Right,
}

/// Typeset `tex` and read KaTeX's markup back as a layout tree.
///
/// Returns the engine's error for a formula KaTeX cannot typeset at all, so a
/// caller can fall back to showing the source.
pub fn parse_formula(tex: &str, display: bool) -> Result<MathDom, String> {
    let html = crate::preview::render_formula(tex, display)?;
    parse_katex_html(&html, display).ok_or_else(|| "KaTeX 输出里找不到 katex-html 节点".to_owned())
}

/// Read already-rendered KaTeX markup back as a layout tree.
pub fn parse_katex_html(html: &str, display: bool) -> Option<MathDom> {
    let mut bytes = html.as_bytes();
    let dom = parse_document(RcDom::default(), ParseOpts::default())
        .from_utf8()
        .read_from(&mut bytes)
        .ok()?;
    let half = find_by_class(&dom.document, "katex-html")?;
    let root = convert(&half, &Ctx::default());
    Some(MathDom { display, root })
}

impl MathDom {
    /// Every character of the formula, for copy/selection and accessibility.
    pub fn flat_text(&self) -> String {
        flat_text(&self.root)
    }
}

/// Flatten a subtree to its characters.
pub fn flat_text(node: &MathNode) -> String {
    let mut out = String::new();
    push_text(node, &mut out);
    out
}

fn push_text(node: &MathNode, out: &mut String) {
    match node {
        MathNode::Row(children) => children.iter().for_each(|child| push_text(child, out)),
        MathNode::Text(text) => out.push_str(&text.text),
        MathNode::Space(_) => {}
        MathNode::Frac {
            numerator,
            denominator,
        } => {
            push_text(numerator, out);
            out.push('/');
            push_text(denominator, out);
        }
        MathNode::Scripts { base, sup, sub } => {
            push_text(base, out);
            if let Some(sub) = sub {
                push_text(sub, out);
            }
            if let Some(sup) = sup {
                push_text(sup, out);
            }
        }
        MathNode::OverUnder { base, over, under } => {
            push_text(base, out);
            if let Some(under) = under {
                push_text(under, out);
            }
            if let Some(over) = over {
                push_text(over, out);
            }
        }
        MathNode::Accent { base, accent } => {
            push_text(base, out);
            push_text(accent, out);
        }
        MathNode::Rule { body, .. }
        | MathNode::Framed { body }
        | MathNode::Radical { body, .. } => push_text(body, out),
        MathNode::Table { rows, .. } => {
            for row in rows {
                for cell in row {
                    push_text(cell, out);
                    out.push(' ');
                }
                out.push('\n');
            }
        }
    }
}

/// True when a node draws nothing. KaTeX pads `\frac` and `\sum` with empty
/// delimiter placeholders that must not become blank boxes.
pub fn is_empty(node: &MathNode) -> bool {
    match node {
        MathNode::Row(children) => children.iter().all(is_empty),
        MathNode::Text(text) => text.text.is_empty(),
        _ => false,
    }
}

/// Inherited state while walking KaTeX's markup.
struct Ctx {
    /// Font size multiplier from a `sizing` wrapper.
    scale: f32,
}

impl Default for Ctx {
    fn default() -> Self {
        Self { scale: 1.0 }
    }
}

/// One child of an element, before scripts are attached to their base.
enum Slot {
    /// A symbol or a structure.
    Node(Handle),
    /// A script belonging to the symbol before it.
    Script { node: Handle, hint: ScriptHint },
    /// Bare text, such as the letters of `\sin`.
    Text(String),
}

/// What an element's name says about the scripts it holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptHint {
    /// `msupsub`: sub or sup, decided by the vertical list it contains.
    Either,
    Sub,
    Sup,
}

/// Convert one node.
fn convert(node: &Handle, ctx: &Ctx) -> MathNode {
    match &node.data {
        NodeData::Text { contents } => {
            let text = contents.borrow().to_string();
            if text.is_empty() {
                return MathNode::Row(Vec::new());
            }
            MathNode::Text(MathText {
                text,
                scale: ctx.scale,
                style: TextStyle::Upright,
                color: None,
            })
        }
        NodeData::Element { name, attrs, .. } => {
            let classes = class_list(attrs);
            if is_skipped(name.local.as_ref(), &classes) {
                return MathNode::Row(Vec::new());
            }
            if let Some(structure) = convert_structure(node, ctx) {
                return structure;
            }

            let children = fold(node, ctx);
            if element_children(node).is_empty() {
                return convert_leaf(node, &classes, ctx);
            }
            if children.len() == 1 {
                children.into_iter().next().unwrap()
            } else {
                MathNode::Row(children)
            }
        }
        _ => MathNode::Row(Vec::new()),
    }
}

/// Convert an element's children, attaching scripts to the symbol they follow.
fn fold(node: &Handle, ctx: &Ctx) -> Vec<MathNode> {
    let mut slots: Vec<Slot> = Vec::new();
    for child in node.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => {
                let text = contents.borrow().to_string();
                if !text.is_empty() {
                    slots.push(Slot::Text(text));
                }
            }
            NodeData::Element { name, attrs, .. } => {
                let classes = class_list(attrs);
                if is_skipped(name.local.as_ref(), &classes) {
                    continue;
                }
                match script_hint(&classes) {
                    Some(hint) => slots.push(Slot::Script {
                        node: child.clone(),
                        hint,
                    }),
                    None => slots.push(Slot::Node(child.clone())),
                }
            }
            _ => {}
        }
    }

    let mut out: Vec<MathNode> = Vec::new();
    // The symbol a following script attaches to.
    let mut pending: Option<MathNode> = None;
    for slot in slots {
        match slot {
            Slot::Node(child) => {
                if let Some(node) = pending.take() {
                    out.push(node);
                }
                let converted = convert(&child, ctx);
                if !is_empty(&converted) {
                    pending = Some(converted);
                }
            }
            Slot::Text(text) => {
                if let Some(node) = pending.take() {
                    out.push(node);
                }
                match out.last_mut() {
                    Some(MathNode::Text(run))
                        if run.style == TextStyle::Upright
                            && run.scale == ctx.scale
                            && run.color.is_none() =>
                    {
                        run.text.push_str(&text);
                    }
                    _ => out.push(MathNode::Text(MathText {
                        text,
                        scale: ctx.scale,
                        style: TextStyle::Upright,
                        color: None,
                    })),
                }
            }
            Slot::Script { node, hint } => {
                let (own_base, sup, sub) = split_script(&node, hint, ctx);
                match own_base.or_else(|| pending.take()) {
                    Some(base) => match (sup, sub) {
                        (None, None) => out.push(base),
                        (sup, sub) => out.push(MathNode::Scripts {
                            base: Box::new(base),
                            sup: sup.map(Box::new),
                            sub: sub.map(Box::new),
                        }),
                    },
                    // A script with nothing to attach to still has to show.
                    None => {
                        let scripts = [sup, sub].into_iter().flatten().collect::<Vec<_>>();
                        if !scripts.is_empty() {
                            out.push(MathNode::Row(scripts));
                        }
                    }
                }
            }
        }
    }
    if let Some(node) = pending.take() {
        out.push(node);
    }
    out
}

/// Is this element a script, and what does its name say about it?
///
/// `msup`, `msub` and `msubsup` can wrap their base (`\frac{a^b}{c}`), while a
/// bare `msupsub` is appended to a base that already sits in the parent (`x^2`
/// is one `mord` holding `x` and its script). [`fold`] handles both shapes.
fn script_hint(classes: &[String]) -> Option<ScriptHint> {
    let has = |name: &str| classes.iter().any(|class| class == name);
    if has("msupsub") {
        return Some(ScriptHint::Either);
    }
    if has("msub") {
        return Some(ScriptHint::Sub);
    }
    if has("msup") {
        return Some(ScriptHint::Sup);
    }
    if has("msubsup") {
        return Some(ScriptHint::Either);
    }
    None
}

/// Split a script element into the base it owns (if any), a superscript and a
/// subscript.
fn split_script(
    node: &Handle,
    hint: ScriptHint,
    ctx: &Ctx,
) -> (Option<MathNode>, Option<MathNode>, Option<MathNode>) {
    let children = element_children(node);
    let is_bare = classes_contain(node, "msupsub");
    let owns_base = !is_bare
        && children
            .first()
            .is_some_and(|first| script_hint(&class_of(first)).is_none());

    let mut base = None;
    let mut holder = None;
    for (index, child) in children.iter().enumerate() {
        if owns_base && index == 0 {
            base = Some(convert(child, ctx));
            continue;
        }
        if let Some(vlist) = find_by_class(child, "vlist") {
            holder = Some(vlist);
            break;
        }
    }

    let Some(vlist) = holder else {
        return (base, None, None);
    };
    let entries = vlist_entries(&vlist);
    let mut scripts: Vec<MathNode> = entries
        .iter()
        .map(|entry| content(&entry.node, ctx))
        .filter(|node| !is_empty(node))
        .collect();

    // A subscript is what makes the list two deep (`vlist-t2`), and that is the
    // only signal separating a lone `msupsub` subscript from a lone
    // superscript. With both, the superscript is the higher entry.
    let two_deep =
        find_by_class(node, "vlist-t2").is_some() || find_by_class(node, "vlist-s").is_some();
    let (sup, sub) = match (hint, scripts.len()) {
        (ScriptHint::Sup, 1) => (Some(scripts.remove(0)), None),
        (ScriptHint::Sub, 1) => (None, Some(scripts.remove(0))),
        (_, 0) => (None, None),
        (_, 1) if two_deep => (None, Some(scripts.remove(0))),
        (_, 1) => (Some(scripts.remove(0)), None),
        (_, 2) => {
            let sup = scripts.remove(0);
            let sub = scripts.remove(0);
            (Some(sup), Some(sub))
        }
        _ => (None, None),
    };
    (base, sup, sub)
}

/// Is this an element KaTeX uses for layout only?
fn is_skipped(tag: &str, classes: &[String]) -> bool {
    if matches!(tag, "svg" | "path" | "rect") {
        return true;
    }
    classes.iter().any(|class| {
        matches!(
            class.as_str(),
            "strut" | "pstrut" | "vlist-s" | "hide-tail" | "overlay" | "fix"
        )
    })
}

/// The structures KaTeX builds out of a vertical list.
///
/// Every one of them is the same shape — a list whose entries are the pieces —
/// so the entries, not the surrounding element, say which structure it is.
fn convert_structure(node: &Handle, ctx: &Ctx) -> Option<MathNode> {
    let NodeData::Element { attrs, .. } = &node.data else {
        return None;
    };
    let classes = class_list(attrs);

    // Scripts are resolved by the sequence logic, which knows the base they
    // belong to; a bare `msupsub` looks like a one-entry vertical list.
    if script_hint(&classes).is_some() {
        return None;
    }
    // `array`, `matrix`, `cases` — a span, named by its class.
    if classes.iter().any(|class| class == "mtable") {
        return Some(convert_table(node, ctx));
    }
    // A child font size multiplies the inherited one. KaTeX also puts a
    // `sizing` class on the empty delimiters it pads a fraction with, so only
    // an element with real content is a size wrapper.
    if classes.iter().any(|class| class == "sizing") && !element_children(node).is_empty() {
        let scale = size_factor(&classes).unwrap_or(1.0);
        let inner = Ctx {
            scale: ctx.scale * scale,
        };
        return Some(content(node, &inner));
    }
    if classes.iter().any(|class| class == "mspace") {
        let width = style_length(node, &["margin-right", "width", "min-width"]).unwrap_or(0.0);
        return Some(MathNode::Space(width));
    }
    if classes.iter().any(|class| class == "arraycolsep") {
        return Some(MathNode::Space(
            style_length(node, &["width"]).unwrap_or(0.5),
        ));
    }

    // The structures below all start with the vertical list they are built
    // from. Requiring that first child keeps a `base` (or any run of atoms)
    // that merely *contains* a formula from being read as one structure.
    let children = element_children(node);
    let first = children.first()?;
    let first_classes = class_of(first);
    let opens_a_list = first_classes
        .iter()
        .any(|class| class.starts_with("vlist") || class == "root");
    if !opens_a_list {
        return None;
    }
    // The index of `\sqrt[n]{…}` sits in its own list, which must not be
    // mistaken for the radical's body.
    let vlist = find_by_class_excluding(node, "vlist", "root")?;
    let entries = vlist_entries(&vlist);

    if entries.iter().any(|entry| {
        entry.has_class("frac-line")
            || entry.has_class("overline-line")
            || entry.has_class("underline-line")
    }) {
        return Some(convert_with_rule(node, &entries, ctx));
    }
    if entries.iter().any(|entry| entry.has_class("hide-tail")) {
        let index = find_by_class(node, "root").and_then(|root| {
            vlist_entries(&root)
                .first()
                .map(|entry| content(&entry.node, ctx))
        });
        let body = entries
            .iter()
            .find(|entry| !entry.has_class("hide-tail") && !entry.is_rule());
        let Some(body) = body else {
            return Some(content(node, ctx));
        };
        return Some(MathNode::Radical {
            body: Box::new(content(&body.node, ctx)),
            index: index.map(Box::new),
        });
    }
    if let Some(body) = entries.iter().find(|entry| entry.has_class("boxpad")) {
        return Some(MathNode::Framed {
            body: Box::new(content(&body.node, ctx)),
        });
    }
    if let Some(accent) = entries
        .iter()
        .position(|entry| entry.has_class("accent-body"))
    {
        let body = entries
            .iter()
            .enumerate()
            .find(|(index, entry)| *index != accent && !entry.is_rule());
        let Some((_, body)) = body else {
            return Some(content(node, ctx));
        };
        // `\hat` and `\bar` set their mark as a character; `\vec` and friends
        // draw it as an SVG, which leaves an empty mark and a bare base.
        return Some(MathNode::Accent {
            base: Box::new(content(&body.node, ctx)),
            accent: Box::new(entries[accent].to_node(ctx)),
        });
    }
    if entries.iter().any(VEntry::is_script) {
        return Some(convert_limits(&entries, ctx));
    }

    None
}

/// A fraction, an overline, or an underline: a rule among the entries.
///
/// Entries arrive top to bottom, so the pieces above the rule are its
/// numerator (or the body of an overline) and the ones below its denominator
/// (or the body of an underline).
fn convert_with_rule(node: &Handle, entries: &[VEntry], ctx: &Ctx) -> MathNode {
    if let Some(index) = entries
        .iter()
        .position(|entry| entry.has_class("frac-line"))
    {
        return MathNode::Frac {
            numerator: Box::new(join(&entries[..index], ctx)),
            denominator: Box::new(join(&entries[index + 1..], ctx)),
        };
    }
    if let Some(index) = entries
        .iter()
        .position(|entry| entry.has_class("overline-line"))
    {
        return MathNode::Rule {
            body: Box::new(join(&entries[index + 1..], ctx)),
            above: true,
        };
    }
    if let Some(index) = entries
        .iter()
        .position(|entry| entry.has_class("underline-line"))
    {
        return MathNode::Rule {
            body: Box::new(join(&entries[..index], ctx)),
            above: false,
        };
    }
    content(node, ctx)
}

/// An operator with scripts above and below it, such as `\sum_{i}^{n}`.
///
/// The operator itself sits in the same list as its scripts, but only the
/// scripts carry a `sizing` wrapper, which is what tells them apart.
fn convert_limits(entries: &[VEntry], ctx: &Ctx) -> MathNode {
    let base = entries
        .iter()
        .position(|entry| !entry.is_script() && !entry.is_rule())
        .unwrap_or(0);
    let base_node = content(&entries[base].node, ctx);

    let mut over = None;
    let mut under = None;
    for (index, entry) in entries.iter().enumerate() {
        if index == base || entry.is_rule() {
            continue;
        }
        // Entries run top to bottom, so an earlier entry sits higher.
        if index < base {
            over = Some(content(&entry.node, ctx));
        } else if under.is_none() {
            under = Some(content(&entry.node, ctx));
        }
    }

    MathNode::OverUnder {
        base: Box::new(base_node),
        over: over.map(Box::new),
        under: under.map(Box::new),
    }
}

/// `array`, `matrix`, `cases`: one column per `col-align-*` child, one row per
/// entry of its list.
fn convert_table(node: &Handle, ctx: &Ctx) -> MathNode {
    let mut columns: Vec<Vec<MathNode>> = Vec::new();
    let mut align: Vec<ColumnAlign> = Vec::new();

    for child in element_children(node) {
        let classes = class_of(&child);
        let Some(kind) = classes
            .iter()
            .find(|class| class.starts_with("col-align-"))
            .cloned()
        else {
            continue;
        };
        align.push(match kind.strip_prefix("col-align-").unwrap_or("c") {
            "l" => ColumnAlign::Left,
            "r" => ColumnAlign::Right,
            _ => ColumnAlign::Center,
        });
        let Some(vlist) = find_by_class(&child, "vlist") else {
            continue;
        };
        let cells: Vec<MathNode> = vlist_entries(&vlist)
            .iter()
            .map(|entry| content(&entry.node, ctx))
            .collect();
        columns.push(cells);
    }

    if columns.is_empty() {
        return content(node, ctx);
    }
    let height = columns.iter().map(Vec::len).max().unwrap_or(0);
    let rows = (0..height)
        .map(|row| {
            columns
                .iter()
                .map(|column| {
                    column
                        .get(row)
                        .cloned()
                        .unwrap_or_else(|| MathNode::Row(Vec::new()))
                })
                .collect()
        })
        .collect();
    MathNode::Table { rows, align }
}

/// One entry of a KaTeX vertical list, with the offset it is stacked at.
#[derive(Clone)]
struct VEntry {
    node: Handle,
    /// `top` from the entry's inline style, in em; smaller is higher up.
    top: f32,
}

impl VEntry {
    /// Does this entry's *own* piece carry this class?
    ///
    /// KaTeX wraps each entry's content in its own span (`<span style="top:…">`
    /// around the rule of a fraction, say), so the class a structure is named
    /// by sits one or two levels down. It has to stop there: the numerator of
    /// `\frac{\frac{1}{2}}{3}` contains a rule of its own, which says nothing
    /// about the outer fraction.
    fn has_class(&self, class: &str) -> bool {
        let mut classes = Vec::new();
        own_classes(&self.node, &mut classes, 0);
        classes.iter().any(|value| value == class)
    }

    /// A script, which is the only thing KaTeX shrinks.
    fn is_script(&self) -> bool {
        self.has_class("sizing")
    }

    /// A rule or a frame: `\fbox` and friends, drawn by the app itself.
    fn is_rule(&self) -> bool {
        self.has_class("frac-line")
            || self.has_class("overline-line")
            || self.has_class("underline-line")
            || self.has_class("stretchy")
    }

    /// The entry's visible content.
    fn to_node(&self, ctx: &Ctx) -> MathNode {
        content(&self.node, ctx)
    }
}

/// The element children of a vertical list, ordered top to bottom.
///
/// KaTeX emits a list's entries in whatever order their constructor pushed
/// them — a fraction is bottom to top, an array's rows are top to bottom — but
/// every entry states where it belongs in its `top` offset, and a more negative
/// offset is higher up. Sorting on that is the only order both shapes agree on.
fn vlist_entries(vlist: &Handle) -> Vec<VEntry> {
    let mut entries: Vec<VEntry> = vlist
        .children
        .borrow()
        .iter()
        .filter(|child| matches!(child.data, NodeData::Element { .. }))
        .map(|child| VEntry {
            node: child.clone(),
            top: style_length(child, &["top"]).unwrap_or(0.0),
        })
        .collect();
    entries.sort_by(|left, right| left.top.total_cmp(&right.top));
    entries
}

/// The classes of a node's own pieces, without those of a formula nested
/// inside it.
///
/// Descent stops at layout-only elements (which carry no meaning) and at
/// anything that opens a structure of its own (which is a separate formula with
/// its own classes). `depth` keeps a pathological document from walking forever.
fn own_classes(node: &Handle, out: &mut Vec<String>, depth: usize) {
    if depth > 8 {
        return;
    }
    for child in element_children(node) {
        let classes = class_of(&child);
        let layout = classes.iter().any(|class| {
            matches!(
                class.as_str(),
                "pstrut" | "strut" | "vlist" | "vlist-t" | "vlist-r" | "vlist-s"
            )
        });
        if layout {
            continue;
        }
        if find_by_class(&child, "vlist").is_some()
            || find_by_class(&child, "msupsub").is_some()
            || script_hint(&classes).is_some()
        {
            // A nested formula or script: its classes belong to it.
            continue;
        }
        out.extend(classes);
        own_classes(&child, out, depth + 1);
    }
}

/// Fold several entries into one node.
fn join(entries: &[VEntry], ctx: &Ctx) -> MathNode {
    let children: Vec<MathNode> = entries
        .iter()
        .map(|entry| content(&entry.node, ctx))
        .filter(|node| !is_empty(node))
        .collect();
    match children.len() {
        0 => MathNode::Row(Vec::new()),
        1 => children.into_iter().next().unwrap(),
        _ => MathNode::Row(children),
    }
}

/// An element's children as a single node.
fn content(node: &Handle, ctx: &Ctx) -> MathNode {
    let children = fold(node, ctx);
    match children.len() {
        0 => MathNode::Row(Vec::new()),
        1 => children.into_iter().next().unwrap(),
        _ => MathNode::Row(children),
    }
}

/// A leaf element with no element children: one styled run of glyphs.
fn convert_leaf(node: &Handle, classes: &[String], ctx: &Ctx) -> MathNode {
    let mut text = String::new();
    collect_visible_text(node, &mut text);
    if text.is_empty() {
        return MathNode::Row(Vec::new());
    }
    let scale = match style_length(node, &["font-size"]) {
        Some(factor) => ctx.scale * factor,
        None => ctx.scale,
    };
    let style = text_style(classes, &text);
    MathNode::Text(MathText {
        text,
        scale,
        style,
        color: style_color(node),
    })
}

/// Text content of a subtree, skipping the SVG glyphs KaTeX draws.
fn collect_visible_text(node: &Handle, out: &mut String) {
    for child in node.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => out.push_str(&contents.borrow()),
            NodeData::Element { name, .. } if name.local.as_ref() != "svg" => {
                collect_visible_text(child, out);
            }
            _ => {}
        }
    }
}

/// The element children of a node.
fn element_children(node: &Handle) -> Vec<Handle> {
    node.children
        .borrow()
        .iter()
        .filter(|child| matches!(child.data, NodeData::Element { .. }))
        .cloned()
        .collect()
}

/// The first descendant carrying `class`, skipping subtrees under `excluded`.
fn find_by_class_excluding(node: &Handle, class: &str, excluded: &str) -> Option<Handle> {
    for child in node.children.borrow().iter() {
        if classes_contain(child, class) {
            return Some(child.clone());
        }
        if classes_contain(child, excluded) {
            continue;
        }
        if matches!(child.data, NodeData::Element { .. })
            && let Some(found) = find_by_class_excluding(child, class, excluded)
        {
            return Some(found);
        }
    }
    None
}

/// The first descendant carrying `class`.
fn find_by_class(node: &Handle, class: &str) -> Option<Handle> {
    for child in node.children.borrow().iter() {
        if classes_contain(child, class) {
            return Some(child.clone());
        }
        if matches!(child.data, NodeData::Element { .. })
            && let Some(found) = find_by_class(child, class)
        {
            return Some(found);
        }
    }
    None
}

fn classes_contain(node: &Handle, class: &str) -> bool {
    class_of(node).iter().any(|value| value == class)
}

fn class_of(node: &Handle) -> Vec<String> {
    let NodeData::Element { attrs, .. } = &node.data else {
        return Vec::new();
    };
    class_list(attrs)
}

fn class_list(attrs: &std::cell::RefCell<Vec<html5ever::Attribute>>) -> Vec<String> {
    attrs
        .borrow()
        .iter()
        .find(|attr| &*attr.name.local == "class")
        .map(|attr| attr.value.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// KaTeX's script sizes. `sizeN` sets the step and `reset-sizeM` the family it
/// belongs to; only the `reset-size6` family (the default text size) is
/// reachable from ordinary writing.
fn size_factor(classes: &[String]) -> Option<f32> {
    let step = classes.iter().find_map(|class| {
        class
            .strip_prefix("size")
            .and_then(|n| n.parse::<usize>().ok())
    })?;
    let step = [0.5f32, 0.6, 0.7, 0.8, 0.9, 1.0, 1.2, 1.44].get(step.checked_sub(1)?)?;
    let family = classes
        .iter()
        .find_map(|class| {
            class
                .strip_prefix("reset-size")
                .and_then(|n| n.parse::<usize>().ok())
        })
        .unwrap_or(6);
    let factor = match family {
        1 => 0.408,
        2 => 0.577,
        3 => 0.707,
        4 => 0.816,
        5 => 0.913,
        _ => 1.0,
    };
    Some(step * factor)
}

/// The first of `properties` present in the inline style, in em.
fn style_length(node: &Handle, properties: &[&str]) -> Option<f32> {
    let style = attr_value(node, "style")?;
    for property in properties {
        for declaration in style.split(';') {
            let Some((key, value)) = declaration.split_once(':') else {
                continue;
            };
            if key.trim() != *property {
                continue;
            }
            let value = value.trim();
            let number: f32 = if let Some(number) = value.strip_suffix("em") {
                number.parse().ok()?
            } else if let Some(number) = value.strip_suffix("px") {
                number.parse::<f32>().ok()? / 16.0
            } else {
                value.parse().ok()?
            };
            return Some(number);
        }
    }
    None
}

/// `#rrggbb` from an inline `color`, used by `\color{…}`.
fn style_color(node: &Handle) -> Option<[u8; 3]> {
    let style = attr_value(node, "style")?;
    for declaration in style.split(';') {
        let Some((key, value)) = declaration.split_once(':') else {
            continue;
        };
        if key.trim() != "color" {
            continue;
        }
        let hex = value.trim().trim_start_matches('#');
        if hex.len() == 6 {
            let channel =
                |index: usize| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok();
            return channel(0).and_then(|red| {
                channel(1).and_then(|green| channel(2).map(|blue| [red, green, blue]))
            });
        }
    }
    None
}

fn attr_value(node: &Handle, name: &str) -> Option<String> {
    let NodeData::Element { attrs, .. } = &node.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|attr| &*attr.name.local == name)
        .map(|attr| attr.value.to_string())
}

/// The style KaTeX asks for on a run of symbols.
fn text_style(classes: &[String], text: &str) -> TextStyle {
    let has = |name: &str| classes.iter().any(|class| class == name);
    if has("mathbf") || has("mathbold") || has("bf") {
        return TextStyle::Bold;
    }
    if has("mathtt") || has("texttt") {
        return TextStyle::Mono;
    }
    if has("mathit") || has("mathnormal") || has("mathdefault") || has("mathitalic") {
        return TextStyle::Italic;
    }
    if has("mathrm") || has("text") || has("mtext") || has("mathsf") || has("operatorname") {
        return TextStyle::Upright;
    }
    // KaTeX marks a lone variable with `mathnormal`, but a bare run of letters
    // in math mode is a variable too.
    if classes.iter().any(|class| class == "mord")
        && text.chars().count() == 1
        && text
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
    {
        return TextStyle::Italic;
    }
    TextStyle::Upright
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(tex: &str, display: bool) -> MathDom {
        parse_formula(tex, display).unwrap_or_else(|err| panic!("{tex}: {err}"))
    }

    #[test]
    fn fraction_becomes_a_fraction() {
        let node = &dom("\\frac{a}{b}", false).root;
        let MathNode::Frac {
            numerator,
            denominator,
        } = node
        else {
            panic!("expected a fraction, got {node:?}");
        };
        assert_eq!(flat_text(numerator), "a");
        assert_eq!(flat_text(denominator), "b");
    }

    #[test]
    fn display_and_nested_fractions_keep_their_shape() {
        assert!(matches!(
            dom("\\frac{a}{b}", true).root,
            MathNode::Frac { .. }
        ));
        assert!(matches!(
            dom("\\dfrac{a}{b}", true).root,
            MathNode::Frac { .. }
        ));
        let MathNode::Frac { numerator, .. } = &dom("\\frac{\\frac{1}{2}}{3}", false).root else {
            panic!("expected a fraction");
        };
        assert!(
            matches!(numerator.as_ref(), MathNode::Frac { .. }),
            "{numerator:?}"
        );
    }

    #[test]
    fn scripts_attach_to_their_base() {
        let MathNode::Scripts { sup, sub, .. } = &dom("a^n", false).root else {
            panic!("{:?}", dom("a^n", false).root);
        };
        assert_eq!(sup.as_deref().map(flat_text).as_deref(), Some("n"));
        assert!(sub.is_none(), "a^n grew a subscript");

        let MathNode::Scripts { sup, sub, .. } = &dom("a_i", false).root else {
            panic!("{:?}", dom("a_i", false).root);
        };
        assert!(sup.is_none(), "a_i grew a superscript");
        assert_eq!(sub.as_deref().map(flat_text).as_deref(), Some("i"));

        let MathNode::Scripts { sup, sub, .. } = &dom("a_{i,j}^{n}", true).root else {
            panic!("{:?}", dom("a_{i,j}^{n}", true).root);
        };
        assert_eq!(sup.as_deref().map(flat_text).as_deref(), Some("n"));
        assert_eq!(sub.as_deref().map(flat_text).as_deref(), Some("i,j"));
    }

    #[test]
    fn a_script_inside_a_wrapper_keeps_that_base() {
        // `\frac{\sin x}{x^2}` puts the script inside an explicit `msup`, which
        // carries its own base; the fraction must not lose either half.
        let MathNode::Frac {
            numerator,
            denominator,
        } = &dom("\\frac{\\sin x}{x^2}", true).root
        else {
            panic!("expected a fraction");
        };
        assert!(flat_text(numerator).contains("sin"), "{numerator:?}");
        let MathNode::Scripts { base, .. } = denominator.as_ref() else {
            panic!("expected a scripted denominator, got {denominator:?}");
        };
        assert_eq!(flat_text(base), "x");
    }

    #[test]
    fn a_wide_operator_keeps_limits_above_and_below() {
        let root = &dom("\\sum_{i=1}^{n} i", true).root;
        let limits = find(root, &|node| matches!(node, MathNode::OverUnder { .. }))
            .unwrap_or_else(|| panic!("expected limits in {root:#?}"));
        let MathNode::OverUnder { base, over, under } = limits else {
            unreachable!()
        };
        assert_eq!(flat_text(base), "∑");
        assert_eq!(over.as_deref().map(flat_text).as_deref(), Some("n"));
        assert_eq!(under.as_deref().map(flat_text).as_deref(), Some("i=1"));

        // `\lim` in display style is also a limits container.
        assert!(
            find(&dom("\\lim_{x \\to 0} f", true).root, &|node| matches!(
                node,
                MathNode::OverUnder { .. }
            ))
            .is_some(),
            "{:#?}",
            dom("\\lim_{x \\to 0} f", true).root
        );
    }

    #[test]
    fn radical_keeps_its_body_and_index() {
        let node = &dom("\\sqrt[3]{x+1}", false).root;
        let MathNode::Radical { body, index } = node else {
            panic!("expected a radical, got {node:?}");
        };
        assert_eq!(index.as_deref().map(flat_text).as_deref(), Some("3"));
        assert!(flat_text(body).contains('x'), "{body:?}");
        assert!(flat_text(body).contains('1'), "{body:?}");

        assert!(
            matches!(&dom("\\sqrt{x}", false).root, MathNode::Radical { .. }),
            "{:?}",
            dom("\\sqrt{x}", false).root
        );
    }

    #[test]
    fn table_keeps_its_rows_in_reading_order() {
        let root = &dom(
            "\\begin{cases} 1 & x > 0 \\\\ 0 & x \\le 0 \\end{cases}",
            true,
        )
        .root;
        let table = find(root, &|node| matches!(node, MathNode::Table { .. }))
            .unwrap_or_else(|| panic!("expected a table in {root:#?}"));
        let MathNode::Table { rows, align } = table else {
            unreachable!()
        };
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].len(), 2, "{rows:?}");
        assert!(flat_text(&rows[0][0]).contains('1'), "{rows:?}");
        assert!(flat_text(&rows[1][0]).contains('0'), "{rows:?}");
        assert_eq!(align.len(), 2, "{align:?}");
    }

    /// The first node anywhere in a tree that matches. KaTeX splits a formula
    /// into several `base` runs, so a structure is rarely the root itself.
    fn find<'a>(node: &'a MathNode, predicate: &dyn Fn(&MathNode) -> bool) -> Option<&'a MathNode> {
        if predicate(node) {
            return Some(node);
        }
        match node {
            MathNode::Row(children) => children.iter().find_map(|child| find(child, predicate)),
            MathNode::Frac {
                numerator,
                denominator,
            } => find(numerator, predicate).or_else(|| find(denominator, predicate)),
            MathNode::Scripts { base, sup, sub } => find(base, predicate)
                .or_else(|| sup.as_deref().and_then(|node| find(node, predicate)))
                .or_else(|| sub.as_deref().and_then(|node| find(node, predicate))),
            MathNode::OverUnder { base, over, under } => find(base, predicate)
                .or_else(|| over.as_deref().and_then(|node| find(node, predicate)))
                .or_else(|| under.as_deref().and_then(|node| find(node, predicate))),
            MathNode::Accent { base, .. }
            | MathNode::Rule { body: base, .. }
            | MathNode::Framed { body: base }
            | MathNode::Radical { body: base, .. } => find(base, predicate),
            MathNode::Table { rows, .. } => {
                rows.iter().flatten().find_map(|cell| find(cell, predicate))
            }
            MathNode::Text(_) | MathNode::Space(_) => None,
        }
    }

    fn text_runs(node: &MathNode) -> Vec<MathText> {
        let mut runs = Vec::new();
        collect_runs(node, &mut runs);
        runs
    }

    fn collect_runs(node: &MathNode, out: &mut Vec<MathText>) {
        match node {
            MathNode::Text(text) => out.push(text.clone()),
            MathNode::Row(children) => children.iter().for_each(|child| collect_runs(child, out)),
            MathNode::Frac {
                numerator,
                denominator,
            } => {
                collect_runs(numerator, out);
                collect_runs(denominator, out);
            }
            MathNode::Scripts { base, sup, sub } => {
                collect_runs(base, out);
                if let Some(node) = sub {
                    collect_runs(node, out);
                }
                if let Some(node) = sup {
                    collect_runs(node, out);
                }
            }
            MathNode::OverUnder { base, over, under } => {
                collect_runs(base, out);
                if let Some(node) = under {
                    collect_runs(node, out);
                }
                if let Some(node) = over {
                    collect_runs(node, out);
                }
            }
            MathNode::Accent { base, accent } => {
                collect_runs(base, out);
                collect_runs(accent, out);
            }
            MathNode::Rule { body, .. }
            | MathNode::Framed { body }
            | MathNode::Radical { body, .. } => collect_runs(body, out),
            MathNode::Table { rows, .. } => {
                rows.iter()
                    .flatten()
                    .for_each(|cell| collect_runs(cell, out));
            }
            MathNode::Space(_) => {}
        }
    }

    #[test]
    fn accent_rule_and_frame_are_recognised() {
        let node = &dom("\\hat{x}", false).root;
        let MathNode::Accent { base, .. } = node else {
            panic!("expected an accent, got {node:?}");
        };
        assert_eq!(flat_text(base), "x");

        let MathNode::Rule { body, above } = &dom("\\overline{AB}", false).root else {
            panic!("expected a rule");
        };
        assert!(above);
        assert_eq!(flat_text(body), "AB");

        let MathNode::Rule { body, above } = &dom("\\underline{x}", false).root else {
            panic!("expected a rule");
        };
        assert!(!above);
        assert_eq!(flat_text(body), "x");

        let node = &dom("\\boxed{x = 1}", false).root;
        let MathNode::Framed { body } = node else {
            panic!("expected a frame, got {node:?}");
        };
        assert!(flat_text(body).contains('x'), "{body:?}");
    }

    #[test]
    fn variables_are_italic_and_operators_are_not() {
        let root = &dom("x + 1", false).root;
        let styles: Vec<TextStyle> = text_runs(root).iter().map(|text| text.style).collect();
        assert!(styles.contains(&TextStyle::Italic), "{styles:?}");
        assert!(styles.contains(&TextStyle::Upright), "{styles:?}");
    }

    #[test]
    fn operators_carry_their_spacing() {
        // `a \le b` needs a relation's worth of space on both sides.
        let root = &dom("a \\leq b", false).root;
        assert!(
            find(root, &|node| matches!(node, MathNode::Space(_))).is_some(),
            "no operator spacing in {root:#?}"
        );
    }

    #[test]
    fn scripts_are_set_smaller_than_their_base() {
        let MathNode::Scripts { base, sup, .. } = &dom("a^n", false).root else {
            panic!("expected scripts");
        };
        let MathNode::Text(script) = sup.as_deref().unwrap() else {
            panic!("expected a text run");
        };
        let MathNode::Text(base_text) = base.as_ref() else {
            panic!("expected a text run");
        };
        assert!(
            script.scale < base_text.scale,
            "{script:?} vs {base_text:?}"
        );
    }

    #[test]
    fn text_mode_keeps_its_cjk_and_upright_style() {
        let node = &dom("\\text{当 } n", false).root;
        assert!(flat_text(node).contains('当'), "{node:?}");
    }

    #[test]
    fn a_formula_the_engine_cannot_typeset_reports_the_error() {
        // The `duktape` backend has no `Array.prototype.fill`, which KaTeX
        // needs for paren-delimited matrices.
        let err = parse_formula("\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}", true);
        assert!(err.is_err(), "{err:?}");
    }

    #[test]
    fn the_display_flag_is_carried_through() {
        assert!(dom("x", true).display);
        assert!(!dom("x", false).display);
    }
}
