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
//! **A result id stands for what the report was computed from** — the same key
//! the analysis cache is filed under. For a document that is the closure the
//! file is in, so a keystroke in a library this file cannot see leaves its
//! report unchanged; for a workspace report it is one id per file, standing for
//! that file's findings. Either way an answer is unchanged exactly when every
//! byte behind it is, so deciding costs a hash rather than a compilation. See
//! `State::current_result_id` for why what goes on the wire is a counter and
//! not the hash itself.

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
    if state.root_of(&path).is_none() {
        return vec![super::response(id, full(None, Vec::new(), Published::new()))];
    }
    // The id this document's last report went out with, when what that report
    // was computed from — this file's closure, and nothing else in the
    // repository — has not moved since.
    if let Some(current) = state.current_result_id(&path) {
        if params.get("previousResultId").and_then(|v| v.as_str()) == Some(current.as_str()) {
            return vec![super::response(
                id,
                Value::object(vec![
                    ("kind", Value::str("unchanged")),
                    ("resultId", Value::str(&current)),
                ]),
            )];
        }
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
    let mut related = if state.related_documents_supported { found } else { Published::new() };
    if state.related_documents_supported {
        retract(state, &uri, &mut related);
    }
    // Issued after the report and not before it: the closure the id stands for
    // is the one this analysis just read, and on the first pull about a file
    // there was no analysis to read it from yet.
    let result_id = state.issue_result_id(&path);
    vec![super::response(id, full(result_id.as_deref(), items, related))]
}

/// Names again, with empty `items`, every related document the last report for
/// this document carried findings for and this one does not.
///
/// A report saying "nothing here" is how a client is told the error it was
/// showing is fixed — the rule the asked-about document is already seeded with.
/// A related file that simply vanished from the map told the client nothing, so
/// the squiggle stayed on screen after the fix.
fn retract(state: &mut State, asked: &str, related: &mut Published) {
    for uri in state.related_reported.get(asked).cloned().unwrap_or_default() {
        if uri != asked {
            related.entry(uri).or_default();
        }
    }
    // Only the ones with findings are worth remembering: an empty entry has
    // just been sent, and sending it forever would be reporting on files this
    // document has nothing to say about.
    let named =
        related.iter().filter(|(_, items)| !items.is_empty()).map(|(uri, _)| uri.clone()).collect();
    state.related_reported.insert(asked.to_string(), named);
}

/// The answer to one `workspace/diagnostic`: every file of every open
/// repository.
///
/// **Every file, not every file with a finding.** A report is how a client is
/// told that a file it was showing squiggles for is clean now, so a repository
/// whose last error was just fixed has to name that file and say `items: []`.
/// The list is the one the fingerprint walks — every `.buri` file under the
/// root, which is sources, every `BUILD.buri` and `REPO.buri`, and every
/// `.proto` schema beside them — so what is reported on and what the result id
/// is computed from cannot drift apart.
pub fn workspace(state: &mut State, params: &Value) -> Value {
    let quoted = previous_result_ids(params);
    let mut items = Vec::new();
    for root in state.roots.clone() {
        let mut found = Published::new();
        let mut files = Vec::new();
        crate::commands::format::collect_with_schemas(&root, &mut files);
        for file in files {
            let uri = convert::uri_of(&file);
            // A build file's own syntax, which no analysis reports: the report
            // walks every `BUILD.buri` in the repository, so this is the one
            // request that finds an unreadable one nobody has open.
            let items = build_file_findings(state, &file, &uri);
            found.insert(uri, items);
        }
        // Both passes over every target — which is the whole point of asking
        // about the repository rather than about a file. Per target and not as
        // one compilation, because that is the unit a keystroke invalidates:
        // see `State::workspace_findings`.
        if let Some(reported) = state.workspace_findings(&root) {
            for (uri, items) in reported {
                found.entry(uri).or_default().extend(items);
            }
        }
        for (uri, diagnostics) in found {
            // The same id a `textDocument/diagnostic` about this file would
            // carry: one per closure, so an edit in a library a file cannot see
            // leaves its entry three fields long instead of its whole report
            // again — and a client that quotes the id back at either request
            // gets the same answer. For a target the sweep has not caught up
            // with it is the id its last report went out with, because that is
            // the state this entry describes.
            let Some(result_id) =
                convert::path_of(&uri).and_then(|path| state.reported_result_id(&path))
            else {
                continue;
            };
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

/// One build file's syntax errors, read from the buffer where the editor has
/// one and from the disk otherwise. Empty for every other kind of file.
fn build_file_findings(state: &State, path: &std::path::Path, uri: &str) -> Vec<Value> {
    if !super::build_files::is_build_file(path) {
        return Vec::new();
    }
    let Some(text) = state.text_of(path) else { return Vec::new() };
    super::build_files::diagnostics(&text)
        .iter()
        .map(|d| convert::diagnostic(&text, d, uri))
        .collect()
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
