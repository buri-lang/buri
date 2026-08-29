//! `workspace/executeCommand` — the verbs behind the lenses.
//!
//! Every other request in this server answers a question. These three *do*
//! something, and each is a call into an entry point `buri` already has at the
//! terminal: `buri test` with a filter, and `buri gen`.
//!
//! **A command that edits writes through the client.** The server has no
//! business writing a file an editor may be holding unsaved, so the edit goes
//! out as a `workspace/applyEdit` request and the command's own answer is what
//! the client says it did with it — which means the command is not finished
//! when its handler returns. See [`regenerate_build_file`].

use super::code_lens;
use super::convert;
use super::state::State;
use crate::build::regenerate;
use crate::json::Value;

/// Rewrite a package's `BUILD.buri` the way `buri gen` would.
pub const REGENERATE: &str = "buri.regenerateBuildFile";

/// Every command this server implements, in the order `capabilities()`
/// advertises them. A client is entitled to send only what is in this list, so
/// the list and the dispatch below are the same three names or one of them is a
/// promise nothing keeps.
pub const COMMANDS: [&str; 3] = [code_lens::RUN_TEST, REGENERATE, code_lens::SHOW_REFERENCES];

pub fn execute(state: &mut State, id: &Value, params: &Value) -> Vec<Value> {
    let command = params.get("command").and_then(|c| c.as_str()).unwrap_or("");
    let empty: &[Value] = &[];
    let arguments = params.get("arguments").and_then(|a| a.as_array()).unwrap_or(empty);
    match command {
        code_lens::RUN_TEST => run_test(state, id, arguments),
        REGENERATE => regenerate_build_file(state, id, arguments),
        // The command a reference lens carries, and it is deliberately a
        // no-op. What the lens has to say is in its arguments — the uri, the
        // position and the `Location[]` the count counted — and showing a list
        // of places is the client's affordance, not something a server can do
        // to an editor. The command exists so that the lens is well formed: a
        // `CodeLens` with a title and no command is not one, and naming a
        // command the server does not implement would be naming one it can be
        // asked for and would refuse.
        code_lens::SHOW_REFERENCES => vec![super::response(id, Value::Null)],
        _ => vec![super::error(
            id,
            super::INVALID_PARAMS,
            &format!("`{command}` is not a command this server implements"),
        )],
    }
}

/// `buri.runTest`: the file the test is in, and the sentence it was written
/// with.
///
/// The file is what says which repository and which target — the same rule
/// every other request follows — and the sentence is what `--filter` takes.
///
/// This compiles and links a test binary from a language-server request, which
/// is the most expensive thing any of them does and is exactly what the reader
/// asked for by clicking the lens. The transcript goes out as a
/// `window/logMessage`, because that is where a client puts several lines, and
/// the last line of it as a `window/showMessage`, because that is where a
/// client puts one. The compiler's own diagnostics are not in either: they go
/// to this process's standard error, and the squiggles for that file are
/// already on screen from the analysis.
fn run_test(state: &mut State, id: &Value, arguments: &[Value]) -> Vec<Value> {
    let path = arguments.first().and_then(|v| v.as_str()).and_then(convert::path_of);
    let name = arguments.get(1).and_then(|v| v.as_str());
    let (Some(path), Some(name)) = (path, name) else {
        return vec![super::error(
            id,
            super::INVALID_PARAMS,
            "`buri.runTest` takes the file's uri and the test's name",
        )];
    };
    let Some(root) = state.root_of(&path) else {
        return vec![super::error(
            id,
            super::REQUEST_FAILED,
            "that file is in no repository this client has open",
        )];
    };
    let Some(label) = state.target_label(&path) else {
        return vec![super::error(
            id,
            super::REQUEST_FAILED,
            "no target in that repository owns that file",
        )];
    };
    let (code, transcript) = crate::commands::test::run_one(&root, &label, name);
    let transcript = transcript.trim_end().to_string();
    let summary = transcript.lines().last().unwrap_or("the suite did not run");
    vec![
        super::message(
            "window/logMessage",
            super::INFO,
            &format!("buri test {label} --filter \"{name}\"\n{transcript}"),
        ),
        super::message(
            "window/showMessage",
            if code == 0 { super::INFO } else { super::ERROR },
            &format!("{label}: {summary}"),
        ),
        // What the run did is in the two messages above, which is where a
        // client renders it. The result is the protocol's acknowledgement.
        super::response(id, Value::Null),
    ]
}

/// `buri.regenerateBuildFile`: a file in the repository, and the package label.
///
/// Two arguments rather than the label alone, and the first one is why: a label
/// is repository-relative, and a client holding two repositories open would be
/// naming a package in both of them. The uri says which repository, exactly as
/// it does for every other request.
///
/// The answer is deferred. `regenerate` produces the whole file `buri gen`
/// would write, that goes out as a `workspace/applyEdit`, and the command's
/// result is the `ApplyWorkspaceEditResult` the client sends back — so
/// "regenerated" means the editor wrote it rather than that the server asked.
/// See `client_response` in `mod.rs` for the other half.
fn regenerate_build_file(state: &mut State, id: &Value, arguments: &[Value]) -> Vec<Value> {
    let path = arguments.first().and_then(|v| v.as_str()).and_then(convert::path_of);
    let label = arguments.get(1).and_then(|v| v.as_str());
    let (Some(path), Some(label)) = (path, label) else {
        return vec![super::error(
            id,
            super::INVALID_PARAMS,
            "`buri.regenerateBuildFile` takes a uri in the repository and a package label",
        )];
    };
    let failed = |why: &str| vec![super::error(id, super::REQUEST_FAILED, why)];
    let Some(root) = state.root_of(&path) else {
        return failed("that file is in no repository this client has open");
    };
    let Some(mut session) = state.overlaid_session(&root) else {
        return failed("that repository would not load");
    };
    let Some(package) = session.workspace.package_by_path(label.trim_start_matches('/')) else {
        return failed(&format!("`{label}` names no package in that repository"));
    };
    let build = session.workspace.package(package).build_path.clone();
    let update = match regenerate::regenerate(&mut session, package) {
        Ok(Some(update)) => update,
        // Nothing to do is not a failure, and saying so is the whole of what
        // the command owes a reader who invoked it from a palette.
        Ok(None) => {
            return vec![
                super::message(
                    "window/showMessage",
                    super::INFO,
                    &format!("{label}: BUILD.buri is already what buri gen would write"),
                ),
                super::response(id, Value::Null),
            ];
        }
        Err(d) => return failed(&d.message),
    };
    // The text the range is measured against is the one `regenerate` read, and
    // the buffer wins over the disk for as long as the editor holds it — the
    // same overlay every analysis runs on.
    let text = session
        .map
        .find(&session.workspace.rel_of(&build))
        .map(|file| session.map.get(file).text.clone())
        .unwrap_or_default();
    state.applying.sent = state.applying.sent.saturating_add(1);
    let edit_id = format!("{}/{}", super::APPLY_EDIT, state.applying.sent);
    state.applying.waiting.insert(edit_id.clone(), id.clone());
    vec![super::request(
        &edit_id,
        "workspace/applyEdit",
        Value::object(vec![
            ("label", Value::str(format!("buri gen {label}: {}", update.summary.join(", ")))),
            (
                "edit",
                Value::object(vec![(
                    "changes",
                    Value::object(vec![(
                        convert::uri_of(&build).as_str(),
                        Value::Array(vec![Value::object(vec![
                            ("range", super::whole(&text)),
                            ("newText", Value::str(&update.text)),
                        ])]),
                    )]),
                )]),
            ),
        ]),
    )]
}
