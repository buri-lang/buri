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
use crate::compiler::semantics::types::{self, FnId, FuncIdx, Prim};
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

/// Where each run's directory of counts goes, from the repository root.
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
    /// Under MC/DC, how it reads as conditions ([`Conditions`]); `None` where
    /// an MC/DC decision of its own already holds them.
    conditions: Option<Conditions>,
}

/// The conditions an MC/DC report holds for a decision that is not an
/// `&&`/`||` chain: the decision's text, and either one condition that is shown
/// once every outcome was taken, or one per outcome, each shown once it was
/// taken (`match` arms, a derived decision's branches).
struct Conditions {
    whole: String,
    per_outcome: Vec<String>,
    /// The one condition's text, if it is one.
    single: Option<String>,
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
    /// Every `&&` and `||` a tree in [`Universe::trees`] holds.
    operators: BTreeSet<TreeId>,
}

/// A `buri test --coverage` run: where its processes write their counts, and
/// what the suites it compiled hold, by file. [`begin`] starts one and
/// [`Run::report`] ends it.
pub struct Run {
    raw: PathBuf,
    mcdc: bool,
    /// Whether the standard library counts too ([`STD`]).
    std: bool,
    /// By file. Suites are compiled on many threads at once.
    files: Mutex<BTreeMap<String, Universe>>,
}

/// The directory of the run under way, for [`raw_dir`].
static CURRENT: Mutex<Option<PathBuf>> = Mutex::new(None);

fn raw() -> MutexGuard<'static, Option<PathBuf>> {
    CURRENT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts a coverage run with an empty directory for the counts. Every
/// process `buri` starts from here on is told where it is.
///
/// The directory is named by this process's id, so runs never mix counts
/// (buri-lang/buri#302). A killed run's directory goes once its process has.
pub fn begin(root: &Path, mcdc: bool) -> Result<Run, String> {
    let runs = root.join(RAW);
    for left in std::fs::read_dir(&runs)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !running(&e.file_name()))
    {
        let _ = std::fs::remove_dir_all(left.path()).or_else(|_| std::fs::remove_file(left.path()));
    }
    let dir = runs.join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let std = std::env::var_os(STD).is_some_and(|v| v == "1");
    *raw() = Some(dir.clone());
    Ok(Run {
        raw: dir,
        mcdc,
        std,
        files: Mutex::new(BTreeMap::new()),
    })
}

/// Whether a run's directory, named by its process id, belongs to a process
/// that is still running.
fn running(name: &std::ffi::OsStr) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    // SAFETY: signal 0 is no signal, only the check that the process exists.
    let pid = name
        .to_str()
        .and_then(|n| n.parse::<u32>().ok())
        .and_then(|n| i32::try_from(n).ok());
    pid.is_some_and(|pid| unsafe { kill(pid, 0) } == 0)
}

/// The directory a process writes its counts to, during a coverage run.
pub fn raw_dir() -> Option<PathBuf> {
    raw().clone()
}

/// Text from the source, on one line, for an lcov record.
///
/// A parenthesised operand keeps the span of what is inside it, so `(n + 1) /
/// d` spans from `n` on. The text takes in the parentheses an end of the span
/// left open.
fn text(map: &SourceMap, span: Span) -> String {
    let source = &map.get(span.file).text;
    let (mut start, mut end) = (span.start as usize, span.end as usize);
    let (open, close) = unbalanced(source.get(start..end).unwrap_or(""));
    // The source parsed, so each parenthesis is there to find.
    for _ in 0..close {
        start = source
            .get(..start)
            .and_then(|s| s.rfind('('))
            .unwrap_or(start);
    }
    for _ in 0..open {
        end = source
            .get(end..)
            .and_then(|s| s.find(')'))
            .map_or(end, |at| end.saturating_add(at).saturating_add(1));
    }
    let raw = source.get(start..end).unwrap_or("");
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How many `(` in `s` close after it, and how many `)` opened before it,
/// leaving out those in string and character literals.
fn unbalanced(s: &str) -> (usize, usize) {
    let (mut open, mut close) = (0usize, 0usize);
    let mut quote = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(_), '\\') => {
                chars.next();
            }
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => open = open.saturating_add(1),
            (None, ')') => match open.checked_sub(1) {
                Some(fewer) => open = fewer,
                None => close = close.saturating_add(1),
            },
            (None, _) => {}
        }
    }
    (open, close)
}

