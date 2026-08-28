//! `textDocument/rename`, and the check the editor runs before it.
//!
//! A rename is the references scan with the answer written back, so nothing
//! here decides what a name refers to — [`symbols`] already did, and the two
//! cannot disagree because there is one scan.
//!
//! What is new is a rule about *spelling*. The scan reports every place a
//! symbol is referred to, and an alias refers to one without spelling it: in
//! `import { listing as shown }`, both `listing` and `shown` are references to
//! the same declaration, and only the first is the name being renamed. So a
//! rename rewrites the places that spell the old name and leaves the rest
//! alone — which is exactly right in both directions. Renaming `listing` to
//! `catalogue` gives `import { catalogue as shown }` and every use of `shown`
//! keeps working.

use crate::json::Value;
use crate::diagnostics::Span;
use std::path::Path;
use super::convert::{self, Position};
use super::state::Analyzed;
use super::symbols::{self, Symbol};

/// Why a name cannot be renamed. Each is a sentence an editor can show.
pub enum Refusal {
    /// Not a name at all, or a name this server does not resolve.
    Nothing,
    /// A module, named by a path. Renaming one means moving its file.
    Module,
    /// The standard library, which is compiled into the binary.
    NoFile,
    /// The replacement is not something Buri could parse as a name.
    NotAName,
    /// A declaration with no name of its own: a tuple field, which is named by
    /// its position.
    Unnamed,
}

impl Refusal {
    pub fn message(&self) -> &'static str {
        match self {
            Refusal::Nothing => "there is no name under the cursor to rename",
            Refusal::Module => {
                "a module is named by its path — rename one by moving its file and editing \
                 the imports that name it"
            }
            Refusal::NoFile => {
                "that is declared in the standard library, which is compiled into the binary \
                 and has no file to edit"
            }
            Refusal::NotAName => "that is not a name Buri can spell",
            Refusal::Unnamed => {
                "that is named by its position rather than by a name, so there is nothing to \
                 rename"
            }
        }
    }
}

/// The range the editor should offer to edit, and the text to offer in it.
///
/// Answered against the target owning the file rather than the whole
/// repository: everything this has to decide — what the cursor names, and
/// whether that declaration has a file — is in the one analysis, and this runs
/// every time a rename is begun.
pub fn prepare(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let found = symbols::at(analyzed, path, text, offset)?;
    let name = renameable(analyzed, &found.symbol).ok()?;
    Some(Value::object(vec![
        ("range", convert::range(text, found.span)),
        // The protocol's third form. The placeholder is the current name, so
        // the editor's box opens with it selected rather than empty.
        ("placeholder", Value::str(name)),
    ]))
}

/// Every edit the rename needs, as one `WorkspaceEdit`.
///
/// The whole repository is analysed, for the reason references is: a name is
/// used wherever it is imported, and a rename that missed one of those files
/// would leave the repository not building.
pub fn edits(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
    new_name: &str,
) -> Result<Value, Refusal> {
    if !is_identifier(new_name) {
        return Err(Refusal::NotAName);
    }
    let offset = convert::offset_of(text, position);
    let found = symbols::at(analyzed, path, text, offset).ok_or(Refusal::Nothing)?;
    let old = renameable(analyzed, &found.symbol)?;

    let mut spans = symbols::references(analyzed, &found.symbol);
    spans.push(symbols::declaration_name(analyzed, &found.symbol));

    let mut sites: Vec<(String, Span)> = spans
        .into_iter()
        .filter(|span| spells(analyzed, *span, &old))
        .filter_map(|span| Some((file_uri(analyzed, span)?, span)))
        .collect();
    sites.sort_by(|a, b| {
        a.0.cmp(&b.0).then(a.1.start.cmp(&b.1.start)).then(a.1.end.cmp(&b.1.end))
    });
    sites.dedup_by(|a, b| a.0 == b.0 && a.1.start == b.1.start && a.1.end == b.1.end);

    let mut changes: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    for (uri, span) in sites {
        let target = &analyzed.session.map.get(span.file).text;
        changes.entry(uri).or_default().push(Value::object(vec![
            ("range", convert::range(target, span)),
            ("newText", Value::str(new_name)),
        ]));
    }
    Ok(Value::object(vec![(
        "changes",
        Value::Object(changes.into_iter().map(|(k, v)| (k, Value::Array(v))).collect()),
    )]))
}

/// The name a symbol currently spells, or the reason it cannot be renamed.
fn renameable(analyzed: &Analyzed, symbol: &Symbol) -> Result<String, Refusal> {
    if matches!(symbol, Symbol::Module(_)) {
        return Err(Refusal::Module);
    }
    let name = symbols::name(analyzed, symbol).ok_or(Refusal::Nothing)?;
    let declaration = symbols::declaration_name(analyzed, symbol);
    if file_uri(analyzed, declaration).is_none() {
        return Err(Refusal::NoFile);
    }
    // The one check that the edit is an edit to a name: whatever else the scan
    // turns up, the declaration itself must spell what is being renamed.
    if !spells(analyzed, declaration, &name) {
        return Err(Refusal::Unnamed);
    }
    Ok(name)
}

/// Whether the source at a span is the name being renamed.
///
/// This is the alias rule, and it is also what keeps a reference the scan
/// found by resolving rather than by spelling — an operator, a leading-dot
/// variant — out of the edit.
fn spells(analyzed: &Analyzed, span: Span, name: &str) -> bool {
    if span.is_none() {
        return false;
    }
    let text = &analyzed.session.map.get(span.file).text;
    text.get(span.start as usize..span.end as usize) == Some(name)
}

/// The file a span is in, or nothing for a span with no file and for the
/// embedded standard library.
fn file_uri(analyzed: &Analyzed, span: Span) -> Option<String> {
    if span.is_none() {
        return None;
    }
    let file = analyzed.session.map.get(span.file);
    if file.abs_path.as_os_str().is_empty() {
        return None;
    }
    Some(convert::uri_of(&file.abs_path))
}

/// Whether a replacement is a name at all — asked of the lexer rather than of
/// a character rule here, so that `fn`, `match` and the words v0.3 reserves are
/// refused for the same reason the compiler would refuse them.
///
/// Refusing here is the difference between a rename the editor reports as
/// impossible and a repository full of files that no longer parse.
fn is_identifier(name: &str) -> bool {
    let lexed = crate::parsing::lexer::lex(name, crate::diagnostics::FileId(0));
    if !lexed.errors.is_empty() {
        return false;
    }
    let mut words = (0..lexed.tokens.len())
        .map(|i| lexed.tokens.kind(i))
        .filter(|k| *k != crate::parsing::lexer::TokenKind::Eof);
    words.next() == Some(crate::parsing::lexer::TokenKind::Ident) && words.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replacement_has_to_be_a_name() {
        for good in ["x", "_x", "Item", "toCents2"] {
            assert!(is_identifier(good), "{good}");
        }
        // A keyword and a word v0.3 reserves are names the compiler refuses,
        // so they are names a rename refuses.
        for bad in ["", "2x", "a b", "a-b", "a.b", "fn(", "fn", "match", "while", "é"] {
            assert!(!is_identifier(bad), "{bad}");
        }
    }
}
