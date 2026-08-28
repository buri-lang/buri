//! `textDocument/signatureHelp` — what goes in the parentheses you just opened.
//!
//! The call is found in the **text** rather than in the typed tree, and that is
//! the whole design decision here. Signature help is asked for at the one
//! moment the file does not check: `listing(` has no arguments yet, so the
//! checker has already reported the arity and replaced the call with poison.
//! Reading the typed tree would mean answering only the calls that are already
//! finished, which are the calls nobody needs help with.
//!
//! So the enclosing `(` and the active parameter come from a scan of the
//! buffer, and only the *callee* is resolved — through [`symbols::at`], which
//! is how every other request decides what a name refers to, and through the
//! module's scope when the body around it did not check.

use crate::compiler::semantics::resolve::Sym;
use crate::json::Value;
use std::path::Path;
use super::convert::{self, Position};
use super::state::Analyzed;
use super::symbols::{self, Symbol};

pub fn help(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let call = enclosing_call(text, offset)?;
    let name = &text.get(call.name.0 as usize..call.name.1 as usize)?.to_string();
    let signatures = callees(analyzed, path, text, call.name.0, name);
    if signatures.is_empty() {
        return None;
    }

    // Which overload the arguments so far could still be. An overload with too
    // few parameters to reach the one being typed is not the one being typed.
    let active = signatures
        .iter()
        .position(|s| s.parameters.len() > call.argument as usize)
        .unwrap_or(0);
    Some(Value::object(vec![
        (
            "signatures",
            Value::Array(signatures.iter().map(rendered).collect()),
        ),
        ("activeSignature", Value::number(active as i64)),
        ("activeParameter", Value::number(i64::from(call.argument))),
    ]))
}

fn rendered(signature: &symbols::Signature) -> Value {
    let parameters: Vec<Value> = signature
        .parameters
        .iter()
        .map(|(start, end)| {
            Value::object(vec![(
                "label",
                Value::Array(vec![Value::number(*start), Value::number(*end)]),
            )])
        })
        .collect();
    let mut fields = vec![
        ("label", Value::str(&signature.label)),
        ("parameters", Value::Array(parameters)),
    ];
    if !signature.docs.is_empty() {
        fields.push((
            "documentation",
            Value::object(vec![
                ("kind", Value::str("markdown")),
                ("value", Value::str(signature.docs.join("\n"))),
            ]),
        ));
    }
    Value::object(fields)
}

/// Every signature the name before the `(` could be calling.
///
/// One, ordinarily. The list is for an overloaded name, which the scope knows
/// about and a single symbol cannot express — the editor shows them together
/// and highlights the one the arguments fit.
fn callees(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    at: u32,
    name: &str,
) -> Vec<symbols::Signature> {
    // The resolver first: it is the one that knows a method call's receiver,
    // and it agrees with hover and definition by construction.
    if let Some(found) = symbols::at(analyzed, path, text, at) {
        if let Some(signature) = symbols::signature(analyzed, &found.symbol) {
            return vec![signature];
        }
    }
    // Then the module's own scope, for the ordinary case this request exists
    // for: a call being typed, whose body therefore did not check.
    let Some(file) = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path)) else {
        return Vec::new();
    };
    let Some(module) = analyzed.analysis.loaded.modules.iter().find(|m| m.file == file) else {
        return Vec::new();
    };
    let Some(scope) = analyzed.analysis.checked.scopes.get(module.id.index()) else {
        return Vec::new();
    };
    let ids = match scope.names.get(name) {
        Some(Sym::Fn(id)) => vec![*id],
        Some(Sym::Overloaded(ids)) => ids.clone(),
        _ => Vec::new(),
    };
    ids.iter().filter_map(|id| symbols::signature(analyzed, &Symbol::Function(*id))).collect()
}

/// The call the offset is inside: where its callee is written, and which
/// argument the cursor is in.
struct Call {
    /// Byte range of the name before the `(`.
    name: (u32, u32),
    /// How many commas separate the cursor from the `(`.
    argument: u32,
}

