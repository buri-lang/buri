//! The fixes for the findings the client is asking about, in two halves.
//!
//! Two sources, and they are the same two `buri lint --fix` has: a finding that
//! carries byte edits becomes a text edit, and one about a build file is handed
//! to `buri gen`, which writes the whole file. Nothing here invents an answer —
//! a `dep-cycle` has no action, because which edge to cut is a decision.
//!
//! **Why there are two halves.** The list arrives on every cursor move onto a
//! squiggle, and the second source is expensive in a way the first is not:
//! `buri gen` opens a writable session of its own and re-analyses the package
//! to derive its dependencies, and that work is not cached by anything. So the
//! list is titles and the association with the squiggle, and the edit is
//! computed once, for the one action a reader accepted. That is what
//! `resolveProvider: true` claims.
//!
//! A client that cannot resolve gets the edit in the list, as before. The claim
//! is read from its own `initialize`: a server that deferred an edit to a
//! request the client will never send would be offering a fix that does
//! nothing.

use crate::build::regenerate;
use crate::build::session::Session;
use crate::json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use super::convert;
use super::state::State;

/// One offered fix, before it is written as protocol.
struct Action {
    title: String,
    /// The lint the fix is for, which is what associates it with a squiggle.
    code: String,
    /// `None` when the edit was left for `codeAction/resolve`.
    edit: Option<Value>,
}

/// `textDocument/codeAction`.
pub fn list(state: &mut State, params: &Value) -> Value {
    let Some(path) = super::uri_param(params) else { return Value::Array(Vec::new()) };
    let asked: Vec<&Value> = params
        .at("context.diagnostics")
        .and_then(|d| d.as_array())
        .map(|d| d.iter().collect())
        .unwrap_or_default();
    let wanted: Vec<String> = asked
        .iter()
        .filter_map(|d| d.get("code").and_then(|c| c.as_str()).map(String::from))
        .collect();
    if wanted.is_empty() {
        return Value::Array(Vec::new());
    }
    let with_edits = !state.code_action_resolve;
    let uri = convert::uri_of(&path);
    Value::Array(
        found(state, &path, &wanted, with_edits)
            .iter()
            .map(|action| rendered(action, &uri, &asked))
            .collect(),
    )
}

/// `codeAction/resolve`. The action comes back as it went out and is computed
/// again from the `data` it carries — the same round trip an inlay hint and a
/// code lens make, and for the same reason: a server that remembered every
/// action it had offered would hold a table nothing removes an entry from.
pub fn resolve(state: &mut State, action: &Value) -> Value {
    let mut resolved = action.clone();
    let edit = (|| {
        let path = action.at("data.uri").and_then(|u| u.as_str()).and_then(convert::path_of)?;
        let code = action.at("data.code")?.as_str()?.to_string();
        let title = action.at("data.title")?.as_str()?;
        // Two findings of one lint are two actions with two titles, so the
        // title is what tells them apart.
        found(state, &path, &[code], true)
            .into_iter()
            .find(|candidate| candidate.title == title)?
            .edit
    })();
    // An action with nothing to resolve is still a legal answer to this
    // request, and it is the action itself.
    let (Some(edit), Value::Object(fields)) = (edit, &mut resolved) else { return resolved };
    fields.insert("edit".to_string(), edit);
    resolved
}

/// The protocol's shape for one action.
///
/// `diagnostics` is what associates the fix with the squiggle it is under —
/// this used to be a `diagnosticCode` string, which is not a field the protocol
/// has and which no client could read.
fn rendered(action: &Action, uri: &str, asked: &[&Value]) -> Value {
    let mut fields = vec![
        ("title", Value::str(action.title.clone())),
        // "quickfix" — the kind a client offers without being asked twice.
        ("kind", Value::str("quickfix")),
        (
            "diagnostics",
            Value::Array(
                asked
                    .iter()
                    .filter(|d| {
                        d.get("code").and_then(|c| c.as_str()) == Some(action.code.as_str())
                    })
                    .map(|d| (*d).clone())
                    .collect(),
            ),
        ),
    ];
    match &action.edit {
        Some(edit) => fields.push(("edit", edit.clone())),
        None => fields.push((
            "data",
            Value::object(vec![
                ("uri", Value::str(uri)),
                ("code", Value::str(action.code.clone())),
                ("title", Value::str(action.title.clone())),
            ]),
        )),
    }
    Value::object(fields)
}

