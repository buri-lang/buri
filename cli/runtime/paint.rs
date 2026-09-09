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
//! the outward margin a `Bleed` writes, sizing, background, border, radius,
//! shadow, opacity, the text properties, and the one transform the vocabulary
//! has — a translate. They are the same CSS `semantics::styles::declaration`
//! writes into the stylesheet and `$tree_declare` writes inline, so a style
//! that folded and one that did not paint alike. Anything else parses and is
//! ignored, which is what lets the vocabulary grow without breaking a scene.
//!
//! A margin is negative wherever the vocabulary wrote one, so boxes overlap:
//! the child reaches back over its parent's padding, or a sibling over the one
//! before it. [`Painter::draw`] walks children in document order, so the later
//! one is painted over the earlier one — which is what the page does with the
//! same markup.
//!
//! A translate is the other way a box leaves where it was put, and the two are
//! not the same: a margin is read by the layout and moves everything after it,
//! while a translate is applied to the box after the layout has finished and
//! moves nothing else at all.
//!
//! A border is four widths and four styles and a radius is four corners, so a
//! rule may name one edge or one corner as well as the whole box. Four equal
//! edges are one stroke around the box; anything else is the ring between the
//! border box and the padding box, which [`stroke_edges`] draws and which is
//! exact here because a border's colour is whole-box.
//!
//! An `e` line may also carry `field:<kind>`, which says what an input accepts:
//! the `type` its markup carries, or `multiline` for the `textarea` that has
//! none. Three of the seven paint differently, and they are the three a browser
//! draws differently under this stylesheet's reset. A **password's value is
//! never painted** — one `•` per character, the way `<input type="password">`
//! is drawn, so a golden holds the width of the secret and none of it. And only
//! `multiline` wraps, because an `<input>` is one line whatever is typed into
//! it.
//!
//! The third is `range`, which carries `range:<min> <max> <value>` beside its
//! kind and no run of text under it: a browser draws a slider and never the
//! number behind one. It paints a bar a quarter of the box's height across the
//! middle, and a round thumb one line across whose centre runs between half a
//! thumb inside either end — the same two shapes, at the same sizes, that the
//! sheet's reset paints with a gradient and a `::-webkit-slider-thumb`. Both
//! take the element's own colour. The value is sanitized the way HTML says a
//! `value` attribute is: clamped into the bounds, and the middle when it is not
//! a number at all.
//!
//! An `e` line may also carry `icon:<artwork>`, which makes the box a drawing
//! written into the scene itself. Its `currentColor` is the colour the element
//! paints in, so an icon follows the `Foreground` around it and turns over
//! between themes — which an `image` cannot, its source being a document of its
//! own, where `currentColor` is black whatever the page says.
//!
//! An `e` line may also carry `mark:<shape>`, which makes the box a mark a
//! widget draws for itself rather than a container: `thumb`, the disc a switch
//! moves from one end of its track to the other, and `tick`, the stroke a
//! checkbox holds when it is on. Neither is a box, which is why neither is a
//! `ui/style` property — there is no radius that makes a tick and no background
//! that draws one. Both take the element's own **foreground**, so the colour
//! that paints the mark is the colour that paints the text beside it, and both
//! are drawn inside whatever box the layout gave the line. The sheet's reset
//! draws the same two on `input[type=checkbox]::before`.
//!
//! An `e` line may also carry `image:<source>`, which makes the box a picture
//! rather than a container. **The painter loads nothing** — no network, no
//! disk — so the only source it can read is a `data:` URI, and [`image`] is
//! what reads one: any PNG, and enough SVG to draw an icon. Every other source
//! — an `http` URL, a path, an interlaced PNG, a media type that module does
//! not decode — paints a **placeholder**: a framed grey box at the size the
//! scene declared for it, or filling the box around it when the scene declared
//! none. A picture that is not there is better shown as a box than as nothing,
//! which is what an image with no source used to be.
//!
//! The sheet is read as class rules, plus **one shape of descendant rule**:
//! `.<class>>*`, which is what `Layout(.Layers)` is written as. Every child of
//! an element carrying the class takes the rule's declarations, so
//! `.lay-layers>*{grid-area:1/1}` puts all of a layer stack's children in the
//! same grid cell rather than in a column of their own rows.
//!
//! Two deliberate simplifications, each visible in a snapshot:
//!
//! * An element with no `display` lays out as a column of its children, which
//!   is what a block box does for the trees this paints.
//! * `list-style-type` is drawn by the element that carries it, beside each of
//!   its own boxes, rather than inherited down to whatever a browser calls a
//!   list item. A list region carries it and its items are its children, so
//!   the picture agrees; a list nested inside one carries its own.
//! * `opacity` multiplies into every colour the subtree paints rather than
//!   compositing the subtree as a group, so two overlapping half-transparent
//!   children show through each other.
//!
//! `position: fixed` is honoured the way the page does it: the element leaves
//! the flow, is laid out against the **viewport** rather than against whatever
//! it was written inside, escapes every ancestor's clip, and paints last so an
//! overlay covers the page it is over.
//!
//! A run of text answers all three intrinsic-width questions a layout engine
//! asks it, so `grid-template-columns: 1fr 2fr` divides the room the other
//! tracks left the way a browser divides it rather than sizing each track to
//! its own sentence. [`paint_with`]'s measure closure is where the three are
//! told apart.
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

use std::borrow::Cow;
use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, CacheKeyFlags, Family, FontSystem, Hinting, Metrics, Shaping, SwashCache, Weight,
    Wrap, fontdb,
};
use taffy::prelude::*;
use taffy::{Overflow, Point, TaffyTree, compute_leaf_layout};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke,
    StrokeDash, Transform,
};

/// What an image source paints: the PNG reader, the SVG subset, and the rule
/// that sizes both. A child module rather than a sibling so that it can reach
/// the painter's own box, blending and deflate tables.
#[path = "image.rs"]
mod image;

use image::{Picture, decode, pixel_bytes};

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

/// A list marker's distance from the item it marks, and a disc's diameter,
/// both as a multiple of the item's font size. What a browser draws.
const MARKER_GAP: f32 = 0.4;
const MARKER_DISC: f32 = 0.35;

/// A slider's bar, as a fraction of the control's height. The sheet paints the
/// same one with `background-size: 100% 25%`.
const TRACK_HEIGHT: f32 = 0.25;

/// The largest viewport the painter will allocate a canvas for.
const MAX_VIEWPORT: u32 = 8192;

// ---------------------------------------------------------------------------
// The public surface
// ---------------------------------------------------------------------------

/// What to paint.
pub struct Request<'a> {
    pub scene: &'a str,
    pub stylesheet: &'a str,
    /// "rest", "hover", "focus", "focus-within", "active", "disabled" or
    /// "checked".
    pub state: &'a str,
    /// The custom-property block the snapshot's themes resolved to — one or
    /// more `:root{--name:value;…}` blocks, exactly what `mount` installs.
    ///
    /// A declaration reading `var(--name)` is worth what this says it is worth,
    /// and is left as it stands where this says nothing about it. Empty is a
    /// program with no design tokens, which is most of them.
    pub variables: &'a str,
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
    let variables = parse_variables(request.variables);

    let styles = resolve(&scene, &sheet, state, &variables);
    let pixmap = paint(&scene, &styles)?;
    Ok(encode(pixmap.width(), pixmap.height(), &straight(&pixmap)))
}

/// The custom properties a `:root` block declares, each name without its
/// dashes.
///
/// The text is what `cli/runtime/ui.rs` wrote, so this reads that rather than
/// CSS in general: `:root{` opens a block, `;` separates declarations, and a
/// name starts with `--`. A later block wins, which is the order the themes
/// were passed in.
fn parse_variables(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for block in text.split(":root{").skip(1) {
        let Some((body, _)) = block.split_once('}') else { continue };
        for entry in body.split(';') {
            let Some((name, value)) = entry.split_once(':') else { continue };
            let Some(name) = name.trim().strip_prefix("--") else { continue };
            let value = value.trim().to_string();
            match out.iter_mut().find(|(k, _)| k == name) {
                Some(slot) => slot.1 = value,
                None => out.push((name.to_string(), value)),
            }
        }
    }
    out
}

/// A declaration's value with every `var(--name)` in it replaced by what the
/// themes said the name was worth.
///
/// A name the themes did not bind is left as it stands, so an unresolved token
/// still reads as one and each property's own arm decides what that means —
/// which is what the painter did before a theme could reach it at all.
///
/// A value a theme bound may hold a `var()` of its own — `Color.alpha` on a
/// token binds one to `color-mix(in srgb, var(--other) 50%, transparent)` — so
/// substitution repeats until nothing moves. The budget is the number of
/// bindings there are, which is [`crate::ui::render`]'s rule for a chain: one
/// that closes on itself stops instead of hanging.
///
/// Borrowed where there is nothing to do, which is every declaration in a
/// program with no design tokens.
fn substitute<'a>(value: &'a str, variables: &[(String, String)]) -> Cow<'a, str> {
    let mut out = substitute_once(value, variables);
    for _ in 0..variables.len() {
        if !out.contains("var(") {
            break;
        }
        let next = substitute_once(&out, variables).into_owned();
        if next == out {
            break;
        }
        out = Cow::Owned(next);
    }
    out
}

fn substitute_once<'a>(value: &'a str, variables: &[(String, String)]) -> Cow<'a, str> {
    if !value.contains("var(") {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find("var(") {
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let Some((inside, after)) = from["var(".len()..].split_once(')') else {
            out.push_str(from);
            return Cow::Owned(out);
        };
        // `var(--name, fallback)`: the fallback is what a browser paints when
        // nothing bound the name, and it is what this paints too.
        let (name, fallback) = match inside.split_once(',') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inside.trim(), None),
        };
        let bound = name
            .strip_prefix("--")
            .and_then(|n| variables.iter().find(|(k, _)| k == n))
            .map(|(_, v)| v.as_str());
        match bound.or(fallback) {
            Some(text) => out.push_str(text),
            None => {
                out.push_str("var(");
                out.push_str(inside);
                out.push(')');
            }
        }
        rest = after;
    }
    out.push_str(rest);
    Cow::Owned(out)
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

/// One line of the scene: a box, a picture, or a run of text inside one.
struct Node {
    /// `Some` for a `t` line. A text run has no children and no declarations.
    text: Option<String>,
    /// `Some` for a box whose declarations named an `image` or an `icon`. A
    /// picture holds no children either: what is inside it is the source.
    picture: Option<Art>,
    classes: Vec<String>,
    declarations: Vec<(String, String)>,
    children: Vec<usize>,
}

/// Where a picture's paint comes from.
///
/// The distinction is the whole of what an `icon` is for. An image's source is
/// a document of its own, so `currentColor` in it is that document's initial
/// colour and no rule on the page reaches it. An icon's artwork is *in* the
/// tree, so `currentColor` is the colour the element itself paints in.
enum Art {
    /// `image:<source>` — read only where the source is a `data:` URI.
    Source(String),
    /// `icon:<artwork>` — the drawing itself, written into the scene.
    Artwork(String),
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
                    picture: None,
                    classes: Vec::new(),
                    declarations: Vec::new(),
                    children: Vec::new(),
                }
            } else {
                let (classes, declarations) = parse_declarations(body)?;
                let named = |want: &str| {
                    declarations.iter().find(|(name, _)| name == want).map(|(_, v)| v.clone())
                };
                let picture = named("image")
                    .map(Art::Source)
                    .or_else(|| named("icon").map(Art::Artwork));
                Node { text: None, picture, classes, declarations, children: Vec::new() }
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

/// A declaration list, split on the semicolons that separate it, with `\;` and
/// `\\` put back as the characters they stand for.
///
/// Anything else after a backslash keeps both characters, which is the rule
/// [`unescape`] follows for a text run.
fn entries(body: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        let last = match out.last_mut() {
            Some(entry) => entry,
            None => return out,
        };
        match c {
            ';' => out.push(String::new()),
            '\\' => match chars.next() {
                Some(escaped @ ('\\' | ';')) => last.push(escaped),
                Some(other) => {
                    last.push('\\');
                    last.push(other);
                }
                None => last.push('\\'),
            },
            _ => last.push(c),
        }
    }
    out
}

/// `name:value` pairs joined by `;`, with `class` lifted out.
///
/// A value may hold the separator, escaped: `\;` is a semicolon and `\\` a
/// backslash. One value needs it — an image source is a data URI, and a data
/// URI is full of both — and every other value in a scene or a sheet is
/// written without a backslash, so nothing else changes shape.
fn parse_declarations(body: &str) -> Result<Declarations, String> {
    let mut classes = Vec::new();
    let mut declarations = Vec::new();
    for entry in entries(body) {
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
    /// The container's own: something inside it has the keyboard. A whole
    /// document is painted in it, the way every other state is.
    FocusWithin,
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
            "focus-within" => Ok(Self::FocusWithin),
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
            "focus-within" => Some(Self::FocusWithin),
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
    /// `.<class>>*` rather than `.<class>`: the declarations land on every
    /// child of an element carrying the class, and on the element itself never.
    child: bool,
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
        let Some((class, state, child)) = parse_selector(selector) else { continue };
        let Ok((_, declarations)) = parse_declarations(body) else { continue };
        rules.push(Rule { class, state, child, min_width, declarations });
    }
    rules
}

/// `.<class><pseudo?>`, or the same followed by `>*`, which is the one rule
/// about descendants the sheet writes: `Layout(.Layers)` is a `display:grid` on
/// the container and a `grid-area:1/1` on each of its children, and the pair is
/// the only way that value is expressed. Anything else after the pseudo-class —
/// a descendant combinator, a second pseudo-class, a named child — is `None`.
///
/// The third answer is whether the rule is the children's.
fn parse_selector(selector: &str) -> Option<(String, Option<State>, bool)> {
    let (selector, child) = match selector.trim_end().strip_suffix('*') {
        Some(head) => (head.trim_end().strip_suffix('>')?.trim_end(), true),
        None => (selector, false),
    };
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
        return Some((class, None, child));
    }
    State::pseudo(&pseudo).map(|state| (class, Some(state), child))
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

