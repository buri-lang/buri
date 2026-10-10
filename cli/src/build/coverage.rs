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
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::{self, FnId, Prim};
use crate::diagnostics::{FileId, SourceMap, Span};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The variable a test process reads the directory for its counts from.
pub const VARIABLE: &str = "BURI_COVERAGE";

/// Set to `1`, the variable that counts the standard library as if it were
/// the repository's own source. For the standard library's own coverage gate
/// (`coverage/stdlib`), not for users, so it isn't a flag.
pub const STD: &str = "BURI_COVERAGE_STD";

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

/// Which generic instantiation a decision ran in, under MC/DC: `None` outside a
/// generic function, `Some("")` for a generic function no suite instantiated.
type Instance = Option<String>;

/// A decision, as the report knows it: where it's reported and how many
/// branches it has. Keyed by what names it, so suites that each loaded the file
/// and instantiations of one generic function count as one.
struct Decision {
    line: usize,
    at: u32,
    branches: usize,
    /// A derived decision's branches have names; the rest are numbered.
    names: Vec<String>,
    /// A derived decision's operation and type, and its place in the walk,
    /// which order the decisions on a `derive` line.
    group: (String, usize),
}

type DecisionId = (coverage::Kind, u32, u32, Instance);

/// An MC/DC decision: its conditions, and the text to name each by.
struct Tree {
    line: usize,
    tree: coverage::Tree,
    text: String,
    conditions: Vec<String>,
}

type TreeId = (u32, u32, Instance);

#[derive(Default)]
struct Universe {
    /// Every line a site starts on.
    lines: BTreeSet<usize>,
    decisions: BTreeMap<DecisionId, Decision>,
    trees: BTreeMap<TreeId, Tree>,
}

struct Run {
    raw: PathBuf,
    mcdc: bool,
    /// Whether the standard library counts too ([`STD`]).
    std: bool,
    /// By file.
    files: BTreeMap<String, Universe>,
}

static RUN: Mutex<Option<Run>> = Mutex::new(None);

fn run() -> MutexGuard<'static, Option<Run>> {
    RUN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts a coverage run with an empty directory for the counts. Every
/// process `buri` starts from here on is told where it is.
pub fn begin(root: &Path, mcdc: bool) -> Result<(), String> {
    let raw = root.join(RAW);
    let _ = std::fs::remove_dir_all(&raw);
    std::fs::create_dir_all(&raw).map_err(|e| format!("cannot create {}: {e}", raw.display()))?;
    let std = std::env::var_os(STD).is_some_and(|v| v == "1");
    *run() = Some(Run { raw, mcdc, std, files: BTreeMap::new() });
    Ok(())
}

/// The directory a process writes its counts to, during a coverage run.
pub fn raw_dir() -> Option<PathBuf> {
    run().as_ref().map(|r| r.raw.clone())
}

