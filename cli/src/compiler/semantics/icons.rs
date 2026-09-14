//! What an icon may hold: the artwork is read at compile time, or refused.
//!
//! `ui/node`'s `image` puts vector artwork *in* the tree when its `alt` is
//! `.Decorative` — the source is written into the document as an `<svg>`, which
//! is what lets `currentColor` inside it be the element's own `Foreground`.
//! Inlining is also the one thing an `<img src=...>` never had to worry about:
//! whatever the source says lands in the document. An `AccessibilityText`
//! picture is the other case — its source is a fetched address, a document of
//! its own — so nothing here reads it.
//!
//! So a decorative source is read here, once, before anything can render it. It
//! has to be written out at the call site — the compiler cannot read a string
//! that does not exist until the program runs — and what it holds has to be an
//! `<svg>` and the shapes inside it. Anything else is `icon-not-drawable`,
//! naming what it found.
//!
//! **The renderer is the other half of the same rule.** `runtime.js`'s
//! `$tree_icon` builds only the elements and sets only the attributes named
//! here, so there is no path in it that creates a script, an event handler, or
//! a reference to somewhere else. This pass is what turns "quietly dropped"
//! into a diagnostic somebody reads.

use core::iter::Peekable;
use core::str::Chars;

use crate::compiler::modules::Loaded;
use crate::compiler::semantics::consteval::{Env, Folder, Value};
use crate::compiler::semantics::resolve::{ModuleScope, Sym};
use crate::compiler::semantics::typed::{self, ExprKind};
use crate::compiler::semantics::types::{ConstId, FnId, Tables, TyConId};
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use crate::hash::Map as HashMap;

/// The elements an icon may hold: a coordinate system, a group, and the shapes
/// the painter's own SVG subset draws (`cli/runtime/image.rs`).
const DRAWN: [&str; 9] =
    ["svg", "g", "path", "rect", "circle", "ellipse", "line", "polyline", "polygon"];

/// The attributes an icon may carry: a frame, a paint, a transform, and each
/// shape's own geometry. Every one of them draws.
///
/// `xmlns` is allowed and nothing is done with it — an artwork pasted out of an
/// icon set carries one, and the element is in that namespace already.
const DRAWING: [&str; 26] = [
    "xmlns",
    "viewBox",
    "width",
    "height",
    "fill",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "opacity",
    "fill-opacity",
    "stroke-opacity",
    "transform",
    "d",
    "points",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
];

/// `NodeKind::Icon`, whose variant order is load-bearing and whose module says
/// so. Only a decorative `image` lowers to one.
const NODE_ICON: usize = 8;

/// `Image<C>`'s fields, in declaration order — which is the order a struct
/// literal stores them in, whatever order the call site wrote them. The two
/// this pass reads are the source it may have to validate and the `alt` that
/// says whether it must.
const IMAGE_SOURCE: usize = 0;
const IMAGE_ALT: usize = 1;

/// `Alt::Decorative`, the variant that inlines an `<svg>` this pass reads. Its
/// sibling `AccessibilityText` is a fetched address, so it is left alone.
const ALT_DECORATIVE: usize = 0;

/// `Prop::Const`, the one variant of a `Prop<Str>` that folds to a string
/// written out at the call site.
const PROP_CONST: usize = 0;

/// Whether this program can build an icon.
///
/// Asked the way `styles::builds_a_theme` is, and for the same reason: the
/// renderer reaches `$tree_icon` through a hole rather than by name, so the
/// parser and the two allow lists — 2.5 KB of them — ship only in an artifact
/// that has artwork in it. `NodeKind` is `ui/node`'s private enum, so a literal
/// of it was written inside that module's own constructors and nowhere else.
pub fn builds_an_icon(e: &mut typed::Expr, node_con: TyConId) -> bool {
    if matches!(&e.kind, ExprKind::EnumLit { con, variant, .. }
        if *con == node_con && *variant == NODE_ICON)
    {
        return true;
    }
    let mut found = false;
    typed::children_mut(e, &mut |child| found = found || builds_an_icon(child, node_con));
    found
}

