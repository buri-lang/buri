//! Whole repositories, each provoking one build-system rule.
//!
//! The `reject/` corpus builds every case as a single-package binary with no
//! dependencies, so nothing in it can express a diagnostic *about the graph* —
//! which is most of what the build system checks. Each case here is instead a
//! small repository checked in whole, with a manifest naming what the CLI does
//! in it and what that must print. See `harness/case.rs` for the format.
//!
//! One test per specification document, so cargo runs them on separate threads
//! and a failure names the document to open.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test build repositories::    # record the goldens
//! BURI_KEEP=1  cargo test -p buri --test build repositories::    # keep the scratch trees
//! ```
use crate::harness::*;
use std::path::{Path, PathBuf};
use std::process::Command;

/// BUILD-FILES.md: what a rule declares, and the diagnostics that fire when
/// the declaration and the code disagree.
#[test]
fn build_file_rules() {
    run_corpus(&tests_dir().join("repositories/build-files"), "build-files", 12);
}

/// LIBRARIES.md: `lib.buri` is a library's entire public surface, and the
/// boundary it draws applies to methods as much as to names.
#[test]
fn library_boundaries() {
    run_corpus(&tests_dir().join("repositories/libraries"), "libraries", 11);
}

/// TAGS.md: a tag is a property of a whole dependency closure, and the two
/// things that follow from one — what may not sit beside it, and where it may
/// be built.
#[test]
fn tag_policy() {
    run_corpus(&tests_dir().join("repositories/tags"), "tags", 7);
}

/// CLI.md: the exit codes, and the commands whose contract is about what they
/// leave on disk rather than what they compute — `gen`, `run`, `clean`,
/// `version`, `add skills`, the `out/` symlink, and the no-argument forms that
/// mean the whole repository from wherever they are run.
#[test]
fn cli_contract() {
    run_corpus(&tests_dir().join("repositories/cli"), "cli", 13);
}

/// CLI.md's `query`: what the graph says, asked without building anything.
/// Its own corpus because the answers are the recorded output rather than a
/// diagnostic — a query that has stopped working prints a plausible wrong
/// answer rather than failing.
#[test]
fn graph_queries() {
    run_corpus(&tests_dir().join("repositories/query"), "query", 1);
}

/// PROTO.md: a `.proto` schema is a source that becomes a module. One case for
/// the build-file half — declared, placed by `gen`, keyed on its contents,
/// internal to the rule that declared it — one for the edition the reader
/// requires and the two syntaxes it refuses, and one for each half of what it
/// otherwise refuses: the constructs that are out of scope, and the files that
/// are not schemas at all. The sixth is `google.protobuf.Any`, which is a
/// message like any other here and is resolved by name rather than recognised.
#[test]
fn proto_schemas() {
    run_corpus(&tests_dir().join("repositories/proto"), "proto", 6);
}

/// CLI.md's lint catalogue: the hygiene rules, which ask about a package's own
/// code rather than about the graph. Each case ends with the edit that makes
/// the finding go away, because a rule nothing can turn off is a rule nobody
/// can check the fix for.
///
/// The `repo_lint_*` cases are the other half: what `REPO.buri`'s `lint` block
/// does to the same finding — when the catalogue runs, how hard a finding
/// lands, and what a misspelled field in the block costs.
///
/// Eleven of them are about a file the front end had something to say about,
/// and together they draw the line the rules stay behind. Six are about the
/// shape of the silence: what is still reported around a file that did not
/// parse, what is rightly not reported inside the declaration that did not,
/// that a package's neighbour going quiet does not quiet it, that a broken
/// file *underneath* a package does not quiet the two packages above it, that
/// a second broken file is not a second reason to stop, and that a *build*
/// file which does not read is the one thing recovery does not read around.
///
/// Nine more are the dead-code family — a type, a field and a variant nothing
/// uses — and four of them are negatives: a type named only by a signature or
/// an alias, a field read only by the module beside it, a type built only by
/// literals that name no type at all, and everything on the library's surface,
/// none of which is reported. The rest are what is: the field elision leaves
/// out of every literal, the variant a `_` arm meets and no shorthand builds,
/// and the two shapes of doubt — an unresolved re-export, which reaches the
/// exported names and no further, and a body that did not check, which reaches
/// the names written inside it and no further.
///
/// The anonymous-literal case is the one that answers a grammar change rather
/// than a rule: a bare `{ … }` builds a struct while naming nothing, so the
/// token half of the census cannot see the construction and the typed tree is
/// the whole of the evidence. It pins both directions at once — the type is
/// alive, including the private one no surface could have exempted, and the
/// field the literal fills is still reported, because filling a field in is
/// not reading it whether or not the literal wears a head.
///
/// The other five say what an error is *not* a reason to go quiet about, which
/// is the harder half and the one that regressed. A declaration the parser
/// recovered whole — an import missing its `;` — hides nothing below it. An
/// error about one declaration the parser read whole — an alias that closes a
/// cycle — hides nothing beside it. A body that did not check hides its own
/// bindings and not its neighbour's. And the two things that genuinely do hide
/// something hide exactly what they cost: a run of declarations the parser
/// skipped hides what it swallowed and nothing that merely sits near it, and a
/// re-export that did not resolve stops `dead-code` from calling the name it
/// meant to reach unreached — one typo, one finding.
///
/// The generated half of that question is `cli/tests/linting.rs`, six hundred
/// of them, with the rate of lost findings pinned as an invariant and the
/// parity between this command and the language server stated alongside it.
#[test]
fn lint_catalogue() {
    run_corpus(&tests_dir().join("repositories/linting"), "linting", 47);
}

