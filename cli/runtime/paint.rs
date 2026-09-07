//! The painter: a scene document and a stylesheet in, PNG bytes out.
//!
//! ```text
//! buri-scene 1
//! viewport 800 600
//! e 0 class:p-8 bg-slate;background-color:rgb(255,255,255)
//! e 1 font-size:20px
//! t 2 Ada
//! ```
//!
//! [`render`] answers the PNG of that tree. [`diff`] compares two PNGs and
//! answers the picture of where they disagree. `buri test` calls both, through
//! `snapshot.rs`, and never sees a window or a browser.
//!
//! # The one thing this file is for
//!
//! **The same scene must produce the same bytes on Linux and on macOS.** A
//! snapshot that drifts by one grey pixel between two developers' laptops is
//! not a test, it is a nuisance, so every choice below is the one that removes
//! a source of divergence:
//!
//! * The fonts are bundled and nothing else is ever loaded. No system font
//!   enumeration, no fontconfig, no locale — [`font_system`] builds an empty
//!   `fontdb` and hands it three faces.
//! * Hinting is off in both places it can be on: `Hinting::Disabled` for
//!   layout, `CacheKeyFlags::DISABLE_HINTING` for the rasterizer. A hinted
//!   glyph is a glyph whose outline depends on the grid it was fitted to.
//! * **Layout rounds in exactly one place**, [`px`], and it rounds the
//!   *absolute* edge of a box. `taffy` computes in `f32` and its own rounding
//!   is turned off, so no number is rounded twice and no box is ever a pixel
//!   wider than the gap it was given.
//! * The PNG is written here, by [`encode`] — row filters, a fixed-Huffman
//!   deflate over a fixed-chain match finder, and a hand-rolled CRC-32 and
//!   Adler-32. Two zlib versions cannot disagree about a byte that no zlib
//!   produced.
//! * Nothing in the output path iterates a hash map.
//!
//! # What it paints, and what it does not
//!
//! The properties [`apply`] names, and no others: flexbox and grid, padding,
//! sizing, background, border, radius, shadow, opacity, and the text
//! properties. They are the same CSS `semantics::styles::declaration` writes
//! into the stylesheet and `$tree_declare` writes inline, so a style that
//! folded and one that did not paint alike. Anything else parses and is
//! ignored, which is what lets the vocabulary grow without breaking a scene.
//!
//! Two deliberate simplifications, each visible in a snapshot:
//!
//! * An element with no `display` lays out as a column of its children, which
//!   is what a block box does for the trees this paints.
//! * `opacity` multiplies into every colour the subtree paints rather than
//!   compositing the subtree as a group, so two overlapping half-transparent
//!   children show through each other.
//!
//! `box-shadow`'s blur is three integer box passes over a coverage mask, which
//! is what the SVG filter specification writes down for a Gaussian and what a
//! browser does for a shadow; `overflow: hidden` clips to the box's own
//! rounded shape. Both are stated at [`blur`] and [`intersect`].
//!
//! # Errors
//!
//! [`render`] never panics. Every refusal is one sentence naming what it could
//! not read.

use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, CacheKeyFlags, Family, FontSystem, Hinting, Metrics, Shaping, SwashCache, Weight,
    Wrap, fontdb,
};
use taffy::prelude::*;
use taffy::{Overflow, Point, TaffyTree, compute_leaf_layout};
use tiny_skia::{
    FillRule, Mask, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, StrokeDash, Transform,
};

// ---------------------------------------------------------------------------
// The bundled family
// ---------------------------------------------------------------------------

/// The one family the painter can draw with. Every `font-family` resolves to
/// it, including `ui-serif` and `ui-monospace`, because a face this archive
/// does not carry is a face no snapshot may depend on.
const FAMILY: &str = "Roboto";

/// Roboto, Latin subset, under the SIL Open Font License — `fonts/LICENSE`.
const REGULAR: &[u8] = include_bytes!("fonts/Roboto-Regular.ttf");
const BOLD: &[u8] = include_bytes!("fonts/Roboto-Bold.ttf");
const ITALIC: &[u8] = include_bytes!("fonts/Roboto-Italic.ttf");

/// One rem, always. A snapshot has no reader preference to follow.
const REM: f32 = 16.0;

/// The font size an element inherits when nothing set one.
const ROOT_FONT_SIZE: f32 = 16.0;

/// `line-height: normal`, as a multiple of the font size.
const NORMAL_LINE_HEIGHT: f32 = 1.2;

/// The largest viewport the painter will allocate a canvas for.
const MAX_VIEWPORT: u32 = 8192;

// ---------------------------------------------------------------------------
// The public surface
// ---------------------------------------------------------------------------

/// What to paint.
pub struct Request<'a> {
    pub scene: &'a str,
    pub stylesheet: &'a str,
    /// "rest", "hover", "focus", "active", "disabled" or "checked".
    pub state: &'a str,
}

/// Lays out, shapes and paints one scene. Answers PNG bytes.
///
/// # Errors
/// Answers `Err` with one sentence for any scene, stylesheet or state it
/// cannot read.
pub fn render(request: &Request) -> Result<Vec<u8>, String> {
    let scene = Scene::parse(request.scene)?;
    let state = State::parse(request.state)?;
    let sheet = parse_stylesheet(request.stylesheet);

    let styles = resolve(&scene, &sheet, state);
    let pixmap = paint(&scene, &styles)?;
    Ok(encode(pixmap.width(), pixmap.height(), &straight(&pixmap)))
}

/// Compares two PNGs. `None` when the bytes are equal; otherwise the PNG of a
/// diff image.
///
/// Byte equality decides, with no tolerance. The diff is as large as the larger
/// input: a pixel that matches is the golden's colour, greyed and lightened; a
/// pixel that differs, or that only one image has, is magenta.
///
/// # Errors
/// Answers `Err` when either input is not a PNG this painter wrote.
pub fn diff(golden: &[u8], actual: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if golden == actual {
        return Ok(None);
    }
    let a = decode(golden)?;
    let b = decode(actual)?;
    let width = a.width.max(b.width);
    let height = a.height.max(b.height);
    let mut out = Vec::with_capacity(pixel_bytes(width, height)?);
    for y in 0..height {
        for x in 0..width {
            match (a.pixel(x, y), b.pixel(x, y)) {
                (Some(g), Some(n)) if g == n => out.extend_from_slice(&faded(g)),
                _ => out.extend_from_slice(&[255, 0, 255, 255]),
            }
        }
    }
    Ok(Some(encode(width, height, &out)))
}

/// A matching pixel, greyed and lightened, so the magenta reads as the subject.
fn faded(p: [u8; 4]) -> [u8; 4] {
    let [r, g, b, _] = p;
    // Rec. 601 luma in integers: the coefficients sum to 256, so the shift is
    // exact and the answer cannot leave a byte.
    let luma = (u32::from(r) * 54 + u32::from(g) * 183 + u32::from(b) * 19) >> 8;
    let light = (luma / 2 + 128).min(255);
    let v = u8::try_from(light).unwrap_or(255);
    [v, v, v, 255]
}

// ---------------------------------------------------------------------------
// The scene document
// ---------------------------------------------------------------------------

/// One line of the scene: a box, or a run of text inside one.
struct Node {
    /// `Some` for a `t` line. A text run has no children and no declarations.
    text: Option<String>,
    classes: Vec<String>,
    declarations: Vec<(String, String)>,
    children: Vec<usize>,
}

struct Scene {
    width: u32,
    height: u32,
    nodes: Vec<Node>,
    roots: Vec<usize>,
}

impl Scene {
    fn parse(source: &str) -> Result<Self, String> {
        let mut lines = source.lines();
        let head = lines.next().unwrap_or_default();
        if head != "buri-scene 1" {
            return Err("the scene does not start with `buri-scene 1`".to_string());
        }
        let (width, height) = parse_viewport(lines.next().unwrap_or_default())?;

        let mut nodes: Vec<Node> = Vec::new();
        let mut roots: Vec<usize> = Vec::new();
        // `open[d]` is the node the next line at depth `d + 1` belongs to.
        let mut open: Vec<usize> = Vec::new();

        for line in lines {
            if line.is_empty() {
                continue;
            }
            let (kind, rest) = line.split_at_checked(2).unwrap_or((line, ""));
            let (depth, body) = match kind {
                "e " | "t " => split_depth(rest)?,
                _ => return Err(format!("the scene line `{line}` is neither an `e` nor a `t`")),
            };
            if depth > open.len() {
                return Err(format!("the scene jumps from depth {} to {depth}", open.len()));
            }
            open.truncate(depth);

            let index = nodes.len();
            let node = if kind == "t " {
                Node {
                    text: Some(unescape(body)),
                    classes: Vec::new(),
                    declarations: Vec::new(),
                    children: Vec::new(),
                }
            } else {
                let (classes, declarations) = parse_declarations(body)?;
                Node { text: None, classes, declarations, children: Vec::new() }
            };
            nodes.push(node);

            match open.last() {
                None => roots.push(index),
                Some(&parent) => {
                    let Some(holder) = nodes.get_mut(parent) else {
                        return Err("the scene names a parent that is not there".to_string());
                    };
                    if holder.text.is_some() {
                        return Err("a scene `t` line has a child, and a text run has none"
                            .to_string());
                    }
                    holder.children.push(index);
                }
            }
            open.push(index);
        }

        Ok(Self { width, height, nodes, roots })
    }

    fn node(&self, index: usize) -> Option<&Node> {
        self.nodes.get(index)
    }
}

fn parse_viewport(line: &str) -> Result<(u32, u32), String> {
    let mut parts = line.split(' ');
    let bad = || format!("the scene's second line `{line}` is not a viewport");
    if parts.next() != Some("viewport") {
        return Err(bad());
    }
    let width: u32 = parts.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
    let height: u32 = parts.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
    if parts.next().is_some() {
        return Err(bad());
    }
    if width == 0 || height == 0 || width > MAX_VIEWPORT || height > MAX_VIEWPORT {
        return Err(format!("the viewport {width}x{height} is not between 1x1 and 8192x8192"));
    }
    Ok((width, height))
}

/// `<depth> <rest>`, or `<depth>` when the rest is empty.
fn split_depth(rest: &str) -> Result<(usize, &str), String> {
    let (digits, body) = match rest.split_once(' ') {
        Some((digits, body)) => (digits, body),
        None => (rest, ""),
    };
    let depth: usize =
        digits.parse().map_err(|_| format!("the scene line depth `{digits}` is not a number"))?;
    Ok((depth, body))
}

/// `\\` is a backslash, `\n` a newline, `\r` a carriage return. No other
/// escape exists, so anything else keeps both characters.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The classes a declaration list named, and everything else it said.
type Declarations = (Vec<String>, Vec<(String, String)>);

/// `name:value` pairs joined by `;`, with `class` lifted out.
fn parse_declarations(body: &str) -> Result<Declarations, String> {
    let mut classes = Vec::new();
    let mut declarations = Vec::new();
    for entry in body.split(';') {
        if entry.is_empty() {
            continue;
        }
        let Some((name, value)) = entry.split_once(':') else {
            return Err(format!("the scene declaration `{entry}` has no `:`"));
        };
        if name == "class" {
            classes.extend(value.split(' ').filter(|c| !c.is_empty()).map(ToString::to_string));
        } else {
            declarations.push((name.to_string(), value.to_string()));
        }
    }
    Ok((classes, declarations))
}

// ---------------------------------------------------------------------------
// The stylesheet
// ---------------------------------------------------------------------------

/// The snapshot's pseudo-class, as `styles.rs` spells it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Rest,
    Hover,
    Focus,
    Active,
    Disabled,
    Checked,
}

impl State {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "rest" => Ok(Self::Rest),
            "hover" => Ok(Self::Hover),
            "focus" => Ok(Self::Focus),
            "active" => Ok(Self::Active),
            "disabled" => Ok(Self::Disabled),
            "checked" => Ok(Self::Checked),
            _ => Err(format!("`{name}` is not a snapshot state")),
        }
    }

    fn pseudo(pseudo: &str) -> Option<Self> {
        match pseudo {
            "hover" => Some(Self::Hover),
            "focus-visible" => Some(Self::Focus),
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            "checked" => Some(Self::Checked),
            _ => None,
        }
    }
}

/// One class rule of the sheet, in the order the sheet wrote it.
struct Rule {
    class: String,
    state: Option<State>,
    /// The `@media (min-width:)` floor in pixels; `0` outside a query.
    min_width: f32,
    declarations: Vec<(String, String)>,
}

/// Reads the sheet `semantics::styles::stylesheet` writes.
///
/// A line it cannot read is skipped rather than refused: the sheet is the
/// compiler's own output, and a painter that stopped on a rule it had not
/// learned yet would fail a test for a property it does not even paint.
fn parse_stylesheet(source: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut min_width = 0.0_f32;
    for line in source.lines() {
        let line = line.trim();
        if line == "}" {
            min_width = 0.0;
            continue;
        }
        if let Some(query) = line.strip_prefix("@media (min-width:") {
            if let Some(width) = query.strip_suffix("rem){").and_then(|n| n.parse::<f32>().ok()) {
                min_width = width * REM;
            }
            continue;
        }
        let Some((selector, body)) = line.split_once('{') else { continue };
        let Some(body) = body.strip_suffix('}') else { continue };
        let Some((class, state)) = parse_selector(selector) else { continue };
        let Ok((_, declarations)) = parse_declarations(body) else { continue };
        rules.push(Rule { class, state, min_width, declarations });
    }
    rules
}

