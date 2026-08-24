//! `buri lsp` — the language server, over stdio.
//!
//! The analysis it serves is the one `buri build` runs; what this module adds
//! is the protocol and the scheduling around it.
//!
//! **Scheduling, stated rather than tuned.** A keystroke re-parses the one
//! buffer that changed and publishes the parse errors — microseconds, no
//! workspace, no standard library. A save runs `driver::analyze` over the whole
//! closure and publishes everything. That split is deliberate: the front end
//! has no incremental mode, so analysing per keystroke would be analysing the
//! standard library per keystroke.
//!
//! **stdout carries protocol and nothing else.** Every log goes to stderr. A
//! stray `println!` in a code path this reaches would corrupt the stream in a
//! way that looks like the editor is broken, so it is worth knowing that the
//! rule is absolute.
//!
//! Requests are handled one at a time, in the order they arrive. That makes
//! responses deterministic, which is what lets a test record a session as a
//! golden file.

mod convert;
mod features;
mod state;

use convert::Position;
use crate::build::regenerate;
use crate::build::session::Session;
use crate::commands::arguments;
use crate::json::{self, Value};
use state::State;
use std::io::{Read, Write};
use std::path::PathBuf;

#[expect(
    clippy::print_stderr,
    reason = "a message that did not parse has no id to answer and no Session to route through, and stdout carries protocol only — stderr is the log channel this server is specified to have"
)]
pub fn cmd_lsp(_args: &arguments::Args) -> i32 {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();

    let mut state = State::new();

    loop {
        match read_message(&mut input) {
            Ok(None) => return 0,
            Ok(Some(text)) => {
                let message = match json::parse(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("buri lsp: unparseable message: {e}");
                        continue;
                    }
                };
                for reply in handle(&mut state, &message) {
                    write_message(&mut output, &reply);
                }
                // Whether the loop is over is the lifecycle's answer, not a
                // second reading of the method string.
                if let Some(code) = state.lifecycle.exit_code() {
                    return code;
                }
            }
            Err(e) => {
                eprintln!("buri lsp: {e}");
                return 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// One `Content-Length: N\r\n\r\n<N bytes>` message, or `None` at end of input.
///
/// Other headers are read and ignored — `Content-Type` is the one that turns
/// up — and the length is what decides where the body ends, so a body
/// containing `\r\n\r\n` is not a problem.
fn read_message(input: &mut impl Read) -> Result<Option<String>, String> {
    let mut length: Option<usize> = None;
    loop {
        let line = match read_line(input)? {
            None => return Ok(None),
            Some(l) => l,
        };
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            length = v.trim().parse().ok();
        }
    }
    let Some(length) = length else {
        return Err("a message arrived with no Content-Length".into());
    };
    // Read up to the declared length rather than allocating it. The header is
    // a number someone else wrote: `Content-Length: 18446744073709551615`
    // reserved that many bytes and aborted the process on the allocation,
    // which is the one failure mode a server must not have. Growing as the
    // bytes actually arrive makes an absurd header a short read and a message
    // instead.
    let mut body = Vec::new();
    input
        .by_ref()
        .take(length as u64)
        .read_to_end(&mut body)
        .map_err(|e| format!("reading a {length}-byte body: {e}"))?;
    if body.len() != length {
        return Err(format!(
            "a message declared {length} bytes and the stream ended after {}",
            body.len()
        ));
    }
    String::from_utf8(body).map(Some).map_err(|e| format!("a message is not UTF-8: {e}"))
}

/// Byte at a time, because the body that follows the headers must not be
/// swallowed by a buffered reader that read ahead.
fn read_line(input: &mut impl Read) -> Result<Option<String>, String> {
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match input.read(&mut byte) {
            Ok(0) => return Ok(if out.is_empty() { None } else { Some(String::new()) }),
            Ok(_) => {
                out.push(byte[0]);
                if byte[0] == b'\n' {
                    return String::from_utf8(out)
                        .map(Some)
                        .map_err(|e| format!("a header is not UTF-8: {e}"));
                }
            }
            Err(e) => return Err(format!("reading a header: {e}")),
        }
    }
}

fn write_message(out: &mut impl Write, v: &Value) {
    let body = v.to_string();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = out.flush();
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

fn response(id: &Value, result: Value) -> Value {
    Value::obj(vec![
        ("jsonrpc", Value::str("2.0")),
        ("id", id.clone()),
        ("result", result),
    ])
}

/// A JSON-RPC error reply. A request that cannot be served still gets an
/// answer, because a client that got nothing waits forever.
fn error(id: &Value, code: i64, message: &str) -> Value {
    Value::obj(vec![
        ("jsonrpc", Value::str("2.0")),
        ("id", id.clone()),
        (
            "error",
            Value::obj(vec![("code", Value::num(code)), ("message", Value::str(message))]),
        ),
    ])
}

/// `ServerNotInitialized`, from the protocol's own table of error codes.
const NOT_INITIALIZED: i64 = -32002;

/// `InvalidRequest`.
const INVALID_REQUEST: i64 = -32600;

fn notification(method: &str, params: Value) -> Value {
    Value::obj(vec![
        ("jsonrpc", Value::str("2.0")),
        ("method", Value::str(method)),
        ("params", params),
    ])
}

fn handle(state: &mut State, msg: &Value) -> Vec<Value> {
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    // The lifecycle answers first. Everything below the three lifecycle
    // methods needs an initialized server, and used to be served without one —
    // against whatever directory the process was started in.
    match (method, &id) {
        ("initialize" | "initialized" | "shutdown" | "exit", _) => {}
        (_, Some(id)) if !state.lifecycle.is_running() => {
            return vec![error(id, NOT_INITIALIZED, "the server has not been initialized")];
        }
        (_, None) if !state.lifecycle.is_running() => return vec![],
        _ => {}
    }

    match (method, id) {
        ("initialize", Some(id)) => {
            let root = params
                .get("rootUri")
                .and_then(|u| u.as_str())
                .and_then(convert::path_of)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            if let Err(why) = state.lifecycle.initialize() {
                return vec![error(&id, INVALID_REQUEST, why)];
            }
            // The rest of the toolchain finds the repository from the working
            // directory, so the server moves to it once rather than teaching
            // every call site about a root.
            let _ = std::env::set_current_dir(&root);
            vec![response(&id, capabilities())]
        }
        ("initialized", _) => vec![],
        ("shutdown", Some(id)) => match state.lifecycle.shutdown() {
            Ok(()) => vec![response(&id, Value::Null)],
            Err(why) => vec![error(&id, INVALID_REQUEST, why)],
        },
        ("exit", _) => {
            state.lifecycle.exit();
            vec![]
        }

        ("textDocument/didOpen", _) => {
            let Some((path, text)) = opened(&params) else { return vec![] };
            state.open.insert(path.clone(), text);
            full_diagnostics(state, &path)
        }
        ("textDocument/didChange", _) => {
            let Some(path) = uri_param(&params) else { return vec![] };
            // Full sync: the last content change is the whole buffer.
            let Some(text) = params
                .at("contentChanges")
                .and_then(|c| c.as_array())
                .and_then(|c| c.last())
                .and_then(|c| c.get("text"))
                .and_then(|t| t.as_str())
            else {
                return vec![];
            };
            state.open.insert(path.clone(), text.to_string());
            vec![parse_diagnostics(&path, text)]
        }
        ("textDocument/didSave", _) => {
            let Some(path) = uri_param(&params) else { return vec![] };
            full_diagnostics(state, &path)
        }
        ("textDocument/didClose", _) => {
            if let Some(path) = uri_param(&params) {
                state.open.remove(&path);
            }
            vec![]
        }

        ("textDocument/formatting", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                // The same dispatch `buri format` uses, so the editor and the
                // command cannot disagree: a `BUILD.buri` goes through the
                // textproto printer and everything else through the source
                // formatter. Either returns `None` for a file that does not
                // parse, so a file mid-edit is left alone rather than mangled.
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let formatted = crate::commands::format::file(&name, &text)?;
                if formatted == text {
                    return None;
                }
                Some(Value::Arr(vec![Value::obj(vec![
                    ("range", whole(&text)),
                    ("newText", Value::str(formatted)),
                ])]))
            })();
            vec![response(&id, result.unwrap_or(Value::Arr(Vec::new())))]
        }

        ("textDocument/documentSymbol", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                Some(features::document_symbols(&text))
            })();
            vec![response(&id, result.unwrap_or(Value::Arr(Vec::new())))]
        }

        ("textDocument/hover", Some(id)) => {
            let result = with_analysis(state, &params, |a, path, text, pos| {
                features::hover(a, path, text, pos)
            });
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        ("textDocument/definition", Some(id)) => {
            let result = with_analysis(state, &params, |a, path, text, pos| {
                features::definition(a, path, text, pos)
            });
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        ("textDocument/completion", Some(id)) => {
            let result = with_analysis(state, &params, |a, path, text, pos| {
                Some(features::completion(a, path, text, pos))
            });
            vec![response(&id, result.unwrap_or(Value::Arr(Vec::new())))]
        }

        ("textDocument/codeAction", Some(id)) => {
            vec![response(&id, code_actions(state, &params))]
        }

        // A request this server does not answer still needs a reply, or the
        // client waits for one that never comes.
        (_, Some(id)) => vec![response(&id, Value::Null)],
        (_, None) => vec![],
    }
}

