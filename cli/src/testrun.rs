//! `buri test`.
//!
//! Tests are ordinary build actions. Because there is no mutable global state,
//! no ambient I/O, and no observable ordering, the runner is free to shard
//! across processes and to run a suite's tests in any order — `--shuffle` is on
//! by default and the seed is printed.

use crate::build;
use crate::buildfile::Platform;
use crate::cli::{self, Session};
use crate::compile::Unit;
use crate::diag::{Diagnostic, Diagnostics, Span};
use crate::mono;
use crate::workspace::TargetId;
use std::path::PathBuf;
use std::time::Instant;

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

pub fn cmd_test(args: &cli::Args) -> i32 {
    let mut s = match cli::open(&args.flags) {
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
    let mut cached = 0usize;
    let mut suites = 0usize;
    let mut hard_error = false;

    for target in targets {
        if !has_tests(&s, target) {
            continue;
        }
        suites += 1;
        match run_suite(&mut s, target, args) {
            Ok(cases) => {
                for c in &cases {
                    if c.cached {
                        cached += 1;
                    }
                    if c.ok {
                        passed += 1;
                    } else {
                        failed += 1;
                        report_failure(&s, target, c);
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
    println!("{passed} passed, {failed} failed ({elapsed:.1}s{note})");
    if hard_error || failed > 0 {
        return 1;
    }
    0
}

fn has_tests(s: &Session, target: TargetId) -> bool {
    let pkg = s.ws.pkg(target.pkg);
    match target.kind {
        crate::workspace::RuleKind::Library => {
            pkg.build.library.as_ref().is_some_and(|l| !l.test.sources.is_empty())
        }
        crate::workspace::RuleKind::Binary => {
            pkg.build.binary.as_ref().is_some_and(|b| !b.test.sources.is_empty())
        }
    }
}

fn suite(s: &Session, target: TargetId) -> Option<crate::buildfile::TestSuite> {
    let pkg = s.ws.pkg(target.pkg);
    match target.kind {
        crate::workspace::RuleKind::Library => pkg.build.library.as_ref().map(|l| l.test.clone()),
        crate::workspace::RuleKind::Binary => pkg.build.binary.as_ref().map(|b| b.test.clone()),
    }
}

fn run_suite(
    s: &mut Session,
    target: TargetId,
    args: &cli::Args,
) -> Result<Vec<Case>, Diagnostics> {
    let mut diags = Diagnostics::new();
    // A suite inherits its target's tags and platform restrictions, so a suite
    // for a `server` library is checked as server code without saying
    // anything. A suite that names no platforms runs once on the host, and the
    // host here is the machine, not the JavaScript the backend emits.
    let declared = suite(s, target).map(|x| x.platforms).unwrap_or_default();
    let platforms: Vec<Platform> = if declared.is_empty() {
        vec![crate::driver::host_native_platform()]
    } else {
        declared.iter().map(|p| p.value).collect()
    };
    for p in &platforms {
        build::check_policy(s, target, *p, &mut diags);
    }
    if diags.has_errors() {
        return Err(diags);
    }

    // A suite whose inputs are unchanged is not re-run and reports as cached;
    // `--force` re-runs anyway, which is the honest way to check that a suite
    // is not accidentally depending on the cache.
    let output = crate::buildfile::Output {
        platform: Some(crate::buildfile::Sp::new(Platform::Js, Span::NONE)),
        ..Default::default()
    };
    let key = build::test_key(s, target, &output, &args.flags);
    let cache = crate::cache::Cache::open(&s.root);
    if !args.flags.force && args.flags.filter.is_none() && !args.flags.accept {
        if let Some(bytes) = cache.get(&key) {
            let text = String::from_utf8_lossy(&bytes).to_string();
            let mut cached = parse_results(&text);
            for c in &mut cached {
                c.cached = true;
            }
            if !cached.is_empty() {
                return Ok(cached);
            }
        }
    }

    let unit = Unit { target: Some(target), platform: Platform::Js, with_tests: true };
    let analysis = crate::driver::analyze(Some(&s.ws), &mut s.map, &unit);
    if analysis.diags.has_errors() {
        return Err(analysis.diags);
    }

    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let program =
        mono::run(&analysis.checked, module_paths, &mut diags, mono::Roots::Tests);
    if diags.has_errors() {
        return Err(diags);
    }
    if program.tests.is_empty() {
        return Ok(Vec::new());
    }

    let mut source = build::emit(&program, &analysis.checked.tables, &args.flags, &mut diags)?;

    // The runner's in-memory `Fs` contains exactly `test { data: [...] }`, and
    // nothing else on disk is visible.
    let data = load_test_data(s, target);
    source.push_str(&format!("\n$t.data={data};\n"));
    let filter = args
        .flags
        .filter
        .as_ref()
        .map(|f| crate::js::quote(f))
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

    let out = std::process::Command::new(js_runtime()).arg(&path).output();
    let out = match out {
        Ok(o) => o,
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
    // to fix, and re-running it should re-run it.
    let mut cases = parse_results(&stdout);
    if !cases.is_empty() && cases.iter().all(|c| c.ok) && args.flags.filter.is_none() {
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
    Ok(cases)
}

fn load_test_data(s: &Session, target: TargetId) -> String {
    let Some(suite) = suite(s, target) else { return "{}".into() };
    let dir = s.ws.pkg(target.pkg).dir.clone();
    let mut fields = Vec::new();
    for d in &suite.data {
        let p: PathBuf = dir.join(&d.value);
        let body = std::fs::read_to_string(&p).unwrap_or_default();
        fields.push(format!("{}:{}", crate::js::quote(&d.value), crate::js::quote(&body)));
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
