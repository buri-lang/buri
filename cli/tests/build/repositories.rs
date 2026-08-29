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
    run_corpus(&tests_dir().join("repositories/libraries"), "libraries", 8);
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
/// `version`, the `out/` symlink, and the no-argument forms that mean the whole
/// repository from wherever they are run.
#[test]
fn cli_contract() {
    run_corpus(&tests_dir().join("repositories/cli"), "cli", 11);
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
/// are not schemas at all.
#[test]
fn proto_schemas() {
    run_corpus(&tests_dir().join("repositories/proto"), "proto", 5);
}

/// CLI.md's lint catalogue: the hygiene rules, which ask about a package's own
/// code rather than about the graph. Each case ends with the edit that makes
/// the finding go away, because a rule nothing can turn off is a rule nobody
/// can check the fix for.
///
/// The `repo_lint_*` cases are the other half: what `REPO.buri`'s `lint` block
/// does to the same finding — when the catalogue runs, how hard a finding
/// lands, and what a misspelled field in the block costs.
#[test]
fn lint_catalogue() {
    run_corpus(&tests_dir().join("repositories/linting"), "linting", 23);
}

/// TESTING.md: where tests live, what a test source may reach, and what the
/// runner does with a suite — the flags, the timeout, the golden-file update
/// mode, and the exact shape of a failure report.
#[test]
fn test_suites() {
    run_corpus(&tests_dir().join("repositories/testing"), "testing", 10);
}

/// The language server. Each case is a recorded session: requests in, decoded
/// responses out, so a change to what the server says shows up as a diff
/// rather than as an editor behaving differently.
#[test]
fn language_server() {
    run_corpus(&tests_dir().join("repositories/lsp"), "lsp", 87);
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
#[test]
fn language_server_speed() {
    if std::env::var("BURI_PERF").is_err() {
        return;
    }
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

    timings.sort_by_key(|(_, took)| std::cmp::Reverse(*took));
    let over: Vec<_> =
        timings.iter().filter(|(_, took)| *took > LANGUAGE_SERVER_BUDGET).cloned().collect();
    assert!(
        over.is_empty(),
        "these answers took longer than the {}ms an editor request has:\n{}\n\nthe five slowest of the run:\n{}",
        LANGUAGE_SERVER_BUDGET.as_millis(),
        listed(&over),
        listed(&timings[..timings.len().min(5)])
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
/// ```text
/// BURI_PERF=1 cargo test --release -p buri --test build repositories::language_server_open_cost
/// ```
#[test]
fn language_server_open_cost() {
    if std::env::var("BURI_PERF").is_err() {
        return;
    }
    let scratch = generated_repository("lsp-open-scale", 24, 4, 86);
    let mut editor = Editor::open(&scratch.root);
    let mut timings = Vec::new();
    for file in sources_of(&scratch.root) {
        timings.push(editor.opened(&file));
    }
    editor.close();

    let over: Vec<_> = timings
        .iter()
        .filter(|(_, took)| *took > LANGUAGE_SERVER_BUDGET)
        .cloned()
        .collect();
    let mut slowest = timings.clone();
    slowest.sort_by_key(|(_, took)| std::cmp::Reverse(*took));
    let five = listed(&slowest[..slowest.len().min(5)]);
    assert!(
        over.is_empty(),
        "these opens took longer than the {}ms an open has:\n{}\n\nthe five slowest of the run:\n{}",
        LANGUAGE_SERVER_BUDGET.as_millis(),
        listed(&over),
        five
    );
    let half = timings.len() / 2;
    let first = median(&timings[..half]);
    let last = median(&timings[half..]);
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
                "from \"//lib/p{package}/m{module}\" export {{ {} }};\n",
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
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        let started = std::time::Instant::now();
        self.write(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#
        ));
        // The keys are sorted, so an answer to this request begins with its id
        // and carries no `"method":` — which a *request* the server sends
        // would. The colon matters: `"method"` on its own is one of the
        // semantic-token types the `initialize` answer lists.
        let wanted = format!(r#"{{"id":{id},"#);
        loop {
            let message = self.read();
            if message.starts_with(&wanted) && !message.contains(r#""method":"#) {
                return (method.to_string(), started.elapsed());
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