/// Every fix the lint pass offers for the codes asked about.
///
/// `with_edits` is the whole difference between the two halves: without it this
/// reads the findings the lint pass already computed and stops, and with it the
/// build-file family also runs `buri gen`.
fn found(state: &mut State, path: &Path, wanted: &[String], with_edits: bool) -> Vec<Action> {
    let Some(linted) = state.lint(path) else { return Vec::new() };
    let session = &linted.session;
    let mut out = Vec::new();
    let mut regenerated: BTreeSet<String> = BTreeSet::new();

    for d in &linted.diagnostics.items {
        let Some(code) = d.code.as_deref() else { continue };
        if !wanted.iter().any(|w| w == code) {
            continue;
        }

        // A finding that already knows its bytes.
        if !d.edits.is_empty() {
            if !d.edits.iter().any(|e| !session.map.get(e.at.file).abs_path.as_os_str().is_empty())
            {
                continue;
            }
            out.push(Action {
                title: d.fix.as_deref().unwrap_or(code).to_string(),
                code: code.to_string(),
                edit: with_edits.then(|| byte_edits(session, d)),
            });
            continue;
        }

        // A finding whose answer is a build file `buri gen` already writes.
        if !matches!(code, "missing-dep" | "unused-library" | "duplicate-source") {
            continue;
        }
        let Some(package) = package_of(session, d.span.file, path) else { continue };
        let where_it_is = session.workspace.package(package).path.clone();
        if !regenerated.insert(where_it_is.clone()) {
            continue;
        }
        // The summary of what changed is not in the title, because reading it
        // means writing the file first — which is the work this half exists to
        // put off. What `buri gen` writes is the whole answer either way.
        let title = format!("{where_it_is}/BUILD.buri: as `buri gen` would write it");
        if !with_edits {
            out.push(Action { title, code: code.to_string(), edit: None });
            continue;
        }
        if let Some(edit) = regenerated_build_file(state, path, package) {
            out.push(Action { title, code: code.to_string(), edit: Some(edit) });
        }
    }
    out
}

/// A finding's own bytes, as a `WorkspaceEdit`.
fn byte_edits(session: &Session, d: &crate::diagnostics::Diagnostic) -> Value {
    let mut by_file: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for e in &d.edits {
        let f = session.map.get(e.at.file);
        if f.abs_path.as_os_str().is_empty() {
            continue;
        }
        let range = Value::object(vec![
            ("start", convert::position_of(&f.text, e.at.start).to_json()),
            ("end", convert::position_of(&f.text, e.at.end).to_json()),
        ]);
        by_file.entry(convert::uri_of(&f.abs_path)).or_default().push(Value::object(vec![
            ("range", range),
            ("newText", Value::str(&e.replacement)),
        ]));
    }
    changes(by_file)
}

/// The whole `BUILD.buri` `buri gen` would write for a package.
///
/// `buri gen` re-analyses the package to derive its dependencies and writes
/// through the session it is handed, so it gets one of its own: the lint
/// session is shared with everything else holding that answer.
fn regenerated_build_file(
    state: &mut State,
    path: &Path,
    package: crate::build::workspace::PackageId,
) -> Option<Value> {
    let root = state.root_of(path)?;
    let mut writable = state.overlaid_session(&root)?;
    let update = regenerate::regenerate(&mut writable, package).ok()??;
    let build = writable.workspace.package(package).build_path.clone();
    let id = writable.map.find(&writable.workspace.rel_of(&build))?;
    let text = writable.map.get(id).text.clone();
    let mut by_file = BTreeMap::new();
    by_file.insert(
        convert::uri_of(&build),
        vec![Value::object(vec![
            ("range", super::whole(&text)),
            ("newText", Value::str(&update.text)),
        ])],
    );
    Some(changes(by_file))
}

fn changes(by_file: BTreeMap<String, Vec<Value>>) -> Value {
    Value::object(vec![(
        "changes",
        Value::Object(by_file.into_iter().map(|(k, v)| (k, Value::Array(v))).collect()),
    )])
}

/// The package a diagnostic is about: the one owning the file it points at.
fn package_of(
    session: &Session,
    file: crate::diagnostics::FileId,
    fallback: &Path,
) -> Option<crate::build::workspace::PackageId> {
    let f = session.map.get(file);
    if !f.abs_path.as_os_str().is_empty() {
        if let Some(p) = session.workspace.owning_package(&f.abs_path) {
            return Some(p);
        }
    }
    session.workspace.owning_package(fallback)
}
