//! The colours a file writes down, and the colours a picker writes back.
//!
//! Buri has first-class colour: `ui/style` declares
//! `enum Color { Rgb(Int, Int, Int), Rgba(Int, Int, Int, Float), … }`, and the
//! checker has already turned every one written in the source into an
//! `EnumLit` carrying its span and its arguments. So `documentColor` is a read
//! of the typed tree rather than a scan of the text — nothing here parses a
//! literal, and a `#ff0000` inside a string is not a colour because the
//! language does not say it is one.
//!
//! **A constructor whose arguments are not all literals is skipped.** A swatch
//! beside `Rgb(shade, 0, 0)` would be a guess at what `shade` is worth at run
//! time, and a colour the server cannot compute is not one it should draw.

use crate::compiler::semantics::resolve::Sym;
use crate::compiler::semantics::typed::{self, ExprKind};
use crate::compiler::semantics::types::{TyConId, Tables};
use crate::diagnostics::Span;
use crate::json::Value;
use std::path::Path;
use super::convert;
use super::state::Analyzed;

/// A colour in the protocol's terms: four channels, each 0.0 to 1.0.
type Rgba = [f64; 4];

/// Every `Color` constructor this file spells out.
///
/// The whole closure's bodies are walked and filtered to the one file, which is
/// what `hover` does with the same tree: a body belongs to a file, so the
/// filter is on the declaring function rather than on every expression in it.
pub fn document_colors(analyzed: &Analyzed, path: &Path, text: &str) -> Option<Value> {
    let file = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path))?;
    let con = color_constructor(analyzed)?;
    let tables = &analyzed.analysis.checked.tables;

    let mut found: Vec<(Span, Rgba)> = Vec::new();
    let mut visit = |e: &typed::Expr| {
        if e.span.file != file {
            return;
        }
        let ExprKind::EnumLit { con: written, variant, args, .. } = &e.kind else { return };
        if *written != con {
            return;
        }
        if let Some(rgba) = color_of(tables, con, *variant, args) {
            found.push((e.span, rgba));
        }
    };
    for (id, body) in &analyzed.analysis.checked.bodies {
        if tables.fn_info(*id).span.file == file {
            typed::walk(&body.expr, &mut visit);
        }
    }
    // A module-level `let` is a const rather than a body, and a palette is
    // exactly the kind of thing written as one.
    for (id, expr) in &analyzed.analysis.checked.consts {
        if tables.const_(*id).span.file == file {
            typed::walk(expr, &mut visit);
        }
    }

    // Both maps are hashed, so the walk order is not the same twice.
    found.sort_by_key(|(span, _)| (span.start, span.end));
    found.dedup_by_key(|(span, _)| (span.start, span.end));
    Some(Value::Array(
        found
            .iter()
            .map(|(span, rgba)| {
                Value::object(vec![
                    ("range", convert::range(text, *span)),
                    ("color", color_value(*rgba)),
                ])
            })
            .collect(),
    ))
}

/// The presentations for a colour the picker chose, over the range it chose it
/// on.
///
/// No analysis: the range came from [`document_colors`], and what has to be
/// written there is decided by the colour and by how the source spelled the
/// call. Which constructor is offered is decided by the colour too — `Rgb`
/// cannot say "half transparent", so a translucent colour is offered as `Rgba`
/// and nothing else.
pub fn presentations(text: &str, params: &Value) -> Value {
    let built = (|| {
        let rgba = [
            number(params.at("color.red"))?,
            number(params.at("color.green"))?,
            number(params.at("color.blue"))?,
            number(params.at("color.alpha"))?,
        ];
        let start = convert::Position::from_json(params.at("range.start")?)?;
        let end = convert::Position::from_json(params.at("range.end")?)?;
        let from = convert::offset_of(text, start) as usize;
        let to = convert::offset_of(text, end) as usize;
        let written = text.get(from..to).unwrap_or("");
        let label = format!("{}{}", qualifier(written), call(rgba));
        Some(Value::Array(vec![Value::object(vec![
            ("label", Value::str(&label)),
            (
                "textEdit",
                Value::object(vec![
                    (
                        "range",
                        Value::object(vec![
                            ("start", start.to_json()),
                            ("end", end.to_json()),
                        ]),
                    ),
                    ("newText", Value::str(&label)),
                ]),
            ),
        ])]))
    })();
    built.unwrap_or(Value::Array(Vec::new()))
}

/// `ui/style`'s `Color`, when this compilation loaded it.
///
/// By the module's path and the name the module itself declares, rather than by
/// any `Color` a repository happens to define: the swatch stands for the
/// standard library's constructors and nothing else.
fn color_constructor(analyzed: &Analyzed) -> Option<TyConId> {
    let index = analyzed.analysis.loaded.modules.iter().position(|m| m.path == "ui/style")?;
    match analyzed.analysis.checked.scopes.get(index)?.own.get("Color")? {
        Sym::Ty(id) => Some(*id),
        _ => None,
    }
}