/// Reads every decorative `image` in the compilation, and refuses one it
/// cannot.
///
/// A compilation that did not load `ui/node` returns immediately, which is
/// every program that is not a user interface.
pub fn run(
    loaded: &Loaded,
    tables: &Tables,
    scopes: &[ModuleScope],
    bodies: &HashMap<FnId, typed::Body>,
    consts: &HashMap<ConstId, typed::Expr>,
    diags: &mut Diagnostics,
    only: Option<&[crate::diagnostics::FileId]>,
) {
    let Some(image) = constructor(loaded, scopes) else { return };
    let wanted = |file| only.is_none_or(|files: &[crate::diagnostics::FileId]| files.contains(&file));

    let mut ids: Vec<ConstId> = consts.keys().copied().collect();
    ids.sort_by_key(|c| c.index());
    for id in ids {
        if wanted(tables.const_(id).span.file) {
            if let Some(init) = consts.get(&id) {
                walk(init, image, tables, bodies, consts, diags);
            }
        }
    }
    let mut fns: Vec<FnId> = bodies.keys().copied().collect();
    fns.sort_by_key(|f| f.index());
    for id in fns {
        if wanted(tables.fn_info(id).span.file) {
            if let Some(body) = bodies.get(&id) {
                walk(&body.expr, image, tables, bodies, consts, diags);
            }
        }
    }
}

/// `ui/node`'s `image`, when this compilation loaded the module. A decorative
/// one is what lowers to an inline `<svg>`, so this is the constructor whose
/// calls the walk reads.
fn constructor(loaded: &Loaded, scopes: &[ModuleScope]) -> Option<FnId> {
    let index = loaded.modules.iter().position(|m| m.path == "ui/node")?;
    match scopes.get(index)?.own.get("image")? {
        Sym::Fn(id) => Some(*id),
        _ => None,
    }
}

/// Every call to it, in walk order.
fn walk(
    e: &typed::Expr,
    image: FnId,
    tables: &Tables,
    bodies: &HashMap<FnId, typed::Body>,
    consts: &HashMap<ConstId, typed::Expr>,
    diags: &mut Diagnostics,
) {
    if let ExprKind::CallFn { func, args } = &e.kind {
        if func.decl() == Some(image) {
            // The one argument is an `Image<C>` struct literal.
            if let Some(arg) = args.first() {
                check(arg, tables, bodies, consts, diags);
            }
        }
    }
    typed::children(e, &mut |child| walk(child, image, tables, bodies, consts, diags));
}

/// One `image` call: read a decorative one's artwork, or say why it could not
/// be — and leave an `AccessibilityText` one, whose source is a fetched
/// address, alone.
fn check(
    arg: &typed::Expr,
    tables: &Tables,
    bodies: &HashMap<FnId, typed::Body>,
    consts: &HashMap<ConstId, typed::Expr>,
    diags: &mut Diagnostics,
) {
    // A struct literal stores its fields in declaration order, so the `source`
    // and the `alt` this pass reads are taken by position.
    let ExprKind::StructLit { fields, .. } = &arg.kind else { return };
    let mut folder = Folder::new(tables, bodies, consts);

    // Only a decorative picture inlines an `<svg>`. An `AccessibilityText` one
    // — and any whose `alt` cannot be reduced, since it may be either — is left
    // for the browser to fetch, so nothing here reads its source.
    let decorative = fields
        .get(IMAGE_ALT)
        .and_then(|e| folder.eval(e, &Env::default()))
        .as_ref()
        .and_then(Value::as_variant)
        .is_some_and(|(variant, _)| variant == ALT_DECORATIVE);
    if !decorative {
        return;
    }

    let Some(source) = fields.get(IMAGE_SOURCE) else { return };
    let folded = folder.eval(source, &Env::default());
    let Some(text) = folded.as_ref().and_then(const_source) else {
        refuse(
            diags,
            source.span,
            "a decorative picture's artwork has to be written out at the call site, because the compiler reads it",
        );
        return;
    };
    if let Some(problem) = problem(text) {
        refuse(diags, source.span, &problem);
    }
}

/// The string a decorative `source` folds to: a `Prop`'s `.Const` holding one.
fn const_source(v: &Value) -> Option<&str> {
    match v {
        Value::Variant { variant, args } if *variant == PROP_CONST => {
            args.first().and_then(Value::as_str)
        }
        _ => None,
    }
}

fn refuse(diags: &mut Diagnostics, span: Span, message: &str) {
    diags
        .items
        .push(Diagnostic::templated("icon-not-drawable", span).with_bind("problem", message));
}