/// Probes `program`, and notes every line and decision the user code
/// `analysis` checked holds, called or not.
///
/// User code is ordinary source read from disk. The standard library, the
/// bundled platforms, generated modules and test sources never are.
impl Run {
    pub fn instrument(
        &self,
        analysis: &Analysis,
        map: &SourceMap,
        program: &mut monomorphize::Program,
    ) {
        let (mcdc, std) = (self.mcdc, self.std);
        let user: HashSet<FileId> = analysis
            .loaded
            .modules
            .iter()
            .filter(|m| {
                (matches!(m.role, Role::Source | Role::Entry) && m.disk.is_some())
                    || (std && m.role == Role::Std)
            })
            .map(|m| m.file)
            .collect();
        let name_of = |span: Span| {
            user.contains(&span.file)
                .then(|| map.get(span.file).name.as_str())
        };
        let line_of = |span: Span| {
            name_of(span).map(|name| (name, map.get(span.file).line_col(span.start).0))
        };
        // Where a span in a body the run counts is: its file's name and line.
        let place = |span: Span| {
            let file = map.get(span.file);
            (file.name.as_str(), file.line_col(span.start).0)
        };
        let tables = &analysis.checked.tables;
        let mut files: BTreeMap<String, Universe> = BTreeMap::new();
        let mut lines: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();

        // Under MC/DC, each instantiation of a generic function is its own, and a
        // function that was built reads its decisions off the code the probes go
        // in: a `==` that's a call when checked can be none once built.
        let mut instances: std::collections::HashMap<FuncIdx, String> =
            std::collections::HashMap::new();
        let mut built: HashMap<FnId, Vec<FuncIdx>> = HashMap::new();
        if mcdc {
            for (&slot, (id, targs)) in &program.instances {
                let info = tables.fn_info(*id);
                if !user.contains(&info.span.file) {
                    continue;
                }
                built.entry(*id).or_default().push(slot);
                if targs.is_empty() {
                    continue;
                }
                // One type argument per generic, the impl's first: monomorphization
                // builds an instance from exactly those.
                let label: Vec<String> = info
                    .generics
                    .iter()
                    .zip(targs)
                    .map(|(g, t)| format!("{} = {}", g.name, types::show(tables, None, &[], t)))
                    .collect();
                instances.insert(slot, label.join(", "));
            }
        }
        // A branching built-in, named by its module and name (`coverage::BUILTINS`).
        let builtin_of = |id: FnId| {
            let info = tables.fn_info(id);
            analysis
                .loaded
                .modules
                .owner(info.module)
                .and_then(|m| coverage::builtin(&m.path, &info.name))
        };
        let by_slot: HashMap<FuncIdx, coverage::Builtin> = program
            .instances
            .iter()
            .filter_map(|(&slot, (id, _))| builtin_of(*id).map(|how| (slot, how)))
            .collect();
        let builtins = |called: coverage::Called| match called {
            coverage::Called::Decl(id) => builtin_of(id),
            coverage::Called::Func(f) => by_slot.get(&f).copied(),
        };
        let mut note = |body: &typed::Expr,
                        instance: Instance,
                        traps: Option<&coverage::TrapCall<'_>>| {
            coverage::decisions(
                body,
                traps,
                &analysis.checked.consts,
                &builtins,
                &mut |d, e| {
                    let at = Span {
                        start: d.at,
                        ..d.span
                    };
                    let (name, line) = place(at);
                    let id = (d.kind, d.span.start, d.span.end, instance.clone());
                    let decision = Decision {
                        line,
                        at: d.at,
                        branches: d.branches,
                        names: Vec::new(),
                        group: Default::default(),
                        conditions: (traps.is_some() && !d.tree).then(|| conditions_of(map, &d, e)),
                    };
                    files
                        .entry(name.to_string())
                        .or_default()
                        .decisions
                        .insert(id, decision);
                },
            );
            if traps.is_some() {
                coverage::trees(body, &mut |tree| {
                    let span = tree.span();
                    let (name, line) = place(span);
                    let mut spans = Vec::new();
                    tree.conditions(&mut spans);
                    let conditions: Vec<String> = spans.iter().map(|s| text(map, *s)).collect();
                    let id = (span.start, span.end, instance.clone());
                    let mut operators = Vec::new();
                    tree.operators(&mut operators);
                    let held = files.entry(name.to_string()).or_default();
                    for o in operators {
                        held.operators.insert((o.start, o.end, instance.clone()));
                    }
                    let whole = render(&tree, &conditions, &mut 0).0;
                    let t = Tree {
                        line,
                        tree,
                        text: whole,
                        conditions,
                    };
                    files
                        .entry(name.to_string())
                        .or_default()
                        .trees
                        .insert(id, t);
                });
            }
        };
        // As `coverage::aborts` judges a built call: a `core/bits` shift or rotate, or an
        // integer type's `divide` or `remainder`.
        let checked_traps = |e: &typed::Expr| match &e.kind {
            typed::ExprKind::CallFn {
                func: typed::Callee { id, .. },
                ..
            } => {
                let info = tables.fn_info(*id);
                let module = analysis
                    .loaded
                    .modules
                    .owner(info.module)
                    .map(|m| m.path.as_str());
                let integer = info.self_ty.is_some_and(|c| {
                    Prim::all()
                        .iter()
                        .any(|p| p.is_integer() && tables.prim_id(*p) == c)
                });
                if !info.intrinsic {
                    None
                } else if module == Some("core/bits")
                    && (info.name.starts_with("shift") || info.name.starts_with("rotate"))
                {
                    Some(coverage::Aborts::shift(&info.name))
                } else {
                    (integer && matches!(info.name.as_str(), "divide" | "remainder"))
                        .then_some(coverage::Aborts::Divide)
                }
            }
            _ => None,
        };
        for (id, body) in analysis.checked.bodies.iter() {
            if !user.contains(&body.expr.span.file) {
                continue;
            }
            coverage::sites(&body.expr, &mut |span| {
                let (name, line) = place(span);
                lines.entry(name.to_string()).or_default().insert(line);
            });
            if !mcdc {
                note(&body.expr, None, None);
                continue;
            }
            if built.contains_key(&id) {
                continue;
            }
            let instance = (!tables.fn_info(id).generics.is_empty()).then(String::new);
            note(&body.expr, instance, Some(&checked_traps));
        }
        if mcdc {
            let mono_traps = |e: &typed::Expr| match &e.kind {
                typed::ExprKind::CallFunc { func: f, .. } => {
                    coverage::aborts(program.funcs.get(*f))
                }
                _ => None,
            };
            for &slot in built.values().flatten() {
                if let Some(body) = program.funcs.get(slot).body() {
                    note(body, instances.get(&slot).cloned(), Some(&mono_traps));
                }
            }
        }