/// The colour one constructor call spells out, or nothing when it is not one of
/// the two that carry channels or an argument is not a literal.
fn color_of(tables: &Tables, con: TyConId, variant: usize, args: &[typed::Expr]) -> Option<Rgba> {
    let name = tables.tycon(con).variants().get(variant)?.name.as_str();
    let opaque = match name {
        "Rgb" => true,
        "Rgba" => false,
        // `Token`, `Transparent` and `Inherit` are colours whose value is
        // decided somewhere else — by the theme, or by the element above.
        _ => return None,
    };
    let mut rgba = [0.0, 0.0, 0.0, 1.0];
    for (channel, arg) in rgba.iter_mut().take(3).zip(args) {
        *channel = eight_bit(arg)?;
    }
    if !opaque {
        rgba[3] = match args.get(3)?.kind {
            ExprKind::Float(a) => a.clamp(0.0, 1.0),
            _ => return None,
        };
    }
    Some(rgba)
}

/// One 0-255 channel as the protocol's 0.0-1.0.
///
/// The language types a channel as `Int` and the doc comment is what says
/// 0-255, so a number outside that is legal Buri. It is clamped rather than
/// refused: the protocol's `Color` has nowhere to put 300, and dropping the
/// whole swatch over one channel would hide a literal the reader can see.
fn eight_bit(e: &typed::Expr) -> Option<f64> {
    match e.kind {
        ExprKind::Int(n, false) => Some((n.min(255) as f64) / 255.0),
        ExprKind::Int(_, true) => Some(0.0),
        _ => None,
    }
}

fn color_value(rgba: Rgba) -> Value {
    let channel = |n: f64| Value::float(n).unwrap_or(Value::Int(0));
    Value::object(vec![
        ("red", channel(rgba[0])),
        ("green", channel(rgba[1])),
        ("blue", channel(rgba[2])),
        ("alpha", channel(rgba[3])),
    ])
}

/// The constructor call a colour is written as.
///
/// An opaque colour is `Rgb`, because that is what a reader writes and what the
/// file already said; anything else needs the fourth argument to be true at
/// all.
fn call(rgba: Rgba) -> String {
    let byte = |n: f64| (n * 255.0).round().clamp(0.0, 255.0) as i64;
    let (red, green, blue) = (byte(rgba[0]), byte(rgba[1]), byte(rgba[2]));
    if rgba[3] >= 1.0 {
        return format!("Rgb({red}, {green}, {blue})");
    }
    // Three decimals: a picker's alpha is a slider position, and
    // `0.5019607843137255` is a rendering of one 8-bit step rather than
    // something anybody meant to type.
    let alpha = (rgba[3] * 1000.0).round() / 1000.0;
    format!("Rgba({red}, {green}, {blue}, {alpha:?})")
}

/// What the source wrote before the constructor's own name.
///
/// `.Rgb(255, 0, 0)` is the shorthand for a variant whose type is known from
/// context and `Color.Rgb(255, 0, 0)` names the enum, and a replacement that
/// dropped either would leave a file that no longer resolves. So the picker
/// writes back what was already there, and only the call changes.
fn qualifier(written: &str) -> &str {
    let head = written.split('(').next().unwrap_or(written);
    match head.rfind('.') {
        Some(dot) => written.get(..=dot).unwrap_or(""),
        None => "",
    }
}

/// A JSON number of either variant as a `f64`. The protocol says a colour
/// channel is a decimal, and `1` is what a client sends for a channel that is
/// fully on.
fn number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Int(n) => Some(*n as f64),
        Value::Float(n) => Some(n.get()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picker_writes_back_the_spelling_it_found() {
        assert_eq!(qualifier(".Rgb(240, 240, 245)"), ".");
        assert_eq!(qualifier("Color.Rgb(1, 2, 3)"), "Color.");
        assert_eq!(qualifier("style.Color.Rgba(1, 2, 3, 0.5)"), "style.Color.");
        assert_eq!(qualifier("Rgb(1, 2, 3)"), "");
        assert_eq!(qualifier(""), "");
    }

    #[test]
    fn an_opaque_colour_is_rgb_and_a_translucent_one_is_rgba() {
        assert_eq!(call([1.0, 0.0, 0.0, 1.0]), "Rgb(255, 0, 0)");
        assert_eq!(call([0.0, 0.5, 1.0, 0.5]), "Rgba(0, 128, 255, 0.5)");
        // One 8-bit step of alpha is a slider position, not a number to keep.
        assert_eq!(call([0.0, 0.0, 0.0, 128.0 / 255.0]), "Rgba(0, 0, 0, 0.502)");
    }
}
