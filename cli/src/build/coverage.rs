//! `buri test --coverage`, on this side of the processes it runs
//! (`design/COVERAGE.md`): the directory they write their counts to, the lines
//! each suite's user code holds, and the report that puts the two together.
//!
//! One run at a time per process, which is all `buri test` is: the state is
//! set by [`begin`] and taken by [`report`], and nothing reads it in between
//! except the suites of that run.

use crate::compiler::driver::Analysis;
use crate::compiler::middle::{coverage, monomorphize};
use crate::compiler::modules::Role;
use crate::compiler::semantics::types::Prim;
use crate::diagnostics::{FileId, SourceMap, Span};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The variable a test process reads the directory for its counts from.
pub const VARIABLE: &str = "BURI_COVERAGE";

/// Where the report is written, from the repository root.
pub const LCOV: &str = ".buri/coverage/lcov.info";

/// Where the processes write their counts, from the repository root.
const RAW: &str = ".buri/coverage/raw";

/// What a JavaScript probe calls, appended to a coverage build's test bundle.
///
/// Here rather than in `runtime.js`, because a test bundle carries the whole
/// runtime unminified and a plain one has to stay the bytes it was. On exit the
/// counts go where `cli/runtime/coverage.rs` puts them, in the same shape.
/// `var` and function declarations, so the probes reach them from above.
pub const JS: &str = r#"
// `buri test --coverage`: the probes' counts (`design/COVERAGE.md`).
var $coverage_counts = null;
function $coverage_hit(key) {
  if ($coverage_counts === null) {
    $coverage_counts = new Map();
    process.on("exit", $coverage_write);
  }
  $coverage_counts.set(key, ($coverage_counts.get(key) || 0) + 1);
  return 0;
}
function $coverage_write() {
  const dir = process.env.BURI_COVERAGE;
  const fs = process.getBuiltinModule ? process.getBuiltinModule("fs") : $fsOrNull();
  if (!dir || !fs) return;
  let text = "";
  for (const [key, count] of $coverage_counts) text += String(key) + " " + count + "\n";
  for (let i = 0; ; i++) {
    try {
      fs.writeFileSync(dir + "/" + process.pid + "-" + i + ".hits", text, { flag: "wx" });
      return;
    } catch (e) {
      if (e.code !== "EEXIST") return;
    }
  }
}
"#;

struct Run {
    raw: PathBuf,
    /// Every line a site starts on, by file.
    lines: BTreeMap<String, BTreeSet<usize>>,
}

static RUN: Mutex<Option<Run>> = Mutex::new(None);

fn run() -> MutexGuard<'static, Option<Run>> {
    RUN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts a coverage run with an empty directory for the counts. Every
/// process `buri` starts from here on is told where it is.
pub fn begin(root: &Path) -> Result<(), String> {
    let raw = root.join(RAW);
    let _ = std::fs::remove_dir_all(&raw);
    std::fs::create_dir_all(&raw).map_err(|e| format!("cannot create {}: {e}", raw.display()))?;
    *run() = Some(Run { raw, lines: BTreeMap::new() });
    Ok(())
}

/// The directory a process writes its counts to, during a coverage run.
pub fn raw_dir() -> Option<PathBuf> {
    run().as_ref().map(|r| r.raw.clone())
}

/// Probes `program`, and notes every line the user code `analysis` checked
/// holds, called or not.
///
/// User code is ordinary source read from disk. The standard library, the
/// bundled platforms, generated modules and test sources never are.
pub fn instrument(analysis: &Analysis, map: &SourceMap, program: &mut monomorphize::Program) {
    let user: HashSet<FileId> = analysis
        .loaded
        .modules
        .iter()
        .filter(|m| matches!(m.role, Role::Source | Role::Entry) && m.disk.is_some())
        .map(|m| m.file)
        .collect();
    let line_of = |span: Span| {
        user.contains(&span.file).then(|| {
            let file = map.get(span.file);
            (file.name.as_str(), file.line_col(span.start).0)
        })
    };
    let mut found: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for (_, body) in analysis.checked.bodies.iter() {
        if !user.contains(&body.expr.span.file) {
            continue;
        }
        coverage::sites(&body.expr, &mut |span| {
            if let Some((name, line)) = line_of(span) {
                found.entry(name.to_string()).or_default().insert(line);
            }
        });
    }
    let i64_ty = analysis.checked.tables.prim(Prim::I64);
    coverage::instrument(program, &i64_ty, &|span| line_of(span).map(|(n, l)| coverage::key(n, l)));
    if let Some(r) = run().as_mut() {
        for (file, lines) in found {
            r.lines.entry(file).or_default().extend(lines);
        }
    }
}

/// Every count the processes wrote, added up and put against the lines the
/// suites hold: a line per file through `out`, and [`LCOV`].
pub fn report(root: &Path, out: &mut dyn FnMut(&str)) {
    let Some(Run { raw, lines }) = run().take() else { return };
    let mut at: HashMap<u64, (&str, usize)> = HashMap::new();
    let mut counts: BTreeMap<&str, BTreeMap<usize, u64>> = BTreeMap::new();
    for (file, set) in &lines {
        let per = counts.entry(file.as_str()).or_default();
        for &line in set {
            at.insert(coverage::key(file, line), (file.as_str(), line));
            per.insert(line, 0);
        }
    }
    for text in std::fs::read_dir(&raw).into_iter().flatten().flatten().filter_map(|e| std::fs::read_to_string(e.path()).ok()) {
        for line in text.lines() {
            let Some((key, count)) = line.split_once(' ') else { continue };
            let (Ok(key), Ok(count)) = (key.parse::<u64>(), count.parse::<u64>()) else { continue };
            let Some(&(file, line)) = at.get(&key) else { continue };
            if let Some(n) = counts.get_mut(file).and_then(|c| c.get_mut(&line)) {
                *n = n.saturating_add(count);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&raw);
    counts.retain(|_, c| !c.is_empty());

    let mut lcov = String::new();
    let mut rows: Vec<(String, usize, usize)> = Vec::new();
    for (file, per) in &counts {
        let hit = per.values().filter(|&&n| n > 0).count();
        lcov.push_str(&format!("TN:\nSF:{file}\n"));
        for (line, n) in per {
            lcov.push_str(&format!("DA:{line},{n}\n"));
        }
        lcov.push_str(&format!("LF:{}\nLH:{hit}\nend_of_record\n", per.len()));
        rows.push((file.to_string(), hit, per.len()));
    }
    let path = root.join(LCOV);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, lcov) {
        out(&format!("coverage: cannot write {LCOV}: {e}"));
        return;
    }
    out("");
    if rows.is_empty() {
        out("coverage: no suite loaded any of this repository's own source");
        return;
    }
    let (hit, total) = rows.iter().fold((0usize, 0usize), |(h, t), r| (h.saturating_add(r.1), t.saturating_add(r.2)));
    rows.push(("total".to_string(), hit, total));
    let name_width = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let fraction = |r: &(String, usize, usize)| format!("{}/{}", r.1, r.2);
    let fraction_width = rows.iter().map(|r| fraction(r).len()).max().unwrap_or(0);
    out("coverage");
    for r in &rows {
        let percent = if r.2 == 0 { 100.0 } else { r.1 as f64 * 100.0 / r.2 as f64 };
        out(&format!(
            "  {:<name_width$}  {:>fraction_width$}  {:>6}",
            r.0,
            fraction(r),
            format!("{percent:.1}%")
        ));
    }
    out(&format!("lcov: {LCOV}"));
}