/// `.<class><pseudo?>` — and `None` for a selector carrying anything after the
/// pseudo-class, because a descendant rule is out of scope for version one.
fn parse_selector(selector: &str) -> Option<(String, Option<State>)> {
    let mut chars = selector.chars();
    if chars.next()? != '.' {
        return None;
    }
    let mut class = String::new();
    let mut pseudo = String::new();
    let mut in_pseudo = false;
    while let Some(c) = chars.next() {
        match c {
            // A class name that holds a `:` writes it `\:`.
            '\\' => class.push(chars.next()?),
            ':' if !in_pseudo => in_pseudo = true,
            _ if in_pseudo && (c.is_ascii_alphanumeric() || c == '-') => pseudo.push(c),
            // A descendant combinator, a second pseudo-class, anything else.
            _ if in_pseudo => return None,
            _ if c.is_whitespace() || c == '>' || c == '*' => return None,
            _ => class.push(c),
        }
    }
    if class.is_empty() {
        return None;
    }
    if !in_pseudo {
        return Some((class, None));
    }
    State::pseudo(&pseudo).map(|state| (class, Some(state)))
}

// ---------------------------------------------------------------------------
// The resolved style
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Len {
    Auto,
    Px(f32),
    Percent(f32),
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Rgba {
    r: u8,
    g: u8,
    b: u8,
    a: f32,
}

impl Rgba {
    const CLEAR: Self = Self { r: 0, g: 0, b: 0, a: 0.0 };
    const BLACK: Self = Self { r: 0, g: 0, b: 0, a: 1.0 };

    fn visible(self) -> bool {
        self.a > 0.0
    }
}

/// What a colour declaration said, before the property decides what `inherit`
/// and an unresolved `var()` mean for it.
enum Spec {
    Value(Rgba),
    Transparent,
    Inherit,
    Token,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Flow {
    Flex,
    Grid,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Decoration {
    None,
    Underline,
    LineThrough,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Case {
    None,
    Upper,
    Lower,
    Capitalize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Border {
    None,
    Solid,
    Dashed,
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Shadow {
    x: f32,
    y: f32,
    blur: f32,
    spread: f32,
    colour: Rgba,
}

/// Every property the painter honours, resolved for one node.
#[derive(Clone, Debug)]
struct Computed {
    flow: Flow,
    column: bool,
    wrap: bool,
    justify: Option<AlignContent>,
    align_items: Option<AlignItems>,
    align_self: Option<AlignItems>,
    grow: f32,
    shrink: f32,
    tracks: Vec<Len>,
    span: Option<u16>,
    gap_column: Len,
    gap_row: Len,
    padding: [Len; 4],
    size: [Len; 2],
    min: [Len; 2],
    max: [Len; 2],
    aspect: Option<f32>,
    absolute: bool,
    inset: [Len; 4],
    clipped: [bool; 2],

    background: Rgba,
    colour: Rgba,
    border_width: Len,
    border_colour: Rgba,
    border_style: Border,
    radius: Len,
    opacity: f32,
    shadow: Option<Shadow>,

    font_size: f32,
    weight: u16,
    italic: bool,
    line_height: f32,
    letter_spacing: f32,
    align_text: cosmic_text::Align,
    case: Case,
    decoration: Decoration,
    nowrap: bool,
}

impl Computed {
    /// The style a scene's outermost box inherits.
    fn root() -> Self {
        Self {
            flow: Flow::Flex,
            column: true,
            wrap: false,
            justify: None,
            align_items: None,
            align_self: None,
            grow: 0.0,
            shrink: 1.0,
            tracks: Vec::new(),
            span: None,
            gap_column: Len::Px(0.0),
            gap_row: Len::Px(0.0),
            padding: [Len::Px(0.0); 4],
            size: [Len::Auto; 2],
            min: [Len::Auto; 2],
            max: [Len::Auto; 2],
            aspect: None,
            absolute: false,
            inset: [Len::Auto; 4],
            clipped: [false; 2],
            background: Rgba::CLEAR,
            colour: Rgba::BLACK,
            border_width: Len::Px(0.0),
            border_colour: Rgba::BLACK,
            border_style: Border::Solid,
            radius: Len::Px(0.0),
            opacity: 1.0,
            shadow: None,
            font_size: ROOT_FONT_SIZE,
            weight: 400,
            italic: false,
            line_height: NORMAL_LINE_HEIGHT,
            letter_spacing: 0.0,
            align_text: cosmic_text::Align::Left,
            case: Case::None,
            decoration: Decoration::None,
            nowrap: false,
        }
    }

    /// A fresh style holding only what CSS inherits from `self`.
    fn inherit(&self) -> Self {
        let mut child = Self::root();
        child.colour = self.colour;
        child.font_size = self.font_size;
        child.weight = self.weight;
        child.italic = self.italic;
        child.line_height = self.line_height;
        child.letter_spacing = self.letter_spacing;
        child.align_text = self.align_text;
        child.case = self.case;
        child.decoration = self.decoration;
        child.nowrap = self.nowrap;
        // Not inherited, but it multiplies down: a subtree under a half
        // transparent box is half transparent.
        child.opacity = self.opacity;
        child
    }
}

/// Resolves every node's style: matching class rules in sheet order, then the
/// element's own declarations, over what the parent passed down.
fn resolve(scene: &Scene, sheet: &[Rule], state: State) -> Vec<Computed> {
    let root = Computed::root();
    let mut styles = vec![root.clone(); scene.nodes.len()];
    let mut stack: Vec<(usize, Computed)> =
        scene.roots.iter().rev().map(|&i| (i, root.clone())).collect();

    while let Some((index, parent)) = stack.pop() {
        let Some(node) = scene.node(index) else { continue };
        let mut style = parent.inherit();
        if node.text.is_none() {
            let width = scene.width as f32;
            for rule in sheet {
                if rule.min_width <= width
                    && rule.state.is_none_or(|s| s == state)
                    && node.classes.contains(&rule.class)
                {
                    for (name, value) in &rule.declarations {
                        apply(&mut style, name, value, &parent);
                    }
                }
            }
            for (name, value) in &node.declarations {
                apply(&mut style, name, value, &parent);
            }
        }
        for &child in node.children.iter().rev() {
            stack.push((child, style.clone()));
        }
        if let Some(slot) = styles.get_mut(index) {
            *slot = style;
        }
    }
    styles
}

/// One declaration, folded into the style. An unknown property is ignored.
#[allow(
    clippy::too_many_lines,
    reason = "one arm per property, in the order the design lists them, so the \
              vocabulary and its reading are read side by side"
)]
fn apply(style: &mut Computed, name: &str, value: &str, parent: &Computed) {
    let font_size = style.font_size;
    let len = |v: &str| length(v, font_size);
    match name {
        "display" => match value {
            "grid" => style.flow = Flow::Grid,
            // `-webkit-box` is the line clamp, which lays out as a column.
            "flex" | "-webkit-box" => style.flow = Flow::Flex,
            _ => {}
        },
        "flex-direction" => style.column = value != "row",
        "flex-wrap" => style.wrap = value == "wrap",
        "justify-content" => style.justify = alignment(value),
        "align-items" => style.align_items = item_alignment(value),
        "align-self" => style.align_self = item_alignment(value),
        "flex-grow" => {
            if let Ok(n) = value.parse() {
                style.grow = n;
            }
        }
        "flex-shrink" => {
            if let Ok(n) = value.parse() {
                style.shrink = n;
            }
        }
        "grid-template-columns" => {
            style.tracks =
                value.split(' ').filter(|t| !t.is_empty()).map(|t| track(t, font_size)).collect();
        }
        "grid-column" => {
            style.span = value.strip_prefix("span ").and_then(|n| n.trim().parse().ok());
        }
        "gap" => {
            if let Some(l) = len(value) {
                style.gap_column = l;
                style.gap_row = l;
            }
        }
        "column-gap" => style.gap_column = len(value).unwrap_or(style.gap_column),
        "row-gap" => style.gap_row = len(value).unwrap_or(style.gap_row),
        "padding" => set_sides(&mut style.padding, [0, 1, 2, 3], len(value)),
        "padding-inline" => set_sides(&mut style.padding, [0, 1], len(value)),
        "padding-block" => set_sides(&mut style.padding, [2, 3], len(value)),
        "padding-inline-start" => set_sides(&mut style.padding, [0], len(value)),
        "padding-inline-end" => set_sides(&mut style.padding, [1], len(value)),
        "padding-block-start" => set_sides(&mut style.padding, [2], len(value)),
        "padding-block-end" => set_sides(&mut style.padding, [3], len(value)),
        "width" => set_sides(&mut style.size, [0], len(value)),
        "height" => set_sides(&mut style.size, [1], len(value)),
        "min-width" => set_sides(&mut style.min, [0], len(value)),
        "min-height" => set_sides(&mut style.min, [1], len(value)),
        "max-width" => set_sides(&mut style.max, [0], len(value)),
        "max-height" => set_sides(&mut style.max, [1], len(value)),
        "aspect-ratio" => style.aspect = value.parse().ok().filter(|r: &f32| *r > 0.0),
        "position" => style.absolute = value == "absolute",
        "inset-inline-start" => set_sides(&mut style.inset, [0], len(value)),
        "inset-inline-end" => set_sides(&mut style.inset, [1], len(value)),
        "inset-block-start" => set_sides(&mut style.inset, [2], len(value)),
        "inset-block-end" => set_sides(&mut style.inset, [3], len(value)),
        "overflow" => style.clipped = [value != "visible"; 2],
        "overflow-x" => style.clipped[0] = value != "visible",
        "overflow-y" => style.clipped[1] = value != "visible",

        "background-color" => {
            style.background = match colour(value) {
                Some(Spec::Value(c)) => c,
                Some(Spec::Inherit) => parent.background,
                _ => Rgba::CLEAR,
            };
        }
        "color" => {
            style.colour = match colour(value) {
                Some(Spec::Value(c)) => c,
                Some(Spec::Transparent) => Rgba::CLEAR,
                _ => parent.colour,
            };
        }
        "border-width" => style.border_width = len(value).unwrap_or(style.border_width),
        "border-color" => {
            style.border_colour = match colour(value) {
                Some(Spec::Value(c)) => c,
                Some(Spec::Transparent) => Rgba::CLEAR,
                _ => style.colour,
            };
        }
        "border-style" => {
            style.border_style = match value {
                "none" => Border::None,
                "dashed" => Border::Dashed,
                _ => Border::Solid,
            };
        }
        "border-radius" => style.radius = len(value).unwrap_or(style.radius),
        "opacity" => {
            if let Ok(o) = value.parse::<f32>() {
                style.opacity = parent.opacity * o.clamp(0.0, 1.0);
            }
        }
        "box-shadow" => style.shadow = shadow(value, font_size),

        "font-size" => {
            style.font_size = match len(value) {
                Some(Len::Px(n)) => n.max(0.0),
                Some(Len::Percent(p)) => parent.font_size * p / 100.0,
                _ => style.font_size,
            };
        }
        "font-weight" => style.weight = value.parse().unwrap_or(style.weight),
        "font-style" => style.italic = value == "italic",
        "line-height" => style.line_height = value.parse().unwrap_or(style.line_height),
        "letter-spacing" => {
            if let Some(Len::Px(n)) = len(value) {
                style.letter_spacing = n;
            }
        }
        "text-align" => {
            style.align_text = match value {
                "center" => cosmic_text::Align::Center,
                "end" => cosmic_text::Align::End,
                "justify" => cosmic_text::Align::Justified,
                _ => cosmic_text::Align::Left,
            };
        }
        "text-transform" => {
            style.case = match value {
                "uppercase" => Case::Upper,
                "lowercase" => Case::Lower,
                "capitalize" => Case::Capitalize,
                _ => Case::None,
            };
        }
        "text-decoration-line" => {
            style.decoration = match value {
                "underline" => Decoration::Underline,
                "line-through" => Decoration::LineThrough,
                _ => Decoration::None,
            };
        }
        "text-wrap" => style.nowrap = value == "nowrap",
        // `font-family` resolves to the bundled family whatever it names, and
        // `cursor` paints nothing. Both parse so that a scene keeps them.
        _ => {}
    }
}

fn set_sides<const N: usize>(sides: &mut [Len], which: [usize; N], value: Option<Len>) {
    let Some(value) = value else { return };
    for i in which {
        if let Some(slot) = sides.get_mut(i) {
            *slot = value;
        }
    }
}

fn length(value: &str, _font_size: f32) -> Option<Len> {
    if value == "auto" {
        return Some(Len::Auto);
    }
    if let Some(n) = value.strip_suffix("px") {
        return n.parse().ok().map(Len::Px);
    }
    if let Some(n) = value.strip_suffix("rem") {
        return n.parse::<f32>().ok().map(|n| Len::Px(n * REM));
    }
    if let Some(n) = value.strip_suffix('%') {
        return n.parse().ok().map(Len::Percent);
    }
    value.parse().ok().map(Len::Px)
}

fn track(value: &str, font_size: f32) -> Len {
    if let Some(n) = value.strip_suffix("fr") {
        // A `fr` track shares the leftover room; the painter carries the share
        // as a percentage of it, which is what one track of `n` fr resolves to
        // once the tracks are summed.
        return n.parse().ok().map_or(Len::Auto, |n: f32| Len::Percent(-n));
    }
    length(value, font_size).unwrap_or(Len::Auto)
}

fn alignment(value: &str) -> Option<AlignContent> {
    match value {
        "flex-start" => Some(AlignContent::FLEX_START),
        "center" => Some(AlignContent::CENTER),
        "flex-end" => Some(AlignContent::FLEX_END),
        "stretch" => Some(AlignContent::STRETCH),
        "space-between" => Some(AlignContent::SPACE_BETWEEN),
        "space-around" => Some(AlignContent::SPACE_AROUND),
        "space-evenly" => Some(AlignContent::SPACE_EVENLY),
        _ => None,
    }
}

/// The same seven as item alignment. `space-*` has no item meaning, so it reads
/// as the start it behaves like.
fn item_alignment(value: &str) -> Option<AlignItems> {
    match value {
        "flex-start" | "space-between" | "space-around" | "space-evenly" => {
            Some(AlignItems::FLEX_START)
        }
        "center" => Some(AlignItems::CENTER),
        "flex-end" => Some(AlignItems::FLEX_END),
        "stretch" => Some(AlignItems::STRETCH),
        _ => None,
    }
}

fn colour(value: &str) -> Option<Spec> {
    match value {
        "transparent" => return Some(Spec::Transparent),
        "inherit" => return Some(Spec::Inherit),
        _ => {}
    }
    if value.starts_with("var(") {
        return Some(Spec::Token);
    }
    let inner = value.strip_prefix("rgba(").or_else(|| value.strip_prefix("rgb("))?;
    let inner = inner.strip_suffix(')')?;
    let mut parts = inner.split(',').map(str::trim);
    let r = parts.next()?.parse().ok()?;
    let g = parts.next()?.parse().ok()?;
    let b = parts.next()?.parse().ok()?;
    let a = match parts.next() {
        Some(a) => a.parse::<f32>().ok()?.clamp(0.0, 1.0),
        None => 1.0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(Spec::Value(Rgba { r, g, b, a }))
}

/// `<x> <y> <blur> <spread> <colour>`.
fn shadow(value: &str, font_size: f32) -> Option<Shadow> {
    let mut parts = value.split(' ').filter(|p| !p.is_empty());
    let px = |v: Option<&str>| match length(v?, font_size) {
        Some(Len::Px(n)) => Some(n),
        _ => Some(0.0),
    };
    let x = px(parts.next())?;
    let y = px(parts.next())?;
    let blur = px(parts.next())?;
    let spread = px(parts.next())?;
    let colour = match colour(parts.next()?)? {
        Spec::Value(c) => c,
        _ => return None,
    };
    Some(Shadow { x, y, blur, spread, colour })
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// **The one rounding policy.** A layout number becomes a device pixel here and
/// nowhere else, and what is rounded is the box's *absolute* edge — its left
/// and its right, never its width — so two boxes that share an edge share a
/// pixel column. Half rounds up, which is one add and one floor and therefore
/// the same instruction sequence on both platforms.
fn px(v: f32) -> i32 {
    if v.is_nan() { 0 } else { (v + 0.5).floor() as i32 }
}

fn dimension(len: Len) -> Dimension {
    match len {
        Len::Auto => Dimension::auto(),
        Len::Px(n) => Dimension::length(n),
        Len::Percent(p) => Dimension::percent(p / 100.0),
    }
}

fn dimension_auto(len: Len) -> LengthPercentageAuto {
    match len {
        Len::Auto => LengthPercentageAuto::auto(),
        Len::Px(n) => LengthPercentageAuto::length(n),
        Len::Percent(p) => LengthPercentageAuto::percent(p / 100.0),
    }
}

fn spacing(len: Len) -> LengthPercentage {
    match len {
        Len::Auto | Len::Px(0.0) => LengthPercentage::length(0.0),
        Len::Px(n) => LengthPercentage::length(n),
        Len::Percent(p) => LengthPercentage::percent(p / 100.0),
    }
}

fn taffy_style(c: &Computed) -> Style {
    let overflow = |on: bool| if on { Overflow::Hidden } else { Overflow::Visible };
    Style {
        display: match c.flow {
            Flow::Flex => Display::Flex,
            Flow::Grid => Display::Grid,
        },
        flex_direction: if c.column { FlexDirection::Column } else { FlexDirection::Row },
        flex_wrap: if c.wrap { FlexWrap::Wrap } else { FlexWrap::NoWrap },
        justify_content: c.justify,
        align_items: c.align_items,
        align_self: c.align_self,
        flex_grow: c.grow,
        flex_shrink: c.shrink,
        // A flex item sizes from its content rather than from `auto` meaning
        // zero, which is what a box holding one text run wants.
        flex_basis: Dimension::auto(),
        grid_template_columns: c.tracks.iter().map(|t| grid_track(*t)).collect(),
        grid_column: c.span.map_or(Line::from_span(1), Line::from_span),
        gap: Size { width: spacing(c.gap_column), height: spacing(c.gap_row) },
        padding: Rect {
            left: spacing(c.padding[0]),
            right: spacing(c.padding[1]),
            top: spacing(c.padding[2]),
            bottom: spacing(c.padding[3]),
        },
        border: match c.border_style {
            Border::None => Rect::length(0.0),
            _ => Rect::length(match c.border_width {
                Len::Px(n) => n,
                _ => 0.0,
            }),
        },
        size: Size { width: dimension(c.size[0]), height: dimension(c.size[1]) },
        min_size: Size { width: dimension_auto(c.min[0]), height: dimension_auto(c.min[1]) },
        max_size: Size { width: dimension_auto(c.max[0]), height: dimension_auto(c.max[1]) },
        aspect_ratio: c.aspect,
        position: if c.absolute { Position::Absolute } else { Position::Relative },
        inset: Rect {
            left: dimension_auto(c.inset[0]),
            right: dimension_auto(c.inset[1]),
            top: dimension_auto(c.inset[2]),
            bottom: dimension_auto(c.inset[3]),
        },
        overflow: Point { x: overflow(c.clipped[0]), y: overflow(c.clipped[1]) },
        ..Style::default()
    }
}

fn grid_track(len: Len) -> GridTemplateComponent<String> {
    let sizing = match len {
        // A negative percent is how `track` carries `<n>fr`.
        Len::Percent(p) if p < 0.0 => TrackSizingFunction {
            min: MinTrackSizingFunction::AUTO,
            max: MaxTrackSizingFunction::fr(-p),
        },
        Len::Percent(p) => TrackSizingFunction::from_percent(p / 100.0),
        Len::Px(n) => TrackSizingFunction::from_length(n),
        Len::Auto => TrackSizingFunction::AUTO,
    };
    GridTemplateComponent::Single(sizing)
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// A `FontSystem` with the three bundled faces in it and nothing else.
///
/// `new_with_locale_and_db` rather than `new_with_fonts`, for two reasons: the
/// second reads the machine's locale, and neither calls `load_system_fonts`.
/// What the platform still contributes is a list of fallback family *names* —
/// "Noto Sans", "Apple Symbols" — and none of them is in this database, so they
/// resolve to nothing on every host and the bundled family is the only face a
/// glyph can come from.
fn font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    for face in [REGULAR, BOLD, ITALIC] {
        db.load_font_source(fontdb::Source::Binary(Arc::new(face)));
    }
    db.set_sans_serif_family(FAMILY);
    db.set_serif_family(FAMILY);
    db.set_monospace_family(FAMILY);
    FontSystem::new_with_locale_and_db("en-US".to_string(), db)
}

fn transformed(text: &str, case: Case) -> String {
    match case {
        Case::None => text.to_string(),
        Case::Upper => text.to_uppercase(),
        Case::Lower => text.to_lowercase(),
        Case::Capitalize => {
            let mut out = String::with_capacity(text.len());
            let mut fresh = true;
            for c in text.chars() {
                if fresh {
                    out.extend(c.to_uppercase());
                } else {
                    out.push(c);
                }
                fresh = c.is_whitespace();
            }
            out
        }
    }
}

/// One run of text, shaped into a buffer at the given width.
fn shape(fonts: &mut FontSystem, text: &str, style: &Computed, width: Option<f32>) -> Buffer {
    let size = style.font_size.max(1.0);
    let mut buffer = Buffer::new(fonts, Metrics::new(size, size * style.line_height));
    buffer.set_hinting(Hinting::Disabled);
    buffer.set_wrap(if style.nowrap { Wrap::None } else { Wrap::WordOrGlyph });
    buffer.set_size(width, None);

    let mut attrs = Attrs::new()
        .family(Family::Name(FAMILY))
        .weight(Weight(style.weight))
        .cache_key_flags(CacheKeyFlags::DISABLE_HINTING);
    if style.italic {
        attrs = attrs.style(cosmic_text::Style::Italic);
    }
    if style.letter_spacing != 0.0 {
        attrs = attrs.letter_spacing(style.letter_spacing / size);
    }
    buffer.set_text(
        &transformed(text, style.case),
        &attrs,
        Shaping::Advanced,
        Some(style.align_text),
    );
    buffer.shape_until_scroll(fonts, false);
    buffer
}

/// The width and height a shaped buffer occupies.
fn extent(buffer: &Buffer) -> (f32, f32) {
    let mut width = 0.0_f32;
    let mut height = 0.0_f32;
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        height = run.line_top + run.line_height;
    }
    (width, height)
}

// ---------------------------------------------------------------------------
// Paint
// ---------------------------------------------------------------------------

/// Lays the scene out and draws it onto an opaque white canvas.
///
/// White rather than transparent because a snapshot is a picture of a page, and
/// a page has a colour before anything is drawn on it.
fn paint(scene: &Scene, styles: &[Computed]) -> Result<Pixmap, String> {
    let mut fonts = font_system();
    let mut tree: TaffyTree<usize> = TaffyTree::new();
    tree.disable_rounding();

    let mut ids: Vec<Option<NodeId>> = vec![None; scene.nodes.len()];
    let roots = build(scene, styles, &mut tree, &scene.roots, &mut ids)?;
    // **The canvas is an unstyled `stack`, sized to the viewport**, and that is
    // load-bearing rather than tidy: wrapping a tree in `ui.stack([], [...])`
    // must not move a pixel. An unstyled `stack` lowers to `display: flex;
    // flex-direction: column` (`$tree_declare`'s tag 6) and takes flexbox's own
    // `align-items: stretch`, so the canvas has to be the same thing. `taffy`'s
    // default is a *row*, whose cross axis is vertical, and a canvas left at it
    // stretches its child's height — which the same child one `stack` deeper
    // would not do.
    let viewport = Style {
        flex_direction: FlexDirection::Column,
        size: Size {
            width: Dimension::length(scene.width as f32),
            height: Dimension::length(scene.height as f32),
        },
        ..Style::default()
    };
    let root = tree.new_with_children(viewport, &roots).map_err(|e| format!("layout: {e}"))?;

    tree.compute_layout_with_measure(
        root,
        Size {
            width: AvailableSpace::Definite(scene.width as f32),
            height: AvailableSpace::Definite(scene.height as f32),
        },
        |input, _, context, style| {
            compute_leaf_layout(input, style, |_, _| 0.0, |known, available| {
                let Some(&mut index) = context else { return Size::ZERO };
                let Some(node) = scene.node(index) else { return Size::ZERO };
                let (Some(text), Some(style)) = (node.text.as_deref(), styles.get(index)) else {
                    return Size::ZERO;
                };
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(w) => Some(w),
                    _ => None,
                });
                let (w, h) = extent(&shape(&mut fonts, text, style, width));
                Size { width: known.width.unwrap_or(w), height: known.height.unwrap_or(h) }
            })
        },
    )
    .map_err(|e| format!("layout: {e}"))?;

    let mut canvas = Pixmap::new(scene.width, scene.height)
        .ok_or_else(|| format!("the viewport {}x{} has no canvas", scene.width, scene.height))?;
    canvas.fill(tiny_skia::Color::WHITE);

    let mut cache = SwashCache::new();
    let mut painter =
        Painter { scene, styles, tree: &tree, ids: &ids, fonts: &mut fonts, cache: &mut cache };
    for &index in &scene.roots {
        painter.draw(&mut canvas, index, 0.0, 0.0, None);
    }
    Ok(canvas)
}

/// Mirrors the scene into a `taffy` tree, remembering each node's id.
fn build(
    scene: &Scene,
    styles: &[Computed],
    tree: &mut TaffyTree<usize>,
    indices: &[usize],
    ids: &mut [Option<NodeId>],
) -> Result<Vec<NodeId>, String> {
    let mut out = Vec::with_capacity(indices.len());
    for &index in indices {
        let Some(node) = scene.node(index) else { continue };
        let style = styles.get(index).cloned().unwrap_or_else(Computed::root);
        let id = if node.text.is_some() {
            tree.new_leaf_with_context(taffy_style(&style), index)
        } else {
            let children = build(scene, styles, tree, &node.children, ids)?;
            tree.new_with_children(taffy_style(&style), &children)
        }
        .map_err(|e| format!("layout: {e}"))?;
        if let Some(slot) = ids.get_mut(index) {
            *slot = Some(id);
        }
        out.push(id);
    }
    Ok(out)
}

struct Painter<'a> {
    scene: &'a Scene,
    styles: &'a [Computed],
    tree: &'a TaffyTree<usize>,
    ids: &'a [Option<NodeId>],
    fonts: &'a mut FontSystem,
    cache: &'a mut SwashCache,
}

impl Painter<'_> {
    /// Draws one node and its children, in document order, which is paint
    /// order: a later sibling covers an earlier one.
    fn draw(&mut self, canvas: &mut Pixmap, index: usize, x: f32, y: f32, clip: Option<&Mask>) {
        let (Some(node), Some(style), Some(&Some(id))) =
            (self.scene.node(index), self.styles.get(index), self.ids.get(index))
        else {
            return;
        };
        let Ok(layout) = self.tree.layout(id) else { return };
        let left = x + layout.location.x;
        let top = y + layout.location.y;
        let right = left + layout.size.width;
        let bottom = top + layout.size.height;
        let box_ = Box2 { l: px(left), t: px(top), r: px(right), b: px(bottom) };

        if let Some(text) = node.text.as_deref() {
            // The *unrounded* width, because that is the one the measure pass
            // wrapped against; a pixel less would wrap the last word again.
            self.text(canvas, text, style, box_, layout.size.width, clip);
            return;
        }

        let radius = resolve_length(style.radius, layout.size.width);
        if let Some(shadow) = style.shadow {
            let cast = box_.offset(shadow.x, shadow.y).grow(shadow.spread);
            let corner = radius + shadow.spread;
            if shadow.blur > 0.0 {
                cast_blurred(canvas, cast, corner, shadow, style.opacity, clip);
            } else {
                fill(canvas, cast, corner, shadow.colour, style.opacity, clip);
            }
        }
        if style.background.visible() {
            fill(canvas, box_, radius, style.background, style.opacity, clip);
        }
        let width = match style.border_width {
            Len::Px(n) if style.border_style != Border::None => n,
            _ => 0.0,
        };
        if width > 0.0 && style.border_colour.visible() {
            stroke(canvas, box_, radius, width, style, clip);
        }

        let mut owned;
        let inner = if style.clipped[0] || style.clipped[1] {
            owned = clip.cloned().or_else(|| full_mask(canvas.width(), canvas.height()));
            if let Some(mask) = owned.as_mut() {
                intersect(mask, box_, radius);
            }
            owned.as_ref()
        } else {
            clip
        };
        for &child in &node.children {
            self.draw(canvas, child, left, top, inner);
        }
    }

    /// Draws one text run at the box the layout gave it.
    fn text(
        &mut self,
        canvas: &mut Pixmap,
        text: &str,
        style: &Computed,
        box_: Box2,
        width: f32,
        clip: Option<&Mask>,
    ) {
        let mut buffer = shape(self.fonts, text, style, Some(width));
        let colour = premultiply(style.colour, style.opacity);
        if colour[3] == 0 {
            return;
        }
        let ink = cosmic_text::Color::rgba(colour[0], colour[1], colour[2], colour[3]);
        let (ox, oy) = (box_.l, box_.t);
        let (cw, ch) = (canvas.width(), canvas.height());
        let clipped = clip;
        let pixels = canvas.pixels_mut();
        buffer.draw(self.fonts, self.cache, ink, |gx, gy, w, h, pixel| {
            for dy in 0..h.min(64) {
                for dx in 0..w.min(4096) {
                    let sx = ox.saturating_add(gx).saturating_add(dx as i32);
                    let sy = oy.saturating_add(gy).saturating_add(dy as i32);
                    if sx < 0 || sy < 0 || sx as u32 >= cw || sy as u32 >= ch {
                        continue;
                    }
                    let coverage = clipped.map_or(255, |mask| {
                        let i = (sy as u32 as usize)
                            .saturating_mul(cw as usize)
                            .saturating_add(sx as usize);
                        mask.data().get(i).copied().unwrap_or(0)
                    });
                    if coverage == 0 {
                        continue;
                    }
                    let a = mul255(pixel.a(), coverage);
                    let src = [
                        mul255(pixel.r(), a),
                        mul255(pixel.g(), a),
                        mul255(pixel.b(), a),
                        a,
                    ];
                    let i = (sy as u32 as usize)
                        .saturating_mul(cw as usize)
                        .saturating_add(sx as usize);
                    if let Some(slot) = pixels.get_mut(i) {
                        *slot = over(src, *slot);
                    }
                }
            }
        });
        if style.decoration != Decoration::None {
            self.rule(canvas, &buffer, style, box_, clip);
        }
    }

    /// The underline or the strike-through, as a one-pixel rule under or
    /// through each line the buffer laid out.
    fn rule(
        &mut self,
        canvas: &mut Pixmap,
        buffer: &Buffer,
        style: &Computed,
        box_: Box2,
        clip: Option<&Mask>,
    ) {
        let thickness = (style.font_size / 14.0).max(1.0);
        for run in buffer.layout_runs() {
            // The glyphs' own extent, not the line's: a centred or right
            // aligned line does not start at the box's left edge, and a
            // right-to-left run stores its glyphs the other way round.
            let mut from = f32::INFINITY;
            let mut to = f32::NEG_INFINITY;
            for glyph in run.glyphs {
                from = from.min(glyph.x);
                to = to.max(glyph.x + glyph.w);
            }
            if to <= from {
                continue;
            }
            let offset = match style.decoration {
                Decoration::Underline => run.line_y + style.font_size * 0.12,
                _ => run.line_y - style.font_size * 0.3,
            };
            let top = box_.t.saturating_add(px(offset));
            let bar = Box2 {
                l: box_.l.saturating_add(px(from)),
                t: top,
                r: box_.l.saturating_add(px(to)),
                b: top.saturating_add(px(thickness).max(1)),
            };
            fill(canvas, bar, 0.0, style.colour, style.opacity, clip);
        }
    }
}

/// A box in whole device pixels.
#[derive(Clone, Copy)]
struct Box2 {
    l: i32,
    t: i32,
    r: i32,
    b: i32,
}

impl Box2 {
    fn offset(self, x: f32, y: f32) -> Self {
        Self {
            l: self.l.saturating_add(px(x)),
            t: self.t.saturating_add(px(y)),
            r: self.r.saturating_add(px(x)),
            b: self.b.saturating_add(px(y)),
        }
    }

    fn grow(self, by: f32) -> Self {
        let n = px(by);
        Self {
            l: self.l.saturating_sub(n),
            t: self.t.saturating_sub(n),
            r: self.r.saturating_add(n),
            b: self.b.saturating_add(n),
        }
    }

    fn path(self, radius: f32) -> Option<tiny_skia::Path> {
        let (l, t) = (self.l as f32, self.t as f32);
        let (r, b) = (self.r as f32, self.b as f32);
        if r <= l || b <= t {
            return None;
        }
        let radius = radius.max(0.0).min((r - l).min(b - t) / 2.0);
        let mut path = PathBuilder::new();
        if radius <= 0.0 {
            path.push_rect(tiny_skia::Rect::from_ltrb(l, t, r, b)?);
            return path.finish();
        }
        // A quarter circle as one cubic; `k` is the classic control-point
        // fraction, and it is a constant so both platforms draw the same arc.
        let k = radius * 0.552_285;
        path.move_to(l + radius, t);
        path.line_to(r - radius, t);
        path.cubic_to(r - radius + k, t, r, t + radius - k, r, t + radius);
        path.line_to(r, b - radius);
        path.cubic_to(r, b - radius + k, r - radius + k, b, r - radius, b);
        path.line_to(l + radius, b);
        path.cubic_to(l + radius - k, b, l, b - radius + k, l, b - radius);
        path.line_to(l, t + radius);
        path.cubic_to(l, t + radius - k, l + radius - k, t, l + radius, t);
        path.close();
        path.finish()
    }
}

fn resolve_length(len: Len, basis: f32) -> f32 {
    match len {
        Len::Auto => 0.0,
        Len::Px(n) => n,
        Len::Percent(p) => basis * p / 100.0,
    }
}

fn fill(
    canvas: &mut Pixmap,
    box_: Box2,
    radius: f32,
    colour: Rgba,
    opacity: f32,
    clip: Option<&Mask>,
) {
    let Some(path) = box_.path(radius) else { return };
    let paint =
        Paint { anti_alias: true, shader: shade(colour, opacity), ..Paint::default() };
    canvas.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), clip);
}