        let i64_ty = tables.prim(Prim::I64);
        let bool_ty = tables.prim(Prim::Bool);
        coverage::instrument(program, &i64_ty, &|span| {
            line_of(span).map(|(n, l)| coverage::key(n, l))
        });
        let file_of = |span: Span| name_of(span).map(str::to_string);
        let options = coverage::Options {
            i64_ty,
            bool_ty,
            file_of: &file_of,
            mcdc: mcdc.then_some(&instances),
            tables: mcdc.then_some(tables),
            builtins: &builtins,
        };
        for d in coverage::branches(program, &options) {
            let (name, line) = place(d.span);
            let id = (
                coverage::Kind::Derived,
                d.span.start,
                d.span.end,
                Some(d.label.clone()),
            );
            let group = (d.group, d.order);
            let decision = Decision {
                line,
                at: d.span.start,
                branches: d.branches.len(),
                conditions: mcdc.then(|| Conditions {
                    whole: d.label.clone(),
                    per_outcome: d.branches.clone(),
                    single: None,
                }),
                names: d.branches,
                group,
            };
            files
                .entry(name.to_string())
                .or_default()
                .decisions
                .insert(id, decision);
        }
        for (file, set) in lines {
            files.entry(file).or_default().lines = set;
        }
        let mut held = self.files.lock().unwrap_or_else(PoisonError::into_inner);
        for (file, u) in files {
            let mine = held.entry(file).or_default();
            mine.lines.extend(u.lines);
            mine.decisions.extend(u.decisions);
            mine.trees.extend(u.trees);
            mine.operators.extend(u.operators);
        }
    }
}

