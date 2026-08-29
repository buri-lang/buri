//! `textDocument/diagnostic` and `workspace/diagnostic` — the findings, asked
//! for rather than pushed.
//!
//! The protocol has two ways of saying the same thing and the server does both,
//! which is what it allows. Push is what a client gets without asking, on an
//! open and on a save; pull is what a client asks for, when it decides and for
//! the document it decides about. The producers are the same two either way —
//! `driver::analyze` and the lint pass, through [`super::findings_for`] — so
//! there is no second opinion to drift: what a pull reports about a file is
//! byte for byte what a publish about it would have carried, `REPO.buri`'s
//! `fail_on_finding` promotion included.
//!
//! **What pull is *for* here.** A push can only ever be about a file the editor
//! opened, and a Buri repository reports most of what it knows about a module
//! from somewhere else: a missing dependency is a fact about a `BUILD.buri`
//! nobody has open, and a type error in a library is found while checking the
//! binary that imports it. `workspace/diagnostic` is the request with a shape
//! for that — one report per file in the repository, including every file the
//! editor has never seen — and it is why `workspaceDiagnostics: true` is
//! advertised rather than left to the default.
//!
//! **A result id stands for an analysis fingerprint** — the same key the
//! analysis cache is filed under. An answer is unchanged exactly when every
//! byte it was computed from is, so deciding costs a read of the repository
//! rather than a compilation of it. See [`State::diagnostic_result_id`] for why
//! what goes on the wire is a counter and not the hash itself.

use super::convert;
use super::state::State;
use super::{Published, INVALID_PARAMS};
use crate::json::Value;

/// The answer to one `textDocument/diagnostic`.
///
/// A `DocumentDiagnosticReport` is the only legal result: this request has no
/// null, so a message that cannot be read is a JSON-RPC *error* rather than the
/// catch-all `result: null` an unimplemented method used to get.
pub fn document(state: &mut State, id: &Value, params: &Value) -> Vec<Value> {
    let Some(path) = super::uri_param(params) else {
        return vec![super::error(
            id,
            INVALID_PARAMS,
            "textDocument/diagnostic needs a `textDocument.uri` naming a file",
        )];
    };
    // A file in no open repository. An empty full report is the honest answer —
    // the server knows nothing about it — and it carries no `resultId`, because
    // there is no analysis behind it for a later request to quote.
    let Some(root) = state.root_of(&path) else {
        return vec![super::response(id, full(None, Vec::new(), Published::new()))];
    };
    let result_id = state.diagnostic_result_id(&root);
    if params.get("previousResultId").and_then(|v| v.as_str()) == Some(result_id.as_str()) {
        return vec![super::response(
            id,
            Value::object(vec![
                ("kind", Value::str("unchanged")),
                ("resultId", Value::str(&result_id)),
            ]),
        )];
    }

    let uri = convert::uri_of(&path);
    // Seeded with the asked-about file, so a file with nothing wrong answers
    // with an empty `items` rather than with no entry at all.
    let mut found = Published::new();
    found.insert(uri.clone(), Vec::new());
    super::findings_for(state, &path, &mut found);
    let items = found.remove(&uri).unwrap_or_default();
    // Everything else the analysis of this file's closure had to say. The
    // protocol's own field for it, and a client that did not claim it gets
    // nothing extra — `workspace/diagnostic` is the answer for that client.
    let related = if state.related_documents_supported { found } else { Published::new() };
    vec![super::response(id, full(Some(&result_id), items, related))]
}

/// The answer to one `workspace/diagnostic`: every file of every open
/// repository.
///
/// **Every file, not every file with a finding.** A report is how a client is
/// told that a file it was showing squiggles for is clean now, so a repository
/// whose last error was just fixed has to name that file and say `items: []`.
/// The list is the one the fingerprint walks — every `.buri` file under the
/// root, which is sources, every `BUILD.buri` and `REPO.buri` — so what is
/// reported on and what the result id is computed from cannot drift apart.
pub fn workspace(state: &mut State, params: &Value) -> Value {
    let quoted = previous_result_ids(params);
    let mut items = Vec::new();
    for root in state.roots.clone() {
        // One id per repository rather than per file: the front end is
        // whole-closure, so there is no file whose answer an edit elsewhere
        // provably does not change. See `State::fingerprint`.
        let result_id = state.diagnostic_result_id(&root);
        let mut found = Published::new();
        let mut files = Vec::new();
        crate::commands::format::collect(&root, &mut files);
        for file in files {
            found.insert(convert::uri_of(&file), Vec::new());
        }
        // One compilation over every target, and one lint pass over every
        // target — which is the whole point of asking about the repository
        // rather than about a file.
        if let Some(analyzed) = state.analyze_workspace(&root) {
            for d in &analyzed.analysis.diagnostics.items {
                super::add_finding(&mut found, &analyzed.session, d);
            }
        }
        if let Some(linted) = state.lint_workspace(&root) {
            for d in &linted.diagnostics.items {
                super::add_finding(&mut found, &linted.session, d);
            }
        }
        for (uri, diagnostics) in found {
            let unchanged = quoted.iter().any(|(u, v)| *u == uri && *v == result_id);
            items.push(if unchanged {
                Value::object(vec![
                    ("kind", Value::str("unchanged")),
                    ("resultId", Value::str(&result_id)),
                    ("uri", Value::str(&uri)),
                    ("version", version()),
                ])
            } else {
                Value::object(vec![
                    ("kind", Value::str("full")),
                    ("items", Value::Array(diagnostics)),
                    ("resultId", Value::str(&result_id)),
                    ("uri", Value::str(&uri)),
                    ("version", version()),
                ])
            });
        }
    }
    Value::object(vec![("items", Value::Array(items))])
}

/// The document version a workspace report is about.
///
/// Always `null`, which the protocol spells as "the server does not know". It
/// does not: the sync is full-text, so what `State::open` holds is a buffer's
/// text and never the number the editor counts edits with, and inventing one
/// would be a claim a later `didChange` could contradict.
fn version() -> Value {
    Value::Null
}

/// One `FullDocumentDiagnosticReport`, with the related documents a client that
/// asked for them gets.
fn full(result_id: Option<&str>, items: Vec<Value>, related: Published) -> Value {
    let mut fields = vec![("kind", Value::str("full")), ("items", Value::Array(items))];
    if let Some(result_id) = result_id {
        fields.push(("resultId", Value::str(result_id)));
    }
    if !related.is_empty() {
        fields.push((
            "relatedDocuments",
            Value::Object(
                related
                    .into_iter()
                    .map(|(uri, items)| {
                        // No `resultId` on a related report: it is not a
                        // document the client asked about, so there is nothing
                        // it could quote the id back on.
                        (uri, full(None, items, Published::new()))
                    })
                    .collect(),
            ),
        ));
    }
    Value::object(fields)
}

/// The `previousResultIds` a `workspace/diagnostic` carries, as pairs.
///
/// A malformed entry is dropped rather than refused: the field is a client's
/// cache and the worst an unreadable entry can cost is a full report where an
/// unchanged one would have done.
fn previous_result_ids(params: &Value) -> Vec<(String, String)> {
    let Some(items) = params.get("previousResultIds").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let uri = item.get("uri")?.as_str()?;
            // A client may spell the uri differently to `uri_of` — an escaped
            // byte, a different case. Round-tripping it through the path is
            // what makes the comparison about the file rather than the string.
            let normalised =
                convert::path_of(uri).map_or_else(|| uri.to_string(), |p| convert::uri_of(&p));
            Some((normalised, item.get("value")?.as_str()?.to_string()))
        })
        .collect()
}