fn stroke(
    canvas: &mut Pixmap,
    box_: Box2,
    radius: f32,
    width: f32,
    style: &Computed,
    clip: Option<&Mask>,
) {
    // A CSS border sits inside the box, so the centreline is half a width in.
    let inset = box_.grow(-width / 2.0);
    let Some(path) = inset.path((radius - width / 2.0).max(0.0)) else { return };
    let paint = Paint {
        anti_alias: true,
        shader: shade(style.border_colour, style.opacity),
        ..Paint::default()
    };
    let mut pen = Stroke { width, ..Stroke::default() };
    if style.border_style == Border::Dashed {
        pen.dash = StrokeDash::new(vec![width * 3.0, width * 2.0], 0.0);
    }
    canvas.stroke_path(&path, &paint, &pen, Transform::identity(), clip);
}

fn shade<'a>(colour: Rgba, opacity: f32) -> tiny_skia::Shader<'a> {
    let a = (colour.a * opacity).clamp(0.0, 1.0);
    let solid = tiny_skia::Color::from_rgba(
        f32::from(colour.r) / 255.0,
        f32::from(colour.g) / 255.0,
        f32::from(colour.b) / 255.0,
        a,
    )
    .unwrap_or(tiny_skia::Color::TRANSPARENT);
    tiny_skia::Shader::SolidColor(solid)
}

