//! `generators`: a tool the build runs, whose output becomes a module.
//!
//! A `generators` entry names a `tool` rule, or a toolchain tool such as
//! `proto`, and the build asks its `generate` entry point about the entry's
//! inputs ([`crate::build::tools`]). Every module it answers with is loaded
//! the way a source is: through the real parser, into the rule that declared
//! the entry, with no file on disk.
//!
//! ```text
//! <- {"modules":[{"name":"point.proto","text":"export struct Point {}\n","anchors":[]}],
//!     "diagnostics":[],"needs":[]}
//! ```
//!
//! **Text plus anchors, never a tree.** An anchor says which region of the
//! generated text came from which span of which input, which is the whole of
//! what go-to-definition and a diagnostic inside generated code need. The
//! compiler then parses the text with its one ordinary parser.
//!
//! The answer is parsed once, by [`crate::json`], in [`tools::exchange`].

use crate::build::buildfile::{self, Generator, Output};
use crate::build::cache::{Action, ActionKey, KeyBuilder};
use crate::build::session::Session;
use crate::build::sources::Overlay;
use crate::build::tools::{self, Tool};
use crate::build::workspace::{RuleKind, TargetId, Workspace};
use crate::commands::arguments::Flags;
use crate::diagnostics::Span;
use crate::json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

/// The `code` of a [`Diagnostic`] whose `message` is already the whole
/// sentence, so the loader prints it rather than a page's wording.
///
/// Not a catalogue code: a file the operating system would not hand over has
/// no rule behind it to explain, and the sentence is the error the read
/// returned. `sources` reports the same file the same way
/// (`compiler::modules`), which is what makes the two agree.
pub const UNREADABLE: &str = "an-input-that-could-not-be-read";

// ---------------------------------------------------------------------------
// The protocol
// ---------------------------------------------------------------------------

/// A position in one of the generator's inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// Repository-relative, as the request spelled it.
    pub file: String,
    /// Byte offsets into that file.
    pub span: (usize, usize),
}

/// Which region of generated text came from which span of which input.
///
/// `start` and `end` are byte offsets into [`GeneratedModule::text`]. Sorted by
/// `start`, outermost first where two share one — so the *last* anchor
/// containing an offset is the innermost node covering it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub start: usize,
    pub end: usize,
    pub file: String,
    pub span: (usize, usize),
}

impl Anchor {
    pub fn origin(&self) -> Origin {
        Origin { file: self.file.clone(), span: self.span }
    }
}

/// One module a generator produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedModule {
    /// The module's name inside the declaring package. A generator naming
    /// `point.proto` in `//lib/wire` produces `//lib/wire/point.proto`.
    pub name: String,
    /// Buri source, as `buri format` would leave it.
    pub text: String,
    pub anchors: Vec<Anchor>,
}

impl GeneratedModule {
    /// The innermost anchor covering a byte offset in [`Self::text`], if there
    /// is one. This is what turns a position in generated text into a position
    /// in the input the generator read.
    pub fn anchor_at(&self, offset: usize) -> Option<&Anchor> {
        self.anchors.iter().rfind(|a| a.start <= offset && offset < a.end)
    }
}

/// Something the generator has to say about its input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub note: Option<String>,
    pub fix: Option<String>,
    /// Where in the input this is about. `None` anchors it on the `generators`
    /// entry that ran the tool.
    pub origin: Option<Origin>,
}

/// What a generator writes back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Response {
    pub modules: Vec<GeneratedModule>,
    pub diagnostics: Vec<Diagnostic>,
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// `null` and an absent field are the same claim, which is what lets `note`,
/// `fix` and `origin` be written either way.
fn present<'v>(value: &'v Value, name: &str) -> Option<&'v Value> {
    value.get(name).filter(|v| !matches!(v, Value::Null))
}

/// A byte offset: a non-negative integer.
fn offset(value: &Value, name: &str) -> Option<usize> {
    match value.get(name)? {
        Value::Int(n) => usize::try_from(*n).ok(),
        _ => None,
    }
}

fn text<'v>(value: &'v Value, name: &str) -> Option<&'v str> {
    value.get(name).and_then(Value::as_str)
}

fn read_span(value: &Value) -> Option<(usize, usize)> {
    Some((offset(value, "start")?, offset(value, "end")?))
}

/// The items of a list field, or none when the field is absent or `null`.
fn items<'v>(value: &'v Value, name: &str) -> Result<&'v [Value], String> {
    match present(value, name) {
        None => Ok(&[]),
        Some(list) => list.as_array().ok_or_else(|| format!("`{name}` is not a list")),
    }
}

/// A tool's `diagnostics`: the shape `core/tool` shares with `core/codegen`,
/// so a check's answer and a `generate`'s are read by this one function.
pub fn diagnostics(value: &Value) -> Result<Vec<Diagnostic>, String> {
    let mut diagnostics = Vec::new();
    for d in items(value, "diagnostics")? {
        let code = text(d, "code").ok_or("a diagnostic has no `code`")?;
        let message = text(d, "message").ok_or("a diagnostic has no `message`")?;
        let text_of = |name: &str| present(d, name).and_then(Value::as_str).map(str::to_string);
        let origin = match present(d, "origin") {
            None => None,
            Some(o) => {
                let file = text(o, "file").ok_or("an origin has no `file`")?;
                let span = o.get("span").and_then(read_span).ok_or("an origin has no `span`")?;
                Some(Origin { file: file.to_string(), span })
            }
        };
        diagnostics.push(Diagnostic {
            code: code.to_string(),
            message: message.to_string(),
            note: text_of("note"),
            fix: text_of("fix"),
            origin,
        });
    }
    Ok(diagnostics)
}

impl Response {
    /// The line a generator writes to its standard output, parsed and read.
    pub fn decode(line: &str) -> Result<Response, String> {
        Response::from_value(&crate::json::parse(line)?)
    }

    /// An answer [`tools::exchange`] has already parsed.
    pub fn from_value(json: &Value) -> Result<Response, String> {
        let mut modules = Vec::new();
        for item in items(json, "modules")? {
            let name = text(item, "name").ok_or("a module has no `name`")?;
            let module_text = text(item, "text").ok_or("a module has no `text`")?;
            let mut anchors = Vec::new();
            for a in items(item, "anchors")? {
                let start = offset(a, "start").ok_or("an anchor has no `start`")?;
                let end = offset(a, "end").ok_or("an anchor has no `end`")?;
                let file = text(a, "file").ok_or("an anchor has no `file`")?;
                let span = a.get("span").and_then(read_span).ok_or("an anchor has no `span`")?;
                anchors.push(Anchor { start, end, file: file.to_string(), span });
            }
            modules.push(GeneratedModule {
                name: name.to_string(),
                text: module_text.to_string(),
                anchors,
            });
        }
        Ok(Response { modules, diagnostics: diagnostics(json)? })
    }
}

// ---------------------------------------------------------------------------
// What a build knows about a generated module
// ---------------------------------------------------------------------------

/// What running one rule's generators produced.
///
/// A failure is a diagnostic and no modules rather than an absence, so a rule
/// whose generator did not run reports the reason once, where the entry is
/// written, instead of failing later as an import that resolves to nothing.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub modules: Vec<Arc<GeneratedModule>>,
    /// Diagnostics, each with the `generators` entry it belongs to.
    pub diagnostics: Vec<(Diagnostic, Span)>,
    /// What checking the inputs found, each with the `generators` entry that
    /// listed the file. An entry with an input that fails is not run.
    pub findings: Vec<(crate::languages::Finding, Span)>,
    /// Repository paths the checks and the tools read besides the inputs: the
    /// schemas.
    pub reads: Vec<String>,
    /// The paths among those that a `generate` asked for, so the next
    /// session's fingerprint can tell whether one moved.
    pub generated_reads: Vec<String>,
}

/// Every module the generators in this repository produced.
///
/// **This is the seam the compiler front end reads generated code through.**
/// Generation needs to build and spawn a tool, which the front end cannot do,
/// so it happens in the build layer ([`prepare`]) and arrives here as data. The
/// store hangs off the [`Workspace`] because the workspace is the one thing
/// already threaded to `compiler::modules::Loader`, and it is shared by every
/// clone of a `Session`, so `buri build`, `buri test`, `buri lint` and the
/// language server all read one answer.
///
/// Interior mutability, because the workspace is behind an `Arc` by the time
/// there is a session to build a tool with. Nothing else about the graph is
/// writable and nothing here rewrites the graph.
///
/// A `Mutex` rather than a `RefCell` because a `Workspace` is `Send + Sync` —
/// the native suites keep one in a `OnceLock` — and one field that is not would
/// take that away from the whole graph.
#[derive(Default)]
pub struct Store {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Per target: the key the outcome was produced under, and the outcome.
    /// The key is what makes a second session a lookup rather than a re-run.
    by_target: BTreeMap<TargetId, (String, Outcome)>,
    /// Canonical module path -> the module, for [`Workspace::resolve_module`]
    /// and for the loader.
    by_path: BTreeMap<String, (TargetId, Arc<GeneratedModule>)>,
}