/// Text from the source, on one line, for an lcov record.
fn text(map: &SourceMap, span: Span) -> String {
    let source = &map.get(span.file).text;
    let raw = source.get(span.start as usize..span.end as usize).unwrap_or("");
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Probes `program`, and notes every line and decision the user code
/// `analysis` checked holds, called or not.
///
/// User code is ordinary source read from disk. The standard library, the
/// bundled platforms, generated modules and test sources never are.
pub fn instrument(analysis: &Analysis, map: &SourceMap, program: &mut monomorphize::Program) {
    let Some((mcdc, std)) = run().as_ref().map(|r| (r.mcdc, r.std)) else { return };
    let user: HashSet<FileId> = analysis
        .loaded
        .modules
        .iter()
        .filter(|m| (matches!(m.role, Role::Source | Role::Entry) && m.disk.is_some()) || (std && m.role == Role::Std))
        .map(|m| m.file)
        .collect();
    let name_of = |span: Span| user.contains(&span.file).then(|| map.get(span.file).name.as_str());
    let line_of = |span: Span| name_of(span).map(|name| (name, map.get(span.file).line_col(span.start).0));
    let tables = &analysis.checked.tables;
    let mut files: BTreeMap<String, Universe> = BTreeMap::new();
    let mut lines: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();

    // Under MC/DC, each instantiation of a generic function is its own.
    let mut instances: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let mut instantiated: HashSet<FnId> = HashSet::new();
    if mcdc {
        for (&slot, (id, targs)) in &program.instances {
            let info = tables.fn_info(*id);
            if !user.contains(&info.span.file) {
                continue;
            }
            let shown: Vec<String> = targs.iter().map(|t| types::show(tables, None, &[], t)).collect();
            let label = if info.generics.len() == shown.len() {
                info.generics.iter().zip(&shown).map(|(g, t)| format!("{} = {t}", g.name)).collect::<Vec<_>>().join(", ")
            } else {
                shown.join(", ")
            };
            instances.insert(slot, label);
            instantiated.insert(*id);
        }
    }
    let mut note = |body: &typed::Expr, instance: Instance, traps: Option<&dyn Fn(&typed::Expr) -> bool>| {
        coverage::decisions(body, traps, &mut |d| {
            let at = Span { start: d.at, ..d.span };
            if let Some((name, line)) = line_of(at) {
                let id = (d.kind, d.span.start, d.span.end, instance.clone());
                let decision = Decision { line, at: d.at, branches: d.branches, names: Vec::new(), group: Default::default() };
                files.entry(name.to_string()).or_default().decisions.insert(id, decision);
            }
        });
        if traps.is_some() {
            coverage::trees(body, &mut |tree| {
                let Some(span) = tree_span(&tree) else { return };
                if !tree.counted() {
                    return;
                }
                if let Some((name, line)) = line_of(span) {
                    let mut spans = Vec::new();
                    tree.conditions(&mut spans);
                    let conditions: Vec<String> = spans.iter().map(|s| text(map, *s)).collect();
                    let id = (span.start, span.end, instance.clone());
                    let whole = render(&tree, &conditions, &mut 0).0;
                    let t = Tree { line, tree, text: whole, conditions };
                    files.entry(name.to_string()).or_default().trees.insert(id, t);
                }
            });
        }
    };
    let checked_traps = |e: &typed::Expr| match &e.kind {
        typed::ExprKind::CallFn { func: typed::Callee::Decl { id, .. }, .. } => {
            let info = tables.fn_info(*id);
            let module = analysis.loaded.modules.get(info.module.index()).map(|m| m.path.as_str());
            info.intrinsic && module == Some("core/bits") && info.name.starts_with("shift")
        }
        _ => false,
    };
    for (id, body) in analysis.checked.bodies.iter() {
        if !user.contains(&body.expr.span.file) {
            continue;
        }
        coverage::sites(&body.expr, &mut |span| {
            if let Some((name, line)) = line_of(span) {
                lines.entry(name.to_string()).or_default().insert(line);
            }
        });
        if !mcdc {
            note(&body.expr, None, None);
            continue;
        }
        if instantiated.contains(&id) {
            continue;
        }
        let instance = (!tables.fn_info(id).generics.is_empty()).then(String::new);
        note(&body.expr, instance, Some(&checked_traps));
    }
    if mcdc {
        let mono_traps = |e: &typed::Expr| match &e.kind {
            typed::ExprKind::CallFn { func: typed::Callee::Func(f), .. } => program
                .funcs
                .get(f.0 as usize)
                .is_some_and(|f| name_of(f.span).is_none() && coverage::aborts(f)),
            _ => false,
        };
        for (slot, label) in &instances {
            if let Some(body) = program.funcs.get(*slot).and_then(|f| f.body()) {
                note(body, Some(label.clone()), Some(&mono_traps));
            }
        }
    }

    let i64_ty = tables.prim(Prim::I64);
    let bool_ty = tables.prim(Prim::Bool);
    coverage::instrument(program, &i64_ty, &|span| line_of(span).map(|(n, l)| coverage::key(n, l)));
    let file_of = |span: Span| name_of(span).map(str::to_string);
    let options = coverage::Options {
        i64_ty,
        bool_ty,
        file_of: &file_of,
        mcdc: mcdc.then_some(&instances),
        tables: mcdc.then_some(tables),
    };
    for d in coverage::branches(program, &options) {
        if let Some((name, line)) = line_of(d.span) {
            let id = (coverage::Kind::Derived, d.span.start, d.span.end, Some(d.label));
            let group = (d.group, d.order);
            let decision = Decision { line, at: d.span.start, branches: d.branches.len(), names: d.branches, group };
            files.entry(name.to_string()).or_default().decisions.insert(id, decision);
        }
    }
    for (file, set) in lines {
        files.entry(file).or_default().lines = set;
    }
    if let Some(r) = run().as_mut() {
        for (file, u) in files {
            let mine = r.files.entry(file).or_default();
            mine.lines.extend(u.lines);
            mine.decisions.extend(u.decisions);
            mine.trees.extend(u.trees);
        }
    }
}

/// A decision's text, from its conditions': its span leaves out a `(` that
/// opens it. With how tightly it binds: `||`, `&&`, or a condition.
fn render(tree: &coverage::Tree, conditions: &[String], next: &mut usize) -> (String, u8) {
    let operand = |t: &coverage::Tree, at_least: u8, next: &mut usize| {
        let (text, binds) = render(t, conditions, next);
        if binds < at_least { format!("({text})") } else { text }
    };
    match tree {
        coverage::Tree::Condition(_) => {
            let text = conditions.get(*next).cloned().unwrap_or_default();
            *next = next.saturating_add(1);
            (text, 2)
        }
        coverage::Tree::And(l, r, _) => {
            let l = operand(l, 1, next);
            (format!("{l} && {}", operand(r, 1, next)), 1)
        }
        coverage::Tree::Or(l, r, _) => {
            let l = operand(l, 0, next);
            (format!("{l} || {}", operand(r, 0, next)), 0)
        }
        coverage::Tree::Not(x) => (format!("!{}", operand(x, 2, next)), 2),
    }
}

/// The span a decision is named by: its outermost `&&` or `||`.
fn tree_span(tree: &coverage::Tree) -> Option<Span> {
    match tree {
        coverage::Tree::And(_, _, s) | coverage::Tree::Or(_, _, s) => Some(*s),
        coverage::Tree::Not(x) => tree_span(x),
        coverage::Tree::Condition(_) => None,
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
    branches: Vec<(usize, usize, String, Option<u64>)>,
    /// `(line, expression, shown)`, a condition each.
    conditions: Vec<(usize, String, bool)>,
    /// MC/DC decisions, and how many have every condition shown.
    decisions: (usize, usize),
}

/// How an instantiation reads in a record.
fn instance_suffix(instance: &Instance) -> String {
    match instance.as_deref() {
        None => String::new(),
        Some("") => " (never instantiated)".to_string(),
        Some(label) => format!(" ({label})"),
    }
}

/// Every count the processes wrote, added up and put against the lines and
/// decisions the suites hold: a row per file through `out`, and [`LCOV`].
pub fn report(root: &Path, out: &mut dyn FnMut(&str)) {
    let Some(Run { raw, mcdc, mut files, .. }) = run().take() else { return };
    let counts = counts(&raw);
    let _ = std::fs::remove_dir_all(&raw);
    let count = |key: u64| counts.get(&key).copied().unwrap_or(0);

    // A generic function one suite instantiated and another didn't is only the
    // first suite's instantiations.
    for u in files.values_mut() {
        let named: HashSet<(coverage::Kind, u32, u32)> =
            u.decisions.keys().filter(|id| id.3.as_deref().is_some_and(|l| !l.is_empty())).map(|id| (id.0, id.1, id.2)).collect();
        u.decisions.retain(|id, _| id.3.as_deref() != Some("") || !named.contains(&(id.0, id.1, id.2)));
        let named: HashSet<(u32, u32)> =
            u.trees.keys().filter(|id| id.2.as_deref().is_some_and(|l| !l.is_empty())).map(|id| (id.0, id.1)).collect();
        u.trees.retain(|id, _| id.2.as_deref() != Some("") || !named.contains(&(id.0, id.1)));
    }

    // Each MC/DC decision's paths count at its first key plus the path.
    let mut bases: Vec<(u64, &str, &TreeId, u64)> = Vec::new();
    for (file, u) in &files {
        for (id, t) in &u.trees {
            let (yes, no) = t.tree.paths();
            let base = coverage::mcdc_key(file, (id.0, id.1), id.2.as_deref());
            bases.push((base, file.as_str(), id, yes.saturating_add(no)));
        }
    }
    bases.sort_by_key(|b| b.0);
    let mut ran: HashMap<(&str, &TreeId), Vec<(u64, u64)>> = HashMap::new();
    for (&key, &n) in &counts {
        let at = bases.partition_point(|b| b.0 <= key);
        let Some(&(base, file, id, paths)) = at.checked_sub(1).and_then(|i| bases.get(i)) else { continue };
        if key.saturating_sub(base) < paths {
            ran.entry((file, id)).or_default().push((key.saturating_sub(base), n));
        }
    }

    let mut report: BTreeMap<&str, File> = BTreeMap::new();
    for (file, u) in &files {
        let f = report.entry(file.as_str()).or_default();
        f.lines = u.lines.iter().map(|&line| (line, count(coverage::key(file, line)))).collect();

        // MC/DC, and the right sides of `&&` and `||` its paths say ran.
        let mut sides: HashMap<(u32, u32, &Instance), [u64; 2]> = HashMap::new();
        let mut ordered: Vec<(&TreeId, &Tree)> = u.trees.iter().collect();
        ordered.sort_by_key(|(id, t)| (t.line, id.0, id.1, id.2.clone()));
        for (id, t) in ordered {
            let paths: Vec<(coverage::Path, u64)> = ran
                .get(&(file.as_str(), id))
                .map(|v| v.iter().map(|&(p, n)| (t.tree.decode(p), n)).collect())
                .unwrap_or_default();
            for (path, n) in &paths {
                for (span, rhs) in &path.sides {
                    let side = sides.entry((span.start, span.end, &id.2)).or_default();
                    let i = usize::from(!*rhs);
                    if let Some(count) = side.get_mut(i) {
                        *count = count.saturating_add(*n);
                    }
                }
            }
            let shown: Vec<bool> = (0..t.conditions.len()).map(|c| independent(&paths, c)).collect();
            f.decisions.0 = f.decisions.0.saturating_add(1);
            if shown.iter().all(|s| *s) {
                f.decisions.1 = f.decisions.1.saturating_add(1);
            }
            let suffix = instance_suffix(&id.2);
            for (c, s) in t.conditions.iter().zip(shown) {
                f.conditions.push((t.line, format!("'{c}' in '{}'{suffix}", t.text), s));
            }
        }

        let mut ordered: Vec<(&DecisionId, &Decision)> = u.decisions.iter().collect();
        ordered.sort_by_key(|(id, d)| (d.line, d.at, id.1, id.2, id.0, d.group.clone(), id.3.clone()));
        let mut block: usize = 0;
        let mut last_line = 0;
        for ((kind, start, end, instance), d) in ordered {
            block = if d.line == last_line { block.saturating_add(1) } else { 0 };
            last_line = d.line;
            let slot = |i| count(coverage::branch_key(file, *kind, (*start, *end), i, instance.as_deref()));
            let taken: Vec<u64> = match kind {
                // How often it was reached, less how often execution went on,
                // is how often `?` returned early, or the operation aborted.
                coverage::Kind::Try | coverage::Kind::Trap => vec![slot(0), slot(1).saturating_sub(slot(0))],
                coverage::Kind::Derived => {
                    let label = instance.as_deref().unwrap_or_default();
                    (0..d.branches).map(|i| count(coverage::derived::derived_key(file, (*start, *end), label, i))).collect()
                }
                coverage::Kind::And | coverage::Kind::Or if mcdc && sides_counted(u, *start, *end, instance) => {
                    sides.get(&(*start, *end, instance)).map_or(vec![0, 0], |s| s.to_vec())
                }
                _ => (0..d.branches).map(slot).collect(),
            };
            let reached = taken.iter().any(|&n| n > 0);
            // lcov wants a line under every branch. One no statement starts on
            // counts how often its decisions were reached.
            let times = taken.iter().fold(0u64, |a, &n| a.saturating_add(n));
            let rows = &mut f.lines;
            match rows.binary_search_by_key(&d.line, |l| l.0) {
                Ok(i) if !u.lines.contains(&d.line) => {
                    if let Some(row) = rows.get_mut(i) {
                        row.1 = row.1.max(times);
                    }
                }
                Ok(_) => {}
                Err(i) => rows.insert(i, (d.line, times)),
            }
            let suffix = instance_suffix(instance);
            for (i, n) in taken.into_iter().enumerate() {
                let branch = match (d.names.get(i), instance) {
                    (Some(name), Some(label)) => format!("{label}: {name}"),
                    _ => format!("{i}{suffix}"),
                };
                f.branches.push((d.line, block, branch, reached.then_some(n)));
            }
        }
    }
    report.retain(|_, f| !f.lines.is_empty() || !f.branches.is_empty());

    let mut lcov = String::new();
    let mut rows: Vec<Row> = Vec::new();
    for (file, f) in &report {
        lcov.push_str(&format!("TN:\nSF:{file}\n"));
        for (line, block, branch, taken) in &f.branches {
            let taken = taken.map_or("-".to_string(), |n| n.to_string());
            lcov.push_str(&format!("BRDA:{line},{block},{branch},{taken}\n"));
        }
        let hit = f.branches.iter().filter(|b| b.3.is_some_and(|n| n > 0)).count();
        lcov.push_str(&format!("BRF:{}\nBRH:{hit}\n", f.branches.len()));
        let shown = f.conditions.iter().filter(|c| c.2).count();
        if mcdc {
            // One group a line: lcov tells groups on a line apart by size alone.
            let mut i = 0;
            while let Some(&(line, _, _)) = f.conditions.get(i) {
                let group: Vec<&(usize, String, bool)> =
                    f.conditions.get(i..).unwrap_or_default().iter().take_while(|c| c.0 == line).collect();
                for (index, (_, expression, s)) in group.iter().enumerate() {
                    for sense in ["t", "f"] {
                        lcov.push_str(&format!("MCDC:{line},{},{sense},{},{index},{expression}\n", group.len(), u8::from(*s)));
                    }
                }
                i = i.saturating_add(group.len());
            }
            let found = f.conditions.len().saturating_mul(2);
            lcov.push_str(&format!("MCF:{found}\nMCH:{}\n", shown.saturating_mul(2)));
        }
        for (line, n) in &f.lines {
            lcov.push_str(&format!("DA:{line},{n}\n"));
        }
        let lines_hit = f.lines.iter().filter(|l| l.1 > 0).count();
        lcov.push_str(&format!("LF:{}\nLH:{lines_hit}\nend_of_record\n", f.lines.len()));
        rows.push(Row {
            name: file.to_string(),
            branches: (hit, f.branches.len()),
            conditions: (shown, f.conditions.len()),
            decisions: (f.decisions.1, f.decisions.0),
        });
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
    let sum = |a: (usize, usize), b: (usize, usize)| (a.0.saturating_add(b.0), a.1.saturating_add(b.1));
    let total = rows.iter().fold(Row { name: "total".to_string(), ..Row::default() }, |t, r| Row {
        name: t.name,
        branches: sum(t.branches, r.branches),
        conditions: sum(t.conditions, r.conditions),
        decisions: sum(t.decisions, r.decisions),
    });
    rows.push(total);
    let columns: Vec<Column> =
        if mcdc { vec![|r| r.branches, |r| r.conditions, |r| r.decisions] } else { vec![|r| r.branches] };
    let cells: Vec<Vec<String>> =
        rows.iter().map(|r| columns.iter().map(|c| cell(c(r))).collect()).collect();
    let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let widths: Vec<usize> =
        (0..columns.len()).map(|i| cells.iter().filter_map(|c| c.get(i)).map(String::len).max().unwrap_or(0)).collect();
    let title = if mcdc { "mc/dc coverage" } else { "branch coverage" };
    // The title heads the names, so under MC/DC they're as wide as it.
    let name_width = if mcdc { name_width.max(title.len().saturating_sub(2)) } else { name_width };
    if mcdc {
        let mut head = format!("{title:<w$}", w = name_width.saturating_add(2));
        for (column, w) in ["branches", "conditions", "decisions"].iter().zip(&widths) {
            head.push_str(&format!("  {column:>w$}"));
        }
        out(&head);
    } else {
        out(title);
    }
    for (r, cells) in rows.iter().zip(&cells) {
        let mut line = format!("  {:<name_width$}", r.name);
        for (c, w) in cells.iter().zip(&widths) {
            line.push_str(&format!("  {c:>w$}"));
        }
        out(&line);
    }
    out(&format!("lcov: {LCOV}"));
}

/// What a column of the summary reads from a row.
type Column = fn(&Row) -> (usize, usize);

/// A row of the summary.
#[derive(Default)]
struct Row {
    name: String,
    branches: (usize, usize),
    conditions: (usize, usize),
    decisions: (usize, usize),
}

/// `covered/total` and the share, or `-` for a share of nothing.
fn cell((covered, total): (usize, usize)) -> String {
    let fraction = format!("{covered}/{total}");
    let percent = if total == 0 { "-".to_string() } else { format!("{:.1}%", covered as f64 * 100.0 / total as f64) };
    format!("{fraction}  {percent:>6}")
}

/// Whether the `&&` or `||` at `start..end` sits in an MC/DC decision, whose
/// paths count its right side.
fn sides_counted(u: &Universe, start: u32, end: u32, instance: &Instance) -> bool {
    u.trees.iter().any(|(id, t)| {
        id.2 == *instance && id.0 <= start && end <= id.1 && contains_side(&t.tree, start, end)
    })
}

fn contains_side(tree: &coverage::Tree, start: u32, end: u32) -> bool {
    match tree {
        coverage::Tree::And(l, r, s) | coverage::Tree::Or(l, r, s) => {
            (s.start == start && s.end == end) || contains_side(l, start, end) || contains_side(r, start, end)
        }
        coverage::Tree::Not(x) => contains_side(x, start, end),
        coverage::Tree::Condition(_) => false,
    }
}

/// Whether two paths that ran show condition `c` deciding the outcome alone:
/// both evaluated it, to different values, with different outcomes, and every
/// other condition either the same or skipped by one of them. Unique-cause
/// MC/DC with masking.
fn independent(paths: &[(coverage::Path, u64)], c: usize) -> bool {
    paths.iter().enumerate().any(|(i, (a, _))| {
        paths.iter().skip(i.saturating_add(1)).any(|(b, _)| {
            let (Some(Some(x)), Some(Some(y))) = (a.values.get(c), b.values.get(c)) else { return false };
            x != y
                && a.outcome != b.outcome
                && a.values.iter().zip(&b.values).enumerate().all(|(k, (p, q))| {
                    k == c || p.is_none() || q.is_none() || p == q
                })
        })
    })
}