fn full_mask(width: u32, height: u32) -> Option<Mask> {
    let mut mask = Mask::new(width, height)?;
    mask.data_mut().fill(255);
    Some(mask)
}

/// Narrows `mask` to a box, **rounded corners included**: `overflow: hidden`
/// on a box with a radius clips to the shape the box paints, so a child does
/// not square off a corner its parent rounded.
fn intersect(mask: &mut Mask, box_: Box2, radius: f32) {
    if let Some(path) = box_.path(radius) {
        mask.intersect_path(&path, FillRule::Winding, true, Transform::identity());
    } else {
        mask.clear();
    }
}

/// Pours a shadow's colour through a blurred coverage mask of its cast shape.
///
/// The shape is rasterized once into an alpha mask, blurred by [`blur`], met
/// with whatever clip the caller was already under, and then used as the clip
/// of a fill over the whole canvas — so the colour lands at exactly the
/// coverage the blur computed, and nowhere the caller had already excluded.
fn cast_blurred(
    canvas: &mut Pixmap,
    cast: Box2,
    radius: f32,
    shadow: Shadow,
    opacity: f32,
    clip: Option<&Mask>,
) {
    let (width, height) = (canvas.width(), canvas.height());
    let (Some(mut mask), Some(path)) = (Mask::new(width, height), cast.path(radius)) else {
        return;
    };
    mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
    blur(&mut mask, shadow.blur);
    if let Some(outer) = clip {
        narrow(&mut mask, outer);
    }
    let all = Box2 {
        l: 0,
        t: 0,
        r: i32::try_from(width).unwrap_or(i32::MAX),
        b: i32::try_from(height).unwrap_or(i32::MAX),
    };
    let Some(path) = all.path(0.0) else { return };
    let paint =
        Paint { anti_alias: false, shader: shade(shadow.colour, opacity), ..Paint::default() };
    canvas.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), Some(&mask));
}

/// Multiplies `mask` by `other`, which is mask intersection on coverage.
fn narrow(mask: &mut Mask, other: &Mask) {
    if mask.width() != other.width() || mask.height() != other.height() {
        return;
    }
    for (a, b) in mask.data_mut().iter_mut().zip(other.data()) {
        *a = mul255(*a, *b);
    }
}

/// Blurs a coverage mask in place, by CSS's reading of a blur radius.
///
/// Three box blurs, which is the approximation the SVG filter specification
/// writes down for a Gaussian and the one every browser uses for a shadow. A
/// CSS blur radius of `n` is a Gaussian of standard deviation `n / 2`, and the
/// box width that stands in for it is
/// `floor(sigma * 3 * sqrt(2 * PI) / 4 + 0.5)`.
///
/// **Integers all the way**, on purpose: the passes are running sums over
/// `u8`s with one rounded divide, so the answer is the same answer on every
/// target. Only the box width is computed in floating point, and it is one
/// `sqrt` of a constant times a length both platforms already agree on.
fn blur(mask: &mut Mask, radius: f32) {
    let sigma = radius / 2.0;
    if sigma <= 0.0 || !sigma.is_finite() {
        return;
    }
    // 3 * sqrt(2 * PI) / 4, the SVG filter primitive's own constant.
    let Ok(d) = u32::try_from((sigma * 1.881_976_2 + 0.5).floor() as i64) else { return };
    if d == 0 {
        return;
    }
    // An odd box has a centre; an even one does not, so the three passes lean
    // left, then right, then take one more sample to land back where they
    // started. SVG filters §15.17 states exactly this.
    let passes = if d % 2 == 1 {
        [(d, d / 2), (d, d / 2), (d, d / 2)]
    } else {
        [(d, d / 2), (d, d / 2 - 1), (d + 1, d / 2)]
    };
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    let mut scratch = vec![0u8; w.saturating_mul(h)];
    for (size, lead) in passes {
        rows(mask.data_mut(), &mut scratch, w, h, size as usize, lead as usize);
    }
    for (size, lead) in passes {
        columns(mask.data_mut(), &mut scratch, w, h, size as usize, lead as usize);
    }
}