/// What marks each item of a list — `ui/style`'s `ListMarker`, which is the
/// only way a list gets one. The reset in the sheet cleared the browser's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Marker {
    None,
    Disc,
    Decimal,
}

/// A slider, as the scene's `range:<min> <max> <value>` gives it: the bounds
/// the program wrote, and where the thumb sits between them.
///
/// The value is sanitized here rather than by whoever wrote the scene, because
/// HTML's own rule is the one a browser applies to the `value` attribute: a
/// number outside the bounds is clamped into them, and text that is not a
/// number at all is the middle.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Slider {
    min: f32,
    max: f32,
    value: f32,
}

impl Slider {
    /// The scene's three words, or nothing when the first two are not numbers.
    fn read(value: &str) -> Option<Self> {
        let mut parts = value.splitn(3, ' ');
        let min: f32 = parts.next()?.trim().parse().ok()?;
        let max: f32 = parts.next()?.trim().parse().ok()?;
        if !min.is_finite() || !max.is_finite() || max < min {
            return None;
        }
        let middle = min + (max - min) / 2.0;
        let value = parts
            .next()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .map_or(middle, |v| v.clamp(min, max));
        Some(Self { min, max, value })
    }

    /// How far along the track the thumb sits, from nought to one. A range with
    /// no width to it is at the start, which is where a browser puts it.
    fn fraction(self) -> f32 {
        if self.max <= self.min { 0.0 } else { (self.value - self.min) / (self.max - self.min) }
    }
}

/// What a widget draws inside its own box — `mark:<shape>` in the scene, and
/// the reset's `::before` in a browser. Not inherited: it belongs to the one
/// line that named it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mark {
    None,
    /// A switch's thumb: the box, fully rounded, filled.
    Thumb,
    /// A checkbox's tick: a stroke through three points of the box's largest
    /// centred square.
    Tick,
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
    /// `column-reverse` or `row-reverse`: the children are laid out backwards
    /// and the document keeps the order it was written in.
    reverse: bool,
    wrap: bool,
    justify: Option<AlignContent>,
    align_items: Option<AlignItems>,
    align_self: Option<AlignItems>,
    grow: f32,
    shrink: f32,
    tracks: Vec<Len>,
    span: Option<u16>,
    /// `grid-area: <row>/<column>`, as the two lines the box starts at. The
    /// sheet writes one of these — `1/1`, for a layer stack's children.
    area: Option<(i16, i16)>,
    gap_column: Len,
    gap_row: Len,
    padding: [Len; 4],
    /// `margin-*`, which the vocabulary writes only as a `Bleed`: how far past
    /// its parent's edge the child reaches, so every one of these is negative
    /// or nothing. Zero everywhere else, the way the reset leaves a page.
    margin: [Len; 4],
    size: [Len; 2],
    min: [Len; 2],
    max: [Len; 2],
    aspect: Option<f32>,
    absolute: bool,
    /// `position: fixed`. Absolute as well, and measured against the viewport
    /// rather than against the box it was written in.
    fixed: bool,
    /// `position: sticky`. In the flow, and the inset it carries is a
    /// threshold rather than an offset — see [`taffy_style`].
    sticky: bool,
    inset: [Len; 4],
    clipped: [bool; 2],

    background: Rgba,
    colour: Rgba,
    /// One width per edge, in the order the padding and the inset use:
    /// inline-start, inline-end, block-start, block-end. `BorderWidth` sets
    /// all four and `BorderEdge` sets one.
    border_width: [Len; 4],
    /// `None` is CSS's initial `currentColor`: the stroke takes the element's
    /// own `colour`, whichever order the two were written in. Whole-box, so a
    /// border painted two colours is not expressible.
    border_colour: Option<Rgba>,
    /// One style per edge, in the same order. An edge whose style is `None`
    /// paints nothing, whatever its width.
    border_style: [Border; 4],
    /// One radius per corner: start-start, start-end, end-start, end-end —
    /// which is top-left, top-right, bottom-left, bottom-right on a
    /// left-to-right page.
    radii: [Len; 4],
    opacity: f32,
    /// Every `box-shadow` layer, in the order they were written — the first
    /// painted over the ones after it.
    shadow: Vec<Shadow>,
    /// `transform: translate(x, y)`, applied after the layout, so nothing
    /// around the box moves with it. A percentage is of the box's own size.
    translate: Option<(Len, Len)>,
    marker: Marker,
    mark: Mark,
    /// A slider, and where its thumb sits. Not inherited: it belongs to the
    /// one line that named it, the way a mark does.
    range: Option<Slider>,

    font_size: f32,
    weight: u16,
    italic: bool,
    line_height: f32,
    letter_spacing: f32,
    align_text: cosmic_text::Align,
    case: Case,
    decoration: Decoration,
    nowrap: bool,
    /// `text-wrap: balance`: break the run into the lines it would take
    /// anyway, evened out.
    balance: bool,
    /// `-webkit-line-clamp`: show at most this many lines and end the last one
    /// in an ellipsis. `None` is no limit.
    clamp: Option<usize>,
    /// A password's text: painted as bullets, never as itself. Inherited, so
    /// that the run inside the input carries it.
    masked: bool,
}

impl Computed {
    /// The style a scene's outermost box inherits.
    fn root() -> Self {
        Self {
            flow: Flow::Flex,
            column: true,
            reverse: false,
            wrap: false,
            justify: None,
            align_items: None,
            align_self: None,
            grow: 0.0,
            shrink: 1.0,
            tracks: Vec::new(),
            span: None,
            area: None,
            gap_column: Len::Px(0.0),
            gap_row: Len::Px(0.0),
            padding: [Len::Px(0.0); 4],
            margin: [Len::Px(0.0); 4],
            size: [Len::Auto; 2],
            min: [Len::Auto; 2],
            max: [Len::Auto; 2],
            aspect: None,
            absolute: false,
            fixed: false,
            sticky: false,
            inset: [Len::Auto; 4],
            clipped: [false; 2],
            background: Rgba::CLEAR,
            colour: Rgba::BLACK,
            border_width: [Len::Px(0.0); 4],
            border_colour: None,
            border_style: [Border::Solid; 4],
            radii: [Len::Px(0.0); 4],
            opacity: 1.0,
            shadow: Vec::new(),
            translate: None,
            marker: Marker::None,
            mark: Mark::None,
            range: None,
            font_size: ROOT_FONT_SIZE,
            weight: 400,
            italic: false,
            line_height: NORMAL_LINE_HEIGHT,
            letter_spacing: 0.0,
            align_text: cosmic_text::Align::Left,
            case: Case::None,
            decoration: Decoration::None,
            nowrap: false,
            balance: false,
            clamp: None,
            masked: false,
        }
    }

    /// The width each edge actually paints, in device pixels.
    ///
    /// A style of `none` is a width of zero whatever the declaration said, and
    /// a border-width in anything but pixels is not a width CSS accepts. Both
    /// the layout and the painter ask this, so neither can disagree about
    /// which edges are there.
    fn border_widths(&self) -> [f32; 4] {
        let mut out = [0.0; 4];
        for (i, slot) in out.iter_mut().enumerate() {
            let present = self.border_style.get(i).copied().unwrap_or(Border::None);
            if present != Border::None {
                if let Some(Len::Px(n)) = self.border_width.get(i).copied() {
                    *slot = n.max(0.0);
                }
            }
        }
        out
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
        child.balance = self.balance;
        // CSS puts the clamp on the block and counts the lines of the inline
        // content inside it. The painter's inline content is a `t` node one
        // level down, so the clamp has to reach it the way an inherited
        // property does.
        child.clamp = self.clamp;
        child.masked = self.masked;
        // Not inherited, but it multiplies down: a subtree under a half
        // transparent box is half transparent.
        child.opacity = self.opacity;
        child
    }
}