fn capabilities() -> Value {
    Value::obj(vec![
        (
            "capabilities",
            Value::obj(vec![
                // 1 = full. Incremental sync buys nothing without an
                // incremental front end, and costs a text-edit applier.
                ("textDocumentSync", Value::num(1)),
                ("hoverProvider", Value::Bool(true)),
                ("definitionProvider", Value::Bool(true)),
                ("documentSymbolProvider", Value::Bool(true)),
                ("documentFormattingProvider", Value::Bool(true)),
                ("codeActionProvider", Value::Bool(true)),
                (
                    "completionProvider",
                    Value::obj(vec![(
                        "triggerCharacters",
                        Value::Arr(vec![Value::str("\""), Value::str("{"), Value::str("/")]),
                    )]),
                ),
            ]),
        ),
        (
            "serverInfo",
            Value::obj(vec![
                ("name", Value::str("buri")),
                ("version", Value::str(arguments::VERSION)),
            ]),
        ),
    ])
}

fn uri_param(params: &Value) -> Option<PathBuf> {
    params.at("textDocument.uri").and_then(|u| u.as_str()).and_then(convert::path_of)
}

fn opened(params: &Value) -> Option<(PathBuf, String)> {
    let path = uri_param(params)?;
    let text = params.at("textDocument.text")?.as_str()?.to_string();
    Some((path, text))
}