/// TESTING.md: where tests live, what a test source may reach, and what the
/// runner does with a suite — the flags, the timeout, the golden-file update
/// mode, the exact shape of a failure report, and the verdict a suite that
/// never compiled gets.
#[test]
fn test_suites() {
    run_corpus(&tests_dir().join("repositories/testing"), "testing", 11);
}

/// The language server. Each case is a recorded session: requests in, decoded
/// responses out, so a change to what the server says shows up as a diff
/// rather than as an editor behaving differently.
#[test]
fn language_server() {
    run_corpus(&tests_dir().join("repositories/lsp"), "lsp", 93);
}

/// Every method a 3.17 client can send is answered by the dispatch, and is
/// sent by at least one recorded session.
///
/// `cli/src/docs/cli/lsp.md`'s table says "there is no third column of things
/// left for later", and until this test nothing held it to that: the claim was
/// prose, the enumeration was prose, and `$/progress` sat with neither an arm
/// nor a golden while the table read complete. The list is
/// `language_server::CLIENT_TO_SERVER`, beside the dispatch it describes.
///
/// The answer has to come from a **running server**. The first version of this
/// test looked the names up in the text of `cli/src/language_server/` — the
/// directory `CLIENT_TO_SERVER` is itself written in — so the list was its own
/// witness: the `$/progress` arm could be deleted with every name still found.
///
/// So each name is asked of one `buri lsp` over a small repository:
///
///  * **a request is sent and answered**, and anything but `-32601` is an arm.
///    A `-32601` counts only where a golden records that refusal naming the
///    method, which is how `documentLink/resolve` and `workspaceSymbol/resolve`
///    are decisions written down rather than omissions;
///  * **a notification is sent** and the request behind it still answers, and
///    its name has to appear in a *pattern* of `dispatch`'s match. That last
///    half is the one thing a running server cannot show: a notification the
///    server handled and one that fell through the catch-all both say nothing;
///  * **a recorded session sends it**, so the answer is a thing that ran rather
///    than a branch nobody has taken.
#[test]
fn the_protocol_surface_is_covered() {
    let surface = buri::language_server::CLIENT_TO_SERVER;
    let (sessions, refusals, cases) = recorded_sessions();
    assert!(cases > 50, "found {cases} lsp cases; the walk is broken");
    let source = std::fs::read_to_string(repo_root().join("cli/src/language_server/mod.rs"))
        .expect("the dispatch's own module");
    let arms = dispatch_arms(&source);
    assert!(arms.len() > 40, "{} arms read out of the dispatch; the scan is broken", arms.len());
    for method in NOTIFICATIONS {
        assert!(
            surface.contains(method),
            "`{method}` is not a method a client sends; the notification list is stale"
        );
    }
    let (notifications, requests): (Vec<&str>, Vec<&str>) =
        surface.iter().copied().partition(|m| NOTIFICATIONS.contains(m));

    let scratch = Scratch::repo("lsp-surface");
    scratch.binary_package("cmd/app", SURFACE_PROGRAM);
    let mut editor = Editor::open(&scratch.root);
    let params = surface_params(&editor.uri("cmd/app/main.buri"), SURFACE_PROGRAM);
    let mut missing = Vec::new();

    // The notifications first, `exit` excepted: that one ends the session. One
    // of them closes the buffer the requests below need, so the file is opened
    // again once they have all been sent.
    for &method in notifications.iter().filter(|m| **m != "exit") {
        editor.notify(method, &params);
        let alive = editor.ask("textDocument/linkedEditingRange", &params);
        assert!(
            alive.contains(r#""result""#),
            "the server stopped answering after the `{method}` notification: {alive}"
        );
        if !arms.iter().any(|arm| arm == method) {
            missing.push(format!("  {method}: no arm in the dispatch's match"));
        }
    }
    editor.notify("textDocument/didOpen", &params);

    // Then every request, `shutdown` last for the reason `exit` is not here.
    for &method in requests.iter().filter(|m| **m != "shutdown") {
        missing.extend(unanswered(&mut editor, method, &params, &refusals));
    }
    missing.extend(unanswered(&mut editor, "shutdown", &params, &refusals));
    if !arms.iter().any(|arm| arm == "exit") {
        missing.push("  exit: no arm in the dispatch's match".to_string());
    }
    editor.notify("exit", &params);
    let stopped = editor.process.wait().expect("the language server did not stop");
    assert!(stopped.success(), "the server left the whole surface badly: {stopped}");

    for method in surface {
        let sent = sessions.contains(&format!("{q}method{q}: {q}{method}{q}", q = '"'))
            || sessions.contains(&format!("{q}method{q}:{q}{method}{q}", q = '"'));
        if !sent {
            missing.push(format!("  {method}: no recorded session sends it"));
        }
    }
    assert!(
        missing.is_empty(),
        "{} of {} client methods are not covered:\n{}\n\nEither answer them in the \
         dispatch and record a session under the lsp corpus, or take them out of \
         `language_server::CLIENT_TO_SERVER` with the reason — but do not leave the \
         `lsp` reference page claiming a surface no case exercises.",
        missing.len(),
        surface.len(),
        missing.join("\n")
    );
    eprintln!("protocol surface: {} client methods, all dispatched and all sent", surface.len());
}

/// One request put to a live server, and what is wrong with the answer.
///
/// `-32601` is the catch-all, so it is a hole unless a golden session records
/// that refusal by name — which is what makes a refusal a decision.
fn unanswered(editor: &mut Editor, method: &str, params: &str, refusals: &str) -> Option<String> {
    let answer = editor.ask(method, params);
    let refused = answer.contains(r#""code":-32601"#);
    let recorded = refusals.contains(&format!("`{method}` is not implemented"));
    (refused && !recorded)
        .then(|| format!("  {method}: answered -32601, and no golden records that refusal"))
}

/// The methods a client sends as notifications rather than as requests.
///
/// Written out because nothing in a name says which it is —
/// `textDocument/willSave` is a notification and `willSaveWaitUntil` is a
/// request — and the test holds every entry to being a method
/// `CLIENT_TO_SERVER` lists.
const NOTIFICATIONS: &[&str] = &[
    "initialized",
    "exit",
    "$/cancelRequest",
    "$/progress",
    "$/setTrace",
    "workspace/didChangeWorkspaceFolders",
    "workspace/didChangeConfiguration",
    "workspace/didChangeWatchedFiles",
    "workspace/didCreateFiles",
    "workspace/didRenameFiles",
    "workspace/didDeleteFiles",
    "textDocument/didOpen",
    "textDocument/didChange",
    "textDocument/willSave",
    "textDocument/didSave",
    "textDocument/didClose",
    "notebookDocument/didOpen",
    "notebookDocument/didChange",
    "notebookDocument/didSave",
    "notebookDocument/didClose",
    "window/workDoneProgress/cancel",
];

/// The program the surface is driven against: one file, one import and one
/// call, so that a position request has something under it.
const SURFACE_PROGRAM: &str = r#"from "core/effect/lib.buri" import { Alloc, Stdout };
from "core/host/lib.buri" import * as host;

fn answer(): Int { 41 }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("answer=${answer()}");
  .Ok(())
}
"#;

/// One params object carrying every field any client method reads, so that a
/// single envelope drives the whole surface: a handler that wants a field finds
/// it, and one that does not ignores it.
///
/// The values are minimal rather than meaningful — position `0:0`, an empty
/// query, a command no server implements. What is being asked is whether the
/// method reaches an arm, and an arm answers a request it cannot serve with
/// something other than `-32601`.
fn surface_params(uri: &str, text: &str) -> String {
    let at = r#"{"line":0,"character":0}"#;
    let range = format!(r#"{{"start":{at},"end":{at}}}"#);
    // What a hierarchy item and a resolvable item both carry: the round trip
    // is through `data`, so that is the whole of either.
    let data = format!(r#"{{"uri":"{uri}","position":{at}}}"#);
    let fields = [
        format!(
            r#""textDocument":{{"uri":"{uri}","languageId":"buri","version":2,"text":{}}}"#,
            quoted(text)
        ),
        format!(r#""position":{at},"positions":[{at}],"range":{range}"#),
        r#""context":{"diagnostics":[],"includeDeclaration":true}"#.to_string(),
        format!(r#""contentChanges":[{{"text":{}}}]"#, quoted(text)),
        format!(r#""item":{{"data":{data}}},"data":{data}"#),
        format!(r#""files":[{{"uri":"{uri}","oldUri":"{uri}","newUri":"{uri}"}}]"#),
        r#""event":{"added":[],"removed":[]},"changes":[]"#.to_string(),
        r#""query":"","command":"buri.notACommand","arguments":[]"#.to_string(),
        r#""newName":"renamed","previousResultIds":[],"settings":{}"#.to_string(),
        // `off` so that the `$/setTrace` does not fill the rest of the session
        // with `$/logTrace`, and an id this client never sends for the cancel.
        r#""value":"off","token":"buri/surface","id":0"#.to_string(),
    ];
    format!("{{{}}}", fields.join(","))
}

/// The method names in the *pattern* position of `dispatch`'s match — the arms
/// the server has, rather than the strings its sources mention.
///
/// A scan rather than a parse: `//` comments and string bodies are stepped
/// over, brackets are counted, and a name counts when it is read before its
/// arm's `=>` at the match's own depth.
fn dispatch_arms(source: &str) -> Vec<String> {
    const OPENS: &str = "match (method, id) {";
    let start = source.find(OPENS).expect("dispatch's match on the method") + OPENS.len();
    let body: Vec<char> = source[start..].chars().collect();
    let mut arms = Vec::new();
    let mut depth = 0i32;
    let mut in_pattern = true;
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            '/' if body.get(i + 1) == Some(&'/') => {
                while i < body.len() && body[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                let mut literal = String::new();
                i += 1;
                while i < body.len() && body[i] != '"' {
                    if body[i] == '\\' {
                        i += 1;
                    }
                    if let Some(c) = body.get(i) {
                        literal.push(*c);
                    }
                    i += 1;
                }
                if in_pattern {
                    arms.push(literal);
                }
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                // Below the match's own depth is the brace that closes it.
                if depth < 0 {
                    break;
                }
                if depth == 0 {
                    in_pattern = true;
                }
            }
            '=' if depth == 0 && body.get(i + 1) == Some(&'>') => {
                in_pattern = false;
                i += 1;
            }
            ',' if depth == 0 => in_pattern = true,
            _ => {}
        }
        i += 1;
    }
    arms
}

/// Every recorded lsp session's text, every golden's text, and the number of
/// cases the two were read from.
fn recorded_sessions() -> (String, String, usize) {
    let dir = tests_dir().join("repositories/lsp");
    let mut sessions = String::new();
    let mut refusals = String::new();
    let mut cases = 0;
    for entry in std::fs::read_dir(&dir).expect("the lsp corpus").filter_map(Result::ok) {
        cases += 1;
        // A case may record several sessions — `session.jsonl` and the
        // `session_*.jsonl` beside it, each a different client.
        let files = std::fs::read_dir(entry.path()).into_iter().flatten().filter_map(Result::ok);
        for file in files {
            let path = file.path();
            let named = path.file_name().is_some_and(|n| {
                let n = n.to_string_lossy();
                n.starts_with("session") && n.ends_with(".jsonl")
            });
            if named {
                sessions.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
        if let Ok(text) = std::fs::read_to_string(entry.path().join("expected/session.txt")) {
            refusals.push_str(&text);
        }
    }
    (sessions, refusals, cases)
}

// ---------------------------------------------------------------------------
// The language server's budget
// ---------------------------------------------------------------------------

/// What an editor request must answer inside.
///
/// Fifty milliseconds is the number a keystroke can hide behind: below it the
/// squiggle arrives with the character that caused it, above it the editor is
/// visibly waiting. The corpus above pins the *work* each request does, which
/// is what the speed is made of and is the same number on every machine; this
/// is the same claim in the unit a reader cares about.
const LANGUAGE_SERVER_BUDGET: std::time::Duration = std::time::Duration::from_millis(50);

/// The bar this run holds a request to: the 50 ms above, widened by
/// `BURI_PERF_BUDGET_SCALE` on a machine slower than the one the number was
/// taken on. A developer's machine sets nothing; CI sets what it measured.
fn language_server_budget() -> std::time::Duration {
    let scale = std::env::var("BURI_PERF_BUDGET_SCALE")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|scale| (1.0..=100.0).contains(scale))
        .unwrap_or(1.0);
    LANGUAGE_SERVER_BUDGET.mul_f64(scale)
}

/// One session's timings, measured again while any of them is over the bar,
/// every request kept at the fastest time it was seen in.
///
/// A budget test is a claim about what a request *costs*, and a wall clock on
/// a shared runner sometimes answers a different question: a request that lost
/// the core between its send and its answer reads as ten times its own work
/// while nothing about the server changed. That is not a number to fail on,
/// and it is not a reason to widen the bar either — a bar moved to fit the
/// unluckiest timeslice stops having an opinion about the work. So one run is
/// not the verdict. A run holding a request over the bar is taken again, from
/// a fresh server against a fresh copy of the repository, and each request is
/// then held to the best of its attempts.
///
/// Which leaves the 50 ms exactly where it was. A request that got slower is
/// slower in every attempt and still fails; a preempted one is not, and the
/// extra session is paid for only by the runs that would otherwise have been a
/// red X nobody could reproduce. It is the *measurement* that repeats and not
/// the assertion: the bar is applied once, to the best readings.
///
/// The fastest attempt rather than the middle one because the distribution is
/// one-sided — a machine can only make a run slower — so the shortest reading
/// is the least noisy reading of the same quantity, which is the view
/// `design/PERFORMANCE.md` §2's protocol takes of the benchmark suite's
/// samples, where the fastest of them is reported beside the median for
/// exactly that reason.
fn best_of(
    budget: std::time::Duration,
    mut session: impl FnMut() -> Vec<(String, std::time::Duration)>,
) -> Vec<(String, std::time::Duration)> {
    /// How many sessions one run may spend. Three: the first, and two more for
    /// an unlucky request to fail to reproduce itself in. A request descheduled
    /// in all three of them is not what being descheduled looks like.
    const ATTEMPTS: u32 = 3;

    let mut best = session();
    for attempt in 2..=ATTEMPTS {
        let over = best.iter().filter(|(_, took)| *took > budget).count();
        if over == 0 {
            break;
        }
        // In the log on the way past, because a run that needed a second
        // session is a run whose runner was busy, and that is worth seeing
        // beside a pass.
        eprintln!(
            "language server: {over} request(s) over the {}ms bar, so the session is measured again (attempt {attempt} of {ATTEMPTS})",
            budget.as_millis()
        );
        let again = session();
        assert_eq!(
            again.len(),
            best.len(),
            "the session measured a different number of requests the second time, so its attempts cannot be compared"
        );
        for (kept, (what, took)) in best.iter_mut().zip(again) {
            assert_eq!(
                kept.0, what,
                "the session made its requests in a different order the second time, so its attempts cannot be compared"
            );
            kept.1 = kept.1.min(took);
        }
    }
    best
}

/// Every request an editor makes around a keystroke, against the worked
/// monorepo, timed.
///
/// Off unless `BURI_PERF` is set, and meaningless without `--release`: a debug
/// build is an order slower, so an assertion about milliseconds in one would
/// fail on the runner rather than on the change.
///
/// ```text
/// BURI_PERF=1 cargo test --release -p buri --test build repositories::language_server_speed
/// ```
///
/// The session has three parts. First the restore: an editor coming back to a
/// project opens every tab it had, so every `.buri` file in the repository is
/// opened one after another and **each open is held to the budget** — what an
/// open costs is the target it opened, and the buffers already open are a hash
/// each. Then the cold `workspace/diagnostic`, which is **not** measured: a
/// cold sweep is one compilation per target and it is paid at startup rather
/// than under a person's hands. Then the interactive loop — a keystroke, then
/// the pulls and the position requests an editor sends after one — every
/// request of which is held to the budget, with all those buffers still open.
///
/// The whole session is what `best_of` above measures, so a request that comes
/// back over the bar is timed again in a fresh one rather than failed on a
/// single reading.
#[test]
fn language_server_speed() {
    if std::env::var("BURI_PERF").is_err() {
        return;
    }
    let budget = language_server_budget();
    let mut timings = best_of(budget, || {
        let scratch = Scratch::copy_of("lsp-speed", &example_repo());
        let mut editor = Editor::open(&scratch.root);

        let mut timings = Vec::new();
        for file in sources_of(&scratch.root) {
            timings.push(editor.opened(&file));
        }
        let watched = ["lib/money/cents.buri", "lib/store/codec.buri", "cmd/server/routes.buri"];
        editor.timed("workspace/diagnostic", r#"{"previousResultIds":[]}"#);

        for round in 1..=3u32 {
            editor.typed("lib/money/cents.buri", round);
            for file in watched {
                timings.push(editor.pull(file));
            }
            timings.push(editor.timed("workspace/diagnostic", r#"{"previousResultIds":[]}"#));
            for method in [
                "textDocument/documentHighlight",
                "textDocument/documentLink",
                "textDocument/documentColor",
                "textDocument/codeAction",
            ] {
                timings.push(editor.at(method, "lib/money/cents.buri"));
            }
        }
        editor.close();
        timings
    });

    timings.sort_by_key(|(_, took)| std::cmp::Reverse(*took));
    let over: Vec<_> = timings.iter().filter(|(_, took)| *took > budget).cloned().collect();
    let five = listed(&timings[..timings.len().min(5)]);
    // Printed on a pass too: a run that stayed under the bar is the record a
    // later recalibration reads.
    eprintln!(
        "language server: {} requests under a {}ms bar; the five slowest:\n{five}",
        timings.len(),
        budget.as_millis()
    );
    assert!(
        over.is_empty(),
        "these answers took longer than the {}ms an editor request has:\n{}\n\nthe five slowest of the run:\n{five}",
        budget.as_millis(),
        listed(&over),
    );
}

/// How much slower the second half of a restore may be than the first.
///
/// The budget above is a constant and this is the *shape*: an open that pays
/// for the buffers already open fails this on any machine, because both halves
/// are measured on the same one. Three times, plus a floor so that a run whose
/// opens are all a millisecond does not fail on jitter.
const RESTORE_DRIFT: u32 = 3;

/// The same restore, against a repository the size a person opens a hundred
/// tabs in.
///
/// `cli/tests/example` is 2.3k lines in eight targets: enough to hold the
/// budget honest and not enough to show what an open costs when the buffer
/// count is what an editor really restores. This one is generated from a
/// template so its size is stated rather than measured — twenty-four libraries
/// of four modules of eighty-six functions, 24,768 lines across 145 files — and
/// every one of those files is opened, under the same 50 ms an editor request
/// has. Before the findings of a target were kept per target the last open here
/// was 58 ms and the first was 4 ms; both halves of that are what fails now.
///
/// The restore is measured through `best_of` like the session above is, which
/// is what the two medians are taken over as well: the shape of a restore is a
/// claim about opens, not about which of them the runner happened to interrupt.
///
/// ```text
/// BURI_PERF=1 cargo test --release -p buri --test build repositories::language_server_open_cost
/// ```
#[test]
fn language_server_open_cost() {
    if std::env::var("BURI_PERF").is_err() {
        return;
    }
    let budget = language_server_budget();
    let timings = best_of(budget, || {
        let scratch = generated_repository("lsp-open-scale", 24, 4, 86);
        let mut editor = Editor::open(&scratch.root);
        let mut timings = Vec::new();
        for file in sources_of(&scratch.root) {
            timings.push(editor.opened(&file));
        }
        editor.close();
        timings
    });

    let over: Vec<_> = timings.iter().filter(|(_, took)| *took > budget).cloned().collect();
    let mut slowest = timings.clone();
    slowest.sort_by_key(|(_, took)| std::cmp::Reverse(*took));
    let five = listed(&slowest[..slowest.len().min(5)]);
    let half = timings.len() / 2;
    let first = median(&timings[..half]);
    let last = median(&timings[half..]);
    // Printed on a pass too: the two medians are the shape, and the five
    // slowest are what a later recalibration reads.
    eprintln!(
        "language server: {} opens under a {}ms bar, {}ms then {}ms per open; the five slowest:\n{five}",
        timings.len(),
        budget.as_millis(),
        first.as_millis(),
        last.as_millis()
    );
    assert!(
        over.is_empty(),
        "these opens took longer than the {}ms an open has:\n{}\n\nthe five slowest of the run:\n{five}",
        budget.as_millis(),
        listed(&over),
    );
    let allowed =
        first.saturating_mul(RESTORE_DRIFT).saturating_add(std::time::Duration::from_millis(10));
    assert!(
        last <= allowed,
        "the restore's second half costs {}ms an open against its first half's {}ms, \
         so an open is still paying for the buffers already open:\n{}",
        last.as_millis(),
        first.as_millis(),
        five
    );
}

/// The middle time of a run, which is what a ratio between two halves of a
/// restore has to be taken over: one slow open would carry a mean.
fn median(rows: &[(String, std::time::Duration)]) -> std::time::Duration {
    let mut times: Vec<std::time::Duration> = rows.iter().map(|(_, took)| *took).collect();
    times.sort();
    times.get(times.len() / 2).copied().unwrap_or_default()
}

/// One timing per line, for the message a failure prints.
fn listed(rows: &[(String, std::time::Duration)]) -> String {
    rows.iter()
        .map(|(what, took)| format!("  {what}: {}ms", took.as_millis()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `.buri` file in a repository, relative to its root and sorted — which
/// is every tab an editor could have had open in it.
fn sources_of(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // What a build wrote is not a tab anyone had open.
                if !matches!(path.file_name().and_then(|n| n.to_str()), Some(".buri" | "out")) {
                    stack.push(path);
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("buri") {
                if let Ok(rel) = path.strip_prefix(root) {
                    found.push(rel.display().to_string().replace('\\', "/"));
                }
            }
        }
    }
    found.sort();
    found
}

/// A repository of a stated size, written from a template.
///
/// `packages` libraries of `modules` modules each, every module `functions`
/// functions that compile and depend on nothing — the size is the point, and a
/// dependency between two of them would only put a second target's compilation
/// inside the first's open.
fn generated_repository(
    name: &str,
    packages: usize,
    modules: usize,
    functions: usize,
) -> Scratch {
    let scratch = Scratch::empty(name);
    scratch.write(
        "REPO.buri",
        "# Generated by repositories.rs for the language server's open budget.\n",
    );
    for package in 0..packages {
        let dir = format!("lib/p{package}");
        let sources: Vec<String> = (0..modules).map(|m| format!("\"m{m}.buri\"")).collect();
        scratch.write(
            &format!("{dir}/BUILD.buri"),
            &format!(
                "library {{\n    sources: [{}]\n    visibility: [\"//visibility:public\"]\n}}\n",
                sources.join(", ")
            ),
        );
        let mut exports = String::new();
        for module in 0..modules {
            let mut body = String::new();
            for f in 0..functions {
                body.push_str(&format!(
                    "/// Adds {f}, in whole units.\n\
                     export fn p{package}m{module}f{f}(n: I64): I64 {{ n + {f} }}\n\n"
                ));
            }
            scratch.write(&format!("{dir}/m{module}.buri"), &body);
            // Every name re-exported: a library's surface is `lib.buri`, and a
            // function nothing reaches is a `dead-code` finding rather than the
            // clean repository this is meant to time an open against.
            let names: Vec<String> =
                (0..functions).map(|f| format!("p{package}m{module}f{f}")).collect();
            exports.push_str(&format!(
                "from \"//lib/p{package}/m{module}.buri\" export {{ {} }};\n",
                names.join(", ")
            ));
        }
        scratch.write(&format!("{dir}/lib.buri"), &exports);
    }
    scratch
}

/// A client for the timed session: one `buri lsp` on a pipe, and the framing
/// around it.
///
/// Deliberately not the recorded-session harness. That one writes every
/// message and reads the answers afterwards, which is the right shape for a
/// golden and the wrong one for a clock: what is being measured here is the
/// time between one request going out and its own answer coming back.
struct Editor {
    process: std::process::Child,
    dir: PathBuf,
    root: String,
    next_id: u64,
}

impl Editor {
    fn open(root: &Path) -> Editor {
        let process = Command::new(buri())
            .arg("lsp")
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the language server did not start");
        let mut editor = Editor {
            process,
            dir: root.to_path_buf(),
            root: format!("file://{}", root.display()),
            next_id: 0,
        };
        let params = format!(r#"{{"rootUri":"{}"}}"#, editor.root);
        editor.timed("initialize", &params);
        editor.notify("initialized", "{}");
        editor
    }

    /// The uri of one file in the repository.
    fn uri(&self, rel: &str) -> String {
        format!("{}/{}", self.root, rel)
    }

    /// One buffer opened, and what the open cost.
    ///
    /// A notification has no answer to time, so the clock is stopped by a
    /// `linkedEditingRange` behind it — a request this server answers `null`
    /// without reading anything, so what the pair measures is the open.
    fn opened(&mut self, rel: &str) -> (String, std::time::Duration) {
        let text = std::fs::read_to_string(self.dir.join(rel)).unwrap();
        let params = format!(
            r#"{{"textDocument":{{"uri":"{}","languageId":"buri","version":1,"text":{}}}}}"#,
            self.uri(rel),
            quoted(&text)
        );
        let started = std::time::Instant::now();
        self.notify("textDocument/didOpen", &params);
        self.at("textDocument/linkedEditingRange", rel);
        (format!("didOpen {rel}"), started.elapsed())
    }

    /// A keystroke: a comment appended, so that nothing above it moves and the
    /// file still compiles.
    fn typed(&mut self, rel: &str, round: u32) {
        let mut text = std::fs::read_to_string(self.dir.join(rel)).unwrap();
        text.push_str(&format!("\n// A keystroke, number {round}.\n"));
        let params = format!(
            r#"{{"textDocument":{{"uri":"{}","version":{}}},"contentChanges":[{{"text":{}}}]}}"#,
            self.uri(rel),
            round.saturating_add(1),
            quoted(&text)
        );
        self.notify("textDocument/didChange", &params);
    }

    fn pull(&mut self, rel: &str) -> (String, std::time::Duration) {
        let params = format!(r#"{{"textDocument":{{"uri":"{}"}}}}"#, self.uri(rel));
        self.timed("textDocument/diagnostic", &params)
    }

    /// One of the requests an editor sends for the cursor's position.
    fn at(&mut self, method: &str, rel: &str) -> (String, std::time::Duration) {
        let params = format!(
            r#"{{"textDocument":{{"uri":"{}"}},"position":{{"line":0,"character":0}},"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":0}}}},"context":{{"diagnostics":[]}}}}"#,
            self.uri(rel)
        );
        self.timed(method, &params)
    }

    /// Sends one request and waits for its own answer, which is what the clock
    /// is on. Notifications the server sends meanwhile are read and dropped.
    fn timed(&mut self, method: &str, params: &str) -> (String, std::time::Duration) {
        let started = std::time::Instant::now();
        self.ask(method, params);
        (method.to_string(), started.elapsed())
    }

    /// Sends one request and returns its own answer, error replies included.
    /// Notifications the server sends meanwhile are read and dropped.
    fn ask(&mut self, method: &str, params: &str) -> String {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.write(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#
        ));
        // The keys are sorted, so `"jsonrpc"` follows the id in every answer —
        // behind `"error"` in a refusal, which is why this is not a prefix.
        // The ids of the requests the *server* sends are strings, so a number
        // here can only be this client's.
        let wanted = format!(r#""id":{id},"jsonrpc""#);
        loop {
            let message = self.read();
            if message.contains(&wanted) {
                return message;
            }
        }
    }

    fn notify(&mut self, method: &str, params: &str) {
        self.write(&format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params}}}"#));
    }

    fn close(&mut self) {
        self.timed("shutdown", "null");
        self.notify("exit", "null");
        let _ = self.process.wait();
    }

    fn write(&mut self, body: &str) {
        use std::io::Write;
        let stdin = self.process.stdin.as_mut().expect("the server's stdin is a pipe");
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
    }

    /// One framed message, read a byte at a time through the headers so the
    /// body that follows them is not swallowed.
    fn read(&mut self) -> String {
        use std::io::Read;
        let stdout = self.process.stdout.as_mut().expect("the server's stdout is a pipe");
        let mut headers = String::new();
        while !headers.ends_with("\r\n\r\n") {
            let mut byte = [0u8; 1];
            assert_eq!(stdout.read(&mut byte).unwrap(), 1, "the server closed the stream");
            headers.push(byte[0] as char);
        }
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("a message with no Content-Length")
            .trim()
            .parse()
            .unwrap();
        let mut body = vec![0u8; length];
        stdout.read_exact(&mut body).unwrap();
        String::from_utf8(body).unwrap()
    }
}

/// One JSON string literal, which is all the escaping a source file needs.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