/// Resolves every node's style: matching class rules in sheet order, then the
/// element's own declarations, over what the parent passed down.
fn resolve(
    scene: &Scene,
    sheet: &[Rule],
    state: State,
    variables: &[(String, String)],
) -> Vec<Computed> {
    let root = Computed::root();
    let mut styles = vec![root.clone(); scene.nodes.len()];
    let mut stack: Vec<(usize, Option<usize>, Computed)> =
        scene.roots.iter().rev().map(|&i| (i, None, root.clone())).collect();

    while let Some((index, holder, parent)) = stack.pop() {
        let Some(node) = scene.node(index) else { continue };
        let mut style = parent.inherit();
        if node.text.is_none() {
            let width = scene.width as f32;
            // A `.<class>>*` rule is the enclosing box's class rather than this
            // one's, so a child rule is matched against the node above.
            let holder = holder.and_then(|i| scene.node(i));
            let mut declarations: Vec<(&str, Cow<'_, str>)> = Vec::new();
            for rule in sheet {
                let named = if rule.child {
                    holder.is_some_and(|h| h.classes.contains(&rule.class))
                } else {
                    node.classes.contains(&rule.class)
                };
                if rule.min_width <= width && rule.state.is_none_or(|s| s == state) && named {
                    for (name, value) in &rule.declarations {
                        declarations.push((name, substitute(value, variables)));
                    }
                }
            }
            for (name, value) in &node.declarations {
                declarations.push((name, substitute(value, variables)));
            }
            // **`font-size` is computed before everything beside it**, because
            // every `em` on this element is a multiple of the size the element
            // ends up at — and the sheet writes `gap` and `padding` before
            // `font-size`, which is a rule about *conflicting* declarations and
            // says nothing about this. The last one wins, and it is itself a
            // multiple of the parent's size, which is what CSS resolves an `em`
            // in a `font-size` against.
            if let Some((name, value)) = declarations.iter().rev().find(|(n, _)| *n == "font-size")
            {
                apply(&mut style, name, value, &parent);
            }
            for (name, value) in &declarations {
                if *name != "font-size" {
                    apply(&mut style, name, value, &parent);
                }
            }
        }
        for &child in node.children.iter().rev() {
            stack.push((child, Some(index), style.clone()));
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
        "flex-direction" => {
            style.column = !value.starts_with("row");
            style.reverse = value.ends_with("-reverse");
        }
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
        "grid-area" => style.area = grid_area(value).or(style.area),
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
        "margin-inline-start" => set_sides(&mut style.margin, [0], len(value)),
        "margin-inline-end" => set_sides(&mut style.margin, [1], len(value)),
        "margin-block-start" => set_sides(&mut style.margin, [2], len(value)),
        "margin-block-end" => set_sides(&mut style.margin, [3], len(value)),
        "width" => set_sides(&mut style.size, [0], len(value)),
        "height" => set_sides(&mut style.size, [1], len(value)),
        "min-width" => set_sides(&mut style.min, [0], len(value)),
        "min-height" => set_sides(&mut style.min, [1], len(value)),
        "max-width" => set_sides(&mut style.max, [0], len(value)),
        "max-height" => set_sides(&mut style.max, [1], len(value)),
        "aspect-ratio" => style.aspect = value.parse().ok().filter(|r: &f32| *r > 0.0),
        "position" => {
            style.fixed = value == "fixed";
            style.absolute = style.fixed || value == "absolute";
            style.sticky = value == "sticky";
        }
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
        "border-width" => set_sides(&mut style.border_width, [0, 1, 2, 3], len(value)),
        "border-inline-start-width" => set_sides(&mut style.border_width, [0], len(value)),
        "border-inline-end-width" => set_sides(&mut style.border_width, [1], len(value)),
        "border-block-start-width" => set_sides(&mut style.border_width, [2], len(value)),
        "border-block-end-width" => set_sides(&mut style.border_width, [3], len(value)),
        "border-color" => {
            style.border_colour = match colour(value) {
                Some(Spec::Value(c)) => Some(c),
                Some(Spec::Transparent) => Some(Rgba::CLEAR),
                // `inherit`, and a `var()` nothing defined: back to the
                // element's own colour, which is where a border with no
                // declaration starts.
                _ => None,
            };
        }
        "border-style" => style.border_style = [border(value); 4],
        "border-inline-start-style" => style.border_style[0] = border(value),
        "border-inline-end-style" => style.border_style[1] = border(value),
        "border-block-start-style" => style.border_style[2] = border(value),
        "border-block-end-style" => style.border_style[3] = border(value),
        "border-radius" => set_sides(&mut style.radii, [0, 1, 2, 3], len(value)),
        "border-start-start-radius" => set_sides(&mut style.radii, [0], len(value)),
        "border-start-end-radius" => set_sides(&mut style.radii, [1], len(value)),
        "border-end-start-radius" => set_sides(&mut style.radii, [2], len(value)),
        "border-end-end-radius" => set_sides(&mut style.radii, [3], len(value)),
        "opacity" => {
            if let Ok(o) = value.parse::<f32>() {
                style.opacity = parent.opacity * o.clamp(0.0, 1.0);
            }
        }
        "box-shadow" => style.shadow = shadows(value, font_size),
        // The one transform the vocabulary writes. An unreadable one is
        // ignored, which is what a browser does with a declaration it cannot
        // parse.
        "transform" => style.translate = translate(value, font_size).or(style.translate),
        "list-style-type" => {
            style.marker = match value {
                "disc" => Marker::Disc,
                "decimal" => Marker::Decimal,
                _ => Marker::None,
            };
        }

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
        "text-wrap" => {
            style.nowrap = value == "nowrap";
            style.balance = value == "balance";
        }
        // `none` is the value `Truncate(0)` writes, and it parses to no limit.
        "-webkit-line-clamp" => style.clamp = value.parse().ok().filter(|&n| n > 0),
        // What a widget draws for itself. Unknown shapes draw nothing, which is
        // the rule every other property here follows.
        "mark" => {
            style.mark = match value {
                "thumb" => Mark::Thumb,
                "tick" => Mark::Tick,
                _ => Mark::None,
            };
        }
        // What the input accepts. A secret is masked; an `<input>` is one line
        // and a `textarea` is the one kind that is not, so the rest is the
        // difference the sheet's own reset leaves — which is none.
        "field" => {
            style.masked = value == "password";
            style.nowrap = value != "multiline";
        }
        // A slider's bounds and where its thumb sits. Bounds that are not two
        // numbers leave the box a box, which is the rule an unreadable image
        // source and an unknown mark both follow.
        "range" => style.range = Slider::read(value),
        // `font-family` resolves to the bundled family whatever it names, and
        // `cursor` paints nothing. Both parse so that a scene keeps them.
        _ => {}
    }
}

fn border(value: &str) -> Border {
    match value {
        "none" => Border::None,
        "dashed" => Border::Dashed,
        _ => Border::Solid,
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

fn length(value: &str, font_size: f32) -> Option<Len> {
    if value == "auto" {
        return Some(Len::Auto);
    }
    if let Some(n) = value.strip_suffix("px") {
        return n.parse().ok().map(Len::Px);
    }
    if let Some(n) = value.strip_suffix("rem") {
        return n.parse::<f32>().ok().map(|n| Len::Px(n * REM));
    }
    // After `rem`, which ends in the same two letters. An em is the element's
    // own text size, and the caller passes the size in force where the
    // declaration was written — so a `font-size` in em reads the size it
    // inherited, exactly as CSS resolves one.
    if let Some(n) = value.strip_suffix("em") {
        return n.parse::<f32>().ok().map(|n| Len::Px(n * font_size));
    }
    if let Some(n) = value.strip_suffix('%') {
        return n.parse().ok().map(Len::Percent);
    }
    value.parse().ok().map(Len::Px)
}

/// `translate(<length>,<length>)`, the one transform the sheet writes.
fn translate(value: &str, font_size: f32) -> Option<(Len, Len)> {
    let inner = value.trim().strip_prefix("translate(")?.strip_suffix(')')?;
    let (x, y) = inner.split_once(',')?;
    Some((length(x.trim(), font_size)?, length(y.trim(), font_size)?))
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

/// `grid-area: <row>/<column>`, the one shorthand the sheet writes. Both halves
/// are line numbers, so `1/1` is the first cell — and the end of each span is
/// left to the row and column the box starts in, which is what CSS does with a
/// two-value `grid-area` too.
fn grid_area(value: &str) -> Option<(i16, i16)> {
    let (row, column) = value.split_once('/')?;
    Some((row.trim().parse().ok()?, column.trim().parse().ok()?))
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

/// Splits on a separator that is not inside brackets.
///
/// `color-mix(in srgb,rgb(1,2,3) 50%,transparent)` has commas in two meanings
/// and spaces inside a function, so neither `split(',')` nor `split(' ')` can
/// read one. Everything the painter takes apart by hand goes through this.
fn split_top(text: &str, separator: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if c == separator && depth == 0 => {
                out.push(text.get(start..at).unwrap_or(""));
                start = at + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(text.get(start..).unwrap_or(""));
    out
}

fn colour(value: &str) -> Option<Spec> {
    colour_at(value, 0)
}

/// How many `color-mix`es may nest. A theme can bind a token to a *faded*
/// token, so a chain of them nests one deep per link; past this a declaration
/// reads as a colour the painter does not know rather than as a recursion.
const MIX_DEPTH: usize = 4;

/// A colour declaration, with the nesting `color-mix` puts in bounded by
/// [`MIX_DEPTH`].
fn colour_at(value: &str, depth: usize) -> Option<Spec> {
    let value = value.trim();
    match value {
        "transparent" => return Some(Spec::Transparent),
        "inherit" => return Some(Spec::Inherit),
        _ => {}
    }
    // `Color.alpha` on a token: the token still decides the hue, and the
    // percentage decides how much of it there is. Read *after* the themes were
    // substituted, so what is left inside is a colour or an unbound name.
    if let Some(rest) = value.strip_prefix("color-mix(in srgb,") {
        if depth >= MIX_DEPTH {
            return None;
        }
        let inner = rest.strip_suffix(')')?;
        let [mixed, rest] = split_top(inner, ',')[..] else { return None };
        if rest.trim() != "transparent" {
            return None;
        }
        let [base, share] = split_top(mixed.trim(), ' ')[..] else { return None };
        let fraction = share.trim().strip_suffix('%')?.parse::<f32>().ok()? / 100.0;
        return match colour_at(base, depth + 1)? {
            Spec::Value(c) => {
                Some(Spec::Value(Rgba { a: (c.a * fraction).clamp(0.0, 1.0), ..c }))
            }
            // Nothing bound the token, so the mix is a colour that is not
            // there — which is what every property already does with one.
            Spec::Token => Some(Spec::Token),
            Spec::Transparent => Some(Spec::Transparent),
            Spec::Inherit => None,
        };
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

/// Every layer of a `box-shadow`, in the order they were written.
///
/// A layer this cannot read drops the whole declaration, which is what a
/// browser does with one invalid value in a list — and what the unbound-token
/// picture in `sweep_themes_tokens` already pins for the one-layer spelling.
fn shadows(value: &str, font_size: f32) -> Vec<Shadow> {
    let mut out = Vec::new();
    for layer in split_top(value, ',') {
        match shadow(layer.trim(), font_size) {
            Some(one) => out.push(one),
            None => return Vec::new(),
        }
    }
    out
}

/// `<x> <y> <blur> <spread> <colour>`.
fn shadow(value: &str, font_size: f32) -> Option<Shadow> {
    let mut parts = split_top(value, ' ').into_iter().filter(|p| !p.is_empty());
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
        flex_direction: match (c.column, c.reverse) {
            (true, false) => FlexDirection::Column,
            (true, true) => FlexDirection::ColumnReverse,
            (false, false) => FlexDirection::Row,
            (false, true) => FlexDirection::RowReverse,
        },
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
        grid_row: match c.area {
            Some((row, _)) => Line::from_line_index(row),
            None => Line::from_span(1),
        },
        grid_column: match c.area {
            Some((_, column)) => Line::from_line_index(column),
            None => c.span.map_or(Line::from_span(1), Line::from_span),
        },
        gap: Size { width: spacing(c.gap_column), height: spacing(c.gap_row) },
        padding: Rect {
            left: spacing(c.padding[0]),
            right: spacing(c.padding[1]),
            top: spacing(c.padding[2]),
            bottom: spacing(c.padding[3]),
        },
        // A margin the vocabulary writes is a `Bleed`, so it is negative:
        // the child reaches past its parent's edge and overlaps whatever it
        // finds there.
        margin: Rect {
            left: dimension_auto(c.margin[0]),
            right: dimension_auto(c.margin[1]),
            top: dimension_auto(c.margin[2]),
            bottom: dimension_auto(c.margin[3]),
        },
        border: {
            let w = c.border_widths();
            Rect {
                left: LengthPercentage::length(w[0]),
                right: LengthPercentage::length(w[1]),
                top: LengthPercentage::length(w[2]),
                bottom: LengthPercentage::length(w[3]),
            }
        },
        size: Size { width: dimension(c.size[0]), height: dimension(c.size[1]) },
        min_size: Size { width: dimension_auto(c.min[0]), height: dimension_auto(c.min[1]) },
        max_size: Size { width: dimension_auto(c.max[0]), height: dimension_auto(c.max[1]) },
        aspect_ratio: c.aspect,
        position: if c.absolute { Position::Absolute } else { Position::Relative },
        // **A sticky box keeps its static position.** Its inset is the edge it
        // would be held at once a scrollport crossed it, and a snapshot has no
        // scrolling, so a browser applies none of it — while `taffy`, which
        // has only `relative` and `absolute`, would take the same numbers as a
        // relative offset and move the box.
        inset: if c.sticky {
            Rect::auto()
        } else {
            Rect {
                left: dimension_auto(c.inset[0]),
                right: dimension_auto(c.inset[1]),
                top: dimension_auto(c.inset[2]),
                bottom: dimension_auto(c.inset[3]),
            }
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

/// The shaper and the glyph rasterizer, kept for the life of the thread.
///
/// **Reuse, not state.** Both are caches over an immutable font database — a
/// shaped run keyed by its text and attributes, a glyph image keyed by its
/// cache key — so the second render of a scene reads what the first computed
/// and answers the same bytes. What it saves is real: parsing the three faces
/// and warming the caches is most of the cost of painting one small tree, and a
/// suite paints one per `test` block.
///
/// A thread local rather than a static, because `SwashCache` is not `Sync` and
/// nothing here wants a lock on the paint path. A suite paints on the thread
/// that ran the block, so the cache is warm exactly where the work is.
///
/// `the_same_scene_renders_to_the_same_bytes_twice` is the assertion that
/// reuse is invisible, and every golden in `cli/tests/repositories/ui/` is the
/// same assertion at fourteen scenes at once: a cache that changed an answer
/// would move a picture.
thread_local! {
    static FACES: std::cell::RefCell<(FontSystem, SwashCache)> =
        std::cell::RefCell::new((font_system(), SwashCache::new()));
}

/// The characters a run is shaped from: the mask, if it is a password's, and
/// otherwise what `text-transform` made of it.
///
/// One bullet per `char`, which is what a browser draws and what keeps the box
/// the width the secret would have taken without the box holding it.
fn transformed(text: &str, style: &Computed) -> String {
    if style.masked {
        return "\u{2022}".repeat(text.chars().count());
    }
    match style.case {
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

/// One run of text, shaped into a buffer at the given width, breaking where
/// `wrap` says it may.
///
/// `wrap` is [`Wrap::WordOrGlyph`] everywhere a picture is being laid out or
/// drawn, and [`Wrap::Word`] in the one place a run is asked how narrow it can
/// be made. A `.TextWrap(.NoWrap)` run overrides both: it has no break in it to
/// take.
///
/// **Both metrics are held to a device pixel**, and the line box's is the one
/// that matters: `.LineHeight(0.0)` is a style a program may write, `0` is a
/// value CSS accepts, and a shaper handed a zero line height panics rather than
/// laying anything out. A line box shorter than a pixel is not a picture either
/// way, so the floor is the honest answer and it is the same floor the size
/// takes.
fn shape(
    fonts: &mut FontSystem,
    text: &str,
    style: &Computed,
    width: Option<f32>,
    wrap: Wrap,
) -> Buffer {
    let size = style.font_size.max(1.0);
    let leading = (size * style.line_height).max(1.0);
    let mut buffer = Buffer::new(fonts, Metrics::new(size, leading));
    buffer.set_hinting(Hinting::Disabled);
    buffer.set_wrap(if style.nowrap { Wrap::None } else { wrap });
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
    let mut content = transformed(text, style);
    lay(fonts, &mut buffer, &content, &attrs, style.align_text);

    // A clamp and a balance are answers about the run at the width it will
    // take. Width `Some(0.0)` is the one place a run is asked how narrow it
    // can be *made*, and neither changes that answer — the longest word is
    // still the longest word; `None` is max-content, which is one line
    // already.
    if let Some(room) = width.filter(|&w| w > 0.0) {
        if let Some(limit) = style.clamp {
            content = clamp(fonts, &mut buffer, &content, &attrs, style.align_text, limit);
        }
        if style.balance {
            balance(fonts, &mut buffer, &content, &attrs, style.align_text, room);
        }
    }
    buffer
}

/// Puts a string in the buffer and shapes it.
fn lay(
    fonts: &mut FontSystem,
    buffer: &mut Buffer,
    text: &str,
    attrs: &Attrs,
    align: cosmic_text::Align,
) {
    buffer.set_text(text, attrs, Shaping::Advanced, Some(align));
    buffer.shape_until_scroll(fonts, false);
}

/// How many lines the buffer laid out.
fn line_count(buffer: &Buffer) -> usize {
    buffer.layout_runs().count()
}

/// Cuts the run down to `limit` lines and ends the last one in an ellipsis —
/// `-webkit-line-clamp`, which is what `.Truncate(n)` lowers to.
///
/// Where a browser cuts is where the shaper broke, so the cut is found by
/// asking the shaper rather than by counting characters: the longest prefix
/// that still lays out in `limit` lines once the ellipsis is on the end of it.
/// A prefix only ever needs more lines as it grows, so that is a binary search
/// over the run's character boundaries — a handful of re-shapes rather than
/// one per character.
fn clamp(
    fonts: &mut FontSystem,
    buffer: &mut Buffer,
    text: &str,
    attrs: &Attrs,
    align: cosmic_text::Align,
    limit: usize,
) -> String {
    if line_count(buffer) <= limit {
        return text.to_string();
    }
    let cuts: Vec<usize> =
        text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
    let ellipsised = |head: &str| format!("{}…", head.trim_end());
    // The empty prefix always fits: an ellipsis on its own is one line.
    let (mut lo, mut hi) = (0_usize, cuts.len().saturating_sub(1));
    while lo < hi {
        let mid = lo.saturating_add(hi.saturating_sub(lo).div_ceil(2));
        let head = cuts.get(mid).and_then(|&at| text.get(..at)).unwrap_or(text);
        lay(fonts, buffer, &ellipsised(head), attrs, align);
        if line_count(buffer) <= limit {
            lo = mid;
        } else {
            hi = mid.saturating_sub(1);
        }
    }
    let head = cuts.get(lo).and_then(|&at| text.get(..at)).unwrap_or(text);
    let cut = ellipsised(head);
    lay(fonts, buffer, &cut, attrs, align);
    cut
}

/// Evens the line lengths out — `text-wrap: balance`, the way a browser
/// approximates it.
///
/// The run keeps the number of lines it took at its full width, and takes them
/// at the narrowest width that still does: a heading whose last line was one
/// short word comes out as lines of a length. Which width that is comes from
/// the shaper, by binary search over whole pixels, because a run only ever
/// needs more lines as its room shrinks.
///
/// The breaks are then written back into the run as newlines and it is laid
/// out in the room it was actually given. Setting the buffer to the narrow
/// width and leaving it there would balance the lines and then align them
/// inside that width, so a centred heading would sit left of centre in its
/// box.
fn balance(
    fonts: &mut FontSystem,
    buffer: &mut Buffer,
    text: &str,
    attrs: &Attrs,
    align: cosmic_text::Align,
    room: f32,
) {
    let target = line_count(buffer);
    if target <= 1 {
        return;
    }
    // `hi` is the room the run already fits in, so it always satisfies the
    // search; `lo` climbs until the two meet on the narrowest width that does.
    let (mut lo, mut hi) = (1_u32, room.ceil().max(1.0) as u32);
    while lo < hi {
        let mid = lo.saturating_add(hi.saturating_sub(lo) / 2);
        buffer.set_size(Some(mid as f32), None);
        lay(fonts, buffer, text, attrs, align);
        if line_count(buffer) <= target {
            hi = mid;
        } else {
            lo = mid.saturating_add(1);
        }
    }
    buffer.set_size(Some(lo as f32), None);
    lay(fonts, buffer, text, attrs, align);

    let broken = broken(buffer, text);
    buffer.set_size(Some(room), None);
    lay(fonts, buffer, &broken, attrs, align);
}

/// The run with a newline wherever the shaper broke it, so the same breaks
/// survive being laid out in a wider box.
fn broken(buffer: &Buffer, text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for run in buffer.layout_runs() {
        let Some((from, to)) = run
            .glyphs
            .iter()
            .map(|g| (g.start, g.end))
            .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
        else {
            continue;
        };
        let line = text.get(from..to).unwrap_or_default().trim_end();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
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
    FACES.with_borrow_mut(|(fonts, cache)| paint_with(scene, styles, fonts, cache))
}

/// [`paint`], with the shaper and the glyph cache handed in.
fn paint_with(
    scene: &Scene,
    styles: &[Computed],
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
) -> Result<Pixmap, String> {
    let mut tree: TaffyTree<usize> = TaffyTree::new();
    tree.disable_rounding();

    // Every source read once, before the layout asks any of them how large it
    // is. `None` beside a node that has an `image` is the placeholder.
    let pictures: Vec<Option<Picture>> =
        scene.nodes.iter().map(|node| node.picture.as_ref().and_then(picture)).collect();

    let mut ids: Vec<Option<NodeId>> = vec![None; scene.nodes.len()];
    let mut fixed: Vec<usize> = Vec::new();
    let roots =
        build(scene, styles, &pictures, &mut tree, &scene.roots, &mut ids, &mut fixed)?;
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
    // A fixed element is measured against the viewport, so it hangs off the
    // canvas rather than off whatever it was written inside — and it comes
    // last, so it is laid out over the page and painted over it.
    let mut children = roots;
    children.extend(fixed.iter().filter_map(|&index| ids.get(index).copied().flatten()));
    let root = tree.new_with_children(viewport, &children).map_err(|e| format!("layout: {e}"))?;

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
                // **The three questions a layout engine asks are three
                // different questions.** How wide is this run under a width I
                // have in mind, how wide would it be if it never wrapped, and
                // how narrow can it be made — and the third one is what a
                // track's automatic minimum, a flex item's automatic minimum
                // and a content-sized box are all built on. Shaping into
                // nothing is how the shaper is asked it, and it is asked with
                // word breaks only, because a word that will not fit still may
                // not be split: the answer is the longest word, not the widest
                // letter.
                //
                // Answering max-content to all three is what made every `fr`
                // track as wide as its own text — `minmax(auto, 1fr)` is what
                // `1fr` means, and an automatic minimum of the whole sentence
                // holds the track open past its share — so `1fr 2fr` painted as
                // two content-sized tracks and a row that ran off the page.
                let (width, wrap) = match (known.width, available.width) {
                    (Some(w), _) | (None, AvailableSpace::Definite(w)) => {
                        (Some(w), Wrap::WordOrGlyph)
                    }
                    (None, AvailableSpace::MinContent) => (Some(0.0), Wrap::Word),
                    (None, AvailableSpace::MaxContent) => (None, Wrap::WordOrGlyph),
                };
                let (w, h) = extent(&shape(fonts, text, style, width, wrap));
                Size { width: known.width.unwrap_or(w), height: known.height.unwrap_or(h) }
            })
        },
    )
    .map_err(|e| format!("layout: {e}"))?;

    let mut canvas = Pixmap::new(scene.width, scene.height)
        .ok_or_else(|| format!("the viewport {}x{} has no canvas", scene.width, scene.height))?;
    canvas.fill(tiny_skia::Color::WHITE);

    let mut painter =
        Painter { scene, styles, pictures: &pictures, tree: &tree, ids: &ids, fonts, cache };
    for &index in &scene.roots {
        if painter.is_fixed(index) {
            continue;
        }
        painter.draw(&mut canvas, index, 0.0, 0.0, None);
    }
    // Out of the flow and out of every ancestor's clip, at the origin the
    // viewport gave it.
    for &index in &fixed {
        painter.draw(&mut canvas, index, 0.0, 0.0, None);
    }
    Ok(canvas)
}

/// Mirrors the scene into a `taffy` tree, remembering each node's id.
///
/// Answers the ids that stay where they were written. A `position: fixed`
/// node is built like any other and then handed up in `fixed` instead, for the
/// caller to hang off the viewport.
fn build(
    scene: &Scene,
    styles: &[Computed],
    pictures: &[Option<Picture>],
    tree: &mut TaffyTree<usize>,
    indices: &[usize],
    ids: &mut [Option<NodeId>],
    fixed: &mut Vec<usize>,
) -> Result<Vec<NodeId>, String> {
    let mut out = Vec::with_capacity(indices.len());
    for &index in indices {
        let Some(node) = scene.node(index) else { continue };
        let style = styles.get(index).cloned().unwrap_or_else(Computed::root);
        let id = if node.text.is_some() {
            tree.new_leaf_with_context(taffy_style(&style), index)
        } else if node.picture.is_some() {
            tree.new_leaf(picture_style(&style, pictures.get(index).and_then(Option::as_ref)))
        } else {
            let children = build(scene, styles, pictures, tree, &node.children, ids, fixed)?;
            tree.new_with_children(taffy_style(&style), &children)
        }
        .map_err(|e| format!("layout: {e}"))?;
        if let Some(slot) = ids.get_mut(index) {
            *slot = Some(id);
        }
        if style.fixed {
            fixed.push(index);
        } else {
            out.push(id);
        }
    }
    Ok(out)
}

/// A picture's box: whatever the scene declared, and where it declared
/// nothing, the size the source has.
///
/// A source the painter read has a size of its own — a PNG's pixels, an SVG's
/// `width` and `height`, or its `viewBox` where it has only that — which is the
/// size an `img` takes in a page when no rule says otherwise. A source it could
/// not read has none, so the placeholder fills the box around it instead of
/// collapsing to nothing — which is the whole complaint an image that painted
/// blank was.
fn picture_style(style: &Computed, picture: Option<&Picture>) -> Style {
    let mut out = taffy_style(style);
    let (width, height) = match picture {
        Some(picture) => {
            let (w, h) = picture.intrinsic();
            (Dimension::length(w), Dimension::length(h))
        }
        None => (Dimension::percent(1.0), Dimension::percent(1.0)),
    };
    if style.size[0] == Len::Auto {
        out.size.width = width;
    }
    if style.size[1] == Len::Auto {
        out.size.height = height;
    }
    out
}

struct Painter<'a> {
    scene: &'a Scene,
    styles: &'a [Computed],
    /// One slot per scene node: the pixels of a picture whose source was read.
    pictures: &'a [Option<Picture>],
    tree: &'a TaffyTree<usize>,
    ids: &'a [Option<NodeId>],
    fonts: &'a mut FontSystem,
    cache: &'a mut SwashCache,
}

impl Painter<'_> {
    /// Whether a node was lifted out of the flow and onto the viewport.
    fn is_fixed(&self, index: usize) -> bool {
        self.styles.get(index).is_some_and(|style| style.fixed)
    }

    /// Draws one node and its children, in document order, which is paint
    /// order: a later sibling covers an earlier one.
    fn draw(&mut self, canvas: &mut Pixmap, index: usize, x: f32, y: f32, clip: Option<&Mask>) {
        let (Some(node), Some(style), Some(&Some(id))) =
            (self.scene.node(index), self.styles.get(index), self.ids.get(index))
        else {
            return;
        };
        let Ok(layout) = self.tree.layout(id) else { return };
        let (across, down) = shift(style, layout.size);
        let left = x + layout.location.x + across;
        let top = y + layout.location.y + down;
        let right = left + layout.size.width;
        let bottom = top + layout.size.height;
        let box_ = Box2 { l: px(left), t: px(top), r: px(right), b: px(bottom) };

        if let Some(text) = node.text.as_deref() {
            // The *unrounded* width, because that is the one the measure pass
            // wrapped against; a pixel less would wrap the last word again.
            self.text(canvas, text, style, box_, layout.size.width, clip);
            return;
        }

        let radii = style.radii.map(|r| resolve_length(r, layout.size.width));
        if !style.shadow.is_empty() {
            // An outer shadow is painted outside the border box and nowhere
            // else, so the box is knocked out of whatever clip was already in
            // force. A ring around a transparent control is the case that
            // needs it: without the knockout the ring fills the control.
            let outside = outside_the_box(box_, radii, clip, canvas.width(), canvas.height());
            let under = outside.as_ref().or(clip);
            // Back to front: a `box-shadow` list paints the first layer over
            // the ones after it, so the last is laid down first.
            for shadow in style.shadow.iter().rev().copied() {
                let cast = box_.offset(shadow.x, shadow.y).grow(shadow.spread);
                let corner = radii.map(|r| r + shadow.spread);
                if shadow.blur > 0.0 {
                    cast_blurred(canvas, cast, corner, shadow, style.opacity, under);
                } else {
                    fill(canvas, cast, corner, shadow.colour, style.opacity, under);
                }
            }
        }
        if style.background.visible() {
            fill(canvas, box_, radii, style.background, style.opacity, clip);
        }
        let widths = style.border_widths();
        if widths.iter().any(|w| *w > 0.0)
            && style.border_colour.unwrap_or(style.colour).visible()
        {
            // Four edges the same is one stroke around the box, which is the
            // border every program wrote before an edge could be named on its
            // own. Anything else is the ring between the two boxes.
            let uniform = widths.iter().all(|w| (*w - widths[0]).abs() < f32::EPSILON);
            if uniform {
                stroke(canvas, box_, radii, widths[0], style, clip);
            } else {
                stroke_edges(canvas, box_, radii, widths, style, clip);
            }
        }

        if let Some(slider) = style.range {
            self.slider(canvas, style, box_, slider, clip);
            return;
        }

        if style.mark != Mark::None {
            self.mark(canvas, style, box_, clip);
            return;
        }

        let mut owned;
        let inner = if style.clipped[0] || style.clipped[1] {
            owned = clip.cloned().or_else(|| full_mask(canvas.width(), canvas.height()));
            if let Some(mask) = owned.as_mut() {
                intersect(mask, box_, radii);
            }
            owned.as_ref()
        } else {
            clip
        };
        if let Some(art) = node.picture.as_ref() {
            // Inside its own padding and border, which is where a browser draws
            // a picture: the box a drawing fills is the content box. Only an
            // icon can reach this — an `image` carries no styles of its own —
            // and without it a padded icon painted over its own frame.
            let basis = layout.size.width;
            let content = box_.shrink([
                resolve_length(style.padding[0], basis) + widths[0],
                resolve_length(style.padding[1], basis) + widths[1],
                resolve_length(style.padding[2], basis) + widths[2],
                resolve_length(style.padding[3], basis) + widths[3],
            ]);
            self.picture(canvas, index, style, content, inner, art);
            return;
        }
        let mut item = 0_u32;
        for &child in &node.children {
            // A fixed child hangs off the viewport, so it is neither placed
            // here nor clipped by anything here. It is drawn last, from the
            // top.
            if self.is_fixed(child) {
                continue;
            }
            if style.marker != Marker::None
                && self.scene.node(child).is_some_and(|c| c.text.is_none())
            {
                item = item.saturating_add(1);
                self.marker(canvas, style.marker, item, child, left, top, inner);
            }
            self.draw(canvas, child, left, top, inner);
        }
    }

    /// Draws a picture into the box the layout gave it.
    ///
    /// A raster is scaled to that box, nearest neighbour, in integers — the
    /// same pixel for the same box on every host. A vector is drawn into it at
    /// the box's own size. A source that was not read is neither: it is a
    /// framed grey placeholder, which says "a picture belongs here" without
    /// pretending to be one.
    fn picture(
        &mut self,
        canvas: &mut Pixmap,
        index: usize,
        style: &Computed,
        box_: Box2,
        clip: Option<&Mask>,
        art: &Art,
    ) {
        let Some(picture) = self.pictures.get(index).and_then(Option::as_ref) else {
            fill(canvas, box_, [0.0; 4], PLACEHOLDER_EDGE, style.opacity, clip);
            fill(canvas, box_.grow(-1.0), [0.0; 4], PLACEHOLDER_FILL, style.opacity, clip);
            return;
        };
        // An icon's `currentColor` is the colour this element paints in, which
        // is the whole of what an icon is for. An image's is not: its source is
        // a document of its own, where `color` is back at its initial value and
        // no rule on the page reaches it.
        let colour = match art {
            Art::Artwork(_) => style.colour,
            Art::Source(_) => Rgba::BLACK,
        };
        picture.draw(canvas, box_, style.opacity, colour, clip);
    }

    /// A slider: a bar across the middle of its box, and a round thumb on it
    /// at the value.
    ///
    /// Both are the box's own colour, which is the sheet's `currentColor` — so
    /// `Foreground` is the one property that paints a slider, and a
    /// `Background` on it is the box behind the bar. The bar is a quarter of
    /// the box's height and has no corners, because a browser paints it with a
    /// gradient and a gradient has none. The thumb is one line across, and its
    /// **centre** runs from half a thumb inside the near end to half a thumb
    /// inside the far one, which is where a browser stops it so that a slider
    /// at either end is still whole.
    ///
    /// A box smaller than a line shrinks the thumb to fit it, which is the one
    /// place this and a browser differ: a browser's thumb is one line whatever
    /// the box is and hangs out of it. A thumb that fits is the better picture
    /// and one that overflows is the better copy, and below a line there is no
    /// size that is both.
    fn slider(
        &mut self,
        canvas: &mut Pixmap,
        style: &Computed,
        box_: Box2,
        slider: Slider,
        clip: Option<&Mask>,
    ) {
        let left = box_.l as f32;
        let top = box_.t as f32;
        let width = box_.r as f32 - left;
        let height = box_.b as f32 - top;
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let bar = height * TRACK_HEIGHT;
        let middle = top + height / 2.0;
        fill(
            canvas,
            Box2 { l: box_.l, t: px(middle - bar / 2.0), r: box_.r, b: px(middle + bar / 2.0) },
            [0.0; 4],
            style.colour,
            style.opacity,
            clip,
        );
        let size = ROOT_FONT_SIZE.min(height).min(width);
        let travel = (width - size).max(0.0);
        let centre = left + size / 2.0 + travel * slider.fraction().clamp(0.0, 1.0);
        fill(
            canvas,
            Box2 {
                l: px(centre - size / 2.0),
                t: px(middle - size / 2.0),
                r: px(centre + size / 2.0),
                b: px(middle + size / 2.0),
            },
            [size / 2.0; 4],
            style.colour,
            style.opacity,
            clip,
        );
    }

    /// The mark a widget draws inside its own box, in the box's own colour.
    ///
    /// A thumb is the box, fully rounded. A tick is a stroke through
    /// `(3.5, 8.5)`, `(6.5, 11.5)` and `(12.5, 4.5)` of a sixteen-unit square,
    /// scaled to the largest square the box holds and centred in it — the same
    /// three points, the same proportional width and the same round ends as
    /// the mask the stylesheet's reset writes, so a browser and this painter
    /// draw one tick.
    fn mark(&mut self, canvas: &mut Pixmap, style: &Computed, box_: Box2, clip: Option<&Mask>) {
        let width = (box_.r - box_.l) as f32;
        let height = (box_.b - box_.t) as f32;
        let side = width.min(height);
        if side <= 0.0 {
            return;
        }
        if style.mark == Mark::Thumb {
            fill(canvas, box_, [side / 2.0; 4], style.colour, style.opacity, clip);
            return;
        }
        let unit = side / 16.0;
        let left = box_.l as f32 + (width - side) / 2.0;
        let top = box_.t as f32 + (height - side) / 2.0;
        let mut pen = PathBuilder::new();
        pen.move_to(left + 3.5 * unit, top + 8.5 * unit);
        pen.line_to(left + 6.5 * unit, top + 11.5 * unit);
        pen.line_to(left + 12.5 * unit, top + 4.5 * unit);
        let Some(path) = pen.finish() else { return };
        let paint = Paint {
            anti_alias: true,
            shader: shade(style.colour, style.opacity),
            ..Paint::default()
        };
        let stroke = Stroke {
            width: 2.5 * unit,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), clip);
    }

    /// The mark beside one item of a list, in the item's own colour and size.
    ///
    /// It hangs outside the item, as `list-style-position: outside` does, so a
    /// list with no padding along the text direction paints its marks off its
    /// own edge — which is what a browser does with it too.
    fn marker(
        &mut self,
        canvas: &mut Pixmap,
        kind: Marker,
        item: u32,
        index: usize,
        x: f32,
        y: f32,
        clip: Option<&Mask>,
    ) {
        let (Some(style), Some(&Some(id))) = (self.styles.get(index), self.ids.get(index))
        else {
            return;
        };
        let Ok(layout) = self.tree.layout(id) else { return };
        let (across, down) = shift(style, layout.size);
        let left = x + layout.location.x + across;
        let top = y + layout.location.y + down;
        let gap = style.font_size * MARKER_GAP;
        match kind {
            Marker::None => {}
            Marker::Disc => {
                // Centred on the item's first line, which is where a reader
                // looks for it.
                let size = style.font_size * MARKER_DISC;
                let middle = top + style.font_size * style.line_height / 2.0;
                let box_ = Box2 {
                    l: px(left - gap - size),
                    t: px(middle - size / 2.0),
                    r: px(left - gap),
                    b: px(middle + size / 2.0),
                };
                fill(canvas, box_, [size / 2.0; 4], style.colour, style.opacity, clip);
            }
            Marker::Decimal => {
                let text = format!("{item}.");
                let (width, height) = extent(&shape(self.fonts, &text, style, None, Wrap::WordOrGlyph));
                let box_ = Box2 {
                    l: px(left - gap - width),
                    t: px(top),
                    r: px(left - gap),
                    b: px(top + height),
                };
                let style = style.clone();
                self.text(canvas, &text, &style, box_, width, clip);
            }
        }
    }

    /// Draws one text run at the box the layout gave it.
    ///
    /// **The ink's alpha is applied here, not by the shaper.** `cosmic_text`
    /// rasterizes a glyph to a coverage mask and hands the callback that
    /// coverage with the ink's red, green and blue only — the alpha it was
    /// given never reaches the pixel, so a translucent foreground used to
    /// paint at full strength beside a background that had faded correctly.
    /// `colour[3]` is the colour's alpha times the element's opacity already,
    /// and the coverage is multiplied by it.
    fn text(
        &mut self,
        canvas: &mut Pixmap,
        text: &str,
        style: &Computed,
        box_: Box2,
        width: f32,
        clip: Option<&Mask>,
    ) {
        let mut buffer = shape(self.fonts, text, style, Some(width), Wrap::WordOrGlyph);
        let colour = premultiply(style.colour, style.opacity);
        if colour[3] == 0 {
            return;
        }
        let alpha = colour[3];
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
                    let a = mul255(mul255(pixel.a(), alpha), coverage);
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
            fill(canvas, bar, [0.0; 4], style.colour, style.opacity, clip);
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

    /// The same box, pulled in by one distance per side: the inline start and
    /// end, then the block start and end, which is `Computed::padding`'s order.
    fn shrink(self, by: [f32; 4]) -> Self {
        Self {
            l: self.l.saturating_add(px(by[0])),
            t: self.t.saturating_add(px(by[2])),
            r: self.r.saturating_sub(px(by[1])),
            b: self.b.saturating_sub(px(by[3])),
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

    fn path(self, radii: [f32; 4]) -> Option<tiny_skia::Path> {
        self.inset_path(0.0, radii)
    }

    /// The same path, pulled `by` device pixels in on every side, in floating
    /// point.
    ///
    /// A stroke's centreline is half a width in, and half of an odd width is
    /// half a pixel — a number [`px`] has no room for. Rounding it is what put
    /// a one-pixel border astride the box's edge, so the inset is applied to
    /// the edges rather than to the box.
    fn inset_path(self, by: f32, radii: [f32; 4]) -> Option<tiny_skia::Path> {
        rounded(self.l as f32 + by, self.t as f32 + by, self.r as f32 - by, self.b as f32 - by, radii)
    }
}

/// Where a `transform: translate` puts a box, against the size it was laid out
/// at. Applied after the layout, so no sibling moves — which is the whole
/// reason a control presses by a pixel this way rather than with a padding.
fn shift(style: &Computed, size: Size<f32>) -> (f32, f32) {
    style.translate.map_or((0.0, 0.0), |(across, down)| {
        (resolve_length(across, size.width), resolve_length(down, size.height))
    })
}

/// A rounded rectangle, in floating point, with a radius per corner.
///
/// The corners run start-start, start-end, end-start, end-end — top-left,
/// top-right, bottom-left, bottom-right on a left-to-right page — which is the
/// order `ui/style`'s `Corner` declares them in.
fn rounded(l: f32, t: f32, r: f32, b: f32, radii: [f32; 4]) -> Option<tiny_skia::Path> {
    if r <= l || b <= t {
        return None;
    }
    let radii = clamp_radii(radii, r - l, b - t);
    let mut path = PathBuilder::new();
    if radii.iter().all(|v| *v <= 0.0) {
        path.push_rect(tiny_skia::Rect::from_ltrb(l, t, r, b)?);
        return path.finish();
    }
    // A quarter circle as one cubic; `K` is the classic control-point
    // fraction, and it is a constant so both platforms draw the same arc.
    const K: f32 = 0.552_285;
    let [tl, tr, bl, br] = radii;
    path.move_to(l + tl, t);
    path.line_to(r - tr, t);
    path.cubic_to(r - tr + tr * K, t, r, t + tr - tr * K, r, t + tr);
    path.line_to(r, b - br);
    path.cubic_to(r, b - br + br * K, r - br + br * K, b, r - br, b);
    path.line_to(l + bl, b);
    path.cubic_to(l + bl - bl * K, b, l, b - bl + bl * K, l, b - bl);
    path.line_to(l, t + tl);
    path.cubic_to(l, t + tl - tl * K, l + tl - tl * K, t, l + tl, t);
    path.close();
    path.finish()
}

/// CSS's own overlapping-radii rule: where two radii on one side add up to
/// more than that side is long, every radius is scaled by the same factor
/// until none of the four sides is over-subscribed.
///
/// Scaling all four together rather than clipping each one is what keeps a box
/// with one big corner and one small one looking like the browser's.
fn clamp_radii(radii: [f32; 4], width: f32, height: f32) -> [f32; 4] {
    let mut out = radii.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
    let [tl, tr, bl, br] = out;
    let mut factor = 1.0_f32;
    for (side, sum) in [(width, tl + tr), (width, bl + br), (height, tl + bl), (height, tr + br)] {
        if sum > 0.0 {
            factor = factor.min(side / sum);
        }
    }
    if factor < 1.0 {
        for v in &mut out {
            *v *= factor;
        }
    }
    out
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
    radii: [f32; 4],
    colour: Rgba,
    opacity: f32,
    clip: Option<&Mask>,
) {
    let Some(path) = box_.path(radii) else { return };
    let paint =
        Paint { anti_alias: true, shader: shade(colour, opacity), ..Paint::default() };
    canvas.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), clip);
}

fn stroke(
    canvas: &mut Pixmap,
    box_: Box2,
    radii: [f32; 4],
    width: f32,
    style: &Computed,
    clip: Option<&Mask>,
) {
    // A CSS border sits inside the box, so the centreline is half a width in —
    // a half pixel for an odd width, which is why this is not a `grow`.
    let half = width / 2.0;
    let inner = radii.map(|r| (r - half).max(0.0));
    let Some(path) = box_.inset_path(half, inner) else { return };
    let paint = Paint {
        anti_alias: true,
        shader: shade(style.border_colour.unwrap_or(style.colour), style.opacity),
        ..Paint::default()
    };
    let mut pen = Stroke { width, ..Stroke::default() };
    if style.border_style.first() == Some(&Border::Dashed) {
        pen.dash = StrokeDash::new(vec![width * 3.0, width * 2.0], 0.0);
    }
    canvas.stroke_path(&path, &paint, &pen, Transform::identity(), clip);
}

/// A border whose edges are not all the same, painted as the region between
/// the border box and the padding box.
///
/// The padding box is inset by each edge's *own* width, so an edge with no
/// width takes nothing out of the ring and paints nothing, while the edges
/// that are there keep their corners. That the ring is one shape rather than
/// four bands is why this works at all: `BorderColor` is whole-box, so the
/// mitre a browser draws between two edges is invisible and there is nothing
/// to divide.
///
/// A dashed border is the one thing a fill cannot say, so each present edge is
/// stroked along its own centreline instead, with the ring as its clip.
fn stroke_edges(
    canvas: &mut Pixmap,
    box_: Box2,
    radii: [f32; 4],
    widths: [f32; 4],
    style: &Computed,
    clip: Option<&Mask>,
) {
    let (width, height) = (canvas.width(), canvas.height());
    let (Some(outer), Some(mut ring)) = (box_.path(radii), Mask::new(width, height)) else {
        return;
    };
    ring.fill_path(&outer, FillRule::Winding, true, Transform::identity());
    let [l, t, r, b] = [box_.l as f32, box_.t as f32, box_.r as f32, box_.b as f32];
    let inner_radii = [
        (radii[0] - (widths[0] + widths[2]) / 2.0).max(0.0),
        (radii[1] - (widths[1] + widths[2]) / 2.0).max(0.0),
        (radii[2] - (widths[0] + widths[3]) / 2.0).max(0.0),
        (radii[3] - (widths[1] + widths[3]) / 2.0).max(0.0),
    ];
    let inner = rounded(l + widths[0], t + widths[2], r - widths[1], b - widths[3], inner_radii);
    if let (Some(inner), Some(mut hole)) = (inner, Mask::new(width, height)) {
        hole.fill_path(&inner, FillRule::Winding, true, Transform::identity());
        for coverage in hole.data_mut() {
            *coverage = 255 - *coverage;
        }
        narrow(&mut ring, &hole);
    }
    if let Some(outside) = clip {
        narrow(&mut ring, outside);
    }
    let paint = Paint {
        anti_alias: true,
        shader: shade(style.border_colour.unwrap_or(style.colour), style.opacity),
        ..Paint::default()
    };
    let dashed = |edge: usize| style.border_style.get(edge) == Some(&Border::Dashed);
    if (0..4).any(dashed) {
        for (edge, &pen_width) in widths.iter().enumerate() {
            if pen_width <= 0.0 {
                continue;
            }
            let half = pen_width / 2.0;
            let mut line = PathBuilder::new();
            match edge {
                0 => {
                    line.move_to(l + half, t);
                    line.line_to(l + half, b);
                }
                1 => {
                    line.move_to(r - half, t);
                    line.line_to(r - half, b);
                }
                2 => {
                    line.move_to(l, t + half);
                    line.line_to(r, t + half);
                }
                _ => {
                    line.move_to(l, b - half);
                    line.line_to(r, b - half);
                }
            }
            let Some(line) = line.finish() else { continue };
            let mut pen = Stroke { width: pen_width, ..Stroke::default() };
            if dashed(edge) {
                pen.dash = StrokeDash::new(vec![pen_width * 3.0, pen_width * 2.0], 0.0);
            }
            canvas.stroke_path(&line, &paint, &pen, Transform::identity(), Some(&ring));
        }
        return;
    }
    canvas.fill_path(&outer, &paint, FillRule::Winding, Transform::identity(), Some(&ring));
}

/// The frame around a picture the painter could not read.
const PLACEHOLDER_EDGE: Rgba = Rgba { r: 153, g: 153, b: 153, a: 1.0 };

/// What is inside that frame.
const PLACEHOLDER_FILL: Rgba = Rgba { r: 224, g: 224, b: 224, a: 1.0 };

/// The pixels or the shapes of a picture, or `None` for a source this painter
/// cannot read. An icon's artwork is always read: it is in the scene.
///
/// **Nothing is fetched and nothing is opened.** A snapshot that reached the
/// network would answer a different picture on a different day, so the only
/// source that can be read is the one the scene carries whole: a `data:` URI
/// holding a PNG or an SVG. Every other source — an `http` URL, a path, a
/// media type this does not decode — is a placeholder, and [`Painter::picture`]
/// paints it as one.
fn picture(art: &Art) -> Option<Picture> {
    match art {
        Art::Source(source) => image::read(source),
        Art::Artwork(source) => image::artwork(source),
    }
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
fn intersect(mask: &mut Mask, box_: Box2, radii: [f32; 4]) {
    if let Some(path) = box_.path(radii) {
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
    radii: [f32; 4],
    shadow: Shadow,
    opacity: f32,
    clip: Option<&Mask>,
) {
    let (width, height) = (canvas.width(), canvas.height());
    let (Some(mut mask), Some(path)) = (Mask::new(width, height), cast.path(radii)) else {
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
    let Some(path) = all.path([0.0; 4]) else { return };
    let paint =
        Paint { anti_alias: false, shader: shade(shadow.colour, opacity), ..Paint::default() };
    canvas.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), Some(&mask));
}

/// The clip an outer shadow paints under: what the caller was already clipped
/// to, minus the element's own border box.
///
/// CSS clips an outer `box-shadow` to the region outside the border box, so a
/// shadow is never under the box that cast it. `None` means nothing could be
/// allocated, and the caller keeps its own clip.
fn outside_the_box(
    box_: Box2,
    radii: [f32; 4],
    clip: Option<&Mask>,
    width: u32,
    height: u32,
) -> Option<Mask> {
    let mut mask = clip.cloned().or_else(|| full_mask(width, height))?;
    let Some(path) = box_.path(radii) else { return Some(mask) };
    let mut hole = Mask::new(width, height)?;
    hole.fill_path(&path, FillRule::Winding, true, Transform::identity());
    for coverage in hole.data_mut() {
        *coverage = 255 - *coverage;
    }
    narrow(&mut mask, &hole);
    Some(mask)
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
/// `squareRoot` of a constant times a length both platforms already agree on.
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::image::Image;
    use super::*;

    /// A scene with a viewport and nothing in it.
    const EMPTY: &str = "buri-scene 1\nviewport 4 3\n";

    fn render_ok(scene: &str, sheet: &str, state: &str) -> Image {
        let png = render(&Request { scene, stylesheet: sheet, state, variables: "" }).unwrap();
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
        let error = render(&Request { scene: "viewport 4 3\n", stylesheet: "", state: "rest", variables: "" });
        assert_eq!(error, Err("the scene does not start with `buri-scene 1`".to_string()));
    }

    #[test]
    fn a_scene_without_a_viewport_is_refused() {
        let scene = "buri-scene 1\ne 0 \n";
        let error = render(&Request { scene, stylesheet: "", state: "rest", variables: "" }).unwrap_err();
        assert!(error.contains("is not a viewport"), "{error}");
    }

    #[test]
    fn a_viewport_of_zero_is_refused() {
        let scene = "buri-scene 1\nviewport 0 3\n";
        let error = render(&Request { scene, stylesheet: "", state: "rest", variables: "" }).unwrap_err();
        assert!(error.contains("0x3"), "{error}");
    }

    #[test]
    fn a_depth_that_jumps_is_refused() {
        let scene = "buri-scene 1\nviewport 4 3\ne 0 \ne 2 \n";
        let error = render(&Request { scene, stylesheet: "", state: "rest", variables: "" }).unwrap_err();
        assert!(error.contains("jumps from depth 1 to 2"), "{error}");
    }

    #[test]
    fn a_line_that_is_neither_e_nor_t_is_refused() {
        let scene = "buri-scene 1\nviewport 4 3\nx 0 \n";
        let error = render(&Request { scene, stylesheet: "", state: "rest", variables: "" }).unwrap_err();
        assert!(error.contains("neither an `e` nor a `t`"), "{error}");
    }

    #[test]
    fn a_declaration_without_a_colon_is_refused() {
        let scene = "buri-scene 1\nviewport 4 3\ne 0 padding\n";
        let error = render(&Request { scene, stylesheet: "", state: "rest", variables: "" }).unwrap_err();
        assert!(error.contains("has no `:`"), "{error}");
    }

    #[test]
    fn a_child_of_a_text_run_is_refused() {
        let scene = "buri-scene 1\nviewport 4 3\nt 0 Ada\ne 1 \n";
        let error = render(&Request { scene, stylesheet: "", state: "rest", variables: "" }).unwrap_err();
        assert!(error.contains("a text run has none"), "{error}");
    }

    #[test]
    fn a_state_the_painter_does_not_know_is_refused() {
        let error = render(&Request { scene: EMPTY, stylesheet: "", state: "wiggly", variables: "" });
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

    /// A bleed is a negative margin, so the child starts outside the padding
    /// its parent put it inside.
    #[test]
    fn a_negative_margin_takes_a_child_out_past_the_padding() {
        let scene = "buri-scene 1\nviewport 20 20\n\
                     e 0 padding:4px;width:20px;height:20px\n\
                     e 1 width:4px;height:4px;margin-inline-start:-4px;\
                     background-color:rgb(255,0,0)\n";
        let image = render_ok(scene, "", "rest");
        // Four of padding, taken back by four: the child starts at the edge.
        assert_eq!(at(&image, 0, 4), [255, 0, 0, 255]);
        assert_eq!(at(&image, 3, 4), [255, 0, 0, 255]);
        assert_eq!(at(&image, 4, 4), [255, 255, 255, 255]);
    }

    /// Two siblings that overlap paint the way the document orders them: the
    /// later one covers the earlier one.
    #[test]
    fn a_later_sibling_is_painted_over_the_one_it_laps() {
        let scene = "buri-scene 1\nviewport 20 20\n\
                     e 0 flex-direction:row;width:20px;height:20px\n\
                     e 1 width:8px;height:8px;background-color:rgb(255,0,0)\n\
                     e 1 width:8px;height:8px;margin-inline-start:-4px;\
                     background-color:rgb(0,255,0)\n";
        let image = render_ok(scene, "", "rest");
        // The first four columns are the first box, and the four it lost are
        // the second one over it.
        assert_eq!(at(&image, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&image, 3, 0), [255, 0, 0, 255]);
        assert_eq!(at(&image, 4, 0), [0, 255, 0, 255]);
        assert_eq!(at(&image, 11, 0), [0, 255, 0, 255]);
        assert_eq!(at(&image, 12, 0), [255, 255, 255, 255]);
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
            let plain = render(&Request { scene, stylesheet: "", state: "rest", variables: "" }).unwrap();
            let deeper = wrapped(scene);
            let nested =
                render(&Request { scene: &deeper, stylesheet: "", state: "rest", variables: "" }).unwrap();
            assert_eq!(plain, nested, "wrapping this scene changed it:\n{deeper}");
        }
    }

    /// The first column holding ink, or `None` for a blank picture.
    fn first_inked_column(image: &Image) -> Option<u32> {
        (0..image.width)
            .find(|&x| (0..image.height).any(|y| at(image, x, y) != [255, 255, 255, 255]))
    }

    /// The last column holding ink, or `None` for a blank picture.
    fn last_inked_column(image: &Image) -> Option<u32> {
        (0..image.width)
            .rev()
            .find(|&x| (0..image.height).any(|y| at(image, x, y) != [255, 255, 255, 255]))
    }

    fn inked_pixels(image: &Image) -> usize {
        (0..image.height)
            .flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .filter(|&(x, y)| at(image, x, y) != [255, 255, 255, 255])
            .count()
    }

    /// The darkest pixel in a picture, as its red channel. Everything here is
    /// ink over a lighter ground, so a smaller number is more of it.
    fn darkest(image: &Image) -> u8 {
        darkest_pixel(image)[0]
    }

    /// The whole of that pixel, for a test that reads a composited colour
    /// rather than an amount of ink.
    fn darkest_pixel(image: &Image) -> [u8; 4] {
        (0..image.height)
            .flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .map(|(x, y)| at(image, x, y))
            .min_by_key(|p| p[0])
            .unwrap_or([255; 4])
    }

    /// One entry per band of consecutive inked rows — one line of text — as
    /// the columns its ink runs between.
    fn inked_lines(image: &Image) -> Vec<(u32, u32)> {
        let mut lines: Vec<(u32, u32)> = Vec::new();
        let mut open = false;
        for y in 0..image.height {
            let mut span: Option<(u32, u32)> = None;
            for x in 0..image.width {
                if at(image, x, y) != [255, 255, 255, 255] {
                    span = Some(match span {
                        None => (x, x),
                        Some((from, _)) => (from, x),
                    });
                }
            }
            match span {
                None => open = false,
                Some((from, to)) => {
                    match lines.last_mut().filter(|_| open) {
                        Some(line) => *line = (line.0.min(from), line.1.max(to)),
                        None => lines.push((from, to)),
                    }
                    open = true;
                }
            }
        }
        lines
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
        let one = render(&Request { scene, stylesheet: "", state: "rest", variables: "" }).unwrap();
        let two = render(&Request { scene, stylesheet: "", state: "rest", variables: "" }).unwrap();
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

    /// A border sits inside the box, whatever its width: a one-pixel border is
    /// one solid row on the box's own first row, and nothing above it.
    #[test]
    fn an_odd_border_width_paints_solid_rows_inside_the_box() {
        let scene = "buri-scene 1\nviewport 20 24\ne 0 padding:4px\n\
                     e 1 width:12px;height:12px;border-style:solid;border-width:1px;\
                     border-color:rgb(0,0,0)\n";
        let one = render_ok(scene, "", "rest");
        assert_eq!(at(&one, 10, 3), [255, 255, 255, 255], "a border paints outside its box");
        assert_eq!(at(&one, 10, 4), [0, 0, 0, 255], "a one-pixel border is one solid row");
        assert_eq!(at(&one, 10, 5), [255, 255, 255, 255]);
        assert_eq!(at(&one, 10, 15), [0, 0, 0, 255], "the box's last row is the border's");

        // Three is the same rule, three rows in: the row above the box is
        // untouched and the three inside it are solid.
        let scene = scene.replace("border-width:1px", "border-width:3px");
        let three = render_ok(&scene, "", "rest");
        assert_eq!(at(&three, 10, 3), [255, 255, 255, 255]);
        for y in 4..7 {
            assert_eq!(at(&three, 10, y), [0, 0, 0, 255], "row {y} of a three-pixel border");
        }
        assert_eq!(at(&three, 10, 7), [255, 255, 255, 255]);
    }

    /// A border with no colour of its own draws in the element's foreground,
    /// which is CSS's `currentColor` — and the `color` beside it may be
    /// written after the border.
    #[test]
    fn a_border_with_no_colour_draws_in_the_foreground() {
        let scene = "buri-scene 1\nviewport 20 20\ne 0 padding:4px\n\
                     e 1 width:12px;height:12px;border-style:solid;border-width:4px;\
                     color:rgb(18,18,28)\n";
        assert_eq!(at(&render_ok(scene, "", "rest"), 5, 10), [18, 18, 28, 255]);
    }

    /// An outer shadow paints outside the border box only, so a spread-only
    /// ring around a transparent box leaves the box's own pixels alone.
    #[test]
    fn an_outer_shadow_paints_outside_the_box_it_was_cast_from() {
        let scene = "buri-scene 1\nviewport 24 24\ne 0 padding:6px\n\
                     e 1 width:12px;height:12px;box-shadow:0px 0px 0px 3px rgb(150,150,150)\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!(at(&image, 4, 12), [150, 150, 150, 255], "the ring is three pixels out");
        assert_eq!(at(&image, 12, 12), [255, 255, 255, 255], "a ring flooded the control");
        assert_eq!(at(&image, 6, 6), [255, 255, 255, 255], "the box's own corner");
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

    /// `.LineHeight(0.0)` is a style a program may write and `0` is a value CSS
    /// accepts, so the painter has to have an answer for it. It had none: the
    /// shaper asserts a non-zero line height, so every snapshot in a suite that
    /// wrote one aborted with a Rust panic instead of a picture.
    #[test]
    fn a_line_height_of_zero_still_paints_the_run() {
        let scene = "buri-scene 1\nviewport 60 40\ne 0 line-height:0;font-size:12px\nt 1 Ada\n";
        assert!(inked_pixels(&render_ok(scene, "", "rest")) > 0);
    }

    /// The size takes the same floor, and has since it was written. Here so
    /// that the two halves of one policy are read together. A one-pixel face
    /// may put no ink on the canvas at all, so what this asserts is the
    /// picture, not the paint.
    #[test]
    fn a_font_size_of_zero_still_paints_a_page() {
        let scene = "buri-scene 1\nviewport 60 40\ne 0 font-size:0px\nt 1 Ada\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!((image.width, image.height), (60, 40));
    }

    /// **The alpha a colour carries reaches the glyphs, not only the boxes.**
    /// The shaper hands the painter a glyph's own coverage and drops the ink
    /// colour's alpha, so a half transparent foreground used to paint hard
    /// black text on a card whose background had faded correctly.
    ///
    /// Three tenths of black over `rgb(206,218,240)` is `rgb(144,153,168)`,
    /// and this is the ground and the answer the issue read off a browser. The
    /// tolerance is one byte because the compositing here is integer
    /// arithmetic: the same alpha on a *border* in the same picture reads
    /// `rgb(144,152,167)`, and the glyphs may not be held to a stricter rule
    /// than the fill beside them.
    #[test]
    fn a_translucent_colour_fades_the_text_written_in_it() {
        let ground = "background-color:rgb(206,218,240);width:60px;height:30px";
        let opaque = format!(
            "buri-scene 1\nviewport 60 30\ne 0 {ground};font-size:24px;color:rgb(0,0,0)\n\
             t 1 Ada\n"
        );
        let faded = format!(
            "buri-scene 1\nviewport 60 30\ne 0 {ground};font-size:24px;color:rgba(0,0,0,0.3)\n\
             t 1 Ada\n"
        );
        assert!(darkest(&render_ok(&opaque, "", "rest")) <= 2);
        let ink = darkest_pixel(&render_ok(&faded, "", "rest"));
        for (was, want) in ink.iter().zip([144_u8, 153, 168]) {
            assert!(
                was.abs_diff(want) <= 1,
                "three tenths of black over rgb(206,218,240) is rgb(144,153,168), not {ink:?}"
            );
        }
    }

    /// And so does the `opacity` multiplied into it, which is what the header
    /// says an opacity is: a factor on every colour the subtree paints.
    #[test]
    fn a_fractional_opacity_fades_the_text_under_it() {
        let scene = "buri-scene 1\nviewport 60 30\ne 0 font-size:24px;opacity:0.5\nt 1 Ada\n";
        let faded = darkest(&render_ok(scene, "", "rest"));
        assert!((120..=134).contains(&faded), "half opacity over white is 127, not {faded}");
    }

    /// The underline takes the same alpha, since it is drawn from the same
    /// colour by a different path.
    #[test]
    fn a_translucent_colour_fades_the_underline_too() {
        let scene = "buri-scene 1\nviewport 60 30\n\
                     e 0 font-size:24px;color:rgba(0,0,0,0.5);text-decoration-line:underline\n\
                     t 1 Ada\n";
        let faded = darkest(&render_ok(scene, "", "rest"));
        assert!((120..=134).contains(&faded), "half of black over white is 127, not {faded}");
    }

    /// **A sticky box stays where the flow put it.** A snapshot has nothing to
    /// scroll, so the inset a sticky element carries is a threshold it never
    /// crosses — an unscrolled browser paints it at its static position. It
    /// used to take the inset as a relative offset, the way `Position(.Flow)`
    /// does.
    #[test]
    fn a_sticky_box_stays_where_the_flow_put_it() {
        let scene = "buri-scene 1\nviewport 12 12\n\
                     e 0 width:12px;height:12px\n\
                     e 1 position:sticky;inset-block-start:4px;inset-inline-start:4px;\
                     width:2px;height:2px;background-color:rgb(0,128,0)\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!(at(&image, 0, 0), [0, 128, 0, 255]);
        assert_eq!(at(&image, 5, 5), [255, 255, 255, 255]);
    }

    /// A `relative` box beside it, which is the answer `sticky` used to give,
    /// so the two are read together.
    #[test]
    fn a_relative_box_does_take_the_inset_it_carries() {
        let scene = "buri-scene 1\nviewport 12 12\n\
                     e 0 width:12px;height:12px\n\
                     e 1 position:relative;inset-block-start:4px;inset-inline-start:4px;\
                     width:2px;height:2px;background-color:rgb(0,128,0)\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!(at(&image, 4, 4), [0, 128, 0, 255]);
        assert_eq!(at(&image, 0, 0), [255, 255, 255, 255]);
    }

    /// The sentence every truncation test below breaks: long enough for six
    /// lines in a box eighty wide.
    const PARAGRAPH: &str =
        "A sentence long enough that it has to break somewhere, twice over. A sentence \
         long enough that it has to break somewhere, twice over.";

    /// **`-webkit-line-clamp` shows at most the lines it names.** It is what
    /// `Truncate(n)` lowers to, and nothing read it, so one, two, three and no
    /// clamp at all painted the same six lines.
    #[test]
    fn a_line_clamp_shows_at_most_the_lines_it_names() {
        let lines = |clamp: &str| {
            let scene = format!(
                "buri-scene 1\nviewport 200 200\n\
                 e 0 width:80px;font-size:12px{clamp}\nt 1 {PARAGRAPH}\n"
            );
            inked_lines(&render_ok(&scene, "", "rest")).len()
        };
        let clamp = |n: u32| {
            format!(";display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:{n};overflow:hidden")
        };
        let free = lines("");
        assert!(free > 3, "the sentence has to break more than three times, not {free}");
        assert_eq!(lines(&clamp(1)), 1);
        assert_eq!(lines(&clamp(2)), 2);
        assert_eq!(lines(&clamp(3)), 3);
        // A clamp no shorter than the run is not a truncation.
        assert_eq!(lines(";display:-webkit-box;-webkit-line-clamp:none;overflow:visible"), free);
    }

    /// And the last line it shows ends in an ellipsis, which is the half of
    /// the property a reader can see. A run the clamp did not cut keeps its
    /// own last glyph.
    #[test]
    fn the_last_clamped_line_ends_in_an_ellipsis() {
        let edge = |text: &str| {
            let scene = format!(
                "buri-scene 1\nviewport 200 40\n\
                 e 0 width:40px;font-size:12px;display:-webkit-box;\
                 -webkit-box-orient:vertical;-webkit-line-clamp:1;overflow:hidden\n\
                 t 1 {text}\n"
            );
            last_inked_column(&render_ok(&scene, "", "rest")).unwrap()
        };
        let whole = edge("Ada");
        let cut = edge("Ada bee");
        assert!(cut > whole, "the ellipsis puts ink past `Ada`: {cut} against {whole}");
    }

    /// **`text-wrap: balance` evens the lines out.** It breaks a run into the
    /// number of lines `wrap` gave it, at the narrowest width that still does,
    /// which is how a browser approximates a balanced heading. The painter
    /// read the property as one bit, so `balance` painted byte for byte like
    /// `wrap`.
    #[test]
    fn a_balanced_run_evens_its_lines_out_without_adding_one() {
        let picture = |mode: &str| {
            let scene = format!(
                "buri-scene 1\nviewport 200 120\n\
                 e 0 width:180px;font-size:13px;text-wrap:{mode}\n\
                 t 1 A sentence long enough that it has to break somewhere, twice over.\n"
            );
            render_ok(&scene, "", "rest")
        };
        let spread = |image: &Image| {
            let widths: Vec<u32> =
                inked_lines(image).iter().map(|&(from, to)| to.saturating_sub(from)).collect();
            let (top, bottom) = (widths.iter().max().copied(), widths.iter().min().copied());
            top.unwrap_or(0).saturating_sub(bottom.unwrap_or(0))
        };
        let wrapped = picture("wrap");
        let balanced = picture("balance");
        assert_eq!(
            inked_lines(&wrapped).len(),
            inked_lines(&balanced).len(),
            "balancing may not cost a line"
        );
        assert!(
            spread(&balanced) < spread(&wrapped),
            "balanced lines are closer in length: {} against {}",
            spread(&balanced),
            spread(&wrapped)
        );
    }

    /// A run that already fits on one line has nothing to balance, so it does
    /// not move.
    #[test]
    fn a_balanced_run_of_one_line_paints_where_it_did() {
        let one = "buri-scene 1\nviewport 200 40\ne 0 width:180px;font-size:13px\nt 1 Ada\n";
        let two = "buri-scene 1\nviewport 200 40\n\
                   e 0 width:180px;font-size:13px;text-wrap:balance\nt 1 Ada\n";
        let render = |scene: &str| {
            render(&Request { scene, stylesheet: "", state: "rest", variables: "" }).unwrap()
        };
        assert_eq!(render(one), render(two));
    }

    /// A source the painter cannot read — an address is the common one, since
    /// nothing here fetches — is a framed grey box at the size the scene gave
    /// it. It used to be nothing at all: an image line carried no source, so a
    /// row of icons painted as blank page.
    #[test]
    fn a_source_the_painter_cannot_read_paints_a_placeholder() {
        let scene = "buri-scene 1\nviewport 8 8\n\
                     e 0 width:6px;height:6px\n\
                     e 1 image:https://example.com/logo.svg\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!(at(&image, 0, 0), [153, 153, 153, 255]);
        assert_eq!(at(&image, 3, 3), [224, 224, 224, 255]);
        // The box the scene declared, and not a pixel past it.
        assert_eq!(at(&image, 6, 6), [255, 255, 255, 255]);
    }

    /// One base64 alphabet, one PNG reader, and the pixels come back where the
    /// layout put them.
    #[test]
    fn a_png_data_uri_paints_its_own_pixels() {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let mut pixels = Vec::new();
        pixels.extend_from_slice(&red);
        pixels.extend_from_slice(&blue);
        // The semicolon in the media type is escaped, as `describe` writes it.
        let source = format!("data:image/png\\;base64,{}", to_base64(&encode(2, 1, &pixels)));
        let scene = format!("buri-scene 1\nviewport 8 8\ne 0 image:{source}\n");
        let image = render_ok(&scene, "", "rest");
        // Its own size, since nothing declared one.
        assert_eq!(at(&image, 0, 0), red);
        assert_eq!(at(&image, 1, 0), blue);
        assert_eq!(at(&image, 0, 1), [255, 255, 255, 255]);
    }

    /// The box wins over the pixels: a source that was read is scaled into
    /// whatever the layout gave it.
    #[test]
    fn a_picture_scales_to_the_box_the_layout_gave_it() {
        let mut pixels = Vec::new();
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        pixels.extend_from_slice(&[0, 0, 255, 255]);
        let source = format!("data:image/png\\;base64,{}", to_base64(&encode(2, 1, &pixels)));
        let scene =
            format!("buri-scene 1\nviewport 8 8\ne 0 width:8px;height:2px;image:{source}\n");
        let image = render_ok(&scene, "", "rest");
        assert_eq!(at(&image, 0, 1), [255, 0, 0, 255]);
        assert_eq!(at(&image, 7, 1), [0, 0, 255, 255]);
    }

    /// A data URI is full of semicolons, and a semicolon separates two
    /// declarations. `describe` escapes both, and this is the other end of it.
    #[test]
    fn a_declaration_value_may_hold_an_escaped_semicolon() {
        let (classes, declarations) = parse_declarations(r"image:a\;b;width:4px").unwrap();
        assert!(classes.is_empty());
        assert_eq!(
            declarations,
            vec![
                ("image".to_string(), "a;b".to_string()),
                ("width".to_string(), "4px".to_string()),
            ]
        );
    }

    /// `position: fixed` is measured against the viewport, not against the box
    /// it was written in — a dock pinned to the bottom right belongs at the
    /// page's bottom right however deep in the tree it was declared.
    #[test]
    fn a_fixed_box_is_pinned_to_the_viewport() {
        let scene = "buri-scene 1\nviewport 10 10\n\
                     e 0 width:4px;height:4px\n\
                     e 1 position:fixed;inset-block-end:0px;inset-inline-end:0px;\
                     width:2px;height:2px;background-color:rgb(0,128,0)\n";
        let image = render_ok(scene, "", "rest");
        assert_eq!(at(&image, 9, 9), [0, 128, 0, 255]);
        assert_eq!(at(&image, 0, 0), [255, 255, 255, 255]);
    }

    /// It also leaves the flow where it was written: the box around it lays
    /// out as though the fixed child were not there.
    #[test]
    fn a_fixed_box_takes_no_room_where_it_was_written() {
        let without = "buri-scene 1\nviewport 10 10\n\
                       e 0 background-color:rgb(255,0,0)\n\
                       t 1 x\n";
        let with = "buri-scene 1\nviewport 10 10\n\
                    e 0 background-color:rgb(255,0,0)\n\
                    t 1 x\n\
                    e 1 position:fixed;width:2px;height:9px\n";
        let rows = |scene: &str| {
            let image = render_ok(scene, "", "rest");
            (0..10).filter(|&y| at(&image, 9, y) == [255, 0, 0, 255]).count()
        };
        assert_eq!(rows(without), rows(with));
    }

    /// The base64 alphabet, written out for the two tests above. Nothing in the
    /// painter encodes one — a scene arrives with its sources already written.
    fn to_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut block = [0_u8; 3];
            if let Some(slot) = block.get_mut(..chunk.len()) {
                slot.copy_from_slice(chunk);
            }
            let n = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    let sextet = (n >> (18 - 6 * i)) & 0x3f;
                    out.push(char::from(ALPHABET[sextet as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
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
    fn a_child_rule_lands_on_the_children_and_not_on_the_box_that_names_it() {
        let scene = "buri-scene 1\nviewport 6 4\ne 0 class:lay\ne 1 width:2px;height:2px\n";
        let sheet = ".lay{width:4px;height:2px;background-color:rgb(0,0,255)}\n\
                     .lay>*{background-color:rgb(255,0,0)}\n";
        let image = render_ok(scene, sheet, "rest");
        // The child took the rule; the box that carries the class did not.
        assert_eq!(at(&image, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&image, 3, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn a_descendant_rule_that_is_not_the_child_one_is_still_skipped() {
        let scene = "buri-scene 1\nviewport 6 4\ne 0 class:lay\ne 1 width:4px;height:2px\n";
        let sheet = ".lay p{background-color:rgb(255,0,0)}\n\
                     .lay *{background-color:rgb(0,255,0)}\n\
                     .lay>p{background-color:rgb(0,0,255)}\n";
        assert_eq!(at(&render_ok(scene, sheet, "rest"), 0, 0), [255, 255, 255, 255]);
    }

    /// buri#77: `.lay-layers{display:grid}` with `.lay-layers>*{grid-area:1/1}`
    /// beside it is how `Layout(.Layers)` is written, and the pair has to put
    /// every child in the same cell rather than in a column of its own rows.
    #[test]
    fn layered_children_share_one_cell() {
        let scene = "buri-scene 1\nviewport 40 20\n\
                     e 0 class:lay-layers;width:40px;height:20px\n\
                     e 1 width:30px;height:15px;background-color:rgb(200,40,40)\n\
                     e 1 width:20px;height:10px;background-color:rgb(40,150,40)\n\
                     e 1 width:10px;height:5px;background-color:rgb(40,40,200)\n";
        let sheet = ".lay-layers{display:grid}\n.lay-layers>*{grid-area:1/1}\n";
        let image = render_ok(scene, sheet, "rest");
        // One origin, and the order they were written in is the order they
        // stack in: the smallest is whole, the largest is only what shows.
        assert_eq!(at(&image, 0, 0), [40, 40, 200, 255]);
        assert_eq!(at(&image, 15, 7), [40, 150, 40, 255]);
        assert_eq!(at(&image, 25, 12), [200, 40, 40, 255]);
        // 15 tall, not 30: three boxes in one cell, never one under another.
        assert_eq!(at(&image, 0, 16), [255, 255, 255, 255]);
    }

    /// buri#89: a snapshot of a password field used to hold the secret as
    /// ordinary text, so `buri test --update` wrote it into a file somebody
    /// commits.
    #[test]
    fn a_password_field_paints_bullets_and_never_the_value() {
        let field = |kind: &str, value: &str| {
            format!(
                "buri-scene 1\nviewport 80 24\n\
                 e 0 field:{kind};font-size:12px\n\
                 t 1 {value}\n"
            )
        };
        let secret = render_ok(&field("password", "Ada"), "", "rest");
        let bullets = render_ok(&field("text", "\u{2022}\u{2022}\u{2022}"), "", "rest");
        let clear = render_ok(&field("text", "Ada"), "", "rest");
        // What a browser draws for `<input type="password">`, one per character.
        assert_eq!(secret.rgba, bullets.rgba);
        // And not the secret: the two are different pictures, and the masked
        // one has ink in it, so "no glyph of Ada" is not "nothing at all".
        assert_ne!(secret.rgba, clear.rgba);
        assert!(inked_pixels(&secret) > 0, "the mask painted nothing");
    }

    /// The other half of the kind, and the only other one this sheet leaves
    /// visible: an `<input>` is one line whatever is typed into it, and a
    /// `textarea` is the one kind that wraps.
    #[test]
    fn only_a_multiline_field_wraps_its_value() {
        let field = |kind: &str| {
            format!(
                "buri-scene 1\nviewport 60 40\n\
                 e 0 field:{kind};width:40px;font-size:12px\n\
                 t 1 one two three four\n"
            )
        };
        let one_line = render_ok(&field("text"), "", "rest");
        let wrapped = render_ok(&field("multiline"), "", "rest");
        assert_ne!(one_line.rgba, wrapped.rgba);
        // The single line runs past the forty pixels the box was given; the
        // wrapped one does not reach the bottom of the viewport on one line.
        assert!(inked_pixels(&wrapped) > inked_pixels(&one_line));
    }

    /// One slider, at a width and height a browser's own reset gives it.
    fn slider(bounds: &str) -> String {
        format!(
            "buri-scene 1\nviewport 200 40\n\
             e 0 field:range;range:{bounds};width:192px;height:16px;color:rgb(0,0,0)\n"
        )
    }

    /// Whether a pixel has anything but the page under it.
    fn inked(image: &Image, x: u32, y: u32) -> bool {
        at(image, x, y) != [255, 255, 255, 255]
    }

    /// buri#139: a range is a slider, and a slider is a track with a thumb on
    /// it at the value. Both are what the sheet's own reset paints in a
    /// browser, so the picture and the page agree.
    #[test]
    fn a_range_paints_a_track_and_a_thumb_where_the_value_is() {
        let low = render_ok(&slider("0.0 100.0 0"), "", "rest");
        let middle = render_ok(&slider("0.0 100.0 50"), "", "rest");
        let high = render_ok(&slider("0.0 100.0 100"), "", "rest");
        // The thumb travels, and never off either end: at the extremes its
        // centre is half a thumb inside the track, which is where a browser
        // stops it so a slider at nought is still a whole disc. Row two is
        // above the bar, so only the thumb can reach it.
        for (image, near, at_all) in
            [(&low, 8, 96), (&middle, 96, 184), (&high, 184, 8)]
        {
            assert!(inked(image, near, 2), "the thumb is not where the value is");
            assert!(!inked(image, at_all, 2), "the thumb is where the value is not");
        }
        // The track is there whatever the value: the middle row is inked end to
        // end in all three, and the bar is a quarter of the sixteen pixels, so
        // one either side of the middle is ink and four is not. Column forty is
        // clear of the thumb in every one of them.
        for image in [&low, &middle, &high] {
            assert!(inked(image, 0, 8) && inked(image, 191, 8));
            assert!(inked(image, 40, 6) && inked(image, 40, 9));
            assert!(!inked(image, 40, 5) && !inked(image, 40, 10));
        }
    }

    /// HTML's own sanitization of a `value` attribute, which is what a browser
    /// applies to the same markup: a number outside the bounds is clamped into
    /// them, and text that is not a number is the middle.
    #[test]
    fn a_range_clamps_what_it_is_given_and_centres_what_it_cannot_read() {
        let under = render_ok(&slider("0.0 100.0 -40"), "", "rest");
        let over = render_ok(&slider("0.0 100.0 400"), "", "rest");
        let words = render_ok(&slider("0.0 100.0 loud"), "", "rest");
        assert_eq!(under.rgba, render_ok(&slider("0.0 100.0 0"), "", "rest").rgba);
        assert_eq!(over.rgba, render_ok(&slider("0.0 100.0 100"), "", "rest").rgba);
        assert_eq!(words.rgba, render_ok(&slider("0.0 100.0 50"), "", "rest").rgba);
        // The bounds are the program's, so the same fraction of a different
        // range is the same picture.
        let shifted = render_ok(&slider("-50.0 50.0 0"), "", "rest");
        assert_eq!(shifted.rgba, render_ok(&slider("0.0 100.0 50"), "", "rest").rgba);
    }

    /// Bounds the painter cannot read leave the box a box, which is the rule an
    /// unreadable image source and an unknown mark both follow.
    #[test]
    fn a_range_whose_bounds_are_not_numbers_paints_nothing_of_its_own() {
        let plain = "buri-scene 1\nviewport 200 40\n\
                     e 0 field:range;width:192px;height:16px;color:rgb(0,0,0)\n";
        let broken = render_ok(&slider("loud louder 3"), "", "rest");
        assert_eq!(broken.rgba, render_ok(plain, "", "rest").rgba);
        assert_eq!(inked_pixels(&broken), 0);
    }

    #[test]
    fn the_same_scene_renders_to_the_same_bytes_twice() {
        let scene = "buri-scene 1\nviewport 60 30\n\
                     e 0 padding:4px;background-color:rgba(20,30,40,0.5);border-radius:3px\n\
                     e 1 font-size:14px;font-weight:700\n\
                     t 2 Ada Lovelace\n";
        let request = Request { scene, stylesheet: "", state: "rest", variables: "" };
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
            image::unfilter(kind, &mut filtered, &previous, 4).unwrap();
            assert_eq!(filtered, line, "filter {kind} did not come back");
        }
    }

    #[test]
    fn a_row_filter_that_does_not_exist_is_refused() {
        let mut row = vec![0_u8; 4];
        assert!(image::unfilter(5, &mut row, &[0; 4], 4).is_err());
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
        assert_eq!(image::inflate(&stream).unwrap(), raw);
    }

    /// Repetition has to actually be spent: a run of one byte is a match, not
    /// four thousand literals.
    #[test]
    fn a_repetitive_stream_comes_out_far_smaller_than_it_went_in() {
        let raw = vec![7_u8; 4096];
        let stream = deflate(&raw);
        assert!(stream.len() < 64, "4096 identical bytes became {} bytes", stream.len());
        assert_eq!(image::inflate(&stream).unwrap(), raw);
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
        assert_eq!(image::inflate(&stream).unwrap(), body);
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

    /// A golden is a file on a disk somebody's editor, archiver or version
    /// control has had its hands on, so half a PNG is a thing a comparison
    /// meets. Every prefix of one comes back as a sentence or as a picture,
    /// never as a panic and never as "these two are equal" — which is the one
    /// wrong answer a snapshot suite could not see.
    #[test]
    fn a_golden_cut_short_is_never_equal_to_the_whole_one() {
        let whole = render(&Request {
            scene: "buri-scene 1\nviewport 24 16\ne 0 background-color:rgb(9,9,9)\n",
            stylesheet: "",
            state: "rest",
            variables: "",
        })
        .unwrap();
        // Every prefix, so no chunk boundary is the only one that was tried.
        for cut in 0..whole.len() {
            match diff(&whole[..cut], &whole) {
                // Refused, and the sentence names what could not be read.
                Err(error) => assert!(error.starts_with("the PNG"), "{cut} bytes: {error}"),
                // Or read, which one prefix is: a file cut just before its
                // `IEND` holds every pixel. It still fails the comparison,
                // because byte equality decides and these are not the same
                // bytes.
                Ok(Some(_)) => {}
                Ok(None) => panic!("a golden cut to {cut} bytes compared equal"),
            }
        }
    }

    /// And a golden the same length as the one that was written, with one byte
    /// of its compressed image changed: either the stream no longer reads, or
    /// it reads and the pictures differ. What it may not be is equal.
    #[test]
    fn a_golden_with_a_byte_changed_is_never_equal() {
        let whole = render(&Request {
            scene: "buri-scene 1\nviewport 24 16\ne 0 background-color:rgb(9,9,9)\n",
            stylesheet: "",
            state: "rest",
            variables: "",
        })
        .unwrap();
        for at in 0..whole.len() {
            let mut broken = whole.clone();
            broken[at] ^= 0xff;
            match diff(&broken, &whole) {
                Ok(Some(_)) | Err(_) => {}
                Ok(None) => panic!("byte {at} changed and the two pictures compared equal"),
            }
        }
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
        let styles = resolve(&scene, &[], State::Rest, &[]);
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
        assert_eq!(parse_selector(".p-8"), Some(("p-8".to_string(), None, false)));
        assert_eq!(
            parse_selector(".p-8:hover"),
            Some(("p-8".to_string(), Some(State::Hover), false))
        );
        let escaped = Some(("hover:bg".to_string(), Some(State::Hover), false));
        assert_eq!(parse_selector(r".hover\:bg:hover"), escaped);
        // The one rule about descendants the sheet writes, with and without
        // the spaces CSS allows around the combinator.
        assert_eq!(parse_selector(".lay>*"), Some(("lay".to_string(), None, true)));
        assert_eq!(parse_selector(".lay > *"), Some(("lay".to_string(), None, true)));
        assert_eq!(
            parse_selector(".lay:hover>*"),
            Some(("lay".to_string(), Some(State::Hover), true))
        );
        // A descendant, and a named child, are neither.
        assert_eq!(parse_selector(".lay *"), None);
        assert_eq!(parse_selector(".lay>p"), None);
        assert_eq!(parse_selector(".p-8:first-child"), None);
        assert_eq!(parse_selector("p-8"), None);
    }

    /// Three cells of one row, each painting its run of text in its own
    /// background colour, so the cell is a solid block and a colour boundary
    /// along a row is a track edge rather than a claim about a number.
    ///
    /// The sentence is far wider than any of the shares below, which is the
    /// whole question: a track's share is what it gets, not what its text
    /// would rather have.
    fn three_cells(columns: &str) -> String {
        let cell = |colour: &str| {
            format!(
                "e 1 background-color:{colour};color:{colour}\n\
                 t 2 a sentence far too long to sit on one line of a third of this page\n"
            )
        };
        format!(
            "buri-scene 1\nviewport 800 60\ne 0 display:grid;grid-template-columns:{columns}\n{}{}{}",
            cell("rgb(255,0,0)"),
            cell("rgb(0,255,0)"),
            cell("rgb(0,0,255)")
        )
    }

    /// Along row `y`: the x each new colour starts at, and the colour.
    fn bands(image: &Image, y: u32) -> Vec<(u32, [u8; 4])> {
        let mut out: Vec<(u32, [u8; 4])> = Vec::new();
        for x in 0..image.width {
            let pixel = at(image, x, y);
            if out.last().is_none_or(|&(_, last)| last != pixel) {
                out.push((x, pixel));
            }
        }
        out
    }

    /// `<n>fr` is a share of the room the other tracks left, the way a browser
    /// divides one: the `80px` track takes its 80 out of the 800 first, and the
    /// remaining 720 goes one part to two, so the edges are at 80 and 320.
    ///
    /// The same three cells under `auto` are sized by what is in them instead,
    /// which is a different picture — and a painter that read every `fr` as an
    /// `auto` painted the two byte for byte.
    #[test]
    fn a_fraction_track_divides_the_room_the_other_tracks_left() {
        const RED: [u8; 4] = [255, 0, 0, 255];
        const GREEN: [u8; 4] = [0, 255, 0, 255];
        const BLUE: [u8; 4] = [0, 0, 255, 255];

        let after_a_fixed_track = render_ok(&three_cells("80px 1fr 2fr"), "", "rest");
        assert_eq!(bands(&after_a_fixed_track, 1), [(0, RED), (80, GREEN), (320, BLUE)]);

        // Nothing taken out first, so the two tracks are a third and two
        // thirds of the whole page. The third cell wraps onto a row of its own
        // and is nothing to do with the row read here.
        let whole_page = render_ok(&three_cells("1fr 2fr"), "", "rest");
        assert_eq!(bands(&whole_page, 1), [(0, RED), (267, GREEN)]);

        // The `auto` twin of the line above: the same two cells, sized by what
        // is in them rather than by a share, which puts the edge in the middle
        // instead. A painter that read every `fr` as an `auto` painted these
        // two byte for byte.
        let content_sized = render_ok(&three_cells("auto auto"), "", "rest");
        assert_eq!(bands(&content_sized, 1), [(0, RED), (400, GREEN)]);
    }
}