/// The longest a condition's or decision's text runs in a record.
const LONGEST: usize = 80;

/// Source text on one line, cut short if it runs long.
fn short(map: &SourceMap, span: Span) -> String {
    let full = text(map, span);
    if full.chars().count() <= LONGEST {
        return full;
    }
    let cut: String = full.chars().take(LONGEST.saturating_sub(3)).collect();
    format!("{cut}...")
}

/// How the decision `d`, made by `e`, reads as conditions: a `match` has one
/// per arm; any other, the condition of an `if` or a guard, a `?`, an abort
/// point, a built-in's call or the right side of an `&&` a tail call keeps out
/// of MC/DC, has one.
fn conditions_of(map: &SourceMap, d: &coverage::Decision, e: &typed::Expr) -> Conditions {
    match &e.kind {
        typed::ExprKind::Match { scrutinee, arms } if d.kind == coverage::Kind::Match => {
            Conditions {
                whole: format!("match {}", short(map, scrutinee.span)),
                per_outcome: arms.iter().map(|a| short(map, a.pattern.span)).collect(),
                single: None,
            }
        }
        typed::ExprKind::If { cond, .. } => single(short(map, cond.span)),
        _ => single(short(map, e.span)),
    }
}

fn single(text: String) -> Conditions {
    Conditions {
        whole: text.clone(),
        per_outcome: Vec::new(),
        single: Some(text),
    }
}