/// One horizontal box pass. The window for the pixel at `at` is
/// `[at - lead, at - lead + size)`, and off the ends the mask reads as zero.
fn rows(data: &mut [u8], scratch: &mut [u8], w: usize, h: usize, size: usize, lead: usize) {
    if size == 0 || w == 0 {
        return;
    }
    let (half, n) = (size as u32 / 2, size as u32);
    let (size, lead) = (size as isize, lead as isize);
    let end = w as isize;
    for y in 0..h {
        let row = y.saturating_mul(w);
        let read = |sum: &mut u32, at: isize, add: bool| {
            if at < 0 || at >= end {
                return;
            }
            let byte = u32::from(data[row.saturating_add(at as usize)]);
            *sum = if add { sum.saturating_add(byte) } else { sum.saturating_sub(byte) };
        };
        let mut sum: u32 = 0;
        for j in 0..size {
            read(&mut sum, j - lead, true);
        }
        for at in 0..end {
            scratch[row.saturating_add(at as usize)] =
                u8::try_from((sum + half) / n).unwrap_or(255);
            read(&mut sum, at - lead, false);
            read(&mut sum, at - lead + size, true);
        }
    }
    data.copy_from_slice(scratch);
}

/// [`rows`], the other way.
fn columns(data: &mut [u8], scratch: &mut [u8], w: usize, h: usize, size: usize, lead: usize) {
    if size == 0 || h == 0 {
        return;
    }
    let (half, n) = (size as u32 / 2, size as u32);
    let (size, lead) = (size as isize, lead as isize);
    let end = h as isize;
    for x in 0..w {
        let read = |sum: &mut u32, at: isize, add: bool| {
            if at < 0 || at >= end {
                return;
            }
            let byte = u32::from(data[(at as usize).saturating_mul(w).saturating_add(x)]);
            *sum = if add { sum.saturating_add(byte) } else { sum.saturating_sub(byte) };
        };
        let mut sum: u32 = 0;
        for j in 0..size {
            read(&mut sum, j - lead, true);
        }
        for at in 0..end {
            scratch[(at as usize).saturating_mul(w).saturating_add(x)] =
                u8::try_from((sum + half) / n).unwrap_or(255);
            read(&mut sum, at - lead, false);
            read(&mut sum, at - lead + size, true);
        }
    }
    data.copy_from_slice(scratch);
}

/// `(x * y + 127) / 255`, without a divide and without a rounding surprise.
fn mul255(x: u8, y: u8) -> u8 {
    let p = u32::from(x) * u32::from(y) + 128;
    u8::try_from((p + (p >> 8)) >> 8).unwrap_or(255)
}

fn premultiply(colour: Rgba, opacity: f32) -> [u8; 4] {
    let a = ((colour.a * opacity).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [colour.r, colour.g, colour.b, a]
}

/// Source-over, on premultiplied bytes.
fn over(src: [u8; 4], dst: PremultipliedColorU8) -> PremultipliedColorU8 {
    let inv = 255_u8.saturating_sub(src[3]);
    let r = src[0].saturating_add(mul255(dst.red(), inv));
    let g = src[1].saturating_add(mul255(dst.green(), inv));
    let b = src[2].saturating_add(mul255(dst.blue(), inv));
    let a = src[3].saturating_add(mul255(dst.alpha(), inv));
    PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a)
        .unwrap_or(PremultipliedColorU8::TRANSPARENT)
}

/// The canvas as straight (un-premultiplied) RGBA rows, which is what a PNG
/// holds.
fn straight(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixmap.data().len());
    for p in pixmap.pixels() {
        let c = p.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

// ---------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------

const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// Bytes to a pixel, which is fixed: this file writes 8-bit RGBA and nothing
/// else. The row filters are defined in terms of it.
const BPP: usize = 4;

/// Writes an 8-bit RGBA PNG: one `IHDR`, one `IDAT`, one `IEND`.
///
/// **The encoder is written here rather than taken from a crate, and that is
/// the whole point of it.** A golden is compared byte for byte, so the same
/// pixels have to produce the same file on every machine and in every version —
/// and a general deflate encoder gives itself freedom (match choice, block
/// splitting, tree building) that two releases of it spend differently. This
/// one has none:
///
/// * **Fixed Huffman**, block type 01, the static trees of RFC 1951 §3.2.6.
///   Nothing is built from the data, so there is no tie to break.
/// * **One hash-chain match finder**, with a fixed window, a fixed chain limit
///   and a greedy choice. No randomness, no time budget, no hash-map walk.
/// * **No floating point anywhere**, so nothing turns on a rounding mode.
///
/// The row filters in front of it are where most of the saving on flat colour
/// comes from. They are picked by the standard minimum-sum-of-absolute-
/// differences rule, in integers, with the lowest filter number winning a tie.
fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&SIGNATURE);

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // 8 bits a channel, colour type 6 (RGBA), deflate, adaptive filtering, no
    // interlace.
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);

    let raw = filter_rows(width, height, rgba);
    chunk(&mut out, b"IDAT", &deflate(&raw));

    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32([kind.as_slice(), data]).to_be_bytes());
}

// ---------------------------------------------------------------------------
// The row filters
// ---------------------------------------------------------------------------

/// PNG's Paeth predictor, §6.6, transcribed. `a` winning a three-way tie is
/// part of the definition rather than a choice made here.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i32::from(a) + i32::from(b) - i32::from(c);
    let pa = (p - i32::from(a)).abs();
    let pb = (p - i32::from(b)).abs();
    let pc = (p - i32::from(c)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// One row through one filter. `previous` is the row above in its unfiltered
/// form, and is all zeroes for the first row.
fn apply_filter(kind: u8, line: &[u8], previous: &[u8], out: &mut [u8]) {
    for i in 0..line.len() {
        let left = |row: &[u8]| i.checked_sub(BPP).and_then(|j| row.get(j)).copied().unwrap_or(0);
        let x = line.get(i).copied().unwrap_or(0);
        let a = left(line);
        let b = previous.get(i).copied().unwrap_or(0);
        let c = left(previous);
        let value = match kind {
            1 => x.wrapping_sub(a),
            2 => x.wrapping_sub(b),
            3 => x.wrapping_sub(((u16::from(a) + u16::from(b)) / 2) as u8),
            4 => x.wrapping_sub(paeth(a, b, c)),
            _ => x,
        };
        if let Some(slot) = out.get_mut(i) {
            *slot = value;
        }
    }
}

/// The heuristic every PNG encoder uses: the filtered bytes summed as signed
/// magnitudes, which is smallest when the row came out closest to flat.
fn filter_cost(row: &[u8]) -> u32 {
    let mut sum = 0_u32;
    for &b in row {
        sum = sum.saturating_add(if b < 128 { u32::from(b) } else { 256 - u32::from(b) });
    }
    sum
}

/// The image as filtered rows: a filter byte, then the row, for each row.
fn filter_rows(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let stride = (width as usize).saturating_mul(BPP);
    let mut out = Vec::with_capacity(rgba.len().saturating_add(height as usize));
    let mut line = vec![0_u8; stride];
    let mut previous = vec![0_u8; stride];
    let mut candidate = vec![0_u8; stride];
    let mut best = vec![0_u8; stride];
    for row in 0..height as usize {
        line.fill(0);
        let start = row.saturating_mul(stride);
        let source = rgba.get(start..).unwrap_or(&[]);
        let taken = source.len().min(stride);
        if let (Some(into), Some(from)) = (line.get_mut(..taken), source.get(..taken)) {
            into.copy_from_slice(from);
        }

        let mut chosen = 0_u8;
        let mut cheapest = u32::MAX;
        for kind in 0..5_u8 {
            apply_filter(kind, &line, &previous, &mut candidate);
            let cost = filter_cost(&candidate);
            // Strictly cheaper, so the lowest filter number wins a tie.
            if cost < cheapest {
                cheapest = cost;
                chosen = kind;
                best.copy_from_slice(&candidate);
            }
        }
        out.push(chosen);
        out.extend_from_slice(&best);
        previous.copy_from_slice(&line);
    }
    out
}

/// One filtered row, put back. `row` is rebuilt left to right, so the byte the
/// filters call `a` is already unfiltered by the time it is read.
fn unfilter(kind: u8, row: &mut [u8], previous: &[u8]) -> Result<(), String> {
    if kind > 4 {
        return Err(format!("the PNG uses row filter {kind}, which does not exist"));
    }
    for i in 0..row.len() {
        let left = |r: &[u8]| i.checked_sub(BPP).and_then(|j| r.get(j)).copied().unwrap_or(0);
        let a = left(row);
        let b = previous.get(i).copied().unwrap_or(0);
        let c = left(previous);
        let add = match kind {
            1 => a,
            2 => b,
            3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
            4 => paeth(a, b, c),
            _ => 0,
        };
        if let Some(slot) = row.get_mut(i) {
            *slot = slot.wrapping_add(add);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Deflate
// ---------------------------------------------------------------------------

/// How far back a match may reach — deflate's maximum, and a constant, so the
/// same input always searches the same bytes.
const WINDOW: usize = 32768;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
/// How many candidates one position tries before taking what it has. A count
/// rather than a time budget: a deadline would make the output depend on how
/// busy the machine was.
const CHAIN: usize = 128;
const HASH_SIZE: usize = 1 << 15;
/// The empty slot in a hash chain.
const NONE: u32 = u32::MAX;

/// `(first length, extra bits)` for symbols 257..=285, RFC 1951 §3.2.5.
const LENGTHS: [(u16, u8); 29] = [
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 0),
    (11, 1),
    (13, 1),
    (15, 1),
    (17, 1),
    (19, 2),
    (23, 2),
    (27, 2),
    (31, 2),
    (35, 3),
    (43, 3),
    (51, 3),
    (59, 3),
    (67, 4),
    (83, 4),
    (99, 4),
    (115, 4),
    (131, 5),
    (163, 5),
    (195, 5),
    (227, 5),
    (258, 0),
];

/// `(first distance, extra bits)` for distance codes 0..=29, same section.
const DISTANCES: [(u16, u8); 30] = [
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 1),
    (7, 1),
    (9, 2),
    (13, 2),
    (17, 3),
    (25, 3),
    (33, 4),
    (49, 4),
    (65, 5),
    (97, 5),
    (129, 6),
    (193, 6),
    (257, 7),
    (385, 7),
    (513, 8),
    (769, 8),
    (1025, 9),
    (1537, 9),
    (2049, 10),
    (3073, 10),
    (4097, 11),
    (6145, 11),
    (8193, 12),
    (12289, 12),
    (16385, 13),
    (24577, 13),
];

/// Deflate packs its own fields low bit first and its Huffman codes high bit
/// first, so this writer offers both and no call site has to remember which.
struct BitWriter {
    out: Vec<u8>,
    held: u32,
    count: u32,
}

impl BitWriter {
    fn new(capacity: usize) -> Self {
        Self { out: Vec::with_capacity(capacity), held: 0, count: 0 }
    }

    /// A plain field: the low bit goes out first.
    fn bits(&mut self, value: u32, width: u32) {
        let mask = if width >= 32 { u32::MAX } else { (1_u32 << width) - 1 };
        self.held |= (value & mask) << self.count;
        self.count = self.count.saturating_add(width);
        while self.count >= 8 {
            self.out.push((self.held & 0xff) as u8);
            self.held >>= 8;
            self.count = self.count.saturating_sub(8);
        }
    }

    /// A Huffman code: the high bit goes out first.
    fn code(&mut self, code: u32, width: u32) {
        for i in (0..width).rev() {
            self.bits((code >> i) & 1, 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.out.push((self.held & 0xff) as u8);
        }
        self.out
    }
}

/// The fixed literal/length tree, RFC 1951 §3.2.6, as `(code, width)`.
fn fixed_code(symbol: u16) -> (u32, u32) {
    match symbol {
        0..=143 => (0x30 + u32::from(symbol), 8),
        144..=255 => (0x190 + u32::from(symbol) - 144, 9),
        256..=279 => (u32::from(symbol) - 256, 7),
        _ => (0xc0 + u32::from(symbol).saturating_sub(280), 8),
    }
}

fn hash_at(data: &[u8], pos: usize) -> Option<usize> {
    let a = u32::from(*data.get(pos)?);
    let b = u32::from(*data.get(pos.checked_add(1)?)?);
    let c = u32::from(*data.get(pos.checked_add(2)?)?);
    Some((((a << 10) ^ (b << 5) ^ c) as usize) & (HASH_SIZE - 1))
}

fn remember(data: &[u8], pos: usize, head: &mut [u32], prev: &mut [u32]) {
    let Some(h) = hash_at(data, pos) else { return };
    let Ok(here) = u32::try_from(pos) else { return };
    let earlier = head.get(h).copied().unwrap_or(NONE);
    if let Some(slot) = prev.get_mut(pos & (WINDOW - 1)) {
        *slot = earlier;
    }
    if let Some(slot) = head.get_mut(h) {
        *slot = here;
    }
}

fn common_prefix(data: &[u8], a: usize, b: usize, limit: usize) -> usize {
    let mut n = 0;
    while n < limit {
        let (Some(x), Some(y)) = (data.get(a.saturating_add(n)), data.get(b.saturating_add(n)))
        else {
            break;
        };
        if x != y {
            break;
        }
        n = n.saturating_add(1);
    }
    n
}

/// The longest match at `at`, or `(0, 0)` for none.
///
/// Greedy, and the chain is walked newest first for at most [`CHAIN`] steps, so
/// what it answers is a function of the bytes alone.
fn longest_match(data: &[u8], at: usize, head: &[u32], prev: &[u32]) -> (usize, usize) {
    let limit = MAX_MATCH.min(data.len().saturating_sub(at));
    if limit < MIN_MATCH {
        return (0, 0);
    }
    let Some(h) = hash_at(data, at) else { return (0, 0) };
    let floor = at.saturating_sub(WINDOW);
    let mut best = 0_usize;
    let mut distance = 0_usize;
    let mut candidate = head.get(h).copied().unwrap_or(NONE);
    let mut steps = CHAIN;
    while candidate != NONE && steps > 0 {
        steps = steps.saturating_sub(1);
        let pos = candidate as usize;
        if pos < floor || pos >= at {
            break;
        }
        let length = common_prefix(data, pos, at, limit);
        if length > best {
            best = length;
            distance = at.saturating_sub(pos);
            if length == limit {
                break;
            }
        }
        let next = prev.get(pos & (WINDOW - 1)).copied().unwrap_or(NONE);
        // A chain always walks backwards. Anything else is a slot the window
        // has wrapped over, and following it would not terminate.
        if next != NONE && next as usize >= pos {
            break;
        }
        candidate = next;
    }
    if best >= MIN_MATCH { (best, distance) } else { (0, 0) }
}

/// The index in `table` of the last entry whose base is at or below `value`.
fn code_for(table: &[(u16, u8)], value: usize) -> usize {
    let mut found = 0;
    for (index, (base, _)) in table.iter().enumerate() {
        if usize::from(*base) <= value {
            found = index;
        }
    }
    found
}

/// One zlib stream: the two-byte header, one fixed-Huffman block, the Adler-32.
fn deflate(raw: &[u8]) -> Vec<u8> {
    let mut bits = BitWriter::new(raw.len() / 2);
    // The last block, and block type 01.
    bits.bits(1, 1);
    bits.bits(1, 2);

    let mut head = vec![NONE; HASH_SIZE];
    let mut prev = vec![NONE; WINDOW];
    let mut at = 0_usize;
    while at < raw.len() {
        let (length, distance) = longest_match(raw, at, &head, &prev);
        if length >= MIN_MATCH {
            let index = code_for(&LENGTHS, length);
            let (base, extra) = LENGTHS.get(index).copied().unwrap_or((3, 0));
            let (code, width) = fixed_code(257_u16.saturating_add(index as u16));
            bits.code(code, width);
            if extra > 0 {
                bits.bits(length.saturating_sub(usize::from(base)) as u32, u32::from(extra));
            }
            let which = code_for(&DISTANCES, distance);
            let (first, dextra) = DISTANCES.get(which).copied().unwrap_or((1, 0));
            bits.code(which as u32, 5);
            if dextra > 0 {
                bits.bits(distance.saturating_sub(usize::from(first)) as u32, u32::from(dextra));
            }
            // Every position inside the match is remembered too, so a later
            // match can start anywhere within it.
            for step in 0..length {
                remember(raw, at.saturating_add(step), &mut head, &mut prev);
            }
            at = at.saturating_add(length);
        } else {
            let byte = raw.get(at).copied().unwrap_or(0);
            let (code, width) = fixed_code(u16::from(byte));
            bits.code(code, width);
            remember(raw, at, &mut head, &mut prev);
            at = at.saturating_add(1);
        }
    }
    let (code, width) = fixed_code(256);
    bits.code(code, width);

    // `0x78 0x01`: deflate with a 32 KiB window and no preset dictionary. The
    // two bytes read as a big-endian number are a multiple of 31, which is what
    // RFC 1950 asks of the header.
    let mut out = vec![0x78, 0x01];
    out.extend_from_slice(&bits.finish());
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// CRC-32, the reflected `0xEDB88320` polynomial PNG asks for, a bit at a time.
///
/// It takes the two halves a chunk's checksum covers — the type and the data —
/// because that is the only shape anything here needs, and joining them into
/// one buffer to hash would copy the whole image.
///
/// A bit at a time rather than a table because a 1 KB table in this archive
/// costs more than the microseconds it saves on an image nobody is waiting for.
fn crc32(parts: [&[u8]; 2]) -> u32 {
    let mut c = 0xffff_ffff_u32;
    for part in parts {
        for &byte in part {
            c ^= u32::from(byte);
            for _ in 0..8 {
                c = if c & 1 == 1 { (c >> 1) ^ 0xedb8_8320 } else { c >> 1 };
            }
        }
    }
    c ^ 0xffff_ffff
}

/// Adler-32, as zlib's trailer.
fn adler32(data: &[u8]) -> u32 {
    let mut a = 1_u32;
    let mut b = 0_u32;
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

// ---------------------------------------------------------------------------
// Reading a PNG back
// ---------------------------------------------------------------------------

/// The pixels of a PNG this painter wrote.
#[derive(Debug)]
struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Image {
    fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize)
            .checked_mul(self.width as usize)?
            .checked_add(x as usize)?
            .checked_mul(4)?;
        let slice = self.rgba.get(i..i.checked_add(4)?)?;
        Some([*slice.first()?, *slice.get(1)?, *slice.get(2)?, *slice.get(3)?])
    }
}

fn pixel_bytes(width: u32, height: u32) -> Result<usize, String> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| format!("the image {width}x{height} is too large to hold"))
}

/// Reads back what [`encode`] wrote: 8-bit RGBA, all five row filters, and
/// deflate's stored and fixed-Huffman blocks.
///
/// Dynamic Huffman is the one thing it refuses, and that is a decision rather
/// than an omission: a golden comes from [`render`], nothing here writes a
/// dynamic block, and reading one would mean carrying a code-length decoder for
/// a case that cannot arise. Stored blocks stay readable because this file used
/// to write them.
fn decode(png: &[u8]) -> Result<Image, String> {
    let bad = |what: &str| format!("the PNG {what}");
    if png.get(..8) != Some(&SIGNATURE) {
        return Err(bad("does not start with a PNG signature"));
    }
    let mut offset = 8_usize;
    let mut width = 0_u32;
    let mut height = 0_u32;
    let mut seen_header = false;
    let mut zlib: Vec<u8> = Vec::new();

    while offset < png.len() {
        let head = png.get(offset..offset.saturating_add(8)).ok_or_else(|| bad("ends mid-chunk"))?;
        let len = be32(head, 0).ok_or_else(|| bad("ends mid-chunk"))? as usize;
        let kind = head.get(4..8).ok_or_else(|| bad("ends mid-chunk"))?.to_vec();
        let start = offset.saturating_add(8);
        let end = start.checked_add(len).ok_or_else(|| bad("names a chunk longer than itself"))?;
        let data = png.get(start..end).ok_or_else(|| bad("names a chunk longer than itself"))?;
        match kind.as_slice() {
            b"IHDR" => {
                if data.len() != 13 {
                    return Err(bad("has an IHDR that is not thirteen bytes"));
                }
                width = be32(data, 0).ok_or_else(|| bad("has an unreadable IHDR"))?;
                height = be32(data, 4).ok_or_else(|| bad("has an unreadable IHDR"))?;
                let tail = data.get(8..13).unwrap_or(&[]);
                if tail != [8, 6, 0, 0, 0] {
                    return Err(bad("is not the 8-bit RGBA form this painter writes"));
                }
                seen_header = true;
            }
            b"IDAT" => zlib.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        offset = end.saturating_add(4);
    }
    if !seen_header {
        return Err(bad("has no IHDR"));
    }
    let raw = inflate(&zlib)?;

    let stride = (width as usize).checked_mul(BPP).ok_or_else(|| bad("is too wide to hold"))?;
    let expected = stride
        .checked_add(1)
        .and_then(|n| n.checked_mul(height as usize))
        .ok_or_else(|| bad("is too large to hold"))?;
    if raw.len() != expected {
        return Err(bad("holds fewer rows than its header says"));
    }
    let mut rgba = Vec::with_capacity(pixel_bytes(width, height)?);
    let mut previous = vec![0_u8; stride];
    let mut row = vec![0_u8; stride];
    for index in 0..height as usize {
        let start = index.saturating_mul(stride.saturating_add(1));
        let kind = raw.get(start).copied().ok_or_else(|| bad("ends mid-row"))?;
        let from = start.saturating_add(1);
        let line = raw.get(from..from.saturating_add(stride)).ok_or_else(|| bad("ends mid-row"))?;
        row.copy_from_slice(line);
        unfilter(kind, &mut row, &previous)?;
        rgba.extend_from_slice(&row);
        previous.copy_from_slice(&row);
    }
    Ok(Image { width, height, rgba })
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    let slice = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([*slice.first()?, *slice.get(1)?, *slice.get(2)?, *slice.get(3)?]))
}

/// The other end of [`BitWriter`], reading the two orders it writes.
struct BitReader<'a> {
    data: &'a [u8],
    at: usize,
    bit: u32,
}

