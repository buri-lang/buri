//! `buri test`.
//!
//! Tests are ordinary build actions. Because there is no mutable global state,
//! no ambient I/O, and no observable ordering, the runner is free to shard
//! across processes and to run a suite's tests in any order. Nothing about a
//! suite's result may depend on that freedom, so the runner does not offer a
//! knob to turn it off — there is no `--shuffle`, and a suite that would need
//! one is a suite with a dependency it has not admitted to
//! (TESTING.md, "Running").
//!
//! `test` is the only action that leaves this process, so it is the only one
//! whose spawn has to be made deterministic: `build/spawn.rs` gives it an
//! explicit environment and a clock frozen at `1970-01-01T00:00:00Z`. That is
//! about determinism rather than confinement — what keeps a suite from reaching
//! the machine is that a test source has no name for `core/host` at all, and
//! that its capabilities are fakes this runner injects.

use crate::build::actions;
use crate::build::buildfile::Platform;
use crate::build::session::{self, Session};
use crate::build::workspace::{RuleKind, TargetId};
use crate::commands::arguments;
use crate::compiler::backend::javascript;
use crate::compiler::modules::Unit;
use crate::compiler::transform::monomorphize;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The JavaScript runtime `buri run` and the test runner execute artifacts
/// with. `bun` unless `BURI_JS` says otherwise.
pub fn js_runtime() -> String {
    std::env::var("BURI_JS").unwrap_or_else(|_| "bun".to_string())
}

struct Case {
    cached: bool,
    name: String,
    module: String,
    location: String,
    ok: bool,
    ms: u64,
    message: String,
    actual: Option<String>,
    expected: Option<String>,
}

/// What one suite produced: the cases that ran, and the ones a `--filter` left
/// out. A skipped test is reported rather than silently absent, so a filter
/// that matches nothing looks different from a suite that holds nothing.
#[derive(Default)]
struct Outcome {
    cases: Vec<Case>,
    skipped: usize,
    /// Golden files `--accept` rewrote, for the blank line before the summary.
    accepted: usize,
}

pub fn cmd_test(args: &arguments::Args) -> i32 {
    let mut s = match session::open(&args.flags) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    if s.report() {
        return 2;
    }
    let targets = match s.resolve_targets(&args.targets) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };

    let started = Instant::now();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut cached = 0usize;
    let mut suites = 0usize;
    let mut printed = false;
    let mut hard_error = false;

    for target in targets {
        if !has_tests(&s, target) {
            continue;
        }
        suites += 1;
        match run_suite(&mut s, target, args) {
            Ok(outcome) => {
                skipped += outcome.skipped;
                printed |= outcome.accepted > 0;
                for c in &outcome.cases {
                    if c.cached {
                        cached += 1;
                    }
                    if c.ok {
                        passed += 1;
                    } else {
                        failed += 1;
                        report_failure(&s, target, c);
                        printed = true;
                    }
                }
            }
            Err(diags) => {
                hard_error |= s.print(&diags);
            }
        }
    }

    let elapsed = started.elapsed().as_secs_f64();
    if suites == 0 {
        println!("no test suites");
        return 0;
    }
    let note = if cached > 0 { format!(", {cached} cached") } else { String::new() };
    if printed {
        println!();
    }
    println!("{passed} passed, {failed} failed, {skipped} skipped ({elapsed:.1}s{note})");
    if hard_error || failed > 0 {
        return 1;
    }
    0
}

fn has_tests(s: &Session, target: TargetId) -> bool {
    let pkg = s.ws.pkg(target.pkg);
    match target.kind {
        RuleKind::Library => {
            pkg.build.library.as_ref().is_some_and(|l| !l.test.sources.is_empty())
        }
        RuleKind::Binary => {
            pkg.build.binary.as_ref().is_some_and(|b| !b.test.sources.is_empty())
        }
    }
}

fn suite(s: &Session, target: TargetId) -> Option<crate::build::buildfile::TestSuite> {
    let pkg = s.ws.pkg(target.pkg);
    match target.kind {
        RuleKind::Library => pkg.build.library.as_ref().map(|l| l.test.clone()),
        RuleKind::Binary => pkg.build.binary.as_ref().map(|b| b.test.clone()),
    }
}