/// The innermost unclosed `(` before the offset, and the callee before it.
///
/// Read forwards from the top of the file rather than backwards from the
/// cursor, because backwards there is no way to tell a `)` inside a string
/// from one that closes a call — a scanner that has already passed the opening
/// quote knows, and one starting in the middle does not.
///
/// Reaching the cursor while still inside a literal or a comment answers
/// nothing. A `${…}` inside a template is a call being written like any other,
/// but the parentheses around it are the template's, and reporting the call the
/// template is an argument to would be worse than reporting none.
fn enclosing_call(text: &str, offset: u32) -> Option<Call> {
    #[derive(Clone, Copy)]
    struct Open {
        delimiter: u8,
        at: usize,
        commas: u32,
    }
    let bytes = text.as_bytes();
    let end = (offset as usize).min(bytes.len());
    let at = |i: usize| bytes.get(i).copied();
    let mut stack: Vec<Open> = Vec::new();
    let mut index = 0usize;
    while index < end {
        let byte = at(index)?;
        let next = match byte {
            // A string or a character literal, past its closing delimiter and
            // honouring the backslash before one.
            b'"' | b'\'' => {
                let mut i = index.saturating_add(1);
                while i < bytes.len() && at(i) != Some(byte) {
                    i = i.saturating_add(if at(i) == Some(b'\\') { 2 } else { 1 });
                }
                i.saturating_add(1)
            }
            b'/' if at(index.saturating_add(1)) == Some(b'/') => {
                let mut i = index;
                while i < bytes.len() && at(i) != Some(b'\n') {
                    i = i.saturating_add(1);
                }
                i.saturating_add(1)
            }
            // `/* … */`, which nests.
            b'/' if at(index.saturating_add(1)) == Some(b'*') => {
                let mut depth = 1u32;
                let mut i = index.saturating_add(2);
                while i < bytes.len() && depth > 0 {
                    if at(i) == Some(b'/') && at(i.saturating_add(1)) == Some(b'*') {
                        depth = depth.saturating_add(1);
                        i = i.saturating_add(2);
                    } else if at(i) == Some(b'*') && at(i.saturating_add(1)) == Some(b'/') {
                        depth = depth.saturating_sub(1);
                        i = i.saturating_add(2);
                    } else {
                        i = i.saturating_add(1);
                    }
                }
                i
            }
            _ => {
                match byte {
                    b'(' | b'[' | b'{' => {
                        stack.push(Open { delimiter: byte, at: index, commas: 0 });
                    }
                    b')' | b']' | b'}' => {
                        stack.pop();
                    }
                    b',' => {
                        if let Some(top) = stack.last_mut() {
                            top.commas = top.commas.saturating_add(1);
                        }
                    }
                    _ => {}
                }
                index.saturating_add(1)
            }
        };
        // Only a literal or a comment can carry the scan past the cursor, and
        // that means the cursor was inside it.
        if next > end {
            return None;
        }
        index = next;
    }
    // The nearest enclosing `(`. A list or a struct literal between the cursor
    // and it is a place whose own commas belong to the list, not to the call.
    let open = stack.iter().rev().find(|o| o.delimiter == b'(')?;
    let name = name_before(text, open.at)?;
    Some(Call { name, argument: open.commas })
}

/// The identifier that ends just before `open`, skipping the spaces between.
fn name_before(text: &str, open: usize) -> Option<(u32, u32)> {
    let bytes = text.as_bytes();
    let part = |b: &u8| b.is_ascii_alphanumeric() || *b == b'_';
    let mut end = open;
    while let Some(previous) = end.checked_sub(1) {
        if !bytes.get(previous).is_some_and(|b| b.is_ascii_whitespace()) {
            break;
        }
        end = previous;
    }
    let mut start = end;
    while let Some(previous) = start.checked_sub(1) {
        if !bytes.get(previous).is_some_and(part) {
            break;
        }
        start = previous;
    }
    (end > start).then_some((start as u32, end as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(text: &str) -> Option<(String, u32)> {
        let offset = text.find('|')? as u32;
        let text = text.replace('|', "");
        let call = enclosing_call(&text, offset)?;
        let name = text.get(call.name.0 as usize..call.name.1 as usize)?.to_string();
        Some((name, call.argument))
    }

    #[test]
    fn the_call_is_the_nearest_unclosed_parenthesis() {
        assert_eq!(call("f(|"), Some(("f".into(), 0)));
        assert_eq!(call("f(a, |"), Some(("f".into(), 1)));
        assert_eq!(call("f(a, b, |)"), Some(("f".into(), 2)));
        assert_eq!(call("x.total(|)"), Some(("total".into(), 0)));
        assert_eq!(call("f (|)"), Some(("f".into(), 0)));
        // A finished call is not the one the cursor is in.
        assert_eq!(call("f(g(a), |)"), Some(("f".into(), 1)));
        // The commas of a list belong to the list.
        assert_eq!(call("f([1, 2, |3])"), Some(("f".into(), 0)));
        assert_eq!(call("f(S { a: 1, b: |2 })"), Some(("f".into(), 0)));
    }

    #[test]
    fn what_is_not_a_call_answers_nothing() {
        assert_eq!(call("let x = |1;"), None);
        assert_eq!(call("f(a)|"), None);
        // A parenthesis inside a string or a comment closes nothing.
        assert_eq!(call("f(\")\", |)"), Some(("f".into(), 1)));
        assert_eq!(call("f('(')|"), None);
        assert_eq!(call("f(a) // )\n|"), None);
        assert_eq!(call("f(a) /* ) /* ) */ */ |"), None);
        // A grouping parenthesis has no callee before it.
        assert_eq!(call("(1 + |2)"), None);
        // Inside a literal or a comment there is no call to name.
        assert_eq!(call("f(\"a|b\")"), None);
        assert_eq!(call("f( // a|b\n)"), None);
    }
}