/// A decision's text, from its conditions': its span leaves out a `(` that
/// opens it. With how tightly it binds: `||`, `&&`, or a condition.
fn render(tree: &coverage::Tree, conditions: &[String], next: &mut usize) -> (String, u8) {
    let operand = |t: &coverage::Tree, at_least: u8, next: &mut usize| {
        let (text, binds) = render(t, conditions, next);
        if binds < at_least {
            format!("({text})")
        } else {
            text
        }
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

/// Every count the processes wrote, summed by key.
fn counts(raw: &Path) -> HashMap<u64, u64> {
    let mut counts: HashMap<u64, u64> = HashMap::new();
    for text in std::fs::read_dir(raw)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
    {
        for (key, count) in text.lines().filter_map(record) {
            let n = counts.entry(key).or_insert(0);
            *n = n.saturating_add(count);
        }
    }
    counts
}

/// A `key count` line, or `None` for one a process cut short.
fn record(line: &str) -> Option<(u64, u64)> {
    let (key, count) = line.split_once(' ')?;
    Some((key.parse().ok()?, count.parse().ok()?))
}

/// One file's report.
#[derive(Default)]
struct File {
    /// Each line's count.
    lines: BTreeMap<usize, u64>,
    /// `(line, block, branch, count)`, with `None` for a decision nothing
    /// reached.
    branches: Vec<(usize, usize, String, Option<u64>)>,
    /// `(line, offset, expression, shown)`, a condition each. In source order.
    conditions: Vec<(usize, u32, String, bool)>,
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

/// Where a test holds `buri test --coverage` before it adds up the counts, or
/// kills it there ([`crate::hold`]).
#[cfg(feature = "test-hooks")]
static MERGING: crate::hold::Point = crate::hold::Point::new("merging");

impl Run {
    /// Every count the processes wrote, added up and put against the lines and
    /// decisions the suites hold: a row per file through `out`, and [`LCOV`].
    pub fn report(self, root: &Path, out: &mut dyn FnMut(&str)) {
        *raw() = None;
        let Run {
            raw, mcdc, files, ..
        } = self;
        let mut files = files.into_inner().unwrap_or_else(PoisonError::into_inner);
        #[cfg(feature = "test-hooks")]
        MERGING.raised();
        let counts = counts(&raw);
        let _ = std::fs::remove_dir_all(&raw);
        // And the directory of runs, once no other run is using it.
        let _ = raw.parent().map(std::fs::remove_dir);
        let count = |key: u64| counts.get(&key).copied().unwrap_or(0);

        // A generic function one suite instantiated and another didn't is only the
        // first suite's instantiations.
        for u in files.values_mut() {
            let named: HashSet<(coverage::Kind, u32, u32)> = u
                .decisions
                .keys()
                .filter(|id| id.3.as_deref().is_some_and(|l| !l.is_empty()))
                .map(|id| (id.0, id.1, id.2))
                .collect();
            u.decisions.retain(|id, _| {
                id.3.as_deref() != Some("") || !named.contains(&(id.0, id.1, id.2))
            });
            let named: HashSet<(u32, u32)> = u
                .trees
                .keys()
                .filter(|id| id.2.as_deref().is_some_and(|l| !l.is_empty()))
                .map(|id| (id.0, id.1))
                .collect();
            u.trees
                .retain(|id, _| id.2.as_deref() != Some("") || !named.contains(&(id.0, id.1)));
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
        let mut ran: HashMap<(&str, &TreeId), Vec<u64>> = HashMap::new();
        for &key in counts.keys() {
            let at = bases.partition_point(|b| b.0 <= key);
            let Some(&(base, file, id, paths)) = at.checked_sub(1).and_then(|i| bases.get(i))
            else {
                continue;
            };
            if key.saturating_sub(base) < paths {
                ran.entry((file, id))
                    .or_default()
                    .push(key.saturating_sub(base));
            }
        }
        // In path order, not `counts`' hash order, so the report runs the same way
        // every time.
        for paths in ran.values_mut() {
            paths.sort_unstable();
        }

        let mut report: BTreeMap<&str, File> = BTreeMap::new();
        for (file, u) in &files {
            let f = report.entry(file.as_str()).or_default();
            f.lines = u
                .lines
                .iter()
                .map(|&line| (line, count(coverage::key(file, line))))
                .collect();

            let mut ordered: Vec<(&TreeId, &Tree)> = u.trees.iter().collect();
            ordered.sort_by_key(|(id, t)| (t.line, id.0, id.1, id.2.clone()));
            for (id, t) in ordered {
                let paths: Vec<coverage::Path> = ran
                    .get(&(file.as_str(), id))
                    .map(|v| v.iter().map(|&p| t.tree.decode(p)).collect())
                    .unwrap_or_default();
                let shown: Vec<bool> = (0..t.conditions.len())
                    .map(|c| independent(&paths, c))
                    .collect();
                f.decisions.0 = f.decisions.0.saturating_add(1);
                if shown.iter().all(|s| *s) {
                    f.decisions.1 = f.decisions.1.saturating_add(1);
                }
                let suffix = instance_suffix(&id.2);
                for (c, s) in t.conditions.iter().zip(shown) {
                    f.conditions
                        .push((t.line, id.0, format!("'{c}' in '{}'{suffix}", t.text), s));
                }
            }

            let mut ordered: Vec<(&DecisionId, &Decision)> = u.decisions.iter().collect();
            ordered.sort_by_key(|(id, d)| {
                (
                    d.line,
                    d.at,
                    id.1,
                    id.2,
                    id.0,
                    d.group.clone(),
                    id.3.clone(),
                )
            });
            let mut block: usize = 0;
            let mut last_line = 0;
            for ((kind, start, end, instance), d) in ordered {
                block = if d.line == last_line {
                    block.saturating_add(1)
                } else {
                    0
                };
                last_line = d.line;
                let slot = |i| {
                    count(coverage::branch_key(
                        file,
                        *kind,
                        (*start, *end),
                        i,
                        instance.as_deref(),
                    ))
                };
                let taken: Vec<u64> = match kind {
                    // How often it was reached, less how often execution went on,
                    // is how often `?` returned early, or the operation aborted.
                    coverage::Kind::Try | coverage::Kind::Trap => {
                        vec![slot(0), slot(1).saturating_sub(slot(0))]
                    }
                    coverage::Kind::Derived => {
                        let label = instance.as_deref().unwrap_or_default();
                        (0..d.branches)
                            .map(|i| {
                                count(coverage::derived::derived_key(
                                    file,
                                    (*start, *end),
                                    label,
                                    i,
                                ))
                            })
                            .collect()
                    }
                    _ => (0..d.branches).map(slot).collect(),
                };
                let reached = taken.iter().any(|&n| n > 0);
                // Under MC/DC every decision is conditions: those a tree holds are
                // its own, and the rest are read off the branches' counts.
                let held = d.conditions.as_ref().filter(|_| {
                    !(matches!(kind, coverage::Kind::And | coverage::Kind::Or)
                        && u.operators.contains(&(*start, *end, instance.clone())))
                });
                if let (true, Some(c)) = (mcdc, held) {
                    let named: Vec<(String, bool)> = match &c.single {
                        Some(one) => vec![(one.clone(), taken.iter().all(|&n| n > 0))],
                        None => c
                            .per_outcome
                            .iter()
                            .cloned()
                            .zip(taken.iter().map(|&n| n > 0))
                            .collect(),
                    };
                    let suffix = match kind {
                        coverage::Kind::Derived => String::new(),
                        _ => instance_suffix(instance),
                    };
                    f.decisions.0 = f.decisions.0.saturating_add(1);
                    if named.iter().all(|c| c.1) {
                        f.decisions.1 = f.decisions.1.saturating_add(1);
                    }
                    for (condition, shown) in named {
                        f.conditions.push((
                            d.line,
                            d.at,
                            format!("'{condition}' in '{}'{suffix}", c.whole),
                            shown,
                        ));
                    }
                }
                // lcov wants a line under every branch. One no statement starts on
                // counts how often its decisions were reached.
                let times = taken.iter().fold(0u64, |a, &n| a.saturating_add(n));
                if !u.lines.contains(&d.line) {
                    let row = f.lines.entry(d.line).or_default();
                    *row = (*row).max(times);
                }
                let suffix = instance_suffix(instance);
                for (i, n) in taken.into_iter().enumerate() {
                    let branch = match (d.names.get(i), instance) {
                        (Some(name), Some(label)) => format!("{label}: {name}"),
                        _ => format!("{i}{suffix}"),
                    };
                    f.branches
                        .push((d.line, block, branch, reached.then_some(n)));
                }
            }
        }

        // Conditions of one line sit together, in the order they're written.
        for f in report.values_mut() {
            f.conditions.sort_by_key(|c| (c.0, c.1));
        }
        let mut lcov = String::new();
        let mut rows: Vec<Row> = Vec::new();
        for (file, f) in &report {
            lcov.push_str(&format!("TN:\nSF:{file}\n"));
            for (line, block, branch, taken) in &f.branches {
                let taken = taken.map_or("-".to_string(), |n| n.to_string());
                lcov.push_str(&format!("BRDA:{line},{block},{branch},{taken}\n"));
            }
            let hit = f
                .branches
                .iter()
                .filter(|b| b.3.is_some_and(|n| n > 0))
                .count();
            lcov.push_str(&format!("BRF:{}\nBRH:{hit}\n", f.branches.len()));
            let shown = f.conditions.iter().filter(|c| c.3).count();
            if mcdc {
                // One group a line: lcov tells groups on a line apart by size alone.
                let mut i = 0;
                while let Some(&(line, ..)) = f.conditions.get(i) {
                    let group: Vec<&(usize, u32, String, bool)> = f
                        .conditions
                        .get(i..)
                        .unwrap_or_default()
                        .iter()
                        .take_while(|c| c.0 == line)
                        .collect();
                    for (index, (_, _, expression, s)) in group.iter().enumerate() {
                        for sense in ["t", "f"] {
                            lcov.push_str(&format!(
                                "MCDC:{line},{},{sense},{},{index},{expression}\n",
                                group.len(),
                                u8::from(*s)
                            ));
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
            let lines_hit = f.lines.values().filter(|n| **n > 0).count();
            lcov.push_str(&format!(
                "LF:{}\nLH:{lines_hit}\nend_of_record\n",
                f.lines.len()
            ));
            rows.push(Row {
                name: file.to_string(),
                branches: (hit, f.branches.len()),
                conditions: (shown, f.conditions.len()),
                decisions: (f.decisions.1, f.decisions.0),
            });
        }
        // `begin` made the directory, for the counts.
        let path = root.join(LCOV);
        if let Err(e) = std::fs::write(&path, lcov) {
            out(&format!("coverage: cannot write {LCOV}: {e}"));
            return;
        }
        out("");
        if rows.is_empty() {
            out("coverage: no suite loaded any of this repository's own source");
            return;
        }
        let sum = |a: (usize, usize), b: (usize, usize)| {
            (a.0.saturating_add(b.0), a.1.saturating_add(b.1))
        };
        let total = rows.iter().fold(
            Row {
                name: "total".to_string(),
                ..Row::default()
            },
            |t, r| Row {
                name: t.name,
                branches: sum(t.branches, r.branches),
                conditions: sum(t.conditions, r.conditions),
                decisions: sum(t.decisions, r.decisions),
            },
        );
        rows.push(total);
        let columns: Vec<Column> = if mcdc {
            vec![|r| r.branches, |r| r.conditions, |r| r.decisions]
        } else {
            vec![|r| r.branches]
        };
        let cells: Vec<Vec<String>> = rows
            .iter()
            .map(|r| columns.iter().map(|c| cell(c(r))).collect())
            .collect();
        let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
        let widths: Vec<usize> = (0..columns.len())
            .map(|i| {
                cells
                    .iter()
                    .filter_map(|c| c.get(i))
                    .map(String::len)
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let title = if mcdc {
            "mc/dc coverage"
        } else {
            "branch coverage"
        };
        // The title heads the names, so under MC/DC they're as wide as it.
        let name_width = if mcdc {
            name_width.max(title.len().saturating_sub(2))
        } else {
            name_width
        };
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
    let percent = if total == 0 {
        "-".to_string()
    } else {
        format!("{:.1}%", covered as f64 * 100.0 / total as f64)
    };
    format!("{fraction}  {percent:>6}")
}

/// Whether two paths that ran show condition `c` deciding the outcome alone:
/// both evaluated it, to different values, with different outcomes, and every
/// other condition either the same or skipped by one of them. Unique-cause
/// MC/DC with masking.
fn independent(paths: &[coverage::Path], c: usize) -> bool {
    paths.iter().enumerate().any(|(i, a)| {
        paths.iter().skip(i.saturating_add(1)).any(|b| {
            let (Some(Some(x)), Some(Some(y))) = (a.values.get(c), b.values.get(c)) else {
                return false;
            };
            x != y
                && a.outcome != b.outcome
                && a.values
                    .iter()
                    .zip(&b.values)
                    .enumerate()
                    .all(|(k, (p, q))| k == c || p.is_none() || q.is_none() || p == q)
        })
    })
}