fn whole(text: &str) -> Value {
    Value::obj(vec![
        ("start", Position { line: 0, character: 0 }.to_json()),
        ("end", convert::position_of(text, text.len() as u32).to_json()),
    ])
}

fn with_analysis<T>(
    state: &mut State,
    params: &Value,
    f: impl Fn(&state::Analyzed, &std::path::Path, &str, Position) -> Option<T>,
) -> Option<T> {
    let path = uri_param(params)?;
    let pos = Position::from_json(params.get("position")?)?;
    let text = state.text_of(&path)?;
    let a = state.analyze(&path)?;
    f(&a, &path, &text, pos)
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// Parse errors for one buffer. No workspace, no standard library, no imports.
fn parse_diagnostics(path: &std::path::Path, text: &str) -> Value {
    let parsed = crate::parsing::parser::parse(text, crate::diagnostics::FileId(0));
    let uri = convert::uri_of(path);
    let items: Vec<Value> =
        parsed.errors.iter().map(|d| convert::diagnostic(text, d, &uri)).collect();
    publish(&uri, items)
}

/// Everything the front end has to say, for every file it looked at.
///
/// Diagnostics are published per file, and a file that had findings a moment
/// ago and has none now must be published *empty* — otherwise the editor keeps
/// showing the errors you just fixed.
fn full_diagnostics(state: &mut State, path: &std::path::Path) -> Vec<Value> {
    let mut published: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    // The file that prompted this always gets a message, empty or not.
    published.insert(convert::uri_of(path), Vec::new());
    for open in state.open.keys() {
        published.insert(convert::uri_of(open), Vec::new());
    }

    let Some(a) = state.analyze(path) else {
        return published.into_iter().map(|(uri, items)| publish(&uri, items)).collect();
    };

    for d in &a.analysis.diags.items {
        if d.span.is_none() {
            continue;
        }
        let f = a.session.map.get(d.span.file);
        if f.abs_path.as_os_str().is_empty() {
            continue;
        }
        let uri = convert::uri_of(&f.abs_path);
        let item = convert::diagnostic(&f.text, d, &uri);
        published.entry(uri).or_default().push(item);
    }

    // The build-graph findings too. An editor that showed only type errors
    // would be showing half of what the toolchain knows, and the half that is
    // easier to notice at the terminal — a missing dependency is exactly the
    // kind of thing you want told about while the import is still on screen.
    if let Some((session, lint)) = state.lint(path) {
        for d in &lint.items {
            if d.span.is_none() {
                continue;
            }
            let f = session.map.get(d.span.file);
            if f.abs_path.as_os_str().is_empty() {
                continue;
            }
            let uri = convert::uri_of(&f.abs_path);
            let item = convert::diagnostic(&f.text, d, &uri);
            let bucket = published.entry(uri).or_default();
            // The lint pass runs its own analysis, so a compile error appears
            // in both. Publishing it twice would put two squiggles on one span.
            if !bucket.iter().any(|existing| same_finding(existing, &item)) {
                bucket.push(item);
            }
        }
    }

    published.into_iter().map(|(uri, items)| publish(&uri, items)).collect()
}

/// Whether two diagnostics are the same one seen twice: same place, same words.
fn same_finding(a: &Value, b: &Value) -> bool {
    a.get("message") == b.get("message") && a.get("range") == b.get("range")
}

fn publish(uri: &str, items: Vec<Value>) -> Value {
    notification(
        "textDocument/publishDiagnostics",
        Value::obj(vec![("uri", Value::str(uri)), ("diagnostics", Value::Arr(items))]),
    )
}


// ---------------------------------------------------------------------------
// Code actions
// ---------------------------------------------------------------------------

/// The fixes for the diagnostics the client is asking about.
///
/// Two sources, and they are the same two `buri lint --fix` has: a finding that
/// carries byte edits becomes a text edit, and one about a build file is handed
/// to `buri gen`, which writes the whole file. Nothing here invents an answer —
/// a `dep-cycle` has no action, because which edge to cut is a decision.
fn code_actions(state: &mut State, params: &Value) -> Value {
    let Some(path) = uri_param(params) else { return Value::Arr(Vec::new()) };
    let asked: Vec<&Value> = params
        .at("context.diagnostics")
        .and_then(|d| d.as_array())
        .map(|d| d.iter().collect())
        .unwrap_or_default();
    if asked.is_empty() {
        return Value::Arr(Vec::new());
    }
    let wanted: Vec<String> = asked
        .iter()
        .filter_map(|d| d.get("code").and_then(|c| c.as_str()).map(String::from))
        .collect();
    if wanted.is_empty() {
        return Value::Arr(Vec::new());
    }

    let Some((mut session, lint)) = state.lint(&path) else { return Value::Arr(Vec::new()) };
    let mut out = Vec::new();
    let mut regenerated: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for d in &lint.items {
        let Some(code) = d.code.as_deref() else { continue };
        if !wanted.iter().any(|w| w == code) {
            continue;
        }

        // A finding that already knows its bytes.
        if !d.edits.is_empty() {
            let mut by_file: std::collections::BTreeMap<String, Vec<Value>> =
                std::collections::BTreeMap::new();
            for e in &d.edits {
                let f = session.map.get(e.at.file);
                if f.abs_path.as_os_str().is_empty() {
                    continue;
                }
                let range = Value::obj(vec![
                    ("start", convert::position_of(&f.text, e.at.start).to_json()),
                    ("end", convert::position_of(&f.text, e.at.end).to_json()),
                ]);
                by_file.entry(convert::uri_of(&f.abs_path)).or_default().push(Value::obj(vec![
                    ("range", range),
                    ("newText", Value::str(&e.replacement)),
                ]));
            }
            if !by_file.is_empty() {
                out.push(action(
                    d.fix.as_deref().unwrap_or(code),
                    code,
                    by_file,
                ));
            }
            continue;
        }

        // A finding whose answer is a build file `buri gen` already writes.
        if !matches!(code, "missing-dep" | "unused-dep" | "undeclared-source" | "duplicate-source")
        {
            continue;
        }
        let Some(pkg) = package_of(&session, d.span.file, &path) else { continue };
        if !regenerated.insert(session.workspace.pkg(pkg).path.clone()) {
            continue;
        }
        let Ok(Some(update)) = regenerate::regenerate(&mut session, pkg) else { continue };
        let build = session.workspace.pkg(pkg).build_path.clone();
        let Some(id) = session.map.find(&session.workspace.rel_of(&build)) else { continue };
        let text = session.map.get(id).text.clone();
        let whole = Value::obj(vec![
            ("start", Position { line: 0, character: 0 }.to_json()),
            ("end", convert::position_of(&text, text.len() as u32).to_json()),
        ]);
        let mut by_file = std::collections::BTreeMap::new();
        by_file.insert(
            convert::uri_of(&build),
            vec![Value::obj(vec![
                ("range", whole),
                ("newText", Value::str(&update.text)),
            ])],
        );
        out.push(action(
            &format!("{}/BUILD.buri: {}", session.workspace.pkg(pkg).path, update.summary.join(", ")),
            code,
            by_file,
        ));
    }

    Value::Arr(out)
}

/// The package a diagnostic is about: the one owning the file it points at.
fn package_of(
    session: &Session,
    file: crate::diagnostics::FileId,
    fallback: &std::path::Path,
) -> Option<crate::build::workspace::PkgId> {
    let f = session.map.get(file);
    if !f.abs_path.as_os_str().is_empty() {
        if let Some(p) = session.workspace.owning_package(&f.abs_path) {
            return Some(p);
        }
    }
    session.workspace.owning_package(fallback)
}

fn action(
    title: &str,
    code: &str,
    edits: std::collections::BTreeMap<String, Vec<Value>>,
) -> Value {
    Value::obj(vec![
        ("title", Value::str(title)),
        // "quickfix" — the kind a client offers without being asked twice.
        ("kind", Value::str("quickfix")),
        ("diagnosticCode", Value::str(code)),
        (
            "edit",
            Value::obj(vec![(
                "changes",
                Value::Obj(edits.into_iter().map(|(k, v)| (k, Value::Arr(v))).collect()),
            )]),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(body: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
    }

    #[test]
    fn framing_reads_one_message_and_stops_at_the_end() {
        let mut input = std::io::Cursor::new(framed(r#"{"a":1}"#));
        assert_eq!(read_message(&mut input).unwrap().as_deref(), Some(r#"{"a":1}"#));
        assert_eq!(read_message(&mut input).unwrap(), None);
    }

    #[test]
    fn framing_uses_the_length_rather_than_looking_for_a_blank_line() {
        // A body containing the header separator must not truncate the message.
        let body = r#"{"text":"a\r\n\r\nb"}"#;
        let mut input = std::io::Cursor::new(framed(body));
        assert_eq!(read_message(&mut input).unwrap().as_deref(), Some(body));
    }

    #[test]
    fn framing_reads_two_messages_back_to_back() {
        let mut bytes = framed(r#"{"a":1}"#);
        bytes.extend(framed(r#"{"b":2}"#));
        let mut input = std::io::Cursor::new(bytes);
        assert_eq!(read_message(&mut input).unwrap().as_deref(), Some(r#"{"a":1}"#));
        assert_eq!(read_message(&mut input).unwrap().as_deref(), Some(r#"{"b":2}"#));
        assert_eq!(read_message(&mut input).unwrap(), None);
    }

    #[test]
    fn a_message_with_no_content_length_is_an_error() {
        let mut input = std::io::Cursor::new(b"Content-Type: x\r\n\r\n{}".to_vec());
        assert!(read_message(&mut input).is_err());
    }

    /// An unknown *request* still gets a reply. A client that sent one and got
    /// nothing back waits forever, which presents as the server having hung.
    fn initialized() -> State {
        let mut state = State::new();
        let init = json::parse(r#"{"id":0,"method":"initialize","params":{}}"#).unwrap();
        assert_eq!(handle(&mut state, &init).len(), 1);
        state
    }

    #[test]
    fn an_unknown_request_is_answered_and_an_unknown_notification_is_not() {
        let mut state = initialized();
        let req = json::parse(r#"{"id":7,"method":"textDocument/rename"}"#).unwrap();
        let out = handle(&mut state, &req);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].get("id").and_then(|v| v.as_u32()), Some(7));

        let note = json::parse(r#"{"method":"$/setTrace"}"#).unwrap();
        assert!(handle(&mut state, &note).is_empty());
    }

    #[test]
    fn shutdown_then_exit_ends_the_loop() {
        let mut state = initialized();
        let shutdown = json::parse(r#"{"id":1,"method":"shutdown"}"#).unwrap();
        assert_eq!(handle(&mut state, &shutdown).len(), 1);
        assert_eq!(state.lifecycle.exit_code(), None, "shutdown is not exit");
        let exit = json::parse(r#"{"method":"exit"}"#).unwrap();
        assert!(handle(&mut state, &exit).is_empty());
        assert_eq!(state.lifecycle.exit_code(), Some(0));
    }

    /// The three orderings the `bool` could not tell apart. Each is now a
    /// refusal rather than something served by accident.
    #[test]
    fn the_lifecycle_refuses_what_is_out_of_order() {
        // A request before `initialize`.
        let mut state = State::new();
        let req = json::parse(r#"{"id":1,"method":"textDocument/hover"}"#).unwrap();
        let out = handle(&mut state, &req);
        assert_eq!(out.len(), 1);
        assert!(out[0].get("error").is_some(), "{}", out[0].to_string());
        assert!(out[0].get("result").is_none());

        // `initialize` twice.
        let mut state = initialized();
        let init = json::parse(r#"{"id":2,"method":"initialize","params":{}}"#).unwrap();
        assert!(handle(&mut state, &init)[0].get("error").is_some());

        // `exit` with no `shutdown` before it is a non-zero exit, which is
        // what the protocol asks for and what the `bool` could not express.
        let mut state = initialized();
        let exit = json::parse(r#"{"method":"exit"}"#).unwrap();
        assert!(handle(&mut state, &exit).is_empty());
        assert_eq!(state.lifecycle.exit_code(), Some(1));
    }
}
