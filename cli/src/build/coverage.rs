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

/// A decision, as the report knows it: where it's reported and how many
/// branches it has. Keyed by what names it, so suites that each loaded the file
/// and instantiations of one generic function count as one.
struct Decision {
    line: usize,
    at: u32,
    branches: usize,
}

type DecisionId = (coverage::Kind, u32, u32);

struct Run {
    raw: PathBuf,
    /// Every line a site starts on, by file.
    lines: BTreeMap<String, BTreeSet<usize>>,
    /// Every decision, by file.
    decisions: BTreeMap<String, BTreeMap<DecisionId, Decision>>,
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
    *run() = Some(Run { raw, lines: BTreeMap::new(), decisions: BTreeMap::new() });
    Ok(())
}

/// The directory a process writes its counts to, during a coverage run.
pub fn raw_dir() -> Option<PathBuf> {
    run().as_ref().map(|r| r.raw.clone())
}

/// Probes `program`, and notes every line and decision the user code
/// `analysis` checked holds, called or not.
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
    let name_of = |span: Span| user.contains(&span.file).then(|| map.get(span.file).name.as_str());
    let line_of = |span: Span| name_of(span).map(|name| (name, map.get(span.file).line_col(span.start).0));
    let mut found: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut decisions: BTreeMap<String, BTreeMap<DecisionId, Decision>> = BTreeMap::new();
    for (_, body) in analysis.checked.bodies.iter() {
        if !user.contains(&body.expr.span.file) {
            continue;
        }
        coverage::sites(&body.expr, &mut |span| {
            if let Some((name, line)) = line_of(span) {
                found.entry(name.to_string()).or_default().insert(line);
            }
        });
        coverage::decisions(&body.expr, &mut |d| {
            let at = Span { start: d.at, ..d.span };
            if let Some((name, line)) = line_of(at) {
                let id = (d.kind, d.span.start, d.span.end);
                let decision = Decision { line, at: d.at, branches: d.branches };
                decisions.entry(name.to_string()).or_default().insert(id, decision);
            }
        });
    }
    let i64_ty = analysis.checked.tables.prim(Prim::I64);
    let bool_ty = analysis.checked.tables.prim(Prim::Bool);
    coverage::instrument(program, &i64_ty, &|span| line_of(span).map(|(n, l)| coverage::key(n, l)));
    coverage::branches(program, &i64_ty, &bool_ty, &|kind, span, slot| {
        name_of(span).map(|n| coverage::branch_key(n, kind, (span.start, span.end), slot))
    });
    if let Some(r) = run().as_mut() {
        for (file, lines) in found {
            r.lines.entry(file).or_default().extend(lines);
        }
        for (file, ds) in decisions {
            r.decisions.entry(file).or_default().extend(ds);
        }
    }
}

/// Every count the processes wrote, summed by key.
fn counts(raw: &Path) -> HashMap<u64, u64> {
    let mut counts: HashMap<u64, u64> = HashMap::new();
    for text in std::fs::read_dir(raw).into_iter().flatten().flatten().filter_map(|e| std::fs::read_to_string(e.path()).ok()) {
        for line in text.lines() {
            let Some((key, count)) = line.split_once(' ') else { continue };
            let (Ok(key), Ok(count)) = (key.parse::<u64>(), count.parse::<u64>()) else { continue };
            let n = counts.entry(key).or_insert(0);
            *n = n.saturating_add(count);
        }
    }
    counts
}

/// One file's report.
#[derive(Default)]
struct File {
    /// `(line, count)`.
    lines: Vec<(usize, u64)>,
    /// `(line, block, branch, count)`, with `None` for a decision nothing
    /// reached.
    branches: Vec<(usize, usize, usize, Option<u64>)>,
}

/// Every count the processes wrote, added up and put against the lines and
/// decisions the suites hold: a row of branches per file through `out`, and
/// [`LCOV`].
pub fn report(root: &Path, out: &mut dyn FnMut(&str)) {
    let Some(Run { raw, lines, decisions }) = run().take() else { return };
    let counts = counts(&raw);
    let _ = std::fs::remove_dir_all(&raw);
    let count = |key: u64| counts.get(&key).copied().unwrap_or(0);

    let mut files: BTreeMap<&str, File> = BTreeMap::new();
    for (file, set) in &lines {
        let f = files.entry(file.as_str()).or_default();
        f.lines = set.iter().map(|&line| (line, count(coverage::key(file, line)))).collect();
    }
    for (file, ds) in &decisions {
        let f = files.entry(file.as_str()).or_default();
        let empty = BTreeSet::new();
        let sites = lines.get(file).unwrap_or(&empty);
        let mut ordered: Vec<(&DecisionId, &Decision)> = ds.iter().collect();
        ordered.sort_by_key(|(id, d)| (d.line, d.at, id.1, id.2, id.0));
        let mut block: usize = 0;
        let mut last_line = 0;
        for (&(kind, start, end), d) in ordered {
            block = if d.line == last_line { block.saturating_add(1) } else { 0 };
            last_line = d.line;
            let slot = |i| count(coverage::branch_key(file, kind, (start, end), i));
            let taken: Vec<u64> = match kind {
                // How often the operand came back, less how often execution
                // went on, is how often `?` returned early.
                coverage::Kind::Try => vec![slot(0), slot(1).saturating_sub(slot(0))],
                _ => (0..d.branches).map(slot).collect(),
            };
            let reached = taken.iter().any(|&n| n > 0);
            // lcov wants a line under every branch. One no statement starts on
            // counts how often its decisions were reached.
            let times = taken.iter().fold(0u64, |a, &n| a.saturating_add(n));
            let rows = &mut f.lines;
            match rows.binary_search_by_key(&d.line, |l| l.0) {
                Ok(i) if !sites.contains(&d.line) => {
                    if let Some(row) = rows.get_mut(i) {
                        row.1 = row.1.max(times);
                    }
                }
                Ok(_) => {}
                Err(i) => rows.insert(i, (d.line, times)),
            }
            for (i, n) in taken.into_iter().enumerate() {
                f.branches.push((d.line, block, i, reached.then_some(n)));
            }
        }
    }
    files.retain(|_, f| !f.lines.is_empty() || !f.branches.is_empty());

    let mut lcov = String::new();
    let mut rows: Vec<(String, usize, usize)> = Vec::new();
    for (file, f) in &files {
        lcov.push_str(&format!("TN:\nSF:{file}\n"));
        for &(line, block, branch, taken) in &f.branches {
            let taken = taken.map_or("-".to_string(), |n| n.to_string());
            lcov.push_str(&format!("BRDA:{line},{block},{branch},{taken}\n"));
        }
        let hit = f.branches.iter().filter(|b| b.3.is_some_and(|n| n > 0)).count();
        lcov.push_str(&format!("BRF:{}\nBRH:{hit}\n", f.branches.len()));
        for (line, n) in &f.lines {
            lcov.push_str(&format!("DA:{line},{n}\n"));
        }
        let lines_hit = f.lines.iter().filter(|l| l.1 > 0).count();
        lcov.push_str(&format!("LF:{}\nLH:{lines_hit}\nend_of_record\n", f.lines.len()));
        rows.push((file.to_string(), hit, f.branches.len()));
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
    out("branch coverage");
    for r in &rows {
        // A file with no branches has nothing to be a share of.
        let percent = if r.2 == 0 { "-".to_string() } else { format!("{:.1}%", r.1 as f64 * 100.0 / r.2 as f64) };
        out(&format!("  {:<name_width$}  {:>fraction_width$}  {percent:>6}", r.0, fraction(r)));
    }
    out(&format!("lcov: {LCOV}"));
}
