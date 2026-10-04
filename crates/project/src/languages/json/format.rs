//! A JSON file as a [`Doc`]: layout decided by the width, every comment kept,
//! and every scalar written exactly as it was.
//!
//! ```text
//! {"a":[1,2],   "b":{}}   ->   { "a": [1, 2], "b": {} }
//! ```
//!
//! A trailing comma follows the dialect: never in JSON or JSONC, and in JSON5
//! wherever a list breaks across lines. An empty line between two members
//! survives when the list breaks.

use super::syntax::{Comment, Dialect, Document, Node, Value};
use crate::layout::Doc;

pub fn document(doc: &Document, dialect: Dialect) -> Doc {
    let mut out = leading(&doc.root.leading);
    out.push(node(&doc.root, dialect));
    out.extend(trailing(&doc.root.trailing));
    for c in &doc.trailing {
        out.push(Doc::HardLine);
        if c.newlines_before >= 2 {
            out.push(Doc::HardLine);
        }
        out.push(comment(c));
    }
    out.push(Doc::HardLine);
    Doc::Concat(out)
}

/// A comment on a line of its own. A block comment whose lines all start
/// with `*` is re-indented; any other is written as it stands.
fn comment(c: &Comment) -> Doc {
    let mut lines = c.text.lines();
    let first = lines.next().unwrap_or_default().to_string();
    let rest: Vec<&str> = lines.collect();
    if rest.is_empty() {
        return Doc::Text(first);
    }
    if rest.iter().all(|l| l.trim_start().starts_with('*')) {
        let mut parts = vec![Doc::Text(first)];
        for l in rest {
            parts.push(Doc::HardLine);
            parts.push(Doc::Text(format!(" {}", l.trim_start())));
        }
        return Doc::Concat(parts);
    }
    Doc::Text(c.text.clone())
}

/// Each on a line of its own, above what they describe.
fn leading(comments: &[Comment]) -> Vec<Doc> {
    let mut out = Vec::new();
    for (i, c) in comments.iter().enumerate() {
        if i > 0 && c.newlines_before >= 2 {
            out.push(Doc::HardLine);
        }
        out.push(comment(c));
        out.push(Doc::HardLine);
    }
    out
}

/// At the end of the line the value ends on.
fn trailing(comments: &[Comment]) -> Vec<Doc> {
    let mut out = Vec::new();
    for c in comments {
        out.push(Doc::LineSuffix(format!(" {}", c.text)));
        out.push(Doc::BreakParent);
    }
    out
}

fn node(n: &Node, dialect: Dialect) -> Doc {
    match &n.value {
        Value::Array(items, dangling) => {
            let elements: Vec<(Doc, &Node)> = items.iter().map(|i| (node(i, dialect), i)).collect();
            list("[", "]", Doc::SoftLine, elements, dangling, dialect)
        }
        Value::Object(members, dangling) => {
            let elements: Vec<(Doc, &Node)> = members
                .iter()
                .map(|m| {
                    (Doc::Concat(vec![Doc::Text(format!("{}: ", m.key_raw)), node(&m.value, dialect)]), &m.value)
                })
                .collect();
            list("{", "}", Doc::Line, elements, dangling, dialect)
        }
        _ => Doc::Text(n.raw.clone()),
    }
}

fn list(
    open: &str,
    close: &str,
    pad: Doc,
    elements: Vec<(Doc, &Node)>,
    dangling: &[Comment],
    dialect: Dialect,
) -> Doc {
    if elements.is_empty() && dangling.is_empty() {
        return Doc::text(format!("{open}{close}"));
    }
    let count = elements.len();
    let mut inner = Vec::new();
    for (i, (doc, n)) in elements.into_iter().enumerate() {
        if i == 0 {
            inner.push(pad.clone());
        } else {
            inner.push(Doc::Line);
            if n.blank_before {
                inner.push(Doc::IfBreak(Box::new(Doc::SoftLine), Box::new(Doc::text(""))));
            }
        }
        inner.extend(leading(&n.leading));
        inner.push(doc);
        if i.saturating_add(1) < count {
            inner.push(Doc::text(","));
        } else if dialect == Dialect::Json5 {
            inner.push(Doc::IfBreak(Box::new(Doc::text(",")), Box::new(Doc::text(""))));
        }
        inner.extend(trailing(&n.trailing));
    }
    for (i, c) in dangling.iter().enumerate() {
        inner.push(Doc::HardLine);
        if (count > 0 || i > 0) && c.newlines_before >= 2 {
            inner.push(Doc::HardLine);
        }
        inner.push(comment(c));
    }
    Doc::Group(vec![Doc::text(open), Doc::Indent(inner), pad, Doc::text(close)])
}

#[cfg(test)]
mod tests {
    use super::super::syntax::parse;
    use super::*;

    fn fmt(text: &str, dialect: Dialect) -> String {
        crate::layout::render(&document(&parse(text, dialect).unwrap(), dialect))
    }

    #[test]
    fn a_short_value_is_one_line() {
        assert_eq!(fmt("{\"a\":[1,2],   \"b\":{}}", Dialect::Json), "{ \"a\": [1, 2], \"b\": {} }\n");
    }

    #[test]
    fn a_long_value_breaks_and_json5_gains_trailing_commas() {
        let long = "x".repeat(70);
        let text = format!("{{\"a\": \"{long}\", \"b\": [1, 2]}}");
        assert_eq!(fmt(&text, Dialect::Json), format!("{{\n    \"a\": \"{long}\",\n    \"b\": [1, 2]\n}}\n"));
        assert_eq!(fmt(&text, Dialect::Json5), format!("{{\n    \"a\": \"{long}\",\n    \"b\": [1, 2],\n}}\n"));
    }

    #[test]
    fn comments_survive_and_trailing_commas_go_in_jsonc() {
        let text = "// head\n{\n  // above\n  \"a\": 1, // beside\n\n  \"b\": 2,\n  // below\n}\n";
        assert_eq!(
            fmt(text, Dialect::Jsonc),
            "// head\n{\n    // above\n    \"a\": 1, // beside\n\n    \"b\": 2\n    // below\n}\n"
        );
    }

    #[test]
    fn the_output_is_a_fixed_point() {
        let text = "/* a\n * b\n */\n{a: [1, /* x */ 2,], 'b': {c: null}, // end\n}";
        let once = fmt(text, Dialect::Json5);
        assert_eq!(fmt(&once, Dialect::Json5), once);
    }
}