impl Store {
    fn read(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while the store was being written leaves what was written,
        // and what was written is a build's answer rather than an invariant.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The outcome recorded for a target, if one has been.
    pub fn outcome(&self, target: TargetId) -> Option<Outcome> {
        self.read().by_target.get(&target).map(|(_, o)| o.clone())
    }

    /// The key an outcome was recorded under, for deciding whether to run.
    pub fn key_of(&self, target: TargetId) -> Option<String> {
        self.read().by_target.get(&target).map(|(k, _)| k.clone())
    }

    /// A generated module by its canonical path, `//lib/wire/point.proto`.
    ///
    /// The accessor the language server reads: the text a diagnostic or a hover
    /// is about, and [`GeneratedModule::anchor_at`] to turn an offset in it
    /// back into a span in the input the generator read.
    pub fn module(&self, path: &str) -> Option<Arc<GeneratedModule>> {
        self.read().by_path.get(path).map(|(_, m)| Arc::clone(m))
    }

    /// The rule that produced the module at this path.
    pub fn owner(&self, path: &str) -> Option<TargetId> {
        self.read().by_path.get(path).map(|(t, _)| *t)
    }

    /// Whether any generator has produced a module at this path.
    pub fn holds(&self, path: &str) -> bool {
        self.read().by_path.contains_key(path)
    }

    fn record(
        &self,
        workspace: &Workspace,
        target: TargetId,
        key: String,
        outcome: Outcome,
    ) {
        let package = workspace.package(target.package);
        let mut inner = self.read();
        // A rule's previous answer goes with it: a module the generator has
        // stopped producing must stop resolving.
        inner.by_path.retain(|_, (owner, _)| *owner != target);
        for module in &outcome.modules {
            inner
                .by_path
                .insert(package.module_path(&module.name), (target, Arc::clone(module)));
        }
        inner.by_target.insert(target, (key, outcome));
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("modules", &self.read().by_path.len()).finish()
    }
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// The generators one rule declares.
pub fn declared(workspace: &Workspace, target: TargetId) -> &[Generator] {
    let p = workspace.package(target.package);
    match target.kind {
        RuleKind::Library => p.build.library.as_ref().map(|l| &l.generators[..]).unwrap_or(&[]),
        RuleKind::Binary => p.build.binary.as_ref().map(|b| &b.generators[..]).unwrap_or(&[]),
        RuleKind::Tool => &[],
    }
}

/// Every input every generator on this rule declares, package-relative and
/// sorted.
///
/// One enumeration, because four things read it: the action key, the watch
/// set, `buri query 'sources(...)'`, and the lint that says every file on disk
/// belongs to a rule.
pub fn inputs(workspace: &Workspace, target: TargetId) -> Vec<String> {
    let mut out: Vec<String> = declared(workspace, target)
        .iter()
        .flat_map(|g| g.inputs.iter().map(|i| i.value.clone()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every module this rule's generators produced, in the order they were named.
///
/// Empty until [`prepare`] has run, and empty for a rule that declares no
/// generator — so a caller folding these into a key gets nothing for a rule
/// that has none.
pub fn modules_of(workspace: &Workspace, target: TargetId) -> Vec<Arc<GeneratedModule>> {
    workspace.generated.outcome(target).map(|o| o.modules).unwrap_or_default()
}

/// The key `--explain` reports for one rule's generators.
///
/// One line per rule rather than one per entry: what a reader is asking is
/// whether *this rule's* generated code moved. The tool is in it as its
/// program key, so editing the tool moves this line too.
pub fn rule_key(
    session: &Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
) -> ActionKey {
    let mut k = KeyBuilder::new(Action::Generate, flags.mode);
    k.platform(output.platform(), output.arch());
    let package = session.workspace.package(target.package);
    let paths = inputs(&session.workspace, target);
    k.rule_identity(&package.label(), "generate", &paths);
    for g in declared(&session.workspace, target) {
        k.input("tool", g.tool.value.as_bytes());
        if let Ok(tool) = tools::resolve(&session.workspace, &g.tool.value) {
            k.dependency(&tools::program_key(session, tool, flags));
        }
    }
    for rel in &paths {
        let full = package.dir.join(rel);
        k.file(&session.workspace.rel_of(&full), std::fs::read(&full).ok().as_deref());
    }
    k.finish()
}

/// Whether a target is a tool with an `accepts` entry, whose types the build
/// generates into it.
pub fn has_contracts(workspace: &Workspace, target: TargetId) -> bool {
    target.kind == RuleKind::Tool
        && workspace.package(target.package).build.tool.as_ref().is_some_and(|t| t.contracts().next().is_some())
}

/// The contract a file is checked under: the first `generators` entry listing
/// it whose tool's `generate` has one for its language.
pub fn contract_for(workspace: &Workspace, rel: &str) -> Option<tools::Contract> {
    let language = workspace.repo.languages.of(rel)?.name.clone();
    for target in workspace.targets() {
        let dir = &workspace.package(target.package).dir;
        for g in declared(workspace, target) {
            if !g.inputs.iter().any(|i| workspace.rel_of(&dir.join(&i.value)) == rel) {
                continue;
            }
            let Ok(tool) = tools::resolve(workspace, &g.tool.value) else { continue };
            if let Some(c) = tools::Contract::of(workspace, tool, "generate", &language) {
                return Some(c);
            }
        }
    }
    None
}

/// The `tool` rule a `//label` names.
pub fn tool_target(workspace: &Workspace, tool: &str) -> Option<TargetId> {
    match tools::resolve(workspace, tool) {
        Ok(Tool::Repo(t)) => Some(t),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Running one
// ---------------------------------------------------------------------------

/// Runs a built tool under the JavaScript runtime, one line in and one line
/// out, and answers with the last non-empty line it wrote.
///
/// The command comes from [`crate::build::spawn::command`] rather than
/// `Command::new`, so a tool's process gets the same explicit environment
/// every other action's does: cleared, then `TZ` and `SOURCE_DATE_EPOCH`.
///
/// What keeps a tool off the clock is its entry points' bound: `ctx` has
/// `Allocator` and nothing else (`tool-effect-unavailable`).
///
/// While a [`Keep`] is held, the process is kept for the tool's next request
/// ([`ask_kept`]). Anything short of an answer from a kept process asks again
/// here, of a process of the request's own, so a tool that fails says what it
/// would have said alone.
pub fn run_artifact(artifact: &std::path::Path, request: &str) -> Result<String, String> {
    if let Some(line) = ask_kept(artifact, request) {
        return Ok(line);
    }
    run_once(artifact, request)
}

/// Tool processes between requests, while anyone holds a [`Keep`].
struct Kept {
    holders: usize,
    idle: Vec<KeptProcess>,
}

struct KeptProcess {
    artifact: PathBuf,
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl KeptProcess {
    /// Ends the tool the way a one-off run does: its input closes, and it
    /// exits once it reads the end.
    fn finish(self) {
        let KeptProcess { mut child, stdin, .. } = self;
        drop(stdin);
        let _ = child.wait();
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static KEPT: Mutex<Kept> = Mutex::new(Kept { holders: 0, idle: Vec::new() });

/// A pass over generators: while one is held, each tool process answers the
/// requests after its first too. Starting the JavaScript runtime and loading a
/// tool costs more than most requests do, and a pass asks one tool many
/// questions — a check per input, a second round for the files a check needs,
/// then `generate`. The last holder to let go ends every kept process.
struct Keep(());

fn keep() -> Keep {
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    kept.holders = kept.holders.saturating_add(1);
    Keep(())
}

impl Drop for Keep {
    fn drop(&mut self) {
        let idle = {
            let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
            kept.holders = kept.holders.saturating_sub(1);
            match kept.holders {
                0 => std::mem::take(&mut kept.idle),
                _ => Vec::new(),
            }
        };
        for process in idle {
            process.finish();
        }
    }
}

/// The answer from a kept process of `artifact`'s, starting one if none is
/// idle. `None` when nothing is kept, or when the process did not answer: it
/// is then ended, and [`run_artifact`] asks a process of the request's own.
///
/// `serve` answers each request with one line, so the next line is the
/// answer. Its standard error is not read: a tool that fails is asked again
/// alone, and that run's is the one reported.
fn ask_kept(artifact: &std::path::Path, request: &str) -> Option<String> {
    use std::io::{BufRead as _, Write as _};
    use std::process::Stdio;

    let found = {
        let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
        if kept.holders == 0 {
            return None;
        }
        let at = kept.idle.iter().position(|p| p.artifact == artifact);
        at.map(|i| kept.idle.swap_remove(i))
    };
    let mut process = match found {
        Some(process) => process,
        None => {
            let program = crate::commands::test::js_runtime();
            let mut cmd = crate::build::spawn::command(&program)?;
            let mut child = crate::build::spawn::start(
                cmd.arg(artifact).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()),
            )
            .ok()?;
            let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            };
            KeptProcess { artifact: artifact.to_path_buf(), child, stdin, stdout: std::io::BufReader::new(stdout) }
        }
    };
    let asked = writeln!(process.stdin, "{request}").and_then(|()| process.stdin.flush());
    let mut line = String::new();
    let answered = asked.is_ok() && matches!(process.stdout.read_line(&mut line), Ok(n) if n > 0 && line.ends_with('\n'));
    let line = line.trim_end_matches('\n').trim_end_matches('\r');
    if !answered || line.trim().is_empty() {
        process.kill();
        return None;
    }
    let line = line.to_string();
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    match kept.holders {
        // The pass ended while this request was out.
        0 => {
            drop(kept);
            process.finish();
        }
        _ => kept.idle.push(process),
    }
    Some(line)
}

/// One request, to a process of its own: the request, then the end of the
/// input.
fn run_once(artifact: &std::path::Path, request: &str) -> Result<String, String> {
    use std::io::{Read as _, Write as _};
    use std::process::Stdio;

    let program = crate::commands::test::js_runtime();
    let Some(mut cmd) = crate::build::spawn::command(&program) else {
        return Err(format!(
            "`{program}` is not on PATH; install bun, or point BURI_JS at a JavaScript runtime"
        ));
    };
    let mut child = crate::build::spawn::start(
        cmd.arg(artifact).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()),
    )
    .map_err(|e| format!("{program}: {e}"))?;
    // Each pipe on a thread of its own, and none of them read after the wait.
    // A pipe holds a page or two: a generator writing more than that — a schema
    // of any size produces far more — blocks on the write, and this process
    // waiting for an exit that the block prevents is two processes waiting on
    // each other, with nothing to end it. Draining while the tool runs is what
    // makes the size of the answer not matter, and it is what makes the wait
    // below safe to be a plain one.
    let mut stdin = child.stdin.take().ok_or("the tool has no standard input")?;
    let line = format!("{}\n", request);
    let feeding = std::thread::spawn(move || {
        // A write that fails because the tool exited before reading is not
        // itself the failure worth reporting: the exit status below says more.
        let _ = stdin.write_all(line.as_bytes());
        let _ = stdin.flush();
    });
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let reading_out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let reading_err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    // **Waited for, not timed.** This used to poll under a sixty-second
    // deadline and kill the tool at it, and that number could only ever measure
    // the machine: a two-thousand-field schema is five seconds on an idle
    // laptop and past sixty on a four-core runner with sixteen tests on it,
    // which is how CI came to fail a build every other host completed. Nothing
    // else the build spawns — `cc`, a linker, the JavaScript runtime — carries
    // a clock either. The one bound in this toolchain that stops a subprocess
    // is `timeout_seconds` on a `test` rule, which a person wrote in a build
    // file about their own tests. A generator that never answers is a program
    // its author can run and interrupt, and the suite that drives this has a
    // cap of its own that can tell a stuck process from a busy one
    // (`cli/tests/harness/hang.rs`).
    crate::profile::reaped(child.id());
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = feeding.join();
    let stdout = reading_out.join().unwrap_or_default();
    let stderr = reading_err.join().unwrap_or_default();
    if !status.success() {
        return Err(said(&format!("the tool {}", how_it_ended(&status)), &stderr));
    }
    // One line out. Anything before it is the tool talking to a person, which
    // is not this protocol — the response is the last non-empty line.
    let Some(line) = stdout.lines().rev().find(|l| !l.trim().is_empty()) else {
        return Err(said("the tool wrote nothing", &stderr));
    };
    Ok(line.to_string())
}

/// How much of what a tool put on standard error a note carries.
///
/// A generator that fills its pipe must not fill the page a person is reading,
/// and a JavaScript runtime's stack trace is long: four kilobytes is a screen
/// or two, which is enough to say what went wrong and short enough to read.
const STDERR_TAIL: usize = 4096;

/// How a generator's process ended, in the words the note carries.
///
/// **A tool with no exit status was killed, and which signal killed it is the
/// diagnosis.** A crash and a stack overflow arrive as `SIGSEGV`; a runner that
/// ran out of memory sends `SIGKILL`. "The generator produced no response" is
/// the same sentence for all three, and a CI job that says only that leaves
/// nothing to go on.
///
/// `commands::test` asks the same question of a test binary that died without
/// writing a record, so this is the one signal table in the toolchain rather
/// than one per caller.
pub(crate) fn how_it_ended(status: &std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exited with {code}");
    }
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(status) {
        return match signal_name(signal) {
            Some(name) => format!("was killed by {name} (signal {signal})"),
            None => format!("was killed by signal {signal}"),
        };
    }
    "ended without a status".to_string()
}

/// The name of a signal, for the numbers macOS and Linux agree on.
///
/// The two disagree about several — `SIGBUS` is 10 on one and 7 on the other —
/// so the ones they disagree about are reported by number. A wrong name is
/// worse than no name.
fn signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _other => return None,
    })
}

/// A sentence, with what the tool put on standard error under it.
///
/// The **tail** of it, at most [`STDERR_TAIL`] bytes: what a program says last
/// is what says why it stopped, and a runtime's stack trace buries its first
/// line under a hundred frames.
///
/// **The runtime's stack trace goes, and the note is what the tool said.** A
/// tool that stops prints its message on a line of its own and then the
/// error's stack (`$failed` in the JavaScript runtime). The stack's frames name
/// lines of JavaScript nobody wrote, under a file named for a cache key, or
/// the runtime's own internals. Nor is the stack the same text from one run
/// to the next: under load, bun has rendered the frames of one crash as
/// `Error: division by zero` with `(native:7:39)` and as a bare `Error` with
/// `(unknown:7:39)`. So every frame goes, and so does the line that heads
/// them when it only restates the message above it.
fn said(sentence: &str, stderr: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut in_frames = false;
    for line in stderr.lines() {
        if is_frame(line) {
            if !in_frames {
                let restated = match kept.as_slice() {
                    [.., said, header] => restates(header, Some(said)),
                    [header] => restates(header, None),
                    [] => false,
                };
                if restated {
                    kept.pop();
                }
            }
            in_frames = true;
            continue;
        }
        in_frames = false;
        kept.push(line);
    }
    let kept = kept.join("\n");
    let text = kept.trim();
    if text.is_empty() {
        return sentence.to_string();
    }
    if text.len() <= STDERR_TAIL {
        return format!("{sentence}\n{text}");
    }
    let skip = text.len().saturating_sub(STDERR_TAIL);
    // Forward to the next character boundary, so a note is never cut through
    // the middle of a character. `len()` is one, so the search always ends.
    let cut = (skip..=text.len()).find(|&i| text.is_char_boundary(i)).unwrap_or(text.len());
    let tail = text.get(cut..).unwrap_or_default();
    format!("{sentence}\n(the first {cut} bytes of standard error are not shown)\n{tail}")
}

/// A JavaScript stack frame: indented, `at`, and a `file:line:column` at the
/// end, bare or in parentheses.
fn is_frame(line: &str) -> bool {
    if !line.starts_with(char::is_whitespace) || !line.trim_start().starts_with("at ") {
        return false;
    }
    let place = line.trim_end().trim_end_matches(')');
    let mut parts = place.rsplitn(3, ':');
    let digits = |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    digits(parts.next()) && digits(parts.next()) && parts.next().is_some()
}

/// Whether the line that heads a stack says nothing the line above it did not:
/// an error's bare name, `Error`, or its name and the message above it,
/// `Error: division by zero`.
fn restates(header: &str, above: Option<&str>) -> bool {
    let (name, message) = match header.split_once(": ") {
        Some((name, message)) => (name, Some(message)),
        None => (header, None),
    };
    let named = name.ends_with("Error") && name.bytes().all(|b| b.is_ascii_alphanumeric());
    named && (message.is_none() || message == above)
}

// ---------------------------------------------------------------------------
// Running every generator in a repository
// ---------------------------------------------------------------------------

/// Runs every generator this repository declares, and records what each one
/// produced on the workspace's [`Store`].
///
/// Called from [`crate::build::sources::Sources::session`], the door `buri lint`,
/// a watch loop and the language server open a repository through, so they all
/// read one answer, produced once. `buri build`, `buri run` and `buri test` run
/// only what their targets read instead ([`prepare_for`]).
///
/// A rule whose recorded answer is already under the keys its inputs and its
/// tool produce now is left alone, so a second session costs the keys rather
/// than a second run of the tool.
pub fn prepare(session: &mut Session, flags: &Flags, overlay: &Overlay) {
    let targets: Vec<TargetId> =
        session.workspace.targets().into_iter().filter(|t| generates(&session.workspace, *t)).collect();
    prepare_rules(session, flags, overlay, targets);
}

/// Runs the generators that building or testing `targets` reads the output
/// of, and no others.
///
/// That is every rule in their closures and their test dependencies' — a
/// test, and a lint, reads a target's test sources too — and, because a tool
/// is built from code as well, every rule in the closure of a tool any of
/// those rules runs, again until nothing new is reached. `//...` reaches every
/// rule, so it runs what [`prepare`] runs.
///
/// `buri build`, `buri run` and `buri test` open the repository through here
/// rather than through [`prepare`]: the generators of the rest of a
/// repository are tool processes whose output nothing in the command reads.
pub fn prepare_for(session: &mut Session, flags: &Flags, targets: &[TargetId]) {
    let workspace = Arc::clone(&session.workspace);
    let mut reached: Vec<TargetId> = Vec::new();
    for &target in targets {
        reached.extend(workspace.closure(target));
        for (dep, _) in workspace.test_dep_edges(target) {
            reached.extend(workspace.closure(dep));
        }
    }
    let mut seen: BTreeSet<TargetId> = BTreeSet::new();
    let every_rule: Vec<TargetId> =
        workspace.targets().into_iter().filter(|t| generates(&workspace, *t)).collect();
    loop {
        while let Some(target) = reached.pop() {
            if !seen.insert(target) {
                continue;
            }
            for tool in tools_of(&workspace, target) {
                reached.extend(workspace.closure(tool));
            }
        }
        // A library a source imports without naming it in `dependencies` is
        // still loaded, and that mistake is `missing-dependency`'s to report.
        // Its generators run too, or the library would fail to check and the
        // reader would be sent into a library with nothing wrong with it. The
        // imports are only read while a rule with generators is still left out.
        if every_rule.iter().all(|t| seen.contains(t)) {
            break;
        }
        reached = imported_but_undeclared(&workspace, &seen);
        if reached.is_empty() {
            break;
        }
    }
    let rules = every_rule.into_iter().filter(|t| seen.contains(t)).collect();
    prepare_rules(session, flags, &Overlay::new(), rules);
}

/// The libraries the sources of `seen`'s packages import that `seen` does not
/// hold, each with its closure.
fn imported_but_undeclared(workspace: &Workspace, seen: &BTreeSet<TargetId>) -> Vec<TargetId> {
    let packages: BTreeSet<_> = seen.iter().map(|t| t.package).collect();
    let mut out: Vec<TargetId> = Vec::new();
    for package in packages {
        let dir = &workspace.package(package).dir;
        for file in workspace.declared_sources(package) {
            for path in crate::build::regenerate::imports_of(dir, &file) {
                let library = workspace
                    .dependency_label(package, &path)
                    .and_then(|label| workspace.dep_target(&label));
                if let Some(library) = library.filter(|l| !seen.contains(l)) {
                    out.extend(workspace.closure(library));
                }
            }
        }
    }
    out
}

/// Whether every rule with generators has an answer recorded: false after a
/// [`prepare_for`] that left some out.
pub fn all_prepared(workspace: &Workspace) -> bool {
    workspace.targets().into_iter().filter(|t| generates(workspace, *t)).all(|t| workspace.generated.key_of(t).is_some())
}

/// Whether a rule has anything for [`prepare`] to run: generators, or a tool's
/// contracts.
fn generates(workspace: &Workspace, target: TargetId) -> bool {
    !declared(workspace, target).is_empty() || has_contracts(workspace, target)
}

/// [`prepare`], over these rules.
fn prepare_rules(session: &mut Session, flags: &Flags, overlay: &Overlay, targets: Vec<TargetId>) {
    let session: &Session = session;
    let _keep = keep();
    // In rounds: every rule whose tools' code is generated already runs beside
    // the others, each recording its own answer, once the tools the round runs
    // are built. At most one rule of a package runs in a round, so two rules of
    // one package still record in order.
    let mut done: BTreeSet<TargetId> = BTreeSet::new();
    let mut waiting = targets.clone();
    while !waiting.is_empty() {
        let mut ready: Vec<TargetId> = Vec::new();
        let mut rest: Vec<TargetId> = Vec::new();
        for &t in &waiting {
            let free = needs(&session.workspace, t, &targets).iter().all(|n| done.contains(n))
                && !ready.iter().chain(&rest).any(|r| r.package == t.package);
            if free { ready.push(t) } else { rest.push(t) }
        }
        if ready.is_empty() {
            // Rules whose tools are built from each other's generated code:
            // one at a time, in order, the way `ensure` breaks such a circle.
            for target in rest {
                ensure(session, target, flags, overlay, &mut done);
            }
            break;
        }
        build_tools(session, &ready, flags);
        std::thread::scope(|scope| {
            for &target in &ready {
                let started = std::thread::Builder::new()
                    .name("buri-generate".into())
                    .stack_size(crate::parallel::STACK)
                    .spawn_scoped(scope, move || run(session, target, flags, overlay));
                if started.is_err() {
                    run(session, target, flags, overlay);
                }
            }
        });
        done.extend(ready);
        waiting = rest;
    }
}

/// One rule's generators, or a tool's contracts.
fn run(session: &Session, target: TargetId, flags: &Flags, overlay: &Overlay) {
    if has_contracts(&session.workspace, target) {
        run_contracts(session, target, flags, overlay);
    } else {
        run_rule(session, target, flags, overlay);
    }
}

/// The rules among `rules` that must run before `target`: those whose
/// generated code a tool `target` runs is built from.
fn needs(workspace: &Workspace, target: TargetId, rules: &[TargetId]) -> Vec<TargetId> {
    let mut out = Vec::new();
    for tool in tools_of(workspace, target) {
        if cycle(workspace, target, tool).is_some() {
            continue;
        }
        out.extend(
            workspace.closure(tool).into_iter().chain([tool]).filter(|m| *m != target && rules.contains(m)),
        );
    }
    out
}

/// Builds the programs of the tools these rules run, side by side, before any
/// of them runs.
///
/// Each tool is a whole compile. Rules used to build their tools one at a time,
/// and rules that share a tool would each build it when run side by side.
/// Building one here is the same call a rule makes when it first needs the tool
/// ([`tools::artifact`]), and it writes the program under the same key, so the
/// rule finds the program already there, and an edited tool still has a key of
/// its own. A tool whose build failed here fails again in that call, which is
/// where its error is reported.
fn build_tools(session: &Session, rules: &[TargetId], flags: &Flags) {
    let workspace = &session.workspace;
    let mut wanted: Vec<Tool> = Vec::new();
    for &rule in rules {
        for tool in tools_run_by(workspace, rule) {
            if !wanted.contains(&tool) {
                wanted.push(tool);
            }
        }
    }
    std::thread::scope(|scope| {
        for &tool in &wanted {
            // A thread that does not start leaves its tool to be built when a
            // rule first needs it.
            let _ = std::thread::Builder::new()
                .name("buri-tool".into())
                .stack_size(crate::parallel::STACK)
                .spawn_scoped(scope, move || tools::artifact(session, tool, flags));
        }
    });
}

/// Every tool with a program that one rule's generators run: each entry's tool,
/// the check of each input's language, and each contract's `generate`.
fn tools_run_by(workspace: &Workspace, target: TargetId) -> Vec<Tool> {
    let languages = &workspace.repo.languages;
    let check_of = |language: &crate::languages::Language| match &language.kind {
        crate::languages::Kind::Proto => Some(Tool::Proto),
        crate::languages::Kind::Textproto => Some(Tool::Textproto),
        crate::languages::Kind::Custom(own) => {
            own.check.as_ref().and_then(|c| tools::resolve(workspace, &c.value).ok())
        }
        crate::languages::Kind::BuiltIn(_) => None,
    };
    let mut out: Vec<Tool> = Vec::new();
    for generator in declared(workspace, target) {
        out.extend(tools::resolve(workspace, &generator.tool.value).ok());
        out.extend(generator.inputs.iter().filter_map(|i| languages.of(&i.value).and_then(check_of)));
    }
    if let Some(rule) = workspace.package(target.package).build.tool.as_ref().filter(|_| target.kind == RuleKind::Tool) {
        for a in rule.contracts() {
            let Some(language) = languages.named(&a.language.value) else { continue };
            out.extend(match language.kind {
                crate::languages::Kind::Textproto => Some(Tool::Textproto),
                _ => language.tools().and_then(|t| t.generate.as_ref()).and_then(|g| tools::resolve(workspace, &g.value).ok()),
            });
        }
    }
    out.retain(|t| *t != Tool::Json);
    out
}

/// One rule's generators, and — first — the generators of whatever its tools
/// are built from.
///
/// `done` is entered *before* the recursion, so a graph that turns back on
/// itself terminates here and is reported by [`cycle`] rather than looping.
fn ensure(
    session: &Session,
    target: TargetId,
    flags: &Flags,
    overlay: &Overlay,
    done: &mut BTreeSet<TargetId>,
) {
    if !done.insert(target) {
        return;
    }
    let workspace = Arc::clone(&session.workspace);
    for tool in tools_of(&workspace, target) {
        if cycle(&workspace, target, tool).is_some() {
            continue;
        }
        let members = workspace.closure(tool);
        for member in members.into_iter().chain([tool]) {
            if !declared(&workspace, member).is_empty() || has_contracts(&workspace, member) {
                ensure(session, member, flags, overlay, done);
            }
        }
    }
    run(session, target, flags, overlay);
}

/// The repository's own tools one rule's generated code comes from: each
/// entry's tool, and the check of each input's language. A tool's contracts
/// run each language's `generate`. The shipped tools are not among them: they
/// are compiled into this binary, and its identity is the toolchain version.
fn tools_of(workspace: &Workspace, target: TargetId) -> Vec<TargetId> {
    let mut tools: Vec<TargetId> = Vec::new();
    if let Some(rule) = workspace.package(target.package).build.tool.as_ref().filter(|_| target.kind == RuleKind::Tool) {
        for a in rule.contracts() {
            let generate = workspace.repo.languages.named(&a.language.value).and_then(|l| l.tools()?.generate.clone());
            tools.extend(generate.and_then(|g| tool_target(workspace, &g.value)));
        }
    }
    for generator in declared(workspace, target) {
        tools.extend(tool_target(workspace, &generator.tool.value));
        for input in &generator.inputs {
            let check = workspace.repo.languages.of(&input.value).and_then(|l| l.tools()?.check.clone());
            tools.extend(check.and_then(|c| tool_target(workspace, &c.value)));
        }
    }
    tools
}

/// Every file on disk one rule's generated code is worked out from, as
/// absolute paths: its inputs, whether or not they are there; every file a
/// check or a tool asked for through `needs`, a contract's schema among them;
/// and every file each tool it ran is built from, through the same walk for
/// whatever generated code the tool is itself built from.
///
/// This is what lets a closure of *files* stand in for a generated module,
/// which has no file of its own: the module, the diagnostics and the findings
/// a rule produced are a function of exactly these bytes and the toolchain.
/// The tool's files are the part that is easy to leave out, because no import
/// names them — editing a tool changes the module every dependent reads.
pub fn worked_out_from(workspace: &Workspace, rule: TargetId) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    walk_worked_out_from(workspace, rule, &mut seen, &mut files);
    files.sort();
    files.dedup();
    files
}

fn walk_worked_out_from(
    workspace: &Workspace,
    rule: TargetId,
    seen: &mut BTreeSet<TargetId>,
    files: &mut Vec<PathBuf>,
) {
    if !seen.insert(rule) {
        return;
    }
    let dir = &workspace.package(rule.package).dir;
    files.extend(inputs(workspace, rule).iter().map(|input| dir.join(input)));
    let reads = workspace.generated.outcome(rule).map(|o| o.reads).unwrap_or_default();
    files.extend(reads.iter().map(|rel| workspace.root.join(rel)));
    for tool in tools_of(workspace, rule) {
        for member in workspace.closure(tool).into_iter().chain([tool]) {
            let package = &workspace.package(member.package).dir;
            files.extend(crate::build::actions::rule_files(workspace, member).iter().map(|f| package.join(f)));
            if !declared(workspace, member).is_empty() || has_contracts(workspace, member) {
                walk_worked_out_from(workspace, member, seen, files);
            }
        }
    }
}

/// Generates a tool's contracts into it: each `accepts` entry's language's
/// `generate`, on its `type_schema`, as the module `<label>/<language>`.
///
/// For the `json` tool that is [`crate::languages::json::contract`]: the types and
/// `decode`. The `textproto` tool, and a language of a repository's own, is asked
/// with `typesOf`, and its one module is filed under the language's name.
fn run_contracts(session: &Session, target: TargetId, flags: &Flags, overlay: &Overlay) {
    let workspace = Arc::clone(&session.workspace);
    let package = workspace.package(target.package);
    let Some(rule) = &package.build.tool else { return };
    let languages = &workspace.repo.languages;
    let read = crate::languages::reader(&session.root, overlay);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut fingerprint = String::new();
    let mut outcome = Outcome::default();
    let mut produced: Vec<GeneratedModule> = Vec::new();
    let mut asks: Vec<(&buildfile::Accepts, tools::Contract, &crate::languages::Language)> = Vec::new();
    for a in rule.contracts() {
        if !seen.insert(a.language.value.clone()) {
            continue;
        }
        let Some(language) = languages.named(&a.language.value) else { continue };
        let contract = tools::Contract { package: package.path.clone(), type_schema: a.type_schema.value.clone() };
        asks.push((a, contract, language));
    }
    // What decides the answer: each schema and every file it reaches for a
    // JSON contract, and the key of the answer for any other.
    let mut answers: Vec<Result<GeneratedModule, Vec<(Diagnostic, Span)>>> = Vec::new();
    for (a, contract, language) in &asks {
        fingerprint.push_str(&format!("{} {}\n", a.language.value, a.type_schema.value));
        match language.dialect() {
            Some(_) => {
                let Some(path) = contract.json_path() else {
                    answers.push(Err(vec![(not_local(&a.type_schema.value), a.type_schema.span)]));
                    continue;
                };
                let mut asked = BTreeSet::new();
                let mut recording = |p: &str| {
                    asked.insert(p.to_string());
                    read(p)
                };
                let dialect_of = |p: &str| languages.dialect_of(p);
                let files = crate::languages::json::schema_closure(&path, &dialect_of, &mut recording);
                for p in &asked {
                    let hash = crate::build::cache::hash_bytes(files.get(p).map_or(&b""[..], |t| t.as_bytes()));
                    fingerprint.push_str(&format!("read {p}: {hash}\n"));
                }
                outcome.reads.extend(asked);
                match crate::languages::json::contract(&path, &dialect_of, &files) {
                    Ok(module) => answers.push(Ok(module_of(&a.language.value, module))),
                    Err(findings) => {
                        outcome.findings.extend(findings.into_iter().map(|f| (f, a.type_schema.span)));
                        answers.push(Err(Vec::new()));
                    }
                }
            }
            None => {
                let generate = language.tools().and_then(|t| t.generate.as_ref());
                let tool = match language.kind {
                    crate::languages::Kind::Textproto => Some(Tool::Textproto),
                    _ => generate.and_then(|g| tools::resolve(&workspace, &g.value).ok()),
                };
                let Some(tool) = tool else { continue };
                let label = workspace.label(target);
                let ask = tools::Ask {
                    tool,
                    entry: "generate",
                    fields: vec![("inputs", crate::json::Value::Array(Vec::new())), ("typesOf", contract.value())],
                    label: &label,
                };
                let failed = |why: String| Diagnostic {
                    code: "tool-failed".to_string(),
                    message: tool.name(&workspace),
                    note: Some(why),
                    fix: None,
                    origin: None,
                };
                let answer = tools::exchange(session, &ask, &read, flags);
                match answer.and_then(|x| Ok((Response::from_value(&x.value)?, x))) {
                    Ok((response, x)) => {
                        fingerprint.push_str(x.key.as_str());
                        fingerprint.push('\n');
                        outcome.reads.extend(x.asked.iter().cloned());
                        outcome.generated_reads.extend(x.asked);
                        let mut problems: Vec<(Diagnostic, Span)> =
                            response.diagnostics.into_iter().map(|d| (d, a.type_schema.span)).collect();
                        problems.extend(x.outside.iter().map(|p| (not_local(p), a.type_schema.span)));
                        match (response.modules.into_iter().next(), problems.is_empty()) {
                            (Some(mut module), true) => {
                                module.name = a.language.value.clone();
                                answers.push(Ok(module));
                            }
                            (None, true) => answers.push(Err(vec![(
                                failed("its `generate` answered `typesOf` with no module".to_string()),
                                a.type_schema.span,
                            )])),
                            (_, false) => answers.push(Err(problems)),
                        }
                    }
                    Err(why) => answers.push(Err(vec![(failed(why), a.type_schema.span)])),
                }
            }
        }
    }
    if workspace.generated.key_of(target).as_deref() == Some(fingerprint.as_str()) && !flags.force {
        return;
    }
    for answer in answers {
        match answer {
            Ok(module) => produced.push(module),
            Err(problems) => outcome.diagnostics.extend(problems),
        }
    }
    outcome.modules = produced.into_iter().map(Arc::new).collect();
    workspace.generated.record(&workspace, target, fingerprint, outcome);
}

fn not_local(path: &str) -> Diagnostic {
    Diagnostic {
        code: "schema-outside-repository".to_string(),
        message: format!("`{path}` is not a file in this repository"),
        note: None,
        fix: Some("check the schema in, and name it by a path relative to the tool or a `//` path".to_string()),
        origin: None,
    }
}

/// A module the `json` tool generated, under `name`.
fn module_of(name: &str, module: crate::languages::json::types::Module) -> GeneratedModule {
    GeneratedModule {
        name: name.to_string(),
        text: module.text,
        anchors: module
            .anchors
            .into_iter()
            .map(|(start, end, file, span)| Anchor { start, end, file, span })
            .collect(),
    }
}

/// Whether a generator's tool is built from the target that declares it.
///
/// Answered off the graph, so it is answered before anything is built: a
/// generator that needs its own output is an error, never a hang. The path
/// comes back with the span of the edge that introduced each step, the way
/// `circular-import` reports one.
fn cycle(workspace: &Workspace, target: TargetId, tool: TargetId) -> Option<CyclePath> {
    workspace.closure(tool).contains(&target).then(|| {
        workspace.dep_path(tool, target).unwrap_or_else(|| vec![(tool, None), (target, None)])
    })
}

/// One entry, ready to run: its inputs, `(repository path, text)` in the
/// order the entry lists them.
struct Entry {
    generator: Generator,
    inputs: Vec<(String, String)>,
    /// Each input's contract, where the tool has one for its language.
    contracts: Vec<Option<tools::Contract>>,
}

fn run_rule(session: &Session, target: TargetId, flags: &Flags, overlay: &Overlay) {
    let workspace = Arc::clone(&session.workspace);
    let mut entries: Vec<Entry> = Vec::new();
    let mut missing: Vec<(Diagnostic, Span)> = Vec::new();
    // The keys, plus a line per input nothing could read and the contents of
    // every file the last answer read. Together they are the whole of what
    // decides this rule's answer, so a session whose fingerprint has not moved
    // has nothing to re-run — and a missing input that appears moves it,
    // which is what makes writing the file enough.
    let mut fingerprint = String::new();
    let mut checks = Checks::default();
    let read = crate::languages::reader(&session.root, overlay);
    checks.start(session, target, overlay, &read, flags);

    for generator in declared(&workspace, target) {
        let package = workspace.package(target.package);
        let mut inputs = Vec::new();
        let mut unreadable = false;
        for input in &generator.inputs {
            let full = package.dir.join(&input.value);
            let rel = workspace.rel_of(&full);
            match input_text(overlay, &full) {
                Ok(text) => inputs.push((rel, text)),
                Err(e) => {
                    unreadable = true;
                    fingerprint.push_str(&format!("unreadable {rel}: {}\n", e.kind()));
                    // **A file that is there is never reported as absent.** A
                    // schema saved in UTF-16 answers `InvalidData` here, and
                    // "create the file" is no advice about a file a person can
                    // see in the directory the diagnostic names.
                    missing.push((
                        match e.kind() {
                            std::io::ErrorKind::NotFound => Diagnostic {
                                code: "unknown-source".to_string(),
                                // The entry, so the loader can name it.
                                message: input.value.clone(),
                                note: None,
                                fix: None,
                                origin: None,
                            },
                            _other => Diagnostic {
                                code: UNREADABLE.to_string(),
                                message: format!("cannot read {rel}: {e}"),
                                note: None,
                                fix: Some("check the file exists and is readable".to_string()),
                                origin: None,
                            },
                        },
                        input.span,
                    ));
                }
            }
        }
        if unreadable {
            continue;
        }
        // Checked before the tool reads them, and the tool does not run on a
        // file that fails. A tool with a contract has its inputs checked
        // against the contract's schema.
        let tool = tools::resolve(&workspace, &generator.tool.value).ok();
        let contracts: Vec<Option<tools::Contract>> =
            inputs.iter().map(|(rel, _)| contract_of(&workspace, tool, rel)).collect();
        let mut failed = false;
        for (((rel, text), contract), input) in inputs.iter().zip(&contracts).zip(&generator.inputs) {
            let kind = workspace.repo.languages.of(rel).map(|l| &l.kind);
            let identity = contract.as_ref().zip(kind).map(|(c, k)| c.identity(k));
            match checks.contracts.get(rel) {
                Some((first, _)) if identity.is_some() && first.is_some() && *first != identity => {
                    failed = true;
                    missing.push((
                        Diagnostic {
                            code: "schema-mismatch".to_string(),
                            message: format!(
                                "`{}` is read under two contracts, `{}` and `{}`",
                                input.value,
                                first.clone().unwrap_or_default(),
                                identity.clone().unwrap_or_default()
                            ),
                            note: Some("a file is checked against one schema, so every tool that reads it must agree on which".to_string()),
                            fix: Some("give the tools one `type_schema`, or read the file with one of them".to_string()),
                            origin: None,
                        },
                        input.span,
                    ));
                    continue;
                }
                Some(_) => {}
                None => {
                    checks.contracts.insert(rel.clone(), (identity, generator.tool.value.clone()));
                }
            }
            let (key, found) = checks.check(session, rel, text, contract.as_ref(), &read, flags);
            if let Some(key) = key {
                fingerprint.push_str(key.as_str());
                fingerprint.push('\n');
            }
            if !found.is_empty() {
                failed = true;
                checks.findings.extend(found.into_iter().map(|f| (f, generator.span)));
            }
        }
        if failed {
            continue;
        }
        fingerprint.push_str(generate_key(session, target, &generator.tool.value, &inputs, flags).as_str());
        fingerprint.push('\n');
        entries.push(Entry { generator: generator.clone(), inputs, contracts });
    }
    for path in workspace.generated.outcome(target).map(|o| o.generated_reads).unwrap_or_default() {
        let contents = read(&path);
        fingerprint.push_str(&format!("read {path}: {}\n", crate::build::cache::hash_bytes(contents.unwrap_or_default().as_bytes())));
    }

    if workspace.generated.key_of(target).as_deref() == Some(fingerprint.as_str()) && !flags.force {
        return;
    }

    let mut outcome = Outcome {
        diagnostics: missing,
        findings: checks.findings,
        reads: checks.reads.into_iter().collect(),
        ..Outcome::default()
    };
    let mut produced: Vec<(GeneratedModule, Span)> = Vec::new();
    for entry in entries {
        match answer(session, &workspace, target, &entry, &read, flags) {
            Ok((response, asked, findings)) => {
                outcome.findings.extend(findings.into_iter().map(|f| (f, entry.generator.span)));
                for module in response.modules {
                    produced.push((module, entry.generator.span));
                }
                for d in response.diagnostics {
                    outcome.diagnostics.push((d, entry.generator.span));
                }
                outcome.generated_reads.extend(asked);
            }
            Err(why) => outcome.diagnostics.push((
                Diagnostic {
                    code: "tool-failed".to_string(),
                    message: entry.generator.tool.value.clone(),
                    note: Some(why),
                    fix: None,
                    origin: None,
                },
                entry.generator.span,
            )),
        }
    }
    outcome.reads.extend(outcome.generated_reads.iter().cloned());
    keep_the_names_that_are_free(&workspace, target, produced, &mut outcome);
    workspace.generated.record(&workspace, target, fingerprint, outcome);
}

/// A check's key, where it has one, and what it found.
type Verdict = (Option<ActionKey>, Vec<crate::languages::Finding>);

/// The checks of one rule's inputs, each file once however many entries list
/// it.
#[derive(Default)]
struct Checks {
    done: BTreeMap<(String, Option<tools::Contract>), Verdict>,
    /// The contract each file was first read under, and by which tool.
    contracts: BTreeMap<String, (Option<String>, String)>,
    findings: Vec<(crate::languages::Finding, Span)>,
    reads: BTreeSet<String>,
    /// Checks run ahead of [`Checks::check`] by [`Checks::start`], each with
    /// the text it checked.
    started: BTreeMap<(String, Option<tools::Contract>), (String, Option<tools::Checked>)>,
}

/// An input's text: the editor's, where it has unsaved text, or the file's.
fn input_text(overlay: &Overlay, full: &std::path::Path) -> std::io::Result<String> {
    match overlay.get(full) {
        Some(text) => Ok(text.clone()),
        None => std::fs::read_to_string(full),
    }
}

/// The contract `tool` reads the input `rel` under, where it has one for the
/// input's language.
fn contract_of(workspace: &Workspace, tool: Option<Tool>, rel: &str) -> Option<tools::Contract> {
    let language = workspace.repo.languages.of(rel)?;
    tools::Contract::of(workspace, tool?, "generate", &language.name)
}

impl Checks {
    /// Runs every check this rule's inputs ask for, side by side.
    ///
    /// Each check is a process of the checking tool's own, and the rule asks
    /// for them one at a time, so a rule with many inputs used to wait for one
    /// process after another. [`Checks::check`] takes the answer started here
    /// for the same file, contract and text, so what a rule records is what it
    /// recorded before: only when each answer was worked out has changed.
    fn start(
        &mut self,
        session: &Session,
        target: TargetId,
        overlay: &Overlay,
        read: &(dyn Fn(&str) -> Option<String> + Sync),
        flags: &Flags,
    ) {
        let workspace = &session.workspace;
        let package = workspace.package(target.package);
        let mut asks: Vec<(String, String, Option<tools::Contract>)> = Vec::new();
        for generator in declared(workspace, target) {
            let tool = tools::resolve(workspace, &generator.tool.value).ok();
            for input in &generator.inputs {
                let full = package.dir.join(&input.value);
                let rel = workspace.rel_of(&full);
                let Ok(text) = input_text(overlay, &full) else { continue };
                let contract = contract_of(workspace, tool, &rel);
                if !asks.iter().any(|(r, _, c)| *r == rel && *c == contract) {
                    asks.push((rel, text, contract));
                }
            }
        }
        let answers = crate::parallel::map(asks.len(), |i| {
            let (rel, text, contract) = asks.get(i)?;
            tools::check_file(session, rel, text, contract.as_ref(), read, flags)
        });
        for ((rel, text, contract), answer) in asks.into_iter().zip(answers) {
            self.started.insert((rel, contract), (text, answer));
        }
    }

    /// Checks one input in a language this repository knows, and answers with
    /// the key the verdict is cached under. A file no tool checks has neither.
    fn check(
        &mut self,
        session: &Session,
        rel: &str,
        text: &str,
        contract: Option<&tools::Contract>,
        read: &dyn Fn(&str) -> Option<String>,
        flags: &Flags,
    ) -> (Option<ActionKey>, Vec<crate::languages::Finding>) {
        let at = (rel.to_string(), contract.cloned());
        if let Some(known) = self.done.get(&at) {
            return known.clone();
        }
        let checked = match self.started.remove(&at) {
            Some((checked_text, checked)) if checked_text == text => checked,
            _ => tools::check_file(session, rel, text, contract, read, flags),
        };
        let answer = match checked {
            Some(checked) => {
                self.reads.extend(checked.asked);
                (Some(checked.key), checked.findings)
            }
            None => (None, Vec::new()),
        };
        self.done.insert(at, answer.clone());
        answer
    }
}

/// One entry's answer, every path the tool asked to read, and what the
/// in-tree `json` tool found.
type Answered = (Response, BTreeSet<String>, Vec<crate::languages::Finding>);

fn answer(
    session: &Session,
    workspace: &Workspace,
    target: TargetId,
    entry: &Entry,
    read: &dyn Fn(&str) -> Option<String>,
    flags: &Flags,
) -> Result<Answered, String> {
    let name = &entry.generator.tool.value;
    let tool = tools::resolve(workspace, name).map_err(|_| format!("`{name}` names no tool"))?;
    if let Tool::Repo(t) = tool {
        if let Some(path) = cycle(workspace, target, t) {
            return Err(cycle_sentence(workspace, &path));
        }
    }
    let languages = &workspace.repo.languages;
    if tool == Tool::Json {
        return Ok(generate_json(languages, entry, read));
    }
    let inputs = entry
        .inputs
        .iter()
        .zip(&entry.contracts)
        .map(|((path, text), contract)| match contract {
            Some(c) => tools::typed_input(languages, path, text, c),
            None => tools::input(path, languages.of(path).map_or("", |l| &l.name), text),
        })
        .collect();
    let label = workspace.label(target);
    let ask = tools::Ask {
        tool,
        entry: "generate",
        fields: vec![("inputs", crate::json::Value::Array(inputs))],
        label: &label,
    };
    let answer = tools::exchange(session, &ask, read, flags)?;
    let mut response = Response::from_value(&answer.value)
        .map_err(|e| format!("the tool's answer is not one `generate` gives: {e}"))?;
    for path in answer.outside {
        response.diagnostics.push(Diagnostic {
            code: "schema-outside-repository".to_string(),
            message: format!("`{path}` is not in this repository"),
            note: None,
            fix: Some("check the file in, and name it by its repository path".to_string()),
            origin: None,
        });
    }
    Ok((response, answer.asked, Vec::new()))
}

/// The `json` tool's `generate`, in-tree: a module per input, named as the entry
/// lists it. A schema gives its types; a data file its schema's types and its
/// contents as a value.
fn generate_json(
    languages: &crate::languages::Languages,
    entry: &Entry,
    read: &dyn Fn(&str) -> Option<String>,
) -> Answered {
    let mut response = Response::default();
    let mut asked = BTreeSet::new();
    let mut findings = Vec::new();
    let dialect_of = |p: &str| languages.dialect_of(p);
    for ((rel, text), input) in entry.inputs.iter().zip(&entry.generator.inputs) {
        if languages.of(rel).and_then(crate::languages::Language::dialect).is_none() {
            findings.push(crate::languages::Finding::new(
                "json-untyped-keyword",
                rel,
                (0, 0),
                vec![
                    ("keyword", "$schema".to_string()),
                    ("why", "the `json` tool generates from `json`, `jsonc` and `json5` files, and this is none of them".to_string()),
                ],
            ));
            continue;
        }
        let mut recording = |p: &str| {
            asked.insert(p.to_string());
            read(p)
        };
        let files = crate::languages::json::schema_files(rel, text, None, &dialect_of, &mut recording);
        match crate::languages::json::generate(rel, text, &dialect_of, &files) {
            Ok(module) => response.modules.push(module_of(&input.value, module)),
            // A schema and a data file naming it find the same problems once.
            Err(found) => {
                for f in found {
                    if !findings.contains(&f) {
                        findings.push(f);
                    }
                }
            }
        }
    }
    (response, asked, findings)
}

/// What one `generators` entry's answer depends on, computed without running
/// anything: the rule, the tool's program key, and every input's contents.
fn generate_key(
    session: &Session,
    target: TargetId,
    tool: &str,
    inputs: &[(String, String)],
    flags: &Flags,
) -> ActionKey {
    let mut k = KeyBuilder::new(Action::Generate, flags.mode);
    let paths: Vec<String> = inputs.iter().map(|(p, _)| p.clone()).collect();
    k.rule_identity(&session.workspace.label(target), "generate", &paths);
    k.input("tool", tool.as_bytes());
    if let Ok(tool) = tools::resolve(&session.workspace, tool) {
        k.dependency(&tools::program_key(session, tool, flags));
    }
    for (path, text) in inputs {
        k.input(path, text.as_bytes());
    }
    k.finish()
}


/// Moves the modules whose names are the generator's own into the outcome, and
/// reports the ones that are not.
///
/// **A name is either a generator's or a person's, never both.** Two entries
/// naming one module used to be one of them silently replacing the other, and a
/// generated `lib.buri` used to replace a library's whole public surface: the
/// program ran, printed the generator's answer, and `lint` had nothing to say
/// about the file nobody was compiling any more.
///
/// The one that is already there wins, so the file on disk keeps meaning what
/// it says while the build reports the collision.
fn keep_the_names_that_are_free(
    workspace: &Workspace,
    target: TargetId,
    produced: Vec<(GeneratedModule, Span)>,
    outcome: &mut Outcome,
) {
    let package = workspace.package(target.package);
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for (module, span) in produced {
        let clash = if taken.contains(&module.name) {
            Some("a generator on this rule has already named it".to_string())
        } else {
            shadowed_source(&package.dir, &module.name)
                .map(|file| format!("`{}` is a source of this package", file))
        };
        match clash {
            None => {
                taken.insert(module.name.clone());
                outcome.modules.push(Arc::new(module));
            }
            Some(note) => outcome.diagnostics.push((
                Diagnostic {
                    code: "generator-duplicate-module".to_string(),
                    // The path a person would write, which is what the page
                    // asks for and what an import would have named.
                    message: package.module_path(&module.name),
                    note: Some(note),
                    fix: None,
                    origin: None,
                },
                span,
            )),
        }
    }
}

/// The source file a generated module's name would take over, if it takes one.
///
/// Only the names that resolve to a *module* of the package count, which is why
/// this is a short list rather than "a file with this name exists":
/// the `proto` tool names its module `point.proto` and `lib/wire/point.proto`
/// is a file on disk, and those two are not a collision — a schema is the
/// generator's input, not a module anybody imports.
fn shadowed_source(dir: &std::path::Path, name: &str) -> Option<String> {
    let file = match name {
        "" | "lib.buri" => "lib.buri",
        "main" | "main.buri" => "main.buri",
        "testing" | "testing/lib.buri" => "testing/lib.buri",
        other if other.ends_with(".buri") => other,
        _other => return None,
    };
    dir.join(file).is_file().then(|| file.to_string())
}

/// One step of a cycle: a target, and the span of the edge that reached the
/// next one.
pub type CyclePath = Vec<(TargetId, Option<Span>)>;

/// The path a cycle took, one label to the next.
pub fn cycle_sentence(workspace: &Workspace, path: &[(TargetId, Option<Span>)]) -> String {
    path.iter().map(|(t, _)| workspace.label(*t)).collect::<Vec<_>>().join(" -> ")
}

/// The cycle a rule's generator closes, if it closes one.
///
/// Public because the loader reports it: the diagnostic belongs beside the
/// `generators` entry that wrote the tool down, which is a source location the
/// build layer has no `SourceMap` to render.
pub fn cycle_of(workspace: &Workspace, target: TargetId) -> Option<(&Generator, CyclePath)> {
    for generator in declared(workspace, target) {
        let Some(tool) = tool_target(workspace, &generator.tool.value) else { continue };
        if let Some(path) = cycle(workspace, target, tool) {
            return Some((generator, path));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hard_strings() -> Vec<String> {
        vec![
            String::new(),
            "plain".to_string(),
            "a \"quoted\" word".to_string(),
            "a back\\slash".to_string(),
            "two\nlines\r\nand\ta tab".to_string(),
            "\u{7}bell and \u{1f} unit separator".to_string(),
            "ünïcödé — 日本語 — 🧪".to_string(),
            "}{[],:\"".to_string(),
        ]
    }

    #[test]
    fn a_request_survives_the_wire() {
        let inputs: Vec<crate::json::Value> = hard_strings()
            .iter()
            .enumerate()
            .map(|(i, s)| tools::input(&format!("lib/x/{i}.schema"), "", s))
            .collect();
        let request = crate::json::Value::object(vec![("inputs", crate::json::Value::Array(inputs))]);
        let line = request.to_string();
        assert!(!line.contains('\n'), "the request is one line: {line}");
        assert_eq!(crate::json::parse(&line).expect("the request decodes"), request);
    }

    #[test]
    fn a_response_survives_the_wire() {
        let response = Response {
            modules: hard_strings()
                .iter()
                .enumerate()
                .map(|(i, s)| GeneratedModule {
                    name: format!("m{i}"),
                    text: s.clone(),
                    anchors: vec![Anchor {
                        start: 0,
                        end: s.len(),
                        file: format!("lib/x/{i}.schema"),
                        span: (3, 9),
                    }],
                })
                .collect(),
            diagnostics: vec![
                Diagnostic {
                    code: "proto-unsupported".to_string(),
                    message: "a \"message\" with\na newline".to_string(),
                    note: Some("a note".to_string()),
                    fix: Some("a fix".to_string()),
                    origin: Some(Origin { file: "lib/x/0.schema".to_string(), span: (18, 29) }),
                },
                Diagnostic {
                    code: "tool-diagnostic".to_string(),
                    message: "nothing to point at".to_string(),
                    note: None,
                    fix: None,
                    origin: None,
                },
            ],
        };
        let span = |(start, end): (usize, usize)| {
            Value::object(vec![("start", Value::number(start as i64)), ("end", Value::number(end as i64))])
        };
        let optional = |v: &Option<String>| v.clone().map_or(Value::Null, Value::Str);
        let modules = response.modules.iter().map(|m| {
            let anchors = m.anchors.iter().map(|a| {
                Value::object(vec![
                    ("start", Value::number(a.start as i64)),
                    ("end", Value::number(a.end as i64)),
                    ("file", Value::str(&a.file)),
                    ("span", span(a.span)),
                ])
            });
            Value::object(vec![
                ("name", Value::str(&m.name)),
                ("text", Value::str(&m.text)),
                ("anchors", Value::Array(anchors.collect())),
            ])
        });
        let diagnostics = response.diagnostics.iter().map(|d| {
            let origin = d.origin.as_ref().map_or(Value::Null, |o| {
                Value::object(vec![("file", Value::str(&o.file)), ("span", span(o.span))])
            });
            Value::object(vec![
                ("code", Value::str(&d.code)),
                ("message", Value::str(&d.message)),
                ("note", optional(&d.note)),
                ("fix", optional(&d.fix)),
                ("origin", origin),
            ])
        });
        let line = Value::object(vec![
            ("modules", Value::Array(modules.collect())),
            ("diagnostics", Value::Array(diagnostics.collect())),
        ])
        .to_string();
        assert!(!line.contains('\n'), "the response is one line: {line}");
        assert_eq!(Response::decode(&line).expect("the response decodes"), response);
    }

    /// The shape the protocol is specified in, written by hand rather than by
    /// this encoder — so the two agree about the document rather than about
    /// each other.
    #[test]
    fn the_documented_wire_shape_decodes() {
        let line = concat!(
            r#"{"modules":[{"name":"point.proto","text":"export struct Point {}\n","#,
            r#""anchors":[{"start":0,"end":24,"file":"lib/wire/point.proto","#,
            r#""span":{"start":18,"end":29}}]}],"#,
            r#" "diagnostics":[{"code":"proto-unsupported","message":"no","note":null,"#,
            r#""fix":null,"origin":{"file":"lib/wire/point.proto","span":{"start":18,"end":29}}}]}"#
        );
        let response = Response::decode(line).expect("the documented shape decodes");
        assert_eq!(response.modules.len(), 1);
        let module = response.modules.first().expect("one module");
        assert_eq!(module.name, "point.proto");
        assert_eq!(module.text, "export struct Point {}\n");
        assert_eq!(module.anchor_at(0).map(Anchor::origin).map(|o| o.span), Some((18, 29)));
        assert_eq!(module.anchor_at(24), None);
        let d = response.diagnostics.first().expect("one diagnostic");
        assert_eq!(d.note, None);
        assert_eq!(d.origin.as_ref().map(|o| o.file.as_str()), Some("lib/wire/point.proto"));
    }

    /// A `\u` escape and a surrogate pair, which a generator written against a
    /// JSON library may well produce even where this encoder would not.
    #[test]
    fn escaped_scalars_decode() {
        let response =
            Response::decode(r#"{"modules":[{"name":"m","text":"é🧪","anchors":[]}]}"#)
                .expect("escapes decode");
        assert_eq!(response.modules.first().map(|m| m.text.as_str()), Some("é🧪"));
    }

    #[test]
    fn what_is_not_a_response_says_so() {
        for bad in [
            "",
            "not json",
            "{",
            "[]}",
            r#"{"modules":[{"text":"x"}]}"#,
            r#"{"modules":[{"name":"m","text":"x","anchors":[{"start":0}]}]}"#,
            r#"{"modules":"a string"}"#,
        ] {
            assert!(Response::decode(bad).is_err(), "`{bad}` decoded as a response");
        }
    }

    /// The failure that sent this suite looking: a generator killed by a
    /// signal, whose note said "a signal" and nothing else. A crash, a stack
    /// overflow and an out-of-memory kill are three different bugs and the
    /// note has to tell them apart.
    #[cfg(unix)]
    #[test]
    fn a_generator_killed_by_a_signal_is_named_with_its_signal() {
        use std::os::unix::process::ExitStatusExt as _;
        let crashed = std::process::ExitStatus::from_raw(11);
        let note = said(&format!("the generator {}", how_it_ended(&crashed)), "");
        assert_eq!(note, "the generator was killed by SIGSEGV (signal 11)");

        let out_of_memory = std::process::ExitStatus::from_raw(9);
        let note = said(&format!("the generator {}", how_it_ended(&out_of_memory)), "");
        assert_eq!(note, "the generator was killed by SIGKILL (signal 9)");

        // A number the two platforms disagree about goes unnamed rather than
        // named wrongly.
        let other = std::process::ExitStatus::from_raw(10);
        assert_eq!(how_it_ended(&other), "was killed by signal 10");
    }

    /// What the tool said on the way down comes with the note, whichever way
    /// it went down.
    #[cfg(unix)]
    #[test]
    fn a_crashing_generator_is_reported_with_its_standard_error() {
        use std::os::unix::process::ExitStatusExt as _;
        let crashed = std::process::ExitStatus::from_raw(11);
        let note = said(
            &format!("the generator {}", how_it_ended(&crashed)),
            "\nRangeError: Maximum call stack size exceeded.\n  at print\n",
        );
        assert_eq!(
            note,
            "the generator was killed by SIGSEGV (signal 11)\n\
             RangeError: Maximum call stack size exceeded.\n  at print"
        );

        let refused = std::process::ExitStatus::from_raw(1 << 8);
        assert_eq!(how_it_ended(&refused), "exited with 1");
    }

    /// The two ways bun has printed one tool's division by zero, both seen in
    /// the suite: the frames and the line heading them differ, and the note
    /// does not.
    #[test]
    fn a_crash_says_the_same_whichever_way_the_runtime_renders_its_stack() {
        let tool = "/tmp/x/.buri/out/tools/7677ca.mjs";
        let first = format!(
            "division by zero\nError: division by zero\n    at $abort ({tool}:4:17)\n    at async main ({tool}:711:16)\n    at processTicksAndRejections (native:7:39)\n"
        );
        let second = format!(
            "division by zero\nError\n    at $abort ({tool}:4:17)\n    at processTicksAndRejections (unknown:7:39)\n"
        );
        let expected = "the tool exited with 1\ndivision by zero";
        assert_eq!(said("the tool exited with 1", &first), expected);
        assert_eq!(said("the tool exited with 1", &second), expected);

        // A heading that says more than the line above it stays, and so does
        // a line that only reads like a frame.
        let other = "the schema is wrong\nTypeError: undefined is not an object\n    at f (x.mjs:1:2)\n  at line 3\n";
        assert_eq!(
            said("the tool exited with 1", other),
            "the tool exited with 1\nthe schema is wrong\nTypeError: undefined is not an object\n  at line 3"
        );
    }

    /// A tool that fills its pipe may not fill the page. The **tail** is kept,
    /// because what a program says last is what says why it stopped.
    #[test]
    fn a_flood_on_standard_error_is_cut_to_its_tail() {
        let flood = format!("{}the last line", "x".repeat(20_000));
        let note = said("the generator exited with 1", &flood);
        assert!(note.len() < STDERR_TAIL + 200, "the note is bounded: {} bytes", note.len());
        assert!(note.starts_with("the generator exited with 1\n"), "{note}");
        assert!(note.ends_with("the last line"), "the tail is what is kept");
        assert!(
            note.contains("bytes of standard error are not shown"),
            "the note says it was cut: {note}"
        );

        // Cut through a character rather than between two: the note is still
        // text, and the character is not half-written.
        let wide = "é".repeat(20_000);
        let note = said("the generator exited with 1", &wide);
        assert!(note.ends_with('é'), "the tail ends on a character");
        assert!(note.chars().all(|c| c != '\u{fffd}'), "no character was cut in half");
    }

    #[test]
    fn the_innermost_anchor_wins() {
        let module = GeneratedModule {
            name: "m".to_string(),
            text: "0123456789".to_string(),
            anchors: vec![
                Anchor { start: 0, end: 10, file: "a".into(), span: (0, 1) },
                Anchor { start: 0, end: 4, file: "a".into(), span: (2, 3) },
                Anchor { start: 6, end: 8, file: "a".into(), span: (4, 5) },
            ],
        };
        assert_eq!(module.anchor_at(1).map(|a| a.span), Some((2, 3)));
        assert_eq!(module.anchor_at(5).map(|a| a.span), Some((0, 1)));
        assert_eq!(module.anchor_at(7).map(|a| a.span), Some((4, 5)));
        assert_eq!(module.anchor_at(10), None);
    }
}
