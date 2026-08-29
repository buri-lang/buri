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
//!
//! **What that makes of a cancel.** One message is read, answered, and only
//! then is the next one read — so a `$/cancelRequest` usually names a request
//! that has already been answered, which the protocol says is a no-op and which
//! this server treats as one. What it can still do is refuse an id whose turn
//! has not come: the id is remembered, and when the request is dequeued it is
//! answered `-32800 RequestCancelled` instead of being run. There is
//! deliberately no read-ahead — a server that pulled the rest of the pipe
//! before dispatching would decide the outcome by how the operating system
//! chunked the client's write, and a recorded session must not depend on that.

mod build_files;
mod call_hierarchy;
mod code_actions;
mod code_lens;
mod color;
mod completion;
mod conformance;
mod convert;
mod execute_command;
mod features;
mod file_operations;
mod formatting;
mod inlay_hints;
mod links;
mod pull_diagnostics;
mod rename;
mod schema;
mod semantic_tokens;
mod signature_help;
mod state;
mod symbols;
mod syntax;

use convert::Position;
use crate::build::session::Session;
use crate::commands::arguments;
use crate::json::{self, Value};
use state::{State, Trace};
use std::io::{Read, Write};
use std::path::PathBuf;

#[expect(
    clippy::print_stderr,
    reason = "a message that did not parse has no id to answer and no Session to route through, and stdout carries protocol only. The same line goes out as a `window/logMessage`; stderr stays as the floor, because a report about a corrupt stream must not depend on that stream"
)]
pub fn command_language_server(_args: &arguments::Args) -> i32 {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();

    let mut state = State::new();

    loop {
        match read_message(&mut input) {
            Ok(None) => return 0,
            Ok(Some(text)) => {
                let incoming = match json::parse(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        // stderr is the floor and the protocol's log channel is
                        // the one an editor actually shows a reader.
                        let said = format!("buri lsp: unparseable message: {e}");
                        eprintln!("{said}");
                        write_message(&mut output, &message("window/logMessage", ERROR, &said));
                        continue;
                    }
                };
                for reply in handle(&mut state, &incoming) {
                    echoed_to_stderr(&reply);
                    write_message(&mut output, &reply);
                }
                // Whether the loop is over is the lifecycle's answer, not a
                // second reading of the method string.
                if let Some(code) = state.lifecycle.exit_code() {
                    return code;
                }
            }
            Err(e) => {
                let said = format!("buri lsp: {e}");
                eprintln!("{said}");
                write_message(&mut output, &message("window/logMessage", ERROR, &said));
                return 1;
            }
        }
    }
}