/// What is wrong with an artwork, or `None` for one the renderer and the
/// painter can both draw.
///
/// The whole of the rule is an allow list, which is why a script and a
/// reference to somewhere else need no clause of their own: neither is a shape,
/// and `href` is not an attribute that draws.
fn problem(text: &str) -> Option<String> {
    let tags = tags(text);
    let Some(first) = tags.first() else {
        return Some("an icon's artwork has to be an `<svg>`, and this one holds no tag at all".into());
    };
    if first.name != "svg" || first.closing {
        return Some(format!(
            "an icon's artwork has to start with an `<svg>`, and this one starts with `<{}>`",
            first.name
        ));
    }
    for tag in &tags {
        if !DRAWN.contains(&tag.name.as_str()) {
            return Some(format!(
                "an icon may hold only an `<svg>` and the shapes inside it, and this one holds a `<{}>`",
                tag.name
            ));
        }
        if let Some(attribute) = tag.attributes.iter().find(|a| !DRAWING.contains(&a.as_str())) {
            return Some(format!(
                "an icon may carry only the attributes that draw a shape, and this `<{}>` carries `{attribute}`",
                tag.name
            ));
        }
    }
    None
}

/// One tag of the artwork.
struct Tag {
    name: String,
    closing: bool,
    attributes: Vec<String>,
}

/// The source as tags, and everything that is not one as a tag named after what
/// it opened with — so a comment, a declaration and a stray `<` are refused by
/// the allow list above rather than skipped.
fn tags(text: &str) -> Vec<Tag> {
    let mut chars = text.chars().peekable();
    let mut out = Vec::new();
    while let Some(c) = chars.next() {
        if c != '<' {
            continue;
        }
        let closing = chars.peek() == Some(&'/');
        if closing {
            chars.next();
        }
        let name = taken(&mut chars, |c| !c.is_whitespace() && c != '>' && c != '/');
        let mut attributes = Vec::new();
        loop {
            space(&mut chars);
            match chars.peek() {
                None => break,
                Some('>') => {
                    chars.next();
                    break;
                }
                // A self-closing tag's slash, which says nothing about what the
                // tag holds — an icon's artwork has no text, so both spellings
                // are the same drawing.
                Some('/') => {
                    chars.next();
                    continue;
                }
                Some(_) => {}
            }
            let name = taken(&mut chars, |c| {
                !c.is_whitespace() && c != '=' && c != '>' && c != '/'
            });
            attributes.push(name);
            space(&mut chars);
            if chars.peek() != Some(&'=') {
                continue;
            }
            chars.next();
            space(&mut chars);
            match chars.peek() {
                Some(&quote @ ('"' | '\'')) => {
                    chars.next();
                    let _ = taken(&mut chars, |c| c != quote);
                    chars.next();
                }
                _ => {
                    let _ = taken(&mut chars, |c| !c.is_whitespace() && c != '>');
                }
            }
        }
        out.push(Tag { name, closing, attributes });
    }
    out
}

/// The characters at the front the predicate accepts, taken off it.
fn taken(chars: &mut Peekable<Chars>, ok: impl Fn(char) -> bool) -> String {
    let mut out = String::new();
    while let Some(&c) = chars.peek() {
        if !ok(c) {
            break;
        }
        out.push(c);
        chars.next();
    }
    out
}

fn space(chars: &mut Peekable<Chars>) {
    let _ = taken(chars, char::is_whitespace);
}

#[cfg(test)]
mod tests {
    use super::problem;

    /// A lucide glyph, which is what an icon set ships.
    const CHECK: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' \
                         stroke='currentColor' stroke-width='2'><path d='M20 6 9 17l-5-5'/></svg>";

    #[test]
    fn a_glyph_out_of_an_icon_set_is_drawable() {
        assert_eq!(problem(CHECK), None);
        assert_eq!(problem("<svg viewBox=\"0 0 1 1\"><g><rect x=\"0\"/></g></svg>"), None);
    }

    #[test]
    fn a_script_is_refused_by_name() {
        let said = problem("<svg viewBox='0 0 1 1'><script>alert(1)</script></svg>").unwrap();
        assert!(said.contains("`<script>`"), "{said}");
    }

    #[test]
    fn a_reference_to_somewhere_else_is_refused_by_name() {
        let said = problem("<svg viewBox='0 0 1 1'><use href='#x'/></svg>").unwrap();
        assert!(said.contains("`<use>`"), "{said}");
        let said = problem("<svg viewBox='0 0 1 1'><path d='M0 0' onload='go()'/></svg>").unwrap();
        assert!(said.contains("`onload`"), "{said}");
    }

    #[test]
    fn artwork_that_is_not_an_svg_is_refused() {
        assert!(problem("a check").unwrap().contains("no tag at all"));
        assert!(problem("<div></div>").unwrap().contains("`<div>`"));
    }

    #[test]
    fn a_comment_is_not_a_shape() {
        let said = problem("<svg viewBox='0 0 1 1'><!-- a check --></svg>").unwrap();
        assert!(said.contains("!--"), "{said}");
    }
}