/// One suite, once per platform it runs on.
fn run_suite(
    s: &mut Session,
    target: TargetId,
    args: &arguments::Args,
) -> Result<Outcome, Diagnostics> {
    let mut diags = Diagnostics::new();
    // A suite inherits its target's tags and platform restrictions, so a suite
    // for a `server` library is checked as server code without saying
    // anything. A suite that names no platforms runs once on the host, and the
    // host here is the machine, not the JavaScript the backend emits.
    let declared: Vec<Platform> =
        suite(s, target).map(|x| x.platforms).unwrap_or_default().iter().map(|p| p.value).collect();
    let checked: Vec<Platform> = if declared.is_empty() {
        vec![crate::compiler::driver::host_native_platform()]
    } else {
        declared.clone()
    };
    // A platform a suite names must be one the target admits: asking for a JS
    // run of a `[LINUX, MACOS]` library is an error, not a skip
    // (TAGS.md, "Tags and tests").
    for p in &checked {
        actions::check_policy(s, target, *p, &mut diags);
    }
    if diags.has_errors() {
        return Err(diags);
    }

    // One run per declared platform. Only the JavaScript backend exists, so a
    // suite that *names* a platform this toolchain cannot execute is told so
    // rather than quietly run through the backend that does exist; a suite that
    // names none is checked against the host and executed as JavaScript, which
    // is what `buri test` has always done.
    let runs: Vec<Platform> = if declared.is_empty() { vec![Platform::Js] } else { declared };
    let mut outcome = Outcome::default();
    for platform in runs {
        if platform != Platform::Js {
            let span = suite(s, target).map(|x| x.span).unwrap_or(Span::NONE);
            diags.push(
                Diagnostic::error(
                    span,
                    format!("the {} backend is not implemented", platform.slug()),
                )
                .with_code("platform-not-implemented")
                .with_note("this toolchain emits JavaScript, so only a JS run can be executed")
                .with_fix(format!(
                    "drop {} from `test.platforms` until the backend exists",
                    platform.proto()
                )),
            );
            continue;
        }
        match run_on(s, target, platform, args) {
            Ok(one) => {
                outcome.cases.extend(one.cases);
                outcome.skipped += one.skipped;
                outcome.accepted += one.accepted;
            }
            Err(d) => diags.extend(d.items),
        }
    }
    if diags.has_errors() {
        return Err(diags);
    }
    Ok(outcome)
}