/// The stderr floor under both log channels.
///
/// A client is free to show neither `window/showMessage` nor
/// `window/logMessage`, and stdout carries protocol only — so every line the
/// server says out loud is written to stderr too. That is where a reader looks
/// when the editor is showing nothing, and it is the one channel a corrupt
/// protocol stream cannot take with it.
#[expect(
    clippy::print_stderr,
    reason = "stderr is the floor this function exists to be: the message also goes out on the protocol channel, and stdout carries protocol only"
)]
fn echoed_to_stderr(reply: &Value) {
    if !matches!(
        reply.get("method").and_then(|m| m.as_str()),
        Some("window/showMessage" | "window/logMessage")
    ) {
        return;
    }
    if let Some(said) = reply.at("params.message").and_then(|m| m.as_str()) {
        eprintln!("buri lsp: {said}");
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
    Value::object(vec![
        ("jsonrpc", Value::str("2.0")),
        ("id", id.clone()),
        ("result", result),
    ])
}

/// A JSON-RPC error reply. A request that cannot be served still gets an
/// answer, because a client that got nothing waits forever.
fn error(id: &Value, code: i64, message: &str) -> Value {
    Value::object(vec![
        ("jsonrpc", Value::str("2.0")),
        ("id", id.clone()),
        (
            "error",
            Value::object(vec![("code", Value::number(code)), ("message", Value::str(message))]),
        ),
    ])
}

/// `ServerNotInitialized`, from the protocol's own table of error codes.
const NOT_INITIALIZED: i64 = -32002;

/// `InvalidRequest`.
const INVALID_REQUEST: i64 = -32600;

/// `InvalidParams` — what a `workspace/executeCommand` naming a command this
/// server does not implement, or missing an argument one needs, is.
const INVALID_PARAMS: i64 = -32602;

/// `MethodNotFound` — the protocol's own reply for a method the server does not
/// implement.
const METHOD_NOT_FOUND: i64 = -32601;

/// `RequestFailed` — the protocol's code for a request the server understood
/// and will not serve, which is what a refused rename is.
const REQUEST_FAILED: i64 = -32803;

/// `RequestCancelled` — what a request the client withdrew is answered with.
/// Withdrawn is still answered: a client that got nothing back waits forever.
const REQUEST_CANCELLED: i64 = -32800;

/// The protocol's `MessageType`, for `window/showMessage` and
/// `window/logMessage`.
pub(super) const ERROR: i64 = 1;
pub(super) const INFO: i64 = 3;

/// One `window/showMessage` or `window/logMessage`.
pub(super) fn message(method: &str, kind: i64, text: &str) -> Value {
    notification(
        method,
        Value::object(vec![("type", Value::number(kind)), ("message", Value::str(text))]),
    )
}

fn notification(method: &str, params: Value) -> Value {
    Value::object(vec![
        ("jsonrpc", Value::str("2.0")),
        ("method", Value::str(method)),
        ("params", params),
    ])
}

/// A request the *server* sends. The id is a name rather than a number, so it
/// cannot collide with the client's counter and a golden records the same
/// string on every run.
fn request(id: &str, method: &str, params: Value) -> Value {
    let mut fields = vec![
        ("jsonrpc", Value::str("2.0")),
        ("id", Value::str(id)),
        ("method", Value::str(method)),
    ];
    // A request with nothing to say leaves the field out. `"params": null` is
    // not what "this request takes no parameters" is spelled as.
    if !matches!(params, Value::Null) {
        fields.push(("params", params));
    }
    Value::object(fields)
}

/// The id of the watcher registration, and of the request that carries it.
const WATCHERS: &str = "buri/watchers";

/// The id of the server's question about which folders are open.
const FOLDERS: &str = "buri/workspaceFolders";

/// The stem of the ids the server's `workspace/applyEdit` requests carry.
const APPLY_EDIT: &str = "buri/applyEdit";

/// The four `workspace/*/refresh` requests this server sends: the method, and
/// the stem of the ids it carries.
///
/// A refresh may go out many times, so each one is numbered — an id still in
/// flight must not be reused. The stems are what the golden sessions record.
const REFRESH_FAMILIES: [(&str, &str); 4] = [
    ("workspace/semanticTokens/refresh", "buri/semanticTokensRefresh"),
    ("workspace/inlayHint/refresh", "buri/inlayHintRefresh"),
    ("workspace/codeLens/refresh", "buri/codeLensRefresh"),
    ("workspace/diagnostic/refresh", "buri/diagnosticRefresh"),
];

/// One message in, and everything the server has to say about it out.
///
/// [`dispatch`] is the answer. What is wrapped around it is the bookkeeping
/// that is about the conversation rather than about the question: the trace
/// lines a `$/setTrace` asked for, and the messages a handler decided to send
/// from somewhere below the request it was serving — a repository that will not
/// load is found several calls down, and the reader has to be told.
fn handle(state: &mut State, msg: &Value) -> Vec<Value> {
    // Nothing read for the last message may be reused for this one: between
    // the two, the client may have written anything.
    state.begin_message();
    let named = traced_name(msg);
    // A request is an id and a method together: a response carries no method,
    // and a notification no id.
    let request_id = msg.get("id").filter(|_| msg.get("method").is_some());
    let mut out = Vec::new();
    if let (Trace::Messages | Trace::Verbose, Some(name)) = (state.trace, &named) {
        out.push(log_trace(&format!("received {name}"), None));
    }
    let started = std::time::Instant::now();
    let before = state.work();
    let replies = dispatch(state, msg);
    // Before the answer: what went wrong was found while the answer was being
    // computed, and the answer is what it came to.
    out.extend(state.take_outgoing());
    if let Some(id) = request_id {
        state.record_answered(id);
    }
    let written: usize = replies.iter().map(|r| r.to_string().len()).sum();
    state.wrote(written as u64);
    out.extend(replies);
    if let (Trace::Messages | Trace::Verbose, Some(name), Some(_)) =
        (state.trace, &named, request_id)
    {
        // Timing is the difference between the two levels, and it is the one
        // thing in this stream a golden session cannot pin — so it is at
        // `verbose` and nowhere else, next to the work it came from. The
        // counters beside it *are* pinnable, and they are the claim: what
        // makes a request fast is how little it did, not what a clock read.
        let detail = (state.trace == Trace::Verbose).then(|| {
            format!(
                "answered in {}ms; {}",
                started.elapsed().as_millis(),
                state.work().since(before).spelled()
            )
        });
        out.push(log_trace(&format!("answered {name}"), detail.as_deref()));
    }
    out
}

/// What a `$/logTrace` line calls the message being handled, or `None` for a
/// client's *response* to one of the server's own requests — which is not a
/// message the server is handling in this sense.
fn traced_name(msg: &Value) -> Option<String> {
    let method = msg.get("method")?.as_str()?;
    Some(match msg.get("id") {
        Some(id) => format!("request {method} ({})", id.to_string()),
        None => format!("notification {method}"),
    })
}

fn log_trace(text: &str, verbose: Option<&str>) -> Value {
    let mut fields = vec![("message", Value::str(text))];
    if let Some(verbose) = verbose {
        fields.push(("verbose", Value::str(verbose)));
    }
    notification("$/logTrace", Value::object(fields))
}

fn dispatch(state: &mut State, msg: &Value) -> Vec<Value> {
    // A message with an id and no method is a *response* to one of the two
    // requests this server sends, and answering it would be answering an answer.
    if msg.get("method").is_none() {
        return client_response(state, msg);
    }
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

    // A request the client has withdrawn is refused rather than run. The two
    // lifecycle requests are exempt: a cancelled `initialize` or `shutdown`
    // would leave the session in a state neither side can get out of.
    if let Some(id) = &id {
        if !matches!(method, "initialize" | "shutdown") && state.was_cancelled(id) {
            return vec![error(id, REQUEST_CANCELLED, "the client cancelled this request")];
        }
    }

    match (method, id) {
        ("initialize", Some(id)) => {
            if let Err(why) = state.lifecycle.initialize() {
                return vec![error(&id, INVALID_REQUEST, why)];
            }
            // The array is what a client that can hold two repositories
            // sends; the single `rootUri` is the older form for one that cannot.
            let mut named = folders(params.get("workspaceFolders"));
            if named.is_empty() {
                named.extend(
                    params.get("rootUri").and_then(|u| u.as_str()).and_then(convert::path_of),
                );
            }
            for folder in &named {
                state.add_root(folder);
            }
            state.can_register_watchers = matches!(
                params.at("capabilities.workspace.didChangeWatchedFiles.dynamicRegistration"),
                Some(&Value::Bool(true))
            );
            state.refreshes.semantic_tokens.supported = matches!(
                params.at("capabilities.workspace.semanticTokens.refreshSupport"),
                Some(&Value::Bool(true))
            );
            state.refreshes.inlay_hints.supported = matches!(
                params.at("capabilities.workspace.inlayHint.refreshSupport"),
                Some(&Value::Bool(true))
            );
            state.refreshes.code_lenses.supported = matches!(
                params.at("capabilities.workspace.codeLens.refreshSupport"),
                Some(&Value::Bool(true))
            );
            // `workspace.diagnostics`, plural — the one refresh family the
            // protocol spells differently to the request it refreshes.
            state.refreshes.diagnostics.supported = matches!(
                params.at("capabilities.workspace.diagnostics.refreshSupport"),
                Some(&Value::Bool(true))
            );
            state.related_documents_supported = matches!(
                params.at("capabilities.textDocument.diagnostic.relatedDocumentSupport"),
                Some(&Value::Bool(true))
            );
            // The outline has two shapes and the client picks: the nested
            // `DocumentSymbol` tree, or the flat `SymbolInformation` list whose
            // required `location` the tree does not carry. A client that did
            // not claim the tree is entitled to read the reply as the list.
            state.hierarchical_symbols = matches!(
                params
                    .at("capabilities.textDocument.documentSymbol.hierarchicalDocumentSymbolSupport"),
                Some(&Value::Bool(true))
            );
            // `contentFormat` is the client's `MarkupKind`s, best first. Naming
            // formats and leaving `markdown` out of them is the one case where
            // a fence would be drawn as three backticks; saying nothing at all
            // leaves the markdown every editor renders.
            state.hover_markup = match params
                .at("capabilities.textDocument.hover.contentFormat")
                .and_then(|f| f.as_array())
            {
                Some(formats) if !formats.iter().any(|f| f.as_str() == Some("markdown")) => {
                    features::Markup::PlainText
                }
                _ => features::Markup::Markdown,
            };
            // The `edit` property by name rather than the presence of
            // `resolveSupport`: a client that lists other properties and not
            // that one will never ask for the edit, and an action whose edit
            // waited for a request nobody sends is a fix that does nothing.
            state.code_action_resolve = params
                .at("capabilities.textDocument.codeAction.resolveSupport.properties")
                .and_then(|p| p.as_array())
                .is_some_and(|p| p.iter().any(|v| v.as_str() == Some("edit")));
            let client_has_folders = matches!(
                params.at("capabilities.workspace.workspaceFolders"),
                Some(&Value::Bool(true))
            );
            // A client that knows about folders and named none is asked; one
            // that does not can only have meant where it started the server.
            state.must_ask_for_folders = named.is_empty() && client_has_folders;
            // The level a client can set before it is able to send a
            // `$/setTrace`, which is every message up to this reply.
            if let Some(level) = params.get("trace").and_then(|t| t.as_str()).and_then(Trace::named)
            {
                state.trace = level;
            }
            if named.is_empty() && !client_has_folders {
                if let Ok(cwd) = std::env::current_dir() {
                    state.add_root(&cwd);
                }
            }
            vec![response(&id, capabilities())]
        }
        // The first moment the protocol lets a server speak unprompted, and
        // both of these are questions the `initialize` exchange left open.
        ("initialized", _) => {
            let mut out = Vec::new();
            if state.can_register_watchers {
                out.push(watcher_registration());
            }
            if state.must_ask_for_folders {
                out.push(request(FOLDERS, "workspace/workspaceFolders", Value::Null));
            }
            out
        }
        ("shutdown", Some(id)) => match state.lifecycle.shutdown() {
            Ok(()) => vec![response(&id, Value::Null)],
            Err(why) => vec![error(&id, INVALID_REQUEST, why)],
        },
        ("exit", _) => {
            state.lifecycle.exit();
            vec![]
        }

        // The client withdrawing a request it has already sent.
        //
        // This server reads one message and answers it before reading the
        // next, so by the time a cancel is read the request it names has
        // usually been answered — and a cancel for an answered request is a
        // no-op, which is what the protocol asks for. What the id is kept for
        // is the other order: a client that cancels a request this server has
        // not reached yet gets that request refused when its turn comes,
        // rather than waiting out an analysis nobody is going to read.
        ("$/cancelRequest", _) => {
            if let Some(cancelled) = params.get("id") {
                state.cancel(cancelled);
            }
            vec![]
        }

        // How much the client wants to be told. A word that is not one of the
        // three levels leaves it where it was: the protocol has no error reply
        // for a notification, and guessing would be worse than ignoring.
        ("$/setTrace", _) => {
            if let Some(level) =
                params.get("value").and_then(|v| v.as_str()).and_then(Trace::named)
            {
                state.trace = level;
            }
            vec![]
        }

        ("textDocument/didOpen", _) => {
            let Some((path, text)) = opened(&params) else { return vec![] };
            state.set_buffer(path.clone(), text);
            let published = full_diagnostics(state, &path);
            // The state the client's colours and hints are about to be computed
            // from. Recorded and not announced: nothing has gone stale yet.
            state.record_refresh_fingerprint();
            published
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
            state.set_buffer(path.clone(), text.to_string());
            parse_diagnostics(state, &path, text)
        }
        ("textDocument/didSave", _) => {
            let Some(path) = uri_param(&params) else { return vec![] };
            let mut out = full_diagnostics(state, &path);
            out.extend(refreshes(state));
            out
        }
        ("textDocument/didClose", _) => {
            if let Some(path) = uri_param(&params) {
                state.drop_buffer(&path);
                // The colours the client held were about a buffer it no longer
                // has, and the id it would quote is now meaningless.
                state.semantic_tokens.results.remove(&path);
                // Nothing left to retract for a document nobody will pull about.
                state.related_reported.remove(&convert::uri_of(&path));
            }
            vec![]
        }

        // Something changed on disk that no buffer holds — `buri gen`, a
        // checkout, or the edit a code action returned and the client applied.
        //
        // Nothing is invalidated by hand: the analysis is kept under a hash of
        // the bytes it read, so the file that changed has moved the key already.
        ("workspace/didChangeWatchedFiles", _) => {
            let mut out = republish_open(state);
            out.extend(refreshes(state));
            out
        }

        // The three questions an editor asks before it touches a file. Each is
        // answered with the edit that keeps the repository building, because a
        // Buri module is a file its `BUILD.buri` lists and a path other modules
        // write — so moving one in the file tree is two rewrites away from
        // moving it in the language. See `file_operations`.
        ("workspace/willCreateFiles", Some(id)) => {
            let edit = file_operations::will_create(state, &params);
            vec![response(&id, edit.unwrap_or(Value::Null))]
        }

        ("workspace/willRenameFiles", Some(id)) => {
            let edit = file_operations::will_rename(state, &params);
            vec![response(&id, edit.unwrap_or(Value::Null))]
        }

        ("workspace/willDeleteFiles", Some(id)) => {
            let edit = file_operations::will_delete(state, &params);
            vec![response(&id, edit.unwrap_or(Value::Null))]
        }

        // And the three that say it happened. The same answer a watched change
        // gets, for the same reason: a file appearing, moving or going away
        // changes what every open buffer means, and nothing else says so.
        ("workspace/didCreateFiles", _) => {
            let mut out = republish_open(state);
            out.extend(refreshes(state));
            out
        }

        ("workspace/didRenameFiles", _) => {
            file_operations::moved(state, &params);
            let mut out = republish_open(state);
            out.extend(refreshes(state));
            out
        }

        ("workspace/didDeleteFiles", _) => {
            let mut out = file_operations::gone(state, &params);
            out.extend(republish_open(state));
            out.extend(refreshes(state));
            out
        }

        // A folder added or dropped changes which repository — if any — owns
        // each open file, so the same re-publish is the honest answer.
        ("workspace/didChangeWorkspaceFolders", _) => {
            for folder in folders(params.at("event.added")) {
                state.add_root(&folder);
            }
            for folder in folders(params.at("event.removed")) {
                state.remove_root(&folder);
            }
            republish_open(state)
        }

        // The same dispatch `buri format` uses, so the editor and the command
        // cannot disagree: a `BUILD.buri` goes through the textproto printer
        // and everything else through the source formatter. Either returns
        // nothing for a file that does not parse, so a file mid-edit is left
        // alone rather than mangled.
        ("textDocument/formatting", Some(id)) => {
            vec![response(&id, whole_file_format(state, &params))]
        }

        // Format on save, and it is the *same* answer: an editor that formats
        // on save and one that asks for the format are asking one question at
        // two moments, and a server that computed them differently would be
        // the reason a saved file and a formatted file could differ.
        ("textDocument/willSaveWaitUntil", Some(id)) => {
            vec![response(&id, whole_file_format(state, &params))]
        }

        // A range of the same canonical output. The file is formatted whole,
        // the result is diffed against what is on screen, and only the hunks
        // the range touches are handed back — so "format the selection" cannot
        // disagree with "format the file", because it *is* that answer with
        // the rest withheld. See `formatting`.
        ("textDocument/rangeFormatting", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                let from =
                    convert::offset_of(&text, Position::from_json(params.at("range.start")?)?);
                let to = convert::offset_of(&text, Position::from_json(params.at("range.end")?)?);
                Some(formatting::ranged(&file_name(&path), &text, from, to))
            })();
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        // The same, scoped to the declaration the typed `}` or `;` is inside.
        // A build file has no such scope — it is textproto, and its printer
        // rewrites the whole document — so a brace typed in one is answered
        // with nothing rather than with the file.
        ("textDocument/onTypeFormatting", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                if build_files::is_build_file(&path) {
                    return None;
                }
                let text = state.text_of(&path)?;
                let offset = convert::offset_of(&text, Position::from_json(params.get("position")?)?);
                let (from, to) = formatting::enclosing_item(&text, offset)?;
                Some(formatting::ranged(&file_name(&path), &text, from, to))
            })();
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        // The three that read a parse and nothing else. No workspace, no
        // standard library, no analysis: they are questions about shape.
        ("textDocument/documentSymbol", Some(id)) => {
            let nested = state.hierarchical_symbols;
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                let outline = syntax::document_symbols(&text);
                if nested {
                    return Some(outline);
                }
                Some(syntax::flattened(&outline, &convert::uri_of(&path)))
            })();
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        ("textDocument/foldingRange", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                Some(syntax::folding_ranges(&text))
            })();
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        ("textDocument/selectionRange", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                let positions: Vec<Position> = params
                    .get("positions")?
                    .as_array()?
                    .iter()
                    .filter_map(Position::from_json)
                    .collect();
                Some(syntax::selection_ranges(&text, &positions))
            })();
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        ("textDocument/hover", Some(id)) => {
            let markup = state.hover_markup;
            let result = hover(state, &params, markup);
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        // `declaration` and `definition` are one request here. The protocol
        // separates them for languages that declare a thing in one file and
        // define it in another; Buri has one place, so answering differently
        // would mean inventing a difference.
        ("textDocument/definition" | "textDocument/declaration", Some(id)) => {
            vec![response(&id, definition(state, &params).unwrap_or(Value::Null))]
        }

        ("textDocument/typeDefinition", Some(id)) => {
            let result = with_analysis(state, &params, features::type_definition);
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        // The whole repository: an `impl` is written in the module that
        // declares its type, which is a target this file need not be in.
        ("textDocument/implementation", Some(id)) => {
            let result = with_workspace(state, &params, conformance::implementations);
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        ("textDocument/prepareTypeHierarchy", Some(id)) => {
            let result = with_workspace(state, &params, conformance::prepare);
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        // The two walks. Each is handed an item back rather than a position,
        // so the symbol is resolved again from what the item carries.
        ("typeHierarchy/supertypes", Some(id)) => {
            let result = hierarchy_symbol(state, &params)
                .map(|(analyzed, symbol)| conformance::supertypes(&analyzed, &symbol));
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        ("typeHierarchy/subtypes", Some(id)) => {
            let result = hierarchy_symbol(state, &params)
                .map(|(analyzed, symbol)| conformance::subtypes(&analyzed, &symbol));
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        // The call hierarchy. The whole repository, for the reason
        // `references` needs it: a function is called from wherever it is
        // imported, and nothing about the file under the cursor bounds that.
        ("textDocument/prepareCallHierarchy", Some(id)) => {
            let result = with_workspace(state, &params, call_hierarchy::prepare);
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        // The two walks, resolved back from the item the client hands over —
        // the same round trip the type hierarchy makes, through the same
        // `data: {uri, position}` and for the same reason.
        ("callHierarchy/incomingCalls", Some(id)) => {
            let result = hierarchy_symbol(state, &params)
                .map(|(analyzed, symbol)| call_hierarchy::incoming(&analyzed, &symbol));
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        ("callHierarchy/outgoingCalls", Some(id)) => {
            let result = hierarchy_symbol(state, &params)
                .map(|(analyzed, symbol)| call_hierarchy::outgoing(&analyzed, &symbol));
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        // The underlines. A `BUILD.buri` is textproto and its links are the
        // build graph's; a source file's are its import paths and the
        // addresses its comments write. One target is enough for either: an
        // import path is resolved by the workspace, which every session has.
        ("textDocument/documentLink", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                if build_files::is_build_file(&path) {
                    let session = state.session_for(&path)?;
                    return Some(build_files::links(&session, &path, &text));
                }
                let analyzed = state.analyze_for_query(&path);
                Some(links::document_links(analyzed.as_deref(), &path, &text))
            })();
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        // One target is enough: a moniker is built from where the declaration
        // is, and the file's own closure is where the names it writes live.
        ("textDocument/moniker", Some(id)) => {
            let result = with_analysis(state, &params, features::moniker);
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        ("textDocument/documentHighlight", Some(id)) => {
            let result = with_analysis(state, &params, features::document_highlight);
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        ("textDocument/signatureHelp", Some(id)) => {
            let result = with_analysis(state, &params, signature_help::help);
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        ("textDocument/prepareRename", Some(id)) => {
            let result = with_analysis(state, &params, rename::prepare);
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        ("textDocument/rename", Some(id)) => {
            // The whole repository, for the reason references analyses it: a
            // rename that missed the file importing the name would leave the
            // repository not building.
            let mut out = progress_begin(&params, "Renaming across the repository");
            let prepared = (|| {
                let path = uri_param(&params)?;
                let position = Position::from_json(params.get("position")?)?;
                let new_name = params.get("newName")?.as_str()?.to_string();
                let text = state.text_of(&path)?;
                let root = state.root_of(&path)?;
                let analyzed = state.analyze_workspace(&root)?;
                Some(rename::edits(&analyzed, &path, &text, position, &new_name))
            })();
            // The `end` goes out before the answer on every path, refusals
            // included: a report left open is a spinner that never stops.
            out.extend(progress_end(&params, ""));
            out.push(match prepared {
                Some(Ok(edit)) => response(&id, edit),
                // A refusal is an error rather than an empty edit: a rename
                // that silently changed nothing looks like the server hung.
                Some(Err(why)) => error(&id, REQUEST_FAILED, why.message()),
                None => response(&id, Value::Null),
            });
            out
        }

        // Every open repository, in the order the client named them: the
        // request asks about the workspace and the workspace is all of them.
        ("workspace/symbol", Some(id)) => {
            let query = params.get("query").and_then(|q| q.as_str()).unwrap_or("").to_string();
            let roots = state.roots.clone();
            // The one request whose work is a loop the client can be told the
            // length of: a repository analysed is a step, and there are as many
            // steps as the client has folders open.
            let mut out = progress_begin(&params, "Searching the workspace");
            let mut found = Vec::new();
            for (done, root) in roots.iter().enumerate() {
                if let Some(analyzed) = state.analyze_workspace(root) {
                    if let Value::Array(items) = features::workspace_symbols(&analyzed, &query) {
                        found.extend(items);
                    }
                }
                let step = u32::try_from(done).unwrap_or(u32::MAX).saturating_add(1);
                let total = u32::try_from(roots.len()).unwrap_or(u32::MAX).max(1);
                out.extend(progress_report(
                    &params,
                    &format!("repository {step} of {total}"),
                    step.saturating_mul(100).checked_div(total).unwrap_or(100),
                ));
            }
            let result = Value::Array(found);
            out.extend(progress_end(&params, &counted(&result, "symbol")));
            out.push(response(&id, result));
            out
        }

        ("textDocument/references", Some(id)) => {
            // The whole repository rather than the target owning the file,
            // because a name is referred to from wherever it is imported.
            //
            // The protocol makes the declaration opt-in, and clients ask both
            // ways: excluded is "where else is this used".
            let include =
                matches!(params.at("context.includeDeclaration"), Some(&Value::Bool(true)));
            // The whole-repository analysis is the most expensive thing this
            // server does, and it is what the client asked to be told about.
            let mut out = progress_begin(&params, "Finding references");
            let result = with_workspace(state, &params, |analyzed, path, text, position| {
                features::references(analyzed, path, text, position, include)
            })
            .unwrap_or(Value::Array(Vec::new()));
            out.extend(progress_end(&params, &counted(&result, "reference")));
            out.push(response(&id, result));
            out
        }

        // Routed by what kind of file the cursor is in, the way `definition`
        // is: a build file's names come from the build schema and from the
        // graph, and nothing about them is in an analysis of the module the
        // package declares.
        ("textDocument/completion", Some(id)) => {
            let result = complete(state, &params);
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        // The prose, for the one item a reader has highlighted. The list
        // carries what an editor draws in the row — a label, a kind and the
        // signature — and the `///` lines of every name in a module are the
        // part that would put a page of text on the wire to show one line of
        // it. The item comes back as it went out and is resolved from the
        // `data` it carries.
        ("completionItem/resolve", Some(id)) => {
            let result = (|| {
                let path =
                    params.at("data.uri").and_then(|u| u.as_str()).and_then(convert::path_of)?;
                let analyzed = state.analyze_for_query(&path)?;
                Some(completion::resolve_completion(&analyzed, &params))
            })();
            // An item with nothing to resolve is still a legal answer to this
            // request, and it is the item itself: `null` is not one.
            vec![response(&id, result.unwrap_or_else(|| params.clone()))]
        }

        // The offer, and then the work. See `code_actions`.
        ("textDocument/codeAction", Some(id)) => {
            vec![response(&id, code_actions::list(state, &params))]
        }

        ("codeAction/resolve", Some(id)) => {
            vec![response(&id, code_actions::resolve(state, &params))]
        }

        // The swatches. One target is enough: a colour is a constructor call
        // written in this file, and the file's own closure is what checked it.
        ("textDocument/documentColor", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                let analyzed = state.analyze_for_query(&path)?;
                color::document_colors(&analyzed, &path, &text)
            })();
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        // The write-back half, and it analyses nothing: the range came from the
        // answer above, and what to write there is the colour plus how the
        // source already spelled the call.
        ("textDocument/colorPresentation", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                Some(color::presentations(&text, &params))
            })();
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        // Colour, in two layers — see `semantic_tokens`. The result is kept
        // under its id so that the next request can be a delta.
        ("textDocument/semanticTokens/full", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                let data = semantic_tokens_of(state, &path, &text);
                let result_id = state.record_semantic_tokens(&path, data.clone());
                Some(Value::object(vec![
                    ("resultId", Value::str(result_id)),
                    ("data", semantic_tokens::numbers(&data)),
                ]))
            })();
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        // The same answer, filtered to what the client can see. No `resultId`:
        // a partial result is not a base a delta could be computed against, and
        // handing out an id for one would invite exactly that.
        ("textDocument/semanticTokens/range", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                let from = convert::offset_of(&text, Position::from_json(params.at("range.start")?)?);
                let to = convert::offset_of(&text, Position::from_json(params.at("range.end")?)?);
                let data = if build_files::is_build_file(&path) {
                    Vec::new()
                } else {
                    let analyzed = state.analyze_for_query(&path);
                    semantic_tokens::encoded_range(analyzed.as_deref(), &path, &text, from, to)
                };
                Some(Value::object(vec![("data", semantic_tokens::numbers(&data))]))
            })();
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        // The tokens are recomputed either way — there is no incremental front
        // end to compute them any other way — so what a delta saves is the wire
        // and the client's re-render, not the work. It is offered only when the
        // client is quoting the result this server still holds for that file; a
        // first request, a reopened buffer, or an id from before an eviction is
        // answered in full, which the protocol allows and cannot be wrong.
        ("textDocument/semanticTokens/full/delta", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                let text = state.text_of(&path)?;
                let quoted = params.get("previousResultId").and_then(|v| v.as_str());
                let held = state
                    .semantic_tokens
                    .results
                    .get(&path)
                    .filter(|(previous_id, _)| Some(previous_id.as_str()) == quoted)
                    .map(|(_, data)| data.clone());
                let data = semantic_tokens_of(state, &path, &text);
                let result_id = state.record_semantic_tokens(&path, data.clone());
                Some(Value::object(vec![
                    ("resultId", Value::str(result_id)),
                    match held {
                        Some(previous) => ("edits", semantic_tokens::edits(&previous, &data)),
                        None => ("data", semantic_tokens::numbers(&data)),
                    },
                ]))
            })();
            vec![response(&id, result.unwrap_or(Value::Null))]
        }

        // The inferred types and the parameter names, for the lines the client
        // can see. One analysis and one walk per request: the resolver is not
        // asked anything, because asking it per hint would lex the buffer per
        // hint. A `BUILD.buri` has neither kind of hint — it is textproto, and
        // nothing in it infers a type or fills a parameter.
        ("textDocument/inlayHint", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                if build_files::is_build_file(&path) {
                    return None;
                }
                let text = state.text_of(&path)?;
                let from = convert::offset_of(&text, Position::from_json(params.at("range.start")?)?);
                let to = convert::offset_of(&text, Position::from_json(params.at("range.end")?)?);
                let analyzed = state.analyze_for_query(&path)?;
                inlay_hints::hints(&analyzed, &path, &text, from, to)
            })();
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        // The tooltip and the go-to-definition on a hint the reader is pointing
        // at. The hint comes back as it went out and is resolved again from the
        // `data` it carries — the same round trip a `TypeHierarchyItem` makes,
        // and for the same reason: a server that remembered every hint it had
        // produced would hold a table nothing removes an entry from.
        ("inlayHint/resolve", Some(id)) => {
            let result = (|| {
                let path = params.at("data.uri").and_then(|u| u.as_str()).and_then(convert::path_of)?;
                let text = state.text_of(&path)?;
                let analyzed = state.analyze_for_query(&path)?;
                Some(inlay_hints::resolve(&analyzed, &path, &text, &params))
            })();
            // A hint with nothing to resolve is still a legal answer to this
            // request, and it is the hint itself: `null` is not one.
            vec![response(&id, result.unwrap_or_else(|| params.clone()))]
        }

        // The lines above a declaration. A parse and nothing else: a client
        // asks for these for every file it scrolls through, and the count the
        // reference lens is about to show costs a whole-repository analysis —
        // so the count is not here. See `code_lens`. A `BUILD.buri` gets
        // nothing: it is textproto, and neither a `test` nor an `export` is
        // something that language writes.
        ("textDocument/codeLens", Some(id)) => {
            let result = (|| {
                let path = uri_param(&params)?;
                if build_files::is_build_file(&path) {
                    return None;
                }
                let text = state.text_of(&path)?;
                Some(code_lens::lenses(&path, &text))
            })();
            vec![response(&id, result.unwrap_or(Value::Array(Vec::new())))]
        }

        // Where the count is paid, for the one lens the editor is about to
        // draw. The whole repository, for the reason `references` needs it: a
        // name is used wherever it is imported.
        ("codeLens/resolve", Some(id)) => {
            let result = (|| {
                let path = params.at("data.uri").and_then(|u| u.as_str()).and_then(convert::path_of)?;
                let text = state.text_of(&path)?;
                let root = state.root_of(&path)?;
                let analyzed = state.analyze_workspace(&root)?;
                Some(code_lens::resolve(&analyzed, &path, &text, &params))
            })();
            // A lens with nothing to resolve is still a legal answer to this
            // request, and it is the lens itself.
            vec![response(&id, result.unwrap_or_else(|| params.clone()))]
        }

        // The findings, asked for. Push publishing is unchanged and still
        // happens — the protocol allows both, and a client that pulls simply
        // does not have to wait for a save to ask. See `pull_diagnostics`.
        ("textDocument/diagnostic", Some(id)) => pull_diagnostics::document(state, &id, &params),

        // The whole repository, which is the half push cannot reach: a finding
        // about a file nobody opened has no publish to ride on.
        ("workspace/diagnostic", Some(id)) => {
            let mut out = progress_begin(&params, "Analysing every file");
            let result = pull_diagnostics::workspace(state, &params);
            out.extend(progress_end(&params, ""));
            out.push(response(&id, result));
            out
        }

        // The verbs. Each is a call into an entry point `buri` already has at
        // the terminal, and the one that edits answers only once the client
        // has written what it was sent.
        ("workspace/executeCommand", Some(id)) => execute_command::execute(state, &id, &params),

        // The request exists for syntax that spells one name at both ends of a
        // construct — an opening tag and its closing one. Buri writes every
        // name once: an `impl Priced for Item { … }` closes with a brace. So
        // `null` everywhere is the complete answer rather than a missing one.
        ("textDocument/linkedEditingRange", Some(id)) => vec![response(&id, Value::Null)],

        // Only ever sent while a client is stopped in a debug session, which is
        // a state nothing can reach: there is no Buri debug adapter. Empty
        // rather than absent, because the request has values to show and this
        // program has none of them stopped.
        ("textDocument/inlineValue", Some(id)) => {
            vec![response(&id, Value::Array(Vec::new()))]
        }

        // The notifications with nothing to do, named rather than left to the
        // catch-all so that the decision is written down where it is made.
        //
        // `willSave`: the server does nothing before a save that it does not do
        // on `didSave`, and doing the analysis twice would only make the save
        // slower. `didChangeConfiguration`: there is no setting — every `Flags`
        // the server builds is `default()`. The four `notebookDocument/*`: a
        // Buri module belongs to a target declared in a `BUILD.buri`, and a
        // notebook cell has no target, so `notebookDocumentSync` is not
        // advertised and a cell is not something this toolchain can compile.
        //
        // `window/workDoneProgress/cancel`: the progress this server reports is
        // around one call into the front end, which nothing interrupts — so the
        // work the client asked to stop finishes, and the `end` it is already
        // waiting for arrives on time. That is why the `begin` says
        // `cancellable: false`: a button that did nothing would be the lie.
        (
            "textDocument/willSave"
            | "window/workDoneProgress/cancel"
            | "workspace/didChangeConfiguration"
            | "notebookDocument/didOpen"
            | "notebookDocument/didChange"
            | "notebookDocument/didSave"
            | "notebookDocument/didClose",
            None,
        ) => vec![],

        // A request this server does not implement is refused with the
        // protocol's own code for that, rather than answered `result: null`.
        // Null is not a legal result for several requests — a
        // `DocumentDiagnosticReport` has no null among its shapes — so a
        // blanket null was a reply the client could not read. An error is
        // still a reply, which is what matters: a client that got nothing back
        // waits forever.
        (_, Some(id)) => {
            vec![error(&id, METHOD_NOT_FOUND, &format!("`{method}` is not implemented"))]
        }
        (_, None) => vec![],
    }
}

/// The encoded semantic tokens for one file.
///
/// The analysis is asked for and may not come — a file in no open repository,
/// or one whose target will not load — and layer one is the answer either way.
/// A `BUILD.buri` gets nothing: it is textproto, and the token kinds this
/// colours by are the Buri lexer's, which are not that language's.
fn semantic_tokens_of(state: &mut State, path: &std::path::Path, text: &str) -> Vec<u32> {
    if build_files::is_build_file(path) {
        return Vec::new();
    }
    let analyzed = state.analyze_for_query(path);
    semantic_tokens::encoded(analyzed.as_deref(), path, text)
}

// ---------------------------------------------------------------------------
// Work-done progress
// ---------------------------------------------------------------------------

/// The token a client sent to be told how a long request is going, if it sent
/// one.
///
/// **Only the client's token is honoured.** A token the *server* invents has to
/// be registered first, with a `window/workDoneProgress/create` the client is
/// entitled to refuse — and a client that did not ask to be told is not missing
/// anything by not being told. So there is no `create` in this server: the
/// requests that report progress report it to whoever asked for it.
fn progress_of(params: &Value, kind: &str, mut fields: Vec<(&str, Value)>) -> Vec<Value> {
    let Some(token) = params.get("workDoneToken") else { return Vec::new() };
    fields.insert(0, ("kind", Value::str(kind)));
    vec![notification(
        "$/progress",
        Value::object(vec![("token", token.clone()), ("value", Value::object(fields))]),
    )]
}

/// The `begin` that opens a report, with the title an editor puts in its status
/// bar.
///
/// `cancellable: false` is a fact rather than a default: the work is one call
/// into a front end with no interruption point in it, and an editor that drew a
/// cancel button on the strength of this would be drawing one that does
/// nothing.
fn progress_begin(params: &Value, title: &str) -> Vec<Value> {
    progress_of(
        params,
        "begin",
        vec![("title", Value::str(title)), ("cancellable", Value::Bool(false))],
    )
}

/// One step of a report, for a request whose work is a loop over something the
/// client can be told the size of.
fn progress_report(params: &Value, text: &str, percentage: u32) -> Vec<Value> {
    progress_of(
        params,
        "report",
        vec![("message", Value::str(text)), ("percentage", Value::number(percentage))],
    )
}

/// The `end` that closes it. Sent on every path out of the request, including
/// the ones that answer nothing: a report left open is a spinner that never
/// stops.
fn progress_end(params: &Value, text: &str) -> Vec<Value> {
    let fields =
        if text.is_empty() { Vec::new() } else { vec![("message", Value::str(text))] };
    progress_of(params, "end", fields)
}

/// How many items an answer carries, for the `end` message that says so.
fn counted(result: &Value, what: &str) -> String {
    match result.as_array() {
        Some(items) if items.len() == 1 => format!("1 {what}"),
        Some(items) => format!("{} {what}s", items.len()),
        None => String::new(),
    }
}

/// The `workspace/*/refresh` requests a save or a watched change earns.
///
/// Sent only when the analysis fingerprint moved. That is what the wave-1a
/// cache makes observable: the key an answer is filed under *is* the state it
/// was computed from, so "something the answers depend on changed" is a
/// comparison rather than a guess, and saving a buffer nobody edited is quiet.
///
/// One function for both families, and the fingerprint is computed **once** —
/// it hashes every byte under every open root, so asking for it twice would
/// read the repository twice to answer the same question. Each family keeps its
/// own last-seen value and its own counter, because a client may accept one and
/// not the other.
///
/// Gated on the client's `refreshSupport`, because a client that never claimed
/// it is entitled to reject the request — and a rejected request is a reply the
/// server would then have to explain away.
fn refreshes(state: &mut State) -> Vec<Value> {
    let now = state.analysis_fingerprint();
    let families = [
        &mut state.refreshes.semantic_tokens,
        &mut state.refreshes.inlay_hints,
        &mut state.refreshes.code_lenses,
        &mut state.refreshes.diagnostics,
    ];
    let mut out = Vec::new();
    for (family, (method, stem)) in families.into_iter().zip(REFRESH_FAMILIES) {
        let changed = family.fingerprint != Some(now);
        family.fingerprint = Some(now);
        if !changed || !family.supported {
            continue;
        }
        family.sent = family.sent.saturating_add(1);
        out.push(request(&format!("{stem}/{}", family.sent), method, Value::Null));
    }
    out
}

fn capabilities() -> Value {
    Value::object(vec![
        (
            "capabilities",
            Value::object(vec![
                // The options object rather than the bare `1` it used to be.
                // The number is the protocol's legacy form and says only how
                // changes arrive: a client reading it has been told nothing
                // about `didOpen`, `didClose` or `didSave`, and one that sends
                // only what was negotiated sends no save — which is the
                // notification the whole analysis hangs off. `change: 1` is
                // still full sync, because incremental sync buys nothing
                // without an incremental front end and costs a text-edit
                // applier. The save carries no text: the server reads the
                // buffer it already has.
                (
                    "textDocumentSync",
                    Value::object(vec![
                        ("openClose", Value::Bool(true)),
                        ("change", Value::number(1)),
                        (
                            "save",
                            Value::object(vec![("includeText", Value::Bool(false))]),
                        ),
                        // Format on save, with nothing for a reader to
                        // configure. `willSave` is deliberately *not* claimed
                        // beside it: the notification has no answer, and
                        // asking for one would be asking a client to send a
                        // message this server drops.
                        ("willSaveWaitUntil", Value::Bool(true)),
                    ]),
                ),
                ("hoverProvider", Value::Bool(true)),
                ("definitionProvider", Value::Bool(true)),
                // The same answer under the protocol's other name for it.
                ("declarationProvider", Value::Bool(true)),
                ("typeDefinitionProvider", Value::Bool(true)),
                // The impl table, read in its three directions.
                ("implementationProvider", Value::Bool(true)),
                ("typeHierarchyProvider", Value::Bool(true)),
                ("callHierarchyProvider", Value::Bool(true)),
                ("monikerProvider", Value::Bool(true)),
                ("referencesProvider", Value::Bool(true)),
                ("documentHighlightProvider", Value::Bool(true)),
                ("documentSymbolProvider", Value::Bool(true)),
                // A swatch beside `Color.Rgb(255, 0, 0)`, and the picker that
                // writes one back.
                ("colorProvider", Value::Bool(true)),
                // The legend is declared once and is protocol: a client reads a
                // token's type as an index into it. `full: { delta: true }`
                // offers the delta and `range` the visible-lines answer.
                (
                    "semanticTokensProvider",
                    Value::object(vec![
                        (
                            "legend",
                            Value::object(vec![
                                (
                                    "tokenTypes",
                                    Value::Array(
                                        semantic_tokens::TYPES
                                            .iter()
                                            .map(|t| Value::str(*t))
                                            .collect(),
                                    ),
                                ),
                                (
                                    "tokenModifiers",
                                    Value::Array(
                                        semantic_tokens::MODIFIERS
                                            .iter()
                                            .map(|m| Value::str(*m))
                                            .collect(),
                                    ),
                                ),
                            ]),
                        ),
                        ("full", Value::object(vec![("delta", Value::Bool(true))])),
                        ("range", Value::Bool(true)),
                    ]),
                ),
                // `resolveProvider: true` here is a claim the other way: the
                // full pass hands out a position, a label and a kind, and the
                // tooltip and the navigation target are computed only for the
                // one hint a reader points at.
                (
                    "inlayHintProvider",
                    Value::object(vec![("resolveProvider", Value::Bool(true))]),
                ),
                // `resolveProvider: false` is a claim rather than a default:
                // the query already returns a complete `Location`, computed
                // from data the scan had in hand, so there is nothing a
                // `workspaceSymbol/resolve` could add.
                (
                    "workspaceSymbolProvider",
                    Value::object(vec![("resolveProvider", Value::Bool(false))]),
                ),
                // `resolveProvider: false` again as a claim: every link this
                // server hands out already carries its `target`, because
                // resolving a module path or a package label is a lookup in a
                // workspace the answer was computed from and not a second,
                // lazier question.
                (
                    "documentLinkProvider",
                    Value::object(vec![("resolveProvider", Value::Bool(false))]),
                ),
                // `resolveProvider: true` is the whole design of the reference
                // lens: the full pass runs for every file scrolled through and
                // the count costs a whole-repository analysis, so the count
                // waits for the one lens an editor is about to draw.
                (
                    "codeLensProvider",
                    Value::object(vec![("resolveProvider", Value::Bool(true))]),
                ),
                // Exactly the commands the server implements, and no more. A
                // client may send only what is listed here, so a name in this
                // list that nothing dispatches would be a promise nothing keeps.
                (
                    "executeCommandProvider",
                    Value::object(vec![(
                        "commands",
                        Value::Array(
                            execute_command::COMMANDS.iter().map(|c| Value::str(*c)).collect(),
                        ),
                    )]),
                ),
                // Pull diagnostics, alongside the push that is still sent.
                //
                // `interFileDependencies: true` is the load-bearing one and it
                // is a fact about the language rather than a preference: the
                // front end is whole-closure, so editing a library changes what
                // is wrong in the binary that imports it. A client reading
                // `false` would re-pull only the file it edited and would keep
                // showing an error somewhere else that no longer exists.
                //
                // `workspaceDiagnostics: true` because most of what this
                // toolchain knows is about a file the editor never opened — a
                // `BUILD.buri` that does not describe its package, above all.
                (
                    "diagnosticProvider",
                    Value::object(vec![
                        ("interFileDependencies", Value::Bool(true)),
                        ("workspaceDiagnostics", Value::Bool(true)),
                    ]),
                ),
                ("documentFormattingProvider", Value::Bool(true)),
                // A range and a keystroke, both filtered out of the whole-file
                // answer — see `formatting`. The trigger characters are the two
                // that close something: a `}` ends a declaration and a `;` ends
                // a statement, and neither is written mid-expression.
                ("documentRangeFormattingProvider", Value::Bool(true)),
                (
                    "documentOnTypeFormattingProvider",
                    Value::object(vec![
                        ("firstTriggerCharacter", Value::str("}")),
                        ("moreTriggerCharacter", Value::Array(vec![Value::str(";")])),
                    ]),
                ),
                ("foldingRangeProvider", Value::Bool(true)),
                ("selectionRangeProvider", Value::Bool(true)),
                // `resolveProvider: true` is where the cost went: the list is
                // titles and their squiggles, and the edit — which for a build
                // file means running `buri gen` — is computed for the one
                // action a reader accepted.
                //
                // `codeActionKinds` is the other half of that claim: every fix
                // here is a `quickfix`, and saying so is what lets a client
                // asking for `source.organizeImports` know not to bother.
                (
                    "codeActionProvider",
                    Value::object(vec![
                        (
                            "codeActionKinds",
                            Value::Array(vec![Value::str(code_actions::QUICKFIX)]),
                        ),
                        ("resolveProvider", Value::Bool(true)),
                    ]),
                ),
                // `prepareProvider` is what lets an editor ask whether a rename
                // is possible before it prompts for a new name.
                (
                    "renameProvider",
                    Value::object(vec![("prepareProvider", Value::Bool(true))]),
                ),
                (
                    "signatureHelpProvider",
                    Value::object(vec![(
                        "triggerCharacters",
                        Value::Array(vec![Value::str("("), Value::str(",")]),
                    )]),
                ),
                // Every item carries its own replacement range, so the client
                // stops guessing which characters a completion is meant to
                // replace. `resolveProvider: true` keeps the `///` prose off
                // the wire until one item is highlighted.
                (
                    "completionProvider",
                    Value::object(vec![
                        ("resolveProvider", Value::Bool(true)),
                        (
                            "triggerCharacters",
                            Value::Array(vec![
                                Value::str("\""),
                                Value::str("{"),
                                Value::str("/"),
                                // A member list has to open on the dot itself:
                                // a client that waits for the first letter of
                                // the name shows nothing for `total.` — which
                                // is exactly when a reader does not know what
                                // to type.
                                Value::str("."),
                            ]),
                        ),
                    ]),
                ),
                // Two open folders are two Buri repositories, kept apart.
                // `changeNotifications` is what makes a folder opened after
                // startup something this server hears about at all.
                (
                    "workspace",
                    Value::object(vec![
                        (
                            "workspaceFolders",
                            Value::object(vec![
                                ("supported", Value::Bool(true)),
                                ("changeNotifications", Value::Bool(true)),
                            ]),
                        ),
                        // A file created, renamed or deleted is a change to
                        // what a `BUILD.buri` says its package holds, and a
                        // rename is a change to every import naming the module.
                        ("fileOperations", file_operations::capability()),
                    ]),
                ),
            ]),
        ),
        (
            "serverInfo",
            Value::object(vec![
                ("name", Value::str("buri")),
                ("version", Value::str(arguments::VERSION)),
            ]),
        ),
    ])
}

/// Watch every `.buri` file in every folder, and tell me when one changes.
///
/// One pattern covers all three kinds, and that is a property of the language
/// rather than a shortcut: a source, a `BUILD.buri` and a `REPO.buri` all wear
/// the `.buri` extension, and `**/` in the protocol's glob matches any number
/// of path segments *including none*, so `REPO.buri` at the root of a folder
/// matches it as surely as `lib/money/BUILD.buri` does.
///
/// `kind` is spelled out as create-change-delete rather than left to its
/// default, because all three change the answer: a build file appearing is as
/// much a change to a package as an edit to one.
fn watcher_registration() -> Value {
    request(
        WATCHERS,
        "client/registerCapability",
        Value::object(vec![(
            "registrations",
            Value::Array(vec![Value::object(vec![
                ("id", Value::str(WATCHERS)),
                ("method", Value::str("workspace/didChangeWatchedFiles")),
                (
                    "registerOptions",
                    Value::object(vec![(
                        "watchers",
                        Value::Array(vec![Value::object(vec![
                            ("globPattern", Value::str("**/*.buri")),
                            ("kind", Value::number(7)),
                        ])]),
                    )]),
                ),
            ])]),
        )]),
    )
}

/// What the client said back.
///
/// The registration's reply carries nothing to read — it either failed, and
/// the client says so in an `error`, or the watcher is running. The folder
/// question's reply is the list this server asked for, and adopting it is the
/// whole point of asking.
///
/// An `applyEdit` reply is the one that produces a message of its own: the
/// `workspace/executeCommand` that sent the edit has been waiting for it, and
/// what the client says it did with the edit *is* that command's result. A
/// client that refused the edit with an error rather than a result did not
/// apply it, and saying so is a better answer than a silence the command's
/// caller cannot tell from success.
fn client_response(state: &mut State, msg: &Value) -> Vec<Value> {
    let id = msg.get("id").and_then(|i| i.as_str()).unwrap_or("");
    if id == FOLDERS {
        for folder in folders(msg.get("result")) {
            state.add_root(&folder);
        }
        return Vec::new();
    }
    if let Some(command) = state.applying.waiting.remove(id) {
        let applied = msg
            .get("result")
            .cloned()
            .unwrap_or_else(|| Value::object(vec![("applied", Value::Bool(false))]));
        return vec![response(&command, applied)];
    }
    Vec::new()
}

/// The folders in a `WorkspaceFolder[]`, wherever one appears: `initialize`
/// params, a `didChangeWorkspaceFolders` event, or the answer to the server's
/// own question.
fn folders(value: Option<&Value>) -> Vec<PathBuf> {
    let Some(items) = value.and_then(|v| v.as_array()) else { return Vec::new() };
    items
        .iter()
        .filter_map(|f| f.get("uri"))
        .filter_map(|u| u.as_str())
        .filter_map(convert::path_of)
        .collect()
}

pub(super) fn uri_param(params: &Value) -> Option<PathBuf> {
    params.at("textDocument.uri").and_then(|u| u.as_str()).and_then(convert::path_of)
}

fn opened(params: &Value) -> Option<(PathBuf, String)> {
    let path = uri_param(params)?;
    let text = params.at("textDocument.text")?.as_str()?.to_string();
    Some((path, text))
}

/// The range covering a whole file, for the edits that replace one.
fn whole(text: &str) -> Value {
    Value::object(vec![
        ("start", Position { line: 0, character: 0 }.to_json()),
        (
            "end",
            convert::position_of(text, u32::try_from(text.len()).unwrap_or(u32::MAX)).to_json(),
        ),
    ])
}

/// Which formatter a file gets is decided by its name, exactly as it is at the
/// terminal — a `BUILD.buri` is textproto and everything else is Buri.
fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// The whole file, formatted — what `formatting` and `willSaveWaitUntil` both
/// answer.
fn whole_file_format(state: &mut State, params: &Value) -> Value {
    (|| {
        let path = uri_param(params)?;
        let text = state.text_of(&path)?;
        Some(formatting::whole_file(&file_name(&path), &text))
    })()
    .unwrap_or(Value::Array(Vec::new()))
}

/// Definition, routed by what kind of file the cursor is in.
///
/// A build file is textproto and is answered before any analysis, because
/// `driver::analyze` never reads one — running it on a `BUILD.buri` would
/// analyse the target that owns it and then look for a module that is not
/// there. Everything else is a module and goes the ordinary way.
fn definition(state: &mut State, params: &Value) -> Option<Value> {
    let path = uri_param(params)?;
    if build_files::is_build_file(&path) {
        let position = Position::from_json(params.get("position")?)?;
        let text = state.text_of(&path)?;
        let session = state.session_for(&path)?;
        return build_files::definition(&session, &path, &text, position);
    }
    with_analysis(state, params, features::definition)
}

/// Hover, routed the way [`definition`] is.
///
/// A build file's answer comes from the schema its fields are declared in, and
/// asking the analysis about one would be asking about a module the package
/// declares rather than about the file the cursor is in.
fn hover(state: &mut State, params: &Value, markup: features::Markup) -> Option<Value> {
    let path = uri_param(params)?;
    if build_files::is_build_file(&path) {
        let position = Position::from_json(params.get("position")?)?;
        let text = state.text_of(&path)?;
        return build_files::hover(&text, position, markup);
    }
    with_analysis(state, params, |analyzed, path, text, position| {
        features::hover(analyzed, path, text, position, markup)
    })
}

/// Completion, routed the same way.
fn complete(state: &mut State, params: &Value) -> Option<Value> {
    let path = uri_param(params)?;
    if build_files::is_build_file(&path) {
        let position = Position::from_json(params.get("position")?)?;
        let text = state.text_of(&path)?;
        let session = state.session_for(&path)?;
        return Some(build_files::completion(&session, &path, &text, position));
    }
    with_analysis(state, params, |analyzed, path, text, position| {
        Some(completion::completion(analyzed, path, text, position))
    })
}

fn with_analysis<T>(
    state: &mut State,
    params: &Value,
    f: impl Fn(&state::Analyzed, &std::path::Path, &str, Position) -> Option<T>,
) -> Option<T> {
    let path = uri_param(params)?;
    let position = Position::from_json(params.get("position")?)?;
    let text = state.text_of(&path)?;
    // Every one of these reads the bodies of the file the cursor is in and
    // filters the rest out by file id, so the rest were never worth checking.
    // See `State::analyze_for_query`.
    let analyzed = state.analyze_for_query(&path)?;
    f(&analyzed, &path, &text, position)
}

/// The same, over every target in the repository at once.
///
/// The requests that ask about a *name* rather than about a file need this
/// one: a name is referred to, and implemented, from wherever it is imported,
/// and nothing about the file under the cursor bounds that set. See
/// `State::analyze_workspace`.
fn with_workspace<T>(
    state: &mut State,
    params: &Value,
    f: impl Fn(&state::Analyzed, &std::path::Path, &str, Position) -> Option<T>,
) -> Option<T> {
    let path = uri_param(params)?;
    let position = Position::from_json(params.get("position")?)?;
    let text = state.text_of(&path)?;
    let root = state.root_of(&path)?;
    let analyzed = state.analyze_workspace(&root)?;
    f(&analyzed, &path, &text, position)
}

/// The symbol a `TypeHierarchyItem` stands for.
///
/// The item is what `prepareTypeHierarchy` handed out and the client hands
/// back, and it carries the file and the position of the declaration's name in
/// its `data`. Resolving it again from those is what keeps the two walking
/// requests stateless: a server that remembered every item it had produced
/// would be holding a table nothing ever removes an entry from.
fn hierarchy_symbol(
    state: &mut State,
    params: &Value,
) -> Option<(std::rc::Rc<state::Analyzed>, symbols::Symbol)> {
    let path = params.at("item.data.uri").and_then(|u| u.as_str()).and_then(convert::path_of)?;
    let position = Position::from_json(params.at("item.data.position")?)?;
    let text = state.text_of(&path)?;
    let root = state.root_of(&path)?;
    let analyzed = state.analyze_workspace(&root)?;
    let offset = convert::offset_of(&text, position);
    let found = symbols::at(&analyzed, &path, &text, offset)?;
    Some((analyzed, found.symbol))
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// Parse errors for one buffer. No workspace, no standard library, no imports.
///
/// **A keystroke may add to what the editor is showing and may not take
/// anything away.** A publish replaces every diagnostic the client holds for
/// that file, so publishing the parse errors on every change erased the type
/// errors the last analysis found: they came back on save and were gone again
/// on the next character, which reads as the server not reporting them at all.
///
/// So a buffer that parses publishes nothing, and the analysis findings stay on
/// screen with the client moving them as the text moves. A buffer that does not
/// parse publishes its parse errors, and when it parses again what the analysis
/// last said goes back.
fn parse_diagnostics(state: &mut State, path: &std::path::Path, text: &str) -> Vec<Value> {
    // Which parser reads the buffer is decided by what kind of file it is. A
    // `BUILD.buri` is textproto, and the Buri lexer refused its every
    // `# comment` — a syntax error on a file that is not in that syntax.
    let errors = if build_files::is_build_file(path) {
        build_files::diagnostics(text)
    } else {
        crate::parsing::parser::parse(text, crate::diagnostics::FileId(0)).errors
    };
    let uri = convert::uri_of(path);
    if errors.is_empty() {
        if !state.showing_parse_errors.remove(&uri) {
            return Vec::new();
        }
        let items = state.published.get(&uri).cloned().unwrap_or_default();
        return vec![publish(&uri, items)];
    }
    state.showing_parse_errors.insert(uri.clone());
    let items: Vec<Value> = errors.iter().map(|d| convert::diagnostic(text, d, &uri)).collect();
    vec![publish(&uri, items)]
}

/// Everything the front end has to say, for every file it looked at.
///
/// Diagnostics are published per file, and a file that had findings a moment
/// ago and has none now must be published *empty* — otherwise the editor keeps
/// showing the errors you just fixed.
///
/// Every open buffer is asked about too, and not only the file that prompted
/// this. The seed publishes each of them empty, so analysing one target and
/// publishing that alone deleted the findings of every buffer outside its
/// closure: opening a file in one target cleared another target's squiggles,
/// and nothing brought them back until something re-analysed that target.
fn full_diagnostics(state: &mut State, path: &std::path::Path) -> Vec<Value> {
    // The file that prompted this always gets a message, empty or not.
    let mut published = seeded(state, Some(path));
    findings_for(state, path, &mut published);
    open_findings(state, &mut published);
    remember(state, published)
}

/// The same, for every buffer the editor has open and no file in particular.
///
/// This is what a change the *editor did not make* asks for: a `BUILD.buri`
/// written by `buri gen` or by the fix a code action returned, a branch
/// switched underneath, a file appearing. There is no one buffer to re-analyse,
/// because a build file decides what every file in its package can see — so
/// each open buffer's own target is asked again and the answers merge into one
/// publish per file.
fn republish_open(state: &mut State) -> Vec<Value> {
    let mut published = seeded(state, None);
    open_findings(state, &mut published);
    remember(state, published)
}

/// Each open buffer's own target, asked again and merged.
///
/// One analysis per target rather than per buffer: the cache is keyed on what
/// was read, so two buffers in one closure ask the same question once.
fn open_findings(state: &mut State, published: &mut Published) {
    for path in state.open.keys().cloned().collect::<Vec<_>>() {
        findings_for(state, &path, published);
    }
}

/// An empty publish for every file that must hear something, so that a file
/// whose findings are gone is told they are gone.
fn seeded(state: &State, path: Option<&std::path::Path>) -> Published {
    let mut published = Published::new();
    if let Some(path) = path {
        published.insert(convert::uri_of(path), Vec::new());
    }
    for open in state.open.keys() {
        published.insert(convert::uri_of(open), Vec::new());
    }
    published
}

type Published = std::collections::BTreeMap<String, Vec<Value>>;

/// Everything both passes have to say about the closure `path` is in.
fn findings_for(state: &mut State, path: &std::path::Path, published: &mut Published) {
    // A build file's own syntax. No analysis reports it — `driver::analyze`
    // never opens one — so an unreadable `BUILD.buri` used to be a repository
    // that quietly stopped answering rather than a file with a squiggle in it.
    if build_files::is_build_file(path) {
        if let Some(text) = state.text_of(path) {
            let uri = convert::uri_of(path);
            let items: Vec<Value> = build_files::diagnostics(&text)
                .iter()
                .map(|d| convert::diagnostic(&text, d, &uri))
                .collect();
            published.entry(uri).or_default().extend(items);
        }
    }
    if let Some(analyzed) = state.analyze(path) {
        for d in &analyzed.analysis.diagnostics.items {
            add_finding(published, &analyzed.session, d);
        }
    }
    // The build-graph findings too. An editor that showed only type errors
    // would be showing half of what the toolchain knows, and the half that is
    // easier to notice at the terminal — a missing dependency is exactly the
    // kind of thing you want told about while the import is still on screen.
    if let Some(linted) = state.lint(path) {
        for d in &linted.diagnostics.items {
            add_finding(published, &linted.session, d);
        }
    }
}

/// One finding, published once.
///
/// The lint pass runs its own analysis, so a compile error is found by both and
/// two buffers in one target are asked the same question twice. Either way the
/// same words at the same place are one squiggle, not two.
fn add_finding(published: &mut Published, session: &Session, d: &crate::diagnostics::Diagnostic) {
    add_finding_rendering(published, &mut Rendered::new(), session, d);
}

/// What a finding is filed under before it is rendered: the file, the span, and
/// the words.
///
/// Exactly what [`same_finding`] compares, one step earlier — a range is a
/// function of a span and the file's text, so two diagnostics that agree here
/// render to the same item.
type Rendered = std::collections::BTreeMap<(String, u32, u32, String), Value>;

/// The same, rendering each distinct finding once however many times it is met.
///
/// Turning a byte offset into a line and a character walks the file from the
/// top, which makes rendering the expensive half of publishing a finding. A
/// repository sweep meets the same error in a shared library once per target
/// that reaches it, and used to pay for the walk every time.
fn add_finding_rendering(
    published: &mut Published,
    rendered: &mut Rendered,
    session: &Session,
    d: &crate::diagnostics::Diagnostic,
) {
    if d.span.is_none() {
        return;
    }
    let f = session.map.get(d.span.file);
    if f.abs_path.as_os_str().is_empty() {
        return;
    }
    let uri = convert::uri_of(&f.abs_path);
    let key = (uri.clone(), d.span.start, d.span.end, d.message.clone());
    let item = match rendered.get(&key) {
        Some(known) => known.clone(),
        None => {
            let item = convert::diagnostic(&f.text, d, &uri);
            rendered.insert(key, item.clone());
            item
        }
    };
    let bucket = published.entry(uri).or_default();
    if !bucket.iter().any(|existing| same_finding(existing, &item)) {
        bucket.push(item);
    }
}

/// Publishes, and keeps a copy so that a keystroke can put it back.
fn remember(state: &mut State, published: Published) -> Vec<Value> {
    let mut out = Vec::new();
    for (uri, items) in published {
        out.push(publish(&uri, items.clone()));
        state.showing_parse_errors.remove(&uri);
        state.published.insert(uri, items);
    }
    out
}

/// Whether two diagnostics are the same one seen twice: same place, same words.
fn same_finding(analyzed: &Value, b: &Value) -> bool {
    analyzed.get("message") == b.get("message") && analyzed.get("range") == b.get("range")
}

fn publish(uri: &str, items: Vec<Value>) -> Value {
    notification(
        "textDocument/publishDiagnostics",
        Value::object(vec![("uri", Value::str(uri)), ("diagnostics", Value::Array(items))]),
    )
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

    fn initialized() -> State {
        let mut state = State::new();
        let init = json::parse(r#"{"id":0,"method":"initialize","params":{}}"#).unwrap();
        assert_eq!(handle(&mut state, &init).len(), 1);
        state
    }

    /// An unknown *request* is refused, and refusing is still replying. A
    /// client that sent one and got nothing back waits forever, which presents
    /// as the server having hung; one that got `result: null` was handed a
    /// shape several of these requests have no legal null for.
    ///
    /// The method is a made-up one rather than a real spec method, so that
    /// implementing something does not quietly turn this into a test of it.
    #[test]
    fn an_unknown_request_is_refused_and_an_unknown_notification_is_not() {
        let mut state = initialized();
        let req = json::parse(r#"{"id":7,"method":"buri/notAMethod"}"#).unwrap();
        let out = handle(&mut state, &req);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].get("id").and_then(|v| v.as_u32()), Some(7));
        assert_eq!(out[0].at("error.code"), Some(&Value::Int(METHOD_NOT_FOUND)));
        assert!(out[0].get("result").is_none());

        let note = json::parse(r#"{"method":"buri/notANotification"}"#).unwrap();
        assert!(handle(&mut state, &note).is_empty());
    }

    /// The two orders a cancel can arrive in, and the trap between them: an id
    /// withdrawn before its turn is refused, and one withdrawn after its answer
    /// is a no-op that must not refuse a *later* request carrying the same id.
    #[test]
    fn a_cancel_refuses_the_request_it_names_and_nothing_else() {
        let mut state = initialized();
        let cancel = json::parse(r#"{"method":"$/cancelRequest","params":{"id":7}}"#).unwrap();
        assert!(handle(&mut state, &cancel).is_empty());

        let request = json::parse(r#"{"id":7,"method":"textDocument/hover"}"#).unwrap();
        let out = handle(&mut state, &request);
        assert_eq!(out[0].at("error.code"), Some(&Value::Int(REQUEST_CANCELLED)));

        // Answered, then cancelled: the cancel has nothing to withdraw.
        let out = handle(&mut state, &request);
        assert!(out[0].get("result").is_some(), "{}", out[0].to_string());
        assert!(handle(&mut state, &cancel).is_empty());
        let out = handle(&mut state, &request);
        assert!(out[0].get("result").is_some(), "{}", out[0].to_string());
    }

    /// A trace level the client asked for, and the lines it turns on.
    #[test]
    fn setting_a_trace_level_turns_the_log_on_and_off() {
        let mut state = initialized();
        let request = json::parse(r#"{"id":1,"method":"textDocument/hover"}"#).unwrap();
        assert_eq!(handle(&mut state, &request).len(), 1, "off says nothing");

        let on = json::parse(r#"{"method":"$/setTrace","params":{"value":"messages"}}"#).unwrap();
        assert!(handle(&mut state, &on).is_empty());
        let out = handle(&mut state, &request);
        assert_eq!(out.len(), 3, "a line on the way in and one on the way out");
        assert_eq!(out[0].get("method").and_then(|m| m.as_str()), Some("$/logTrace"));
        assert_eq!(out[2].get("method").and_then(|m| m.as_str()), Some("$/logTrace"));

        // A word that is not a level leaves the level where it was.
        let bad = json::parse(r#"{"method":"$/setTrace","params":{"value":"shout"}}"#).unwrap();
        handle(&mut state, &bad);
        assert_eq!(handle(&mut state, &request).len(), 3);
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