impl BitReader<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.at)?;
        let value = (u32::from(byte) >> self.bit) & 1;
        self.bit = self.bit.saturating_add(1);
        if self.bit == 8 {
            self.bit = 0;
            self.at = self.at.saturating_add(1);
        }
        Some(value)
    }

    fn bits(&mut self, width: u32) -> Option<u32> {
        let mut value = 0;
        for i in 0..width {
            value |= self.bit()? << i;
        }
        Some(value)
    }

    fn code(&mut self, width: u32) -> Option<u32> {
        let mut value = 0;
        for _ in 0..width {
            value = (value << 1) | self.bit()?;
        }
        Some(value)
    }

    /// To the next byte boundary, which is where a stored block's length sits.
    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.at = self.at.saturating_add(1);
        }
    }
}

/// One symbol of the fixed literal/length tree.
///
/// RFC 1951 §3.2.6's table, read the other way: seven bits up to `0010111` is
/// an end-or-length symbol, and every longer code starts above the range the
/// shorter one claimed, so the width tells itself apart with no table.
fn fixed_symbol(reader: &mut BitReader) -> Option<u16> {
    let seven = reader.code(7)?;
    if seven <= 0x17 {
        return u16::try_from(256 + seven).ok();
    }
    let eight = (seven << 1) | reader.bit()?;
    if (0x30..=0xbf).contains(&eight) {
        return u16::try_from(eight - 0x30).ok();
    }
    if (0xc0..=0xc7).contains(&eight) {
        return u16::try_from(280 + eight - 0xc0).ok();
    }
    let nine = (eight << 1) | reader.bit()?;
    if (0x190..=0x1ff).contains(&nine) {
        return u16::try_from(144 + nine - 0x190).ok();
    }
    None
}