fn run_on(
    s: &mut Session,
    target: TargetId,
    platform: Platform,
    args: &arguments::Args,
) -> Result<Outcome, Diagnostics> {
    let mut diags = Diagnostics::new();

    // A suite whose inputs are unchanged is not re-run and reports as cached;
    // `--force` re-runs anyway, which is the honest way to check that a suite
    // is not accidentally depending on the cache.
    let output = crate::build::buildfile::Output {
        platform: Some(crate::build::buildfile::Sp::new(platform, Span::NONE)),
        ..Default::default()
    };
    let key = actions::test_key(s, target, &output, &args.flags);
    let cache = crate::build::cache::Cache::open(&s.root);
    let label = s.ws.label(target);
    if !args.flags.force && args.flags.filter.is_none() && !args.flags.accept {
        if let Some(bytes) = cache.get(&key) {
            let text = String::from_utf8_lossy(&bytes).to_string();
            let mut cached = parse_results(&text);
            for c in &mut cached {
                c.cached = true;
            }
            if !cached.is_empty() {
                crate::build::cache::explain(
                    args.flags.explain,
                    crate::build::cache::Status::Cached,
                    crate::build::cache::Action::Test,
                    &label,
                    platform,
                    &key,
                );
                return Ok(Outcome { cases: cached, skipped: 0, accepted: 0 });
            }
        }
    }
    crate::build::cache::explain(
        args.flags.explain,
        crate::build::cache::Status::Run,
        crate::build::cache::Action::Test,
        &label,
        platform,
        &key,
    );

    let unit = Unit { target: Some(target), platform, with_tests: true };
    let analysis = crate::compiler::driver::analyze(Some(&s.ws), &mut s.map, &unit);
    if analysis.diags.has_errors() {
        return Err(analysis.diags);
    }

    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut program =
        monomorphize::run(&analysis.checked, module_paths, &mut diags, monomorphize::Roots::Tests);
    if diags.has_errors() {
        return Err(diags);
    }
    if program.tests.is_empty() {
        return Ok(Outcome::default());
    }

    // What a `--filter` leaves out is counted here rather than in the runner:
    // the names are known before the binary is built, and a count nobody has to
    // run a process to learn is one the summary can always print.
    let skipped = match &args.flags.filter {
        Some(f) => program.tests.iter().filter(|t| !t.name.contains(f.as_str())).count(),
        None => 0,
    };

    let mut source = actions::emit(&mut program, &analysis.checked.tables, &args.flags, &mut diags)?;

    // The runner's in-memory `Fs` contains exactly `test { data: [...] }`, and
    // nothing else on disk is visible.
    let data = load_test_data(s, target);
    source.push_str(&format!("\n$t.data={data};\n"));
    // The action's clock, spliced in after the runtime is defined and before a
    // test could reach one. A test has no name for `core/host` to begin with;
    // this is what keeps a suite's *record* the same bytes twice, so that
    // reproducibility is a question worth asking about a suite.
    source.push_str(crate::build::spawn::FIXED_CLOCK_JS);
    let filter = args
        .flags
        .filter
        .as_ref()
        .map(|f| javascript::quote(f))
        .unwrap_or_else(|| "null".into());
    source.push_str(&format!(
        "$write(1,JSON.stringify($run({filter})));\n"
    ));

    let dir = s.root.join(".buri/out/js").join(&s.ws.pkg(target.pkg).path);
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("test.mjs");
    if let Err(e) = std::fs::write(&path, &source) {
        diags.push(
            Diagnostic::error(Span::NONE, format!("cannot write {}: {e}", path.display()))
                .with_fix("check the directory exists and is writable"),
        );
        return Err(diags);
    }

    let limit = suite(s, target).and_then(|x| x.timeout_seconds);
    let out = match execute(&path, limit) {
        Ok(Execution::Finished(out)) => out,
        Ok(Execution::TimedOut) => {
            let span = suite(s, target).map(|x| x.span).unwrap_or(Span::NONE);
            let seconds = limit.unwrap_or(0);
            diags.push(
                Diagnostic::error(
                    span,
                    format!("{label}'s test suite ran longer than {seconds}s"),
                )
                .with_code("test-timeout")
                .with_label("the timeout this suite declares")
                .with_note(
                    "the run was killed, so no test in this suite has a result — a timeout is \
                     the suite's, not one test's",
                )
                .with_fix(
                    "raise `timeout_seconds`, or find the test that does not finish; a test that \
                     never returns is a loop with no exit, since nothing here blocks on I/O",
                ),
            );
            return Err(diags);
        }
        Err(e) => {
            diags.push(
                Diagnostic::error(Span::NONE, format!("cannot run the test binary: {e}"))
                    .with_fix("install bun, or point BURI_JS at a JavaScript runtime"),
            );
            return Err(diags);
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    // Only a clean run is worth remembering: a failure is what you are trying
    // to fix, and re-running it should re-run it. `--accept` is outside the
    // cache in both directions — it is the one mode that writes to the source
    // tree, and a mode that writes must not also be able to be served.
    let mut cases = parse_results(&stdout);
    if !cases.is_empty()
        && cases.iter().all(|c| c.ok)
        && args.flags.filter.is_none()
        && !args.flags.accept
    {
        cache.put(&key, stdout.as_bytes());
    }
    for c in &mut cases {
        if let Some(t) = program.tests.iter().find(|t| t.name == c.name) {
            if !t.span.is_none() {
                let f = s.map.get(t.span.file);
                let (line, col) = f.line_col(t.span.start);
                c.location = format!("{}:{line}:{col}", f.name);
            }
        }
    }
    if cases.is_empty() && !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        diags.push(
            Diagnostic::error(Span::NONE, "the test binary did not run")
                .with_fix("read the runtime's own message below; it is what failed")
                .with_note(err.trim().to_string()),
        );
        return Err(diags);
    }
    let accepted = if args.flags.accept { accept_goldens(s, target, &cases) } else { 0 };
    Ok(Outcome { cases, skipped, accepted })
}

enum Execution {
    Finished(std::process::Output),
    TimedOut,
}

/// Runs the test binary, killing it if the suite declared a `timeout_seconds`
/// and it runs past one.
///
/// A test cannot block on I/O — every effect it can reach is one the runner
/// supplied — so the only way to run forever is a loop with no exit, and the
/// only thing to do about one is to stop it. The wait is a poll rather than a
/// thread because the suite is the only child and there is nothing else for
/// this process to do while it runs.
///
/// The command comes from `build/spawn.rs` rather than from
/// `Command::new(js_runtime())`, which is the whole of what distinguishes an
/// action's process from any other: an explicit environment and a frozen clock,
/// so that the same suite produces the same record on a machine set to a
/// different time zone.
fn execute(path: &std::path::Path, limit: Option<u32>) -> std::io::Result<Execution> {
    use std::process::Stdio;
    let runtime = js_runtime();
    let Some(mut cmd) = crate::build::spawn::command(&runtime) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("`{runtime}` is not on PATH"),
        ));
    };
    let mut child = cmd
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let Some(limit) = limit else {
        return Ok(Execution::Finished(child.wait_with_output()?));
    };
    let deadline = Instant::now() + Duration::from_secs(u64::from(limit));
    loop {
        if child.try_wait()?.is_some() {
            return Ok(Execution::Finished(child.wait_with_output()?));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Execution::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ---------------------------------------------------------------------------
// --accept
// ---------------------------------------------------------------------------

/// Rewrites the declared `data` files whose contents a failing `assert.eq`
/// expected, and prints a diff for each.
///
/// Rewriting a golden file is not something a hermetic action may do, so this
/// is a separate mode rather than a step in the normal path (TESTING.md, "Test
/// data and golden files"). Three things bound it, and all three are visible
/// here: only a file listed in `data` is considered, a file that does not exist
/// is never created, and the value written is the one the test actually
/// produced. Everything else about the run is unchanged — the failure is still
/// reported and still counted, because what `--accept` changes is the source
/// tree, not the verdict.
fn accept_goldens(s: &Session, target: TargetId, cases: &[Case]) -> usize {
    let Some(suite) = suite(s, target) else { return 0 };
    if suite.data.is_empty() {
        return 0;
    }
    let dir = s.ws.pkg(target.pkg).dir.clone();
    let label = s.ws.label(target);
    let mut accepted = 0usize;
    for c in cases.iter().filter(|c| !c.ok) {
        let (Some(actual), Some(expected)) = (&c.actual, &c.expected) else { continue };
        // Only a text assertion can name a file's contents, and `$show` renders
        // a `Str` as a JSON string. Anything else is a failure `--accept` has
        // no opinion about.
        let (Some(actual), Some(expected)) = (unquote(actual), unquote(expected)) else {
            continue;
        };
        for d in &suite.data {
            let path = dir.join(&d.value);
            // Never creates a file: a golden that is not there is a golden
            // nobody declared the contents of, and inventing one would make
            // `--accept` a way to add test data by running the tests.
            let Ok(body) = std::fs::read_to_string(&path) else { continue };
            if body != expected {
                continue;
            }
            if std::fs::write(&path, &actual).is_err() {
                continue;
            }
            print_diff(&label, &d.value, &body, &actual);
            accepted += 1;
            break;
        }
    }
    accepted
}

/// The diff `--accept` prints for one rewritten golden.
///
/// Line-oriented, with the common head and tail elided, because what a reader
/// checks is the lines that moved.
fn print_diff(label: &str, file: &str, before: &str, after: &str) {
    println!("accepted {label}  {file}");
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let mut head = 0;
    while head < old.len() && head < new.len() && old[head] == new[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < old.len() - head && tail < new.len() - head
        && old[old.len() - 1 - tail] == new[new.len() - 1 - tail]
    {
        tail += 1;
    }
    for line in &old[head..old.len() - tail] {
        println!("  -{line}");
    }
    for line in &new[head..new.len() - tail] {
        println!("  +{line}");
    }
}

/// A JSON string literal back to the text it stands for, or `None` when the
/// rendering is not one.
fn unquote(shown: &str) -> Option<String> {
    let inner = shown.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'u' => {
                let hex: String = chars.by_ref().take(4).collect();
                let code = u32::from_str_radix(&hex, 16).ok()?;
                out.push(char::from_u32(code)?);
            }
            other => out.push(other),
        }
    }
    Some(out)
}

fn load_test_data(s: &Session, target: TargetId) -> String {
    let Some(suite) = suite(s, target) else { return "{}".into() };
    let dir = s.ws.pkg(target.pkg).dir.clone();
    let mut fields = Vec::new();
    for d in &suite.data {
        let p: PathBuf = dir.join(&d.value);
        let body = std::fs::read_to_string(&p).unwrap_or_default();
        fields.push(format!("{}:{}", javascript::quote(&d.value), javascript::quote(&body)));
    }
    format!("{{{}}}", fields.join(","))
}

/// The runner writes one JSON array; this reads it without a JSON library,
/// because the shape is fixed and known.
fn parse_results(text: &str) -> Vec<Case> {
    let start = match text.find('[') {
        Some(i) => i,
        None => return Vec::new(),
    };
    let json = &text[start..];
    let mut cases = Vec::new();
    for chunk in split_objects(json) {
        let name = field(&chunk, "name").unwrap_or_default();
        let module = field(&chunk, "module").unwrap_or_default();
        let ok = chunk.contains("\"ok\":true");
        let ms = field_raw(&chunk, "ms").and_then(|s| s.parse().ok()).unwrap_or(0);
        let message = field(&chunk, "message").unwrap_or_default();
        let actual = field(&chunk, "actual");
        let expected = field(&chunk, "expected");
        cases.push(Case { cached: false, name, module, ok, ms, message, actual, expected, location: String::new() });
    }
    cases
}

fn split_objects(json: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut in_str = false;
    let bytes = json.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'{' if !in_str => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    out.push(json[start..=i].to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn field(chunk: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":\"");
    let at = chunk.find(&key)? + key.len();
    let rest = &chunk[at..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    Some(out)
}

fn field_raw(chunk: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":");
    let at = chunk.find(&key)? + key.len();
    let rest = &chunk[at..];
    let end = rest.find([',', '}'])?;
    Some(rest[..end].trim().to_string())
}

/// Output names the target, the file, and the test (TESTING.md, "Running").
fn report_failure(s: &Session, target: TargetId, c: &Case) {
    let label = s.ws.label(target);
    let file = c.module.trim_start_matches("//");
    let file = file.strip_prefix(&s.ws.pkg(target.pkg).path).unwrap_or(file);
    let file = file.trim_start_matches('/');
    println!("FAIL {label}  {file}.buri  \"{}\"", c.name);
    println!("  {}", c.message);
    if let (Some(a), Some(e)) = (&c.actual, &c.expected) {
        println!("    actual:   {a}");
        println!("    expected: {e}");
    }
    if !c.location.is_empty() {
        println!("  --> {}", c.location);
    }
    let _ = c.ms;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_string_comes_back_as_the_text_it_stands_for() {
        assert_eq!(unquote("\"zero\"").as_deref(), Some("zero"));
        assert_eq!(unquote("\"a\\tb\\n\"").as_deref(), Some("a\tb\n"));
        assert_eq!(unquote("\"say \\\"hi\\\"\"").as_deref(), Some("say \"hi\""));
        assert_eq!(unquote("\"\\u0041\"").as_deref(), Some("A"));
        // Not a string rendering: `--accept` has no opinion about these.
        assert_eq!(unquote("19"), None);
        assert_eq!(unquote(".Some(1)"), None);
    }
}