/// A zlib stream of stored and fixed-Huffman blocks, unwrapped.
fn inflate(zlib: &[u8]) -> Result<Vec<u8>, String> {
    let bad = |what: &str| format!("the PNG's compressed data {what}");
    if zlib.len() < 2 {
        return Err(bad("is shorter than a zlib header"));
    }
    let mut reader = BitReader { data: zlib, at: 2, bit: 0 };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = reader.bits(1).ok_or_else(|| bad("ends mid-block"))?;
        match reader.bits(2).ok_or_else(|| bad("ends mid-block"))? {
            0 => {
                reader.align();
                let len = reader.bits(16).ok_or_else(|| bad("ends mid-block"))? as usize;
                reader.bits(16).ok_or_else(|| bad("ends mid-block"))?;
                for _ in 0..len {
                    let byte = reader.bits(8).ok_or_else(|| bad("ends mid-block"))?;
                    out.push(byte as u8);
                }
            }
            1 => inflate_fixed(&mut reader, &mut out)?,
            _ => return Err(bad("is not the deflate this painter writes")),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

fn inflate_fixed(reader: &mut BitReader, out: &mut Vec<u8>) -> Result<(), String> {
    let bad = |what: &str| format!("the PNG's compressed data {what}");
    loop {
        let symbol = fixed_symbol(reader).ok_or_else(|| bad("holds a code no tree has"))?;
        match symbol {
            0..=255 => out.push(symbol as u8),
            256 => return Ok(()),
            257..=285 => {
                let index = usize::from(symbol).saturating_sub(257);
                let (base, extra) =
                    LENGTHS.get(index).copied().ok_or_else(|| bad("names no length"))?;
                let more = reader.bits(u32::from(extra)).ok_or_else(|| bad("ends mid-match"))?;
                let length = usize::from(base).saturating_add(more as usize);

                let which = reader.code(5).ok_or_else(|| bad("ends mid-match"))? as usize;
                let (first, dextra) =
                    DISTANCES.get(which).copied().ok_or_else(|| bad("names no distance"))?;
                let more = reader.bits(u32::from(dextra)).ok_or_else(|| bad("ends mid-match"))?;
                let distance = usize::from(first).saturating_add(more as usize);

                if distance == 0 || distance > out.len() {
                    return Err(bad("copies from before the start of the image"));
                }
                let from = out.len().saturating_sub(distance);
                for step in 0..length {
                    let byte = out
                        .get(from.saturating_add(step))
                        .copied()
                        .ok_or_else(|| bad("copies past what it has written"))?;
                    out.push(byte);
                }
            }
            _ => return Err(bad("names a symbol the fixed tree does not have")),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene with a viewport and nothing in it.
    const EMPTY: &str = "buri-scene 1\nviewport 4 3\n";

    fn render_ok(scene: &str, sheet: &str, state: &str) -> Image {
        let png = render(&Request { scene, stylesheet: sheet, state }).unwrap();
        decode(&png).unwrap()
    }

    fn at(image: &Image, x: u32, y: u32) -> [u8; 4] {
        image.pixel(x, y).unwrap()
    }

    #[test]
    fn an_empty_scene_is_a_white_canvas_of_the_viewport() {
        let image = render_ok(EMPTY, "", "rest");
        assert_eq!((image.width, image.height), (4, 3));
        for y in 0..3 {
            for x in 0..4 {
                assert_eq!(at(&image, x, y), [255, 255, 255, 255]);
            }
        }
    }

    #[test]
    fn a_scene_without_the_banner_is_refused() {
        let error = render(&Request { scene: "viewport 4 3\n", stylesheet: "", state: "rest" });
        assert_eq!(error, Err("the scene does not start with `buri-scene 1`".to_string()));
    }

    #[test]
    fn a_scene_without_a_viewport_is_refused() {
        let scene = "buri-scene 1\ne 0 \n";
        let error = render(&Request { scene, stylesheet: "", state: "rest" }).unwrap_err();
        assert!(error.contains("is not a viewport"), "{error}");
    }

    #[test]
    fn a_viewport_of_zero_is_refused() {
        let scene = "buri-scene 1\nviewport 0 3\n";
        let error = render(&Request { scene, stylesheet: "", state: "rest" }).unwrap_err();
        assert!(error.contains("0x3"), "{error}");
    }

    #[test]
    fn a_depth_that_jumps_is_refused() {
        let scene = "buri-scene 1\nviewport 4 3\ne 0 \ne 2 \n";
        let error = render(&Request { scene, stylesheet: "", state: "rest" }).unwrap_err();
        assert!(error.contains("jumps from depth 1 to 2"), "{error}");
    }

    #[test]
    fn a_line_that_is_neither_e_nor_t_is_refused() {
        let scene = "buri-scene 1\nviewport 4 3\nx 0 \n";
        let error = render(&Request { scene, stylesheet: "", state: "rest" }).unwrap_err();
        assert!(error.contains("neither an `e` nor a `t`"), "{error}");
    }

    #[test]
    fn a_declaration_without_a_colon_is_refused() {
        let scene = "buri-scene 1\nviewport 4 3\ne 0 padding\n";
        let error = render(&Request { scene, stylesheet: "", state: "rest" }).unwrap_err();
        assert!(error.contains("has no `:`"), "{error}");
    }

    #[test]
    fn a_child_of_a_text_run_is_refused() {
        let scene = "buri-scene 1\nviewport 4 3\nt 0 Ada\ne 1 \n";
        let error = render(&Request { scene, stylesheet: "", state: "rest" }).unwrap_err();
        assert!(error.contains("a text run has none"), "{error}");
    }

    #[test]
    fn a_state_the_painter_does_not_know_is_refused() {
        let error = render(&Request { scene: EMPTY, stylesheet: "", state: "wiggly" });
        assert_eq!(error, Err("`wiggly` is not a snapshot state".to_string()));
    }

    #[test]
    fn a_box_paints_its_background_over_the_area_it_was_given() {
        let scene = "buri-scene 1\nviewport 6 4\ne 0 width:4px;height:2px;\
                     background-color:rgb(0,0,255)\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!(at(&image, 0, 0), [0, 0, 255, 255]);
        assert_eq!(at(&image, 3, 1), [0, 0, 255, 255]);
        assert_eq!(at(&image, 4, 0), [255, 255, 255, 255]);
        assert_eq!(at(&image, 0, 2), [255, 255, 255, 255]);
    }

    /// A radius rounds a corner, so the corner pixel is not the fill.
    #[test]
    fn a_radius_rounds_the_corner_off() {
        let scene = "buri-scene 1\nviewport 12 12\ne 0 width:12px;height:12px;\
                     border-radius:6px;background-color:rgb(0,0,0)\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!(at(&image, 6, 6), [0, 0, 0, 255]);
        assert_eq!(at(&image, 0, 0), [255, 255, 255, 255]);
        assert_eq!(at(&image, 11, 11), [255, 255, 255, 255]);
    }

    /// Padding indents the child; a column gap separates two of them.
    #[test]
    fn padding_and_a_row_gap_put_two_children_where_they_belong() {
        let scene = "buri-scene 1\nviewport 20 20\n\
                     e 0 padding:2px;gap:3px;width:20px;height:20px\n\
                     e 1 width:4px;height:4px;background-color:rgb(255,0,0)\n\
                     e 1 width:4px;height:4px;background-color:rgb(0,255,0)\n";
        let image = render_ok(scene, "", "rest");
        // The first child starts at the padding.
        assert_eq!(at(&image, 2, 2), [255, 0, 0, 255]);
        assert_eq!(at(&image, 5, 5), [255, 0, 0, 255]);
        // Then four of it, then three of gap, then the second.
        assert_eq!(at(&image, 2, 8), [255, 255, 255, 255]);
        assert_eq!(at(&image, 2, 9), [0, 255, 0, 255]);
    }

    #[test]
    fn a_column_gap_separates_a_row_the_same_way() {
        let scene = "buri-scene 1\nviewport 20 20\n\
                     e 0 flex-direction:row;gap:3px;width:20px;height:20px\n\
                     e 1 width:4px;height:4px;background-color:rgb(255,0,0)\n\
                     e 1 width:4px;height:4px;background-color:rgb(0,255,0)\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!(at(&image, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&image, 3, 0), [255, 0, 0, 255]);
        assert_eq!(at(&image, 4, 0), [255, 255, 255, 255]);
        assert_eq!(at(&image, 7, 0), [0, 255, 0, 255]);
    }

    /// The glyphs land inside the box the shaper measured, and nowhere else.
    #[test]
    fn a_text_run_paints_where_the_shaping_says() {
        let scene = "buri-scene 1\nviewport 120 40\n\
                     e 0 font-size:24px;color:rgb(0,0,0)\n\
                     t 1 Ada\n";
        let image = render_ok(scene, "", "rest");
        let ink = |x0: u32, x1: u32| {
            (x0..x1).any(|x| (0..40).any(|y| at(&image, x, y) != [255, 255, 255, 255]))
        };
        assert!(ink(0, 45), "the run should have drawn near the left edge");
        assert!(!ink(60, 120), "the run should not reach the right half");
    }

    /// The same scene with one declaration-less box wrapped around everything,
    /// which is exactly what `ui.stack([], [tree])` writes.
    fn wrapped(scene: &str) -> String {
        let mut out = String::new();
        for (index, line) in scene.lines().enumerate() {
            out.push_str(line);
            out.push('\n');
            if index == 1 {
                out.push_str("e 0 \n");
                continue;
            }
            if index < 2 {
                continue;
            }
            out.truncate(out.len() - line.len() - 1);
            let (kind, rest) = line.split_at(2);
            let (digits, body) = rest.split_once(' ').unwrap_or((rest, ""));
            let depth: usize = digits.parse::<usize>().unwrap() + 1;
            out.push_str(&format!("{kind}{depth} {body}\n"));
        }
        out
    }

    /// The claim the whole feature rests on: a `stack` that says nothing is not
    /// a thing a snapshot can see.
    #[test]
    fn an_unstyled_box_around_the_tree_changes_nothing() {
        let scenes = [
            "buri-scene 1\nviewport 200 120\n\
             e 0 font-size:18px;color:rgb(20,20,20)\n\
             t 1 Ada Lovelace\n",
            "buri-scene 1\nviewport 200 120\n\
             e 0 padding:12px;gap:6px;background-color:rgb(240,240,245)\n\
             e 1 width:60px;height:24px;background-color:rgb(220,40,40);border-radius:4px\n\
             t 1 under the box\n",
        ];
        for scene in scenes {
            let plain = render(&Request { scene, stylesheet: "", state: "rest" }).unwrap();
            let deeper = wrapped(scene);
            let nested =
                render(&Request { scene: &deeper, stylesheet: "", state: "rest" }).unwrap();
            assert_eq!(plain, nested, "wrapping this scene changed it:\n{deeper}");
        }
    }

    /// The first column holding ink, or `None` for a blank picture.
    fn first_inked_column(image: &Image) -> Option<u32> {
        (0..image.width)
            .find(|&x| (0..image.height).any(|y| at(image, x, y) != [255, 255, 255, 255]))
    }

    fn inked_pixels(image: &Image) -> usize {
        (0..image.height)
            .flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .filter(|&(x, y)| at(image, x, y) != [255, 255, 255, 255])
            .count()
    }

    #[test]
    fn centring_a_run_moves_it_off_the_left_edge() {
        let one = "buri-scene 1\nviewport 200 40\ne 0 font-size:20px;width:200px\nt 1 Ada\n";
        let two = "buri-scene 1\nviewport 200 40\n\
                   e 0 font-size:20px;width:200px;text-align:center\nt 1 Ada\n";
        let left = first_inked_column(&render_ok(one, "", "rest")).unwrap();
        let centre = first_inked_column(&render_ok(two, "", "rest")).unwrap();
        assert!(left < 4, "a start-aligned run begins at the box's edge, not {left}");
        assert!(centre > 60, "a centred run begins in the middle, not at {centre}");
    }

    /// Three faces are bundled, and the weight picks between two of them.
    #[test]
    fn the_bold_face_is_not_the_regular_one() {
        let one = "buri-scene 1\nviewport 200 40\ne 0 font-size:24px\nt 1 Handgloves\n";
        let two = "buri-scene 1\nviewport 200 40\ne 0 font-size:24px;font-weight:700\n\
                   t 1 Handgloves\n";
        let regular = inked_pixels(&render_ok(one, "", "rest"));
        let bold = inked_pixels(&render_ok(two, "", "rest"));
        assert!(regular > 0 && bold > regular, "regular {regular}, bold {bold}");
    }

    /// A blur radius spreads the shadow past the shape it was cast from, and
    /// fades as it goes. With no blur the same shadow stops at its edge.
    #[test]
    fn a_blur_radius_spreads_the_shadow_past_its_edge() {
        let sharp = "buri-scene 1\nviewport 40 40\ne 0 padding:16px\n\
                     e 1 width:8px;height:8px;background-color:rgb(255,255,255);\
                     box-shadow:0px 0px 0px 0px rgb(0,0,0)\n";
        let soft = "buri-scene 1\nviewport 40 40\ne 0 padding:16px\n\
                    e 1 width:8px;height:8px;background-color:rgb(255,255,255);\
                    box-shadow:0px 0px 8px 0px rgb(0,0,0)\n";
        let sharp = render_ok(sharp, "", "rest");
        let soft = render_ok(soft, "", "rest");
        // Four pixels out from the box's left edge: outside the cast shape, so
        // only a blur puts ink there.
        assert_eq!(at(&sharp, 12, 20), [255, 255, 255, 255]);
        let spread = at(&soft, 12, 20);
        assert!(spread[0] < 255, "a blurred shadow reaches four pixels out, not {spread:?}");
        // And it fades: further out is lighter than nearer in.
        let near = at(&soft, 14, 20)[0];
        let far = at(&soft, 10, 20)[0];
        assert!(near < far, "a blur fades outward: {near} at 14 against {far} at 10");
    }

    /// The blur is the same bytes twice, which is the whole reason it is
    /// integer arithmetic: a golden is compared byte for byte.
    #[test]
    fn a_blurred_shadow_paints_the_same_bytes_twice() {
        let scene = "buri-scene 1\nviewport 40 40\ne 0 padding:16px\n\
                     e 1 width:8px;height:8px;background-color:rgb(255,255,255);\
                     box-shadow:2px 2px 6px 1px rgb(0,0,255)\n";
        let one = render(&Request { scene, stylesheet: "", state: "rest" }).unwrap();
        let two = render(&Request { scene, stylesheet: "", state: "rest" }).unwrap();
        assert_eq!(one, two);
    }

    /// A blur of zero is the shape itself, so it paints what the unblurred
    /// path paints — the goldens recorded before the blur existed do not move.
    #[test]
    fn a_blur_of_zero_paints_the_shape_it_was_cast_from() {
        let scene = "buri-scene 1\nviewport 20 20\ne 0 padding:4px\n\
                     e 1 width:8px;height:8px;background-color:rgb(255,255,255);\
                     box-shadow:0px 0px 0px 0px rgb(0,0,0)\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!(at(&image, 3, 10), [255, 255, 255, 255]);
        assert_eq!(at(&image, 5, 10), [255, 255, 255, 255]);
    }

    /// `overflow: hidden` on a rounded box clips to the rounded shape: the
    /// corner pixel a child would have squared off stays the canvas.
    #[test]
    fn overflow_hidden_clips_to_the_rounded_corner() {
        let square = "buri-scene 1\nviewport 16 16\n\
                      e 0 width:16px;height:16px;overflow:hidden\n\
                      e 1 width:16px;height:16px;background-color:rgb(255,0,0)\n";
        let round = "buri-scene 1\nviewport 16 16\n\
                     e 0 width:16px;height:16px;border-radius:8px;overflow:hidden\n\
                     e 1 width:16px;height:16px;background-color:rgb(255,0,0)\n";
        assert_eq!(at(&render_ok(square, "", "rest"), 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&render_ok(round, "", "rest"), 0, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn overflow_hidden_clips_a_child_to_its_box() {
        let scene = "buri-scene 1\nviewport 10 10\n\
                     e 0 width:10px;height:4px;overflow:hidden\n\
                     e 1 width:10px;height:10px;background-color:rgb(255,0,0)\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!(at(&image, 0, 3), [255, 0, 0, 255]);
        assert_eq!(at(&image, 0, 4), [255, 255, 255, 255]);
    }

    #[test]
    fn a_run_of_two_sizes_is_two_heights() {
        let small = "buri-scene 1\nviewport 80 60\ne 0 font-size:8px\nt 1 Ada\n";
        let large = "buri-scene 1\nviewport 80 60\ne 0 font-size:32px\nt 1 Ada\n";
        let rows = |scene: &str| {
            let image = render_ok(scene, "", "rest");
            (0..60).filter(|&y| (0..80).any(|x| at(&image, x, y) != [255, 255, 255, 255])).count()
        };
        assert!(rows(small) < rows(large));
    }

    #[test]
    fn a_class_comes_out_of_the_stylesheet() {
        let scene = "buri-scene 1\nviewport 6 4\ne 0 class:box\n";
        let sheet = ".box{width:4px;height:2px;background-color:rgb(0,0,255)}\n";
        let image = render_ok(scene, sheet, "rest");
        assert_eq!(at(&image, 0, 0), [0, 0, 255, 255]);
        assert_eq!(at(&image, 4, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn an_inline_declaration_beats_a_class() {
        let scene =
            "buri-scene 1\nviewport 6 4\ne 0 class:box;background-color:rgb(255,0,0)\n";
        let sheet = ".box{width:4px;height:2px;background-color:rgb(0,0,255)}\n";
        let image = render_ok(scene, sheet, "rest");
        assert_eq!(at(&image, 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn a_hover_rule_applies_only_in_the_hover_state() {
        let scene = "buri-scene 1\nviewport 6 4\ne 0 class:box hov\n";
        let sheet = ".box{width:4px;height:2px;background-color:rgb(0,0,255)}\n\
                     .hov:hover{background-color:rgb(255,0,0)}\n";
        assert_eq!(at(&render_ok(scene, sheet, "rest"), 0, 0), [0, 0, 255, 255]);
        assert_eq!(at(&render_ok(scene, sheet, "hover"), 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn a_media_query_the_viewport_does_not_reach_does_not_apply() {
        let scene = "buri-scene 1\nviewport 100 4\ne 0 class:box md\n";
        let sheet = ".box{width:4px;height:2px;background-color:rgb(0,0,255)}\n\
                     @media (min-width:48rem){\n.md{background-color:rgb(255,0,0)}\n}\n";
        // 100 px is under 48 rem, so the query loses.
        assert_eq!(at(&render_ok(scene, sheet, "rest"), 0, 0), [0, 0, 255, 255]);
        let wide = "buri-scene 1\nviewport 800 4\ne 0 class:box md\n";
        assert_eq!(at(&render_ok(wide, sheet, "rest"), 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn a_descendant_rule_is_skipped() {
        let scene = "buri-scene 1\nviewport 6 4\ne 0 class:lay\n";
        let sheet = ".lay{width:4px;height:2px;background-color:rgb(0,0,255)}\n\
                     .lay>*{background-color:rgb(255,0,0)}\n";
        assert_eq!(at(&render_ok(scene, sheet, "rest"), 0, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn the_same_scene_renders_to_the_same_bytes_twice() {
        let scene = "buri-scene 1\nviewport 60 30\n\
                     e 0 padding:4px;background-color:rgba(20,30,40,0.5);border-radius:3px\n\
                     e 1 font-size:14px;font-weight:700\n\
                     t 2 Ada Lovelace\n";
        let request = Request { scene, stylesheet: "", state: "rest" };
        assert_eq!(render(&request).unwrap(), render(&request).unwrap());
    }

    #[test]
    fn the_png_header_is_the_one_a_reader_expects() {
        let png = encode(1, 1, &[1, 2, 3, 4]);
        assert_eq!(png.get(..8), Some(SIGNATURE.as_slice()));
        // Length 13, then `IHDR`, then 1x1, 8-bit, colour type 6.
        assert_eq!(png.get(8..16), Some([0, 0, 0, 13, b'I', b'H', b'D', b'R'].as_slice()));
        assert_eq!(png.get(16..29), Some([0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0].as_slice()));
        let end = [b'I', b'E', b'N', b'D', 174, 66, 96, 130];
        assert_eq!(png.get(png.len() - 8..), Some(end.as_slice()));
    }

    /// The two checksums, against values computed by hand rather than by the
    /// same code that writes them.
    #[test]
    fn the_checksums_are_the_ones_the_formats_specify() {
        // The CRC-32 of "IEND" with no data is the constant every PNG ends on.
        assert_eq!(crc32([b"IEND", &[]]), 0xae42_6082);
        // The `check` value every CRC-32 catalogue quotes for this polynomial.
        assert_eq!(crc32([b"12345", b"6789"]), 0xcbf4_3926);
        // Adler-32 of "Wikipedia" is the value RFC 1950 quotes.
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(adler32(&[]), 1);
        // And one worked out by hand: over "abc", `a` runs 1, 98, 196, 295 and
        // `b` is 98 + 196 + 295 = 589, so the answer is 589 << 16 | 295.
        assert_eq!(adler32(b"abc"), 0x024d_0127);
    }

    #[test]
    fn each_row_filter_puts_its_row_back() {
        let previous: Vec<u8> = (0..24_u8).map(|i| i.wrapping_mul(17)).collect();
        let line: Vec<u8> = (0..24_u8).map(|i| i.wrapping_mul(31).wrapping_add(7)).collect();
        for kind in 0..5_u8 {
            let mut filtered = vec![0_u8; line.len()];
            apply_filter(kind, &line, &previous, &mut filtered);
            unfilter(kind, &mut filtered, &previous).unwrap();
            assert_eq!(filtered, line, "filter {kind} did not come back");
        }
    }

    #[test]
    fn a_row_filter_that_does_not_exist_is_refused() {
        let mut row = vec![0_u8; 4];
        assert!(unfilter(5, &mut row, &[0; 4]).is_err());
    }

    /// A row identical to the one above costs nothing under both `Up` and
    /// `Paeth`, so this is also the tie-break: the lower number wins.
    #[test]
    fn a_repeated_row_is_filtered_against_the_one_above_it() {
        let row: Vec<u8> = (0..40_u8).map(|i| i.wrapping_mul(23)).collect();
        let mut rgba = Vec::new();
        for _ in 0..4 {
            rgba.extend_from_slice(&row);
        }
        let raw = filter_rows(10, 4, &rgba);
        let stride = 41;
        assert_eq!(raw.get(stride), Some(&2));
        assert_eq!(raw.get(stride * 2), Some(&2));
        assert!(raw[stride + 1..stride * 2].iter().all(|&b| b == 0));
    }

    #[test]
    fn the_encoder_gives_back_the_pixels_it_was_given() {
        let flat = vec![255_u8; 32 * 24 * 4];
        let gradient: Vec<u8> = (0..32_u32 * 24)
            .flat_map(|i| {
                let x = (i % 32) as u8;
                let y = (i / 32) as u8;
                // The alpha channel is its own ramp, so a filter that only
                // suited the colour channels would show up here.
                [x.wrapping_mul(8), y.wrapping_mul(10), 128, x.wrapping_add(y).wrapping_mul(3)]
            })
            .collect();
        let noisy: Vec<u8> =
            (0..32_u64 * 24 * 4).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        for pixels in [flat, gradient, noisy] {
            let back = decode(&encode(32, 24, &pixels)).unwrap();
            assert_eq!((back.width, back.height), (32, 24));
            assert_eq!(back.rgba, pixels);
        }
    }

    #[test]
    fn two_encodes_of_the_same_pixels_are_the_same_bytes() {
        let pixels: Vec<u8> = (0..64_u32 * 64 * 4).map(|i| ((i / 7) % 251) as u8).collect();
        assert_eq!(encode(64, 64, &pixels), encode(64, 64, &pixels));
    }

    #[test]
    fn the_compressed_stream_is_one_final_fixed_huffman_block() {
        let raw = b"the same words the same words the same words".to_vec();
        let stream = deflate(&raw);
        // RFC 1950's header check: the first two bytes, big-endian, divide by 31.
        assert_eq!(stream[0], 0x78);
        assert_eq!(u16::from_be_bytes([stream[0], stream[1]]) % 31, 0);
        // BFINAL then BTYPE, low bit first: 1, then 01, is `0b011`.
        assert_eq!(stream[2] & 0b111, 0b011);
        assert_eq!(&stream[stream.len() - 4..], adler32(&raw).to_be_bytes());
        assert_eq!(inflate(&stream).unwrap(), raw);
    }

    /// Repetition has to actually be spent: a run of one byte is a match, not
    /// four thousand literals.
    #[test]
    fn a_repetitive_stream_comes_out_far_smaller_than_it_went_in() {
        let raw = vec![7_u8; 4096];
        let stream = deflate(&raw);
        assert!(stream.len() < 64, "4096 identical bytes became {} bytes", stream.len());
        assert_eq!(inflate(&stream).unwrap(), raw);
    }

    /// The stored blocks this file used to write are still readable, so a
    /// golden recorded before the encoder changed still opens.
    #[test]
    fn a_stored_block_still_inflates() {
        let body = b"stored, uncompressed, and final";
        let len = u16::try_from(body.len()).unwrap();
        let mut stream = vec![0x78, 0x01, 1];
        stream.extend_from_slice(&len.to_le_bytes());
        stream.extend_from_slice(&(!len).to_le_bytes());
        stream.extend_from_slice(body);
        stream.extend_from_slice(&adler32(body).to_be_bytes());
        assert_eq!(inflate(&stream).unwrap(), body);
    }

    #[test]
    fn the_png_round_trips_through_the_reader() {
        let rgba: Vec<u8> = (0..2 * 3 * 4).map(|i| (i * 7 % 251) as u8).collect();
        let image = decode(&encode(2, 3, &rgba)).unwrap();
        assert_eq!((image.width, image.height), (2, 3));
        assert_eq!(image.rgba, rgba);
    }

    #[test]
    fn a_png_larger_than_one_deflate_block_still_round_trips() {
        // 200 x 100 x 4 plus the filter bytes is over 65 535, so the encoder
        // writes more than one stored block.
        let rgba: Vec<u8> = (0..200 * 100 * 4).map(|i| (i % 256) as u8).collect();
        let image = decode(&encode(200, 100, &rgba)).unwrap();
        assert_eq!(image.rgba, rgba);
    }

    #[test]
    fn something_that_is_not_a_png_is_refused() {
        let error = decode(b"not a png at all").unwrap_err();
        assert!(error.contains("signature"), "{error}");
    }

    #[test]
    fn diff_answers_nothing_when_the_bytes_are_equal() {
        let png = encode(1, 1, &[1, 2, 3, 4]);
        assert_eq!(diff(&png, &png).unwrap(), None);
    }

    #[test]
    fn diff_paints_magenta_where_two_images_disagree() {
        let golden = encode(2, 1, &[255, 255, 255, 255, 10, 20, 30, 255]);
        let actual = encode(2, 1, &[255, 255, 255, 255, 40, 50, 60, 255]);
        let out = diff(&golden, &actual).unwrap().unwrap();
        let image = decode(&out).unwrap();
        assert_eq!(image.pixel(1, 0), Some([255, 0, 255, 255]));
        // The matching pixel is grey and lighter than the golden's white.
        let [r, g, b, a] = image.pixel(0, 0).unwrap();
        assert_eq!((r, g, b, a), (255, 255, 255, 255));
    }

    #[test]
    fn diff_paints_magenta_where_only_one_image_has_a_pixel() {
        let golden = encode(1, 1, &[0, 0, 0, 255]);
        let actual = encode(2, 1, &[0, 0, 0, 255, 0, 0, 0, 255]);
        let image = decode(&diff(&golden, &actual).unwrap().unwrap()).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixel(1, 0), Some([255, 0, 255, 255]));
    }

    #[test]
    fn a_matching_pixel_is_greyed_and_lightened() {
        assert_eq!(faded([255, 0, 0, 255]), [154, 154, 154, 255]);
        assert_eq!(faded([0, 0, 0, 255]), [128, 128, 128, 255]);
    }

    #[test]
    fn the_rounding_policy_sends_a_half_upward() {
        assert_eq!(px(0.5), 1);
        assert_eq!(px(-0.5), 0);
        assert_eq!(px(1.4999), 1);
        assert_eq!(px(f32::NAN), 0);
    }

    #[test]
    fn a_rem_is_sixteen_pixels() {
        assert_eq!(length("1rem", 16.0), Some(Len::Px(16.0)));
        assert_eq!(length("8px", 16.0), Some(Len::Px(8.0)));
        assert_eq!(length("50%", 16.0), Some(Len::Percent(50.0)));
        assert_eq!(length("auto", 16.0), Some(Len::Auto));
    }

    #[test]
    fn an_unresolved_token_is_transparent_behind_and_inherited_in_front() {
        let scene = "buri-scene 1\nviewport 4 2\n\
                     e 0 color:rgb(1,2,3)\n\
                     e 1 background-color:var(--x);color:var(--y);width:4px;height:2px\n";
        let scene = Scene::parse(scene).unwrap();
        let styles = resolve(&scene, &[], State::Rest);
        assert_eq!(styles.get(1).map(|s| s.background), Some(Rgba::CLEAR));
        assert_eq!(styles.get(1).map(|s| s.colour.r), Some(1));
    }

    #[test]
    fn the_escapes_are_the_three_the_format_has() {
        assert_eq!(unescape(r"a\nb\\c\rd"), "a\nb\\c\rd");
        assert_eq!(unescape(r"a\qb"), r"a\qb");
    }

    #[test]
    fn a_selector_with_a_suffix_or_an_unknown_pseudo_is_not_a_rule() {
        assert_eq!(parse_selector(".p-8"), Some(("p-8".to_string(), None)));
        assert_eq!(parse_selector(".p-8:hover"), Some(("p-8".to_string(), Some(State::Hover))));
        let escaped = Some(("hover:bg".to_string(), Some(State::Hover)));
        assert_eq!(parse_selector(r".hover\:bg:hover"), escaped);
        assert_eq!(parse_selector(".lay > *"), None);
        assert_eq!(parse_selector(".p-8:first-child"), None);
        assert_eq!(parse_selector("p-8"), None);
    }
}
