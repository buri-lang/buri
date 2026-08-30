//! `Hermetic()` to `core/host/testing` — the migration, as a program.
//!
//! `core/testing/context` offers one assembled world and ten free functions
//! that build the doubles in it; `core/host/testing` offers one constructor per
//! double and configures it by method. Moving a corpus of a thousand call sites
//! between the two is mechanical, and a thousand hand edits is not — so this is
//! the edit, written once.
//!
//! # The discipline
//!
//! Two rules, both taken from the import flag-day (`reports/import-flag-day.md`),
//! and both about the same thing: **nothing here reads the shape of the text.**
//!
//! 1. **Every site comes from the parse tree.** A call is a [`Call`] node whose
//!    callee resolves to a name the file imported from `core/testing/context`,
//!    and the replacement is built out of the *source spans* of that call's
//!    arguments — so an argument keeps whatever the author wrote inside it,
//!    down to the whitespace. A file whose parse has an error, or whose local
//!    declarations shadow a name being migrated, is refused rather than
//!    guessed at.
//!
//! 2. **Every binding comes from the compiler.** `Hermetic` binds nine effects
//!    whether a test needs one or not, and the note's rule is that a test
//!    context names only what the function under test needs — so the new
//!    context cannot be read off the old one. It is *asked for*: each site
//!    starts at `context { }`, the package is compiled, and every
//!    `unsatisfied-bound` the checker reports names the effect the context is
//!    missing (`inference.rs`'s `Ty::Ctx(id) => ctx_type(id).has(tr)` is what
//!    makes that message exact). Add, recompile, repeat. The fixpoint is the
//!    minimal binding set, derived rather than inferred by eye.
//!
//! # What it cannot do yet
//!
//! `net()`, `.faults()` and `.calls()` do not exist (design slices E3, E5, E6),
//! so a site whose test needs `Net` has nothing to bind it to. Those sites are
//! left spelled `Hermetic()`, and [`Report::unmigrated`] names every one.
//! `core/testing/context` coexists with `core/host/testing` until E12 deletes
//! it, so a file may hold both imports and still compile.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use buri::diagnostics::{FileId, Span};
use buri::parsing::flat::{BlockId, CtxBodyId, ExprId, ExprView, PartView, StmtKind, Tree, NONE};
use buri::parsing::tree::{ImportClause, Item, Module};

// ---------------------------------------------------------------------------
// The two surfaces, side by side
// ---------------------------------------------------------------------------

/// The module the migration reads from.
pub const OLD_MODULE: &str = "core/testing/context/lib.buri";
/// The module it writes to.
pub const NEW_MODULE: &str = "core/host/testing/lib.buri";
/// Where the effect names a context binds come from.
pub const EFFECT_MODULE: &str = "core/effect/lib.buri";

/// The effects a test context can bind, in `core/effect`'s declaration order,
/// each with the `core/host/testing` constructor that supplies one.
///
/// `Net` is in the table with no constructor on purpose. `net()` is design
/// slice E3 and has not landed, so the honest thing for a site that needs
/// `Net` is to stay unmigrated and be counted — not to be quietly bound to
/// something else.
pub const EFFECTS: &[(&str, Option<&str>)] = &[
    ("Alloc", Some("alloc")),
    ("Stdout", Some("stdout")),
    ("Stderr", Some("stderr")),
    ("Stdin", Some("stdin")),
    ("Fs", Some("fs")),
    ("Net", None),
    ("Clock", Some("clock")),
    ("Rand", Some("rand")),
    ("Env", Some("env")),
    ("Proc", Some("proc")),
];

/// The widest context this migration can write: every effect in [`EFFECTS`]
/// that has a constructor.
///
/// It is what a site falls back to when the compiler cannot be made to name
/// the effects for it — see [`Site::approximated`]. It is *not* `Hermetic`'s
/// nine bindings: `Net` is missing from it, and `Proc` is in it.
pub fn over_approximation() -> BTreeSet<&'static str> {
    EFFECTS.iter().filter(|(_, c)| c.is_some()).map(|(e, _)| *e).collect()
}

/// The constructor call that binds one effect, or `None` where there is none.
pub fn constructor_for(effect: &str) -> Option<&'static str> {
    EFFECTS.iter().find(|(e, _)| *e == effect).and_then(|(_, c)| *c)
}

/// One old free function, and the constructor-plus-builders that replace it.
///
/// `methods` is applied to the call's arguments in order, so its length is the
/// old function's arity — a call with any other number of arguments is refused
/// rather than truncated.
struct Rewrite {
    ctor: &'static str,
    methods: &'static [&'static str],
}

/// `core/testing/context`'s free functions and what each becomes.
///
/// The pairs are the ones `reports/wave1-e1.md` and `reports/wave2-e2e4.md`
/// record as behaviour-preserving. `noNet` is absent because it has no twin,
/// and `readOnly` is absent because it is not a constructor: it wraps one, and
/// [`rewrite_call`] handles it as the method it became.
const FUNCTIONS: &[(&str, Rewrite)] = &[
    ("alloc", Rewrite { ctor: "alloc", methods: &[] }),
    ("captureOut", Rewrite { ctor: "stdout", methods: &[] }),
    ("captureErr", Rewrite { ctor: "stderr", methods: &[] }),
    ("stdin", Rewrite { ctor: "stdin", methods: &["lines"] }),
    ("stdinBytes", Rewrite { ctor: "stdin", methods: &["bytes"] }),
    ("data", Rewrite { ctor: "fs", methods: &[] }),
    ("files", Rewrite { ctor: "fs", methods: &["files"] }),
    ("filesBytes", Rewrite { ctor: "fs", methods: &["filesBytes"] }),
    ("clockAt", Rewrite { ctor: "clock", methods: &["at"] }),
    ("randSeed", Rewrite { ctor: "rand", methods: &["seed"] }),
    ("envOf", Rewrite { ctor: "env", methods: &["variables", "arguments"] }),
];

/// Method renames on a *value* rather than on a call: `capturedErr` reads back
/// a `TestStderr` and is spelled `captured` on the new one, and `advance` is
/// gone because the new `TestClock` advances through `sleepMillis`.
///
/// The third column is the constructor whose value the method belongs to, and
/// it is what makes the rename safe: a receiver is rewritten only where the
/// same block bound it to a call this migration itself rewrote to that
/// constructor. Nothing here renames a method on a name it did not watch being
/// bound.
const METHODS: &[(&str, &str, &str)] =
    &[("capturedErr", "captured", "stderr"), ("advance", "sleepMillis", "clock")];

// ---------------------------------------------------------------------------
// The plan for one file
// ---------------------------------------------------------------------------

/// A `Hermetic()` call, and the context that will replace it.
pub struct Site {
    /// Where `Hermetic()` is written, in the original text.
    pub span: Span,
    /// The index of the top-level declaration it is written inside. The
    /// compiler reports a missing binding at the *use* of the context, so this
    /// is what carries a diagnostic back to the site that has to change.
    pub item: usize,
    /// The effects the compiler asked for, once the fixpoint has run.
    pub effects: BTreeSet<&'static str>,
    /// Set when the binding set was not derived one effect at a time — see
    /// [`Report::approximated`].
    pub approximated: bool,
    /// Set when the site cannot be migrated at all. The `Hermetic()` call is
    /// left exactly as it was, and this says why.
    pub blocked: Option<String>,
}

/// A replacement of one source span.
enum Edit {
    /// Literal text, built from the spans of what it replaces.
    Text(Span, String),
    /// The `context { ... }` that replaces `Hermetic()` at site `n`. Rendered
    /// late, because its bindings are not known until the fixpoint has run.
    Context(Span, usize),
}

impl Edit {
    fn span(&self) -> Span {
        match self {
            Edit::Text(s, _) | Edit::Context(s, _) => *s,
        }
    }
}

/// Everything the migration knows about one file, before any of it is written.
pub struct Plan {
    /// Repository-relative, as a diagnostic names it.
    pub rel: String,
    pub original: String,
    pub sites: Vec<Site>,
    /// Why this file was not touched at all, if it was not.
    pub refused: Option<String>,
    /// What the file did with `core/testing/context` that this cannot move.
    pub notes: Vec<String>,
    edits: Vec<Edit>,
    /// The whole line the `core/testing/context` import is written on.
    old_import: Option<Span>,
    /// The names the file still needs from `core/testing/context` after the
    /// rewrite — `Hermetic`, where a site is blocked, and nothing else.
    old_names_kept: Vec<String>,
    /// The constructors the rewritten body calls, other than the ones the
    /// contexts themselves call.
    ctors_used: BTreeSet<&'static str>,
    /// The `core/effect` import already in the file: the whole line, and the
    /// names in its braces.
    effect_import: Option<(Span, Vec<String>)>,
    /// The item spans, in source order, for carrying a diagnostic to a site.
    items: Vec<Span>,
}

/// Whether a rendering is for the compiler or for the tree.
///
/// The probe form is *line-stable*: every context is one line whatever it
/// binds, both import lines are always written, and every constructor and
/// effect name is imported whether it is used or not. That is what lets the
/// fixpoint map a diagnostic's line back to a site once and keep the mapping
/// as bindings are added round after round. The final form writes the minimum:
/// only the names the file uses, and a context over more than one line when
/// one line would be too long.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Probe,
    Final,
}

/// The column a context stops fitting on one line at. The corpus is written to
/// a hundred columns, and so is the toolchain.
const WIDTH: usize = 100;

// ---------------------------------------------------------------------------
// Reading a file
// ---------------------------------------------------------------------------

/// Reads one file and works out what has to change in it.
///
/// Answers a plan with no sites and no edits for a file that never names
/// `core/testing/context` — the migration is then a no-op on it, which is what
/// makes a second run over a migrated tree a zero-diff run.
pub fn plan(rel: &str, text: &str) -> Plan {
    let mut plan = Plan {
        rel: rel.to_string(),
        original: text.to_string(),
        sites: Vec::new(),
        refused: None,
        notes: Vec::new(),
        edits: Vec::new(),
        old_import: None,
        old_names_kept: Vec::new(),
        ctors_used: BTreeSet::new(),
        effect_import: None,
        items: Vec::new(),
    };

    let file = FileId(0);
    let parsed = buri::parsing::parser::parse(text, file);
    if !parsed.errors.is_empty() {
        plan.refused = Some(format!("{} does not parse", rel));
        return plan;
    }
    let module = &parsed.module;
    let tree = &module.tree;

    // The names the file bound out of the old module, and the names it bound
    // out of `core/effect`. A namespace import (`import * as tc`) is read too:
    // `tc.files(...)` is the same call written the other way.
    let mut locals: BTreeMap<String, String> = BTreeMap::new();
    let mut namespace: Option<String> = None;
    let mut hermetic: Option<String> = None;
    for item in &module.items {
        let Item::Import(import) = item else { continue };
        if import.path == OLD_MODULE {
            plan.old_import = Some(whole_lines(text, import.span));
            match &import.clause {
                ImportClause::Named(specs) => {
                    for spec in specs {
                        let declared = tree.name(spec.name).to_string();
                        let local = tree.name(spec.local()).to_string();
                        if declared == "Hermetic" {
                            hermetic = Some(local);
                        } else {
                            locals.insert(local, declared);
                        }
                    }
                }
                ImportClause::Namespace(name) => namespace = Some(tree.name(*name).to_string()),
            }
        } else if import.path == EFFECT_MODULE {
            if let ImportClause::Named(specs) = &import.clause {
                let names = specs.iter().map(|s| tree.name(s.name).to_string()).collect();
                plan.effect_import = Some((whole_lines(text, import.span), names));
            }
        }
    }
    if plan.old_import.is_none() {
        return plan;
    }

    // A name this file declares for itself, or binds as a parameter, shadows
    // the import — and a rewrite that ignored that would change what the
    // program means. The migration does not try to resolve the shadow; it
    // refuses the file and says so, which is a list somebody reads.
    let watched: BTreeSet<&str> =
        locals.keys().map(String::as_str).chain(hermetic.as_deref()).collect();
    if let Some(clash) = shadowing(module, &watched) {
        plan.refused =
            Some(format!("{rel} binds `{clash}` locally as well as importing it; not rewritten"));
        return plan;
    }

    for item in &module.items {
        plan.items.push(item.span());
    }

    // The values this file binds to a constructor, so a method on one of them
    // can be renamed. Collected before the walk that uses it, because a `let`
    // and the read-back it feeds are two statements and the walk sees them in
    // order only by accident.
    let receivers = receivers_of(module, tree, &locals, namespace.as_deref());

    // Every call in the file, from the tree. `body_exprs` yields each
    // expression once, so a `Hermetic()` inside a lambda inside a match arm is
    // reached exactly like one at the top of a test.
    let mut sites = Vec::new();
    let mut edits = Vec::new();
    let mut ctors = BTreeSet::new();
    let mut notes = Vec::new();
    for (item_index, item) in module.items.iter().enumerate() {
        walk_item(tree, item, &mut |tree, id| {
            let view = tree.expr(id);
            // A method rename: `out.capturedErr()` where `out` was bound to
            // what `captureErr()` became.
            if let ExprView::Call { callee, .. } = view {
                if let ExprView::Field { base, name, name_span, .. } =
                    tree.expr(tree.strip_type_args(callee))
                {
                    if let ExprView::Ident { name: recv, .. } = tree.expr(base) {
                        if let Some(ctor) = receivers.get(recv) {
                            if let Some((_, new, on)) =
                                METHODS.iter().find(|(old, _, on)| *old == name && on == ctor)
                            {
                                let _ = on;
                                edits.push(Edit::Text(name_span, (*new).to_string()));
                            }
                        }
                    }
                }
            }
            let ExprView::Call { callee, args, span } = view else { return };
            let Some(name) = called_name(tree, callee, &locals, namespace.as_deref(), &hermetic)
            else {
                return;
            };
            if name == "Hermetic" {
                if !args.is_empty() {
                    notes.push(format!("{rel}: `Hermetic` called with arguments; left alone"));
                    return;
                }
                sites.push(Site {
                    span,
                    item: item_index,
                    effects: BTreeSet::new(),
                    approximated: false,
                    blocked: None,
                });
                edits.push(Edit::Context(span, sites.len() - 1));
                return;
            }
            match rewrite_call(tree, &name, args, &locals, namespace.as_deref(), &mut ctors) {
                Ok(text) => edits.push(Edit::Text(span, text)),
                Err(why) => notes.push(format!("{rel}: {why}")),
            }
        });
    }

    // `readOnly(files(...))` is one replacement built out of two calls, and the
    // inner call is also a site this walk saw. An edit written inside another
    // edit's span would splice twice over the same bytes, so the outer wins:
    // its replacement already holds the inner one, rewritten.
    edits.sort_by_key(|e| (e.span().start, std::cmp::Reverse(e.span().end)));
    let mut kept: Vec<Edit> = Vec::new();
    for edit in edits {
        if kept.last().is_none_or(|last| edit.span().start >= last.span().end) {
            kept.push(edit);
        }
    }

    plan.sites = sites;
    plan.edits = kept;
    plan.ctors_used = ctors;
    plan.notes = notes;
    plan
}

/// The declared name a call names, if the call is to something this file
/// imported from `core/testing/context`.
fn called_name(
    tree: &Tree,
    callee: ExprId,
    locals: &BTreeMap<String, String>,
    namespace: Option<&str>,
    hermetic: &Option<String>,
) -> Option<String> {
    match tree.expr(tree.strip_type_args(callee)) {
        ExprView::Ident { name, .. } => {
            if Some(name) == hermetic.as_deref() {
                return Some("Hermetic".to_string());
            }
            locals.get(name).cloned()
        }
        ExprView::Field { base, name, .. } => {
            let ExprView::Ident { name: ns, .. } = tree.expr(base) else { return None };
            (Some(ns) == namespace).then(|| name.to_string())
        }
        _ => None,
    }
}

/// The replacement for one call to an old free function.
///
/// Built out of the argument spans, so `files([("a", "b")])` keeps its list
/// exactly as written, comments and line breaks included.
fn rewrite_call(
    tree: &Tree,
    name: &str,
    args: &[ExprId],
    locals: &BTreeMap<String, String>,
    namespace: Option<&str>,
    ctors: &mut BTreeSet<&'static str>,
) -> Result<String, String> {
    // `readOnly(x)` folded into a method on what `x` became. It is the one old
    // function that takes a double rather than making one, so it is the one
    // whose replacement has to know what its argument is.
    if name == "readOnly" {
        let [inner] = args else {
            return Err("`readOnly` takes one argument".to_string());
        };
        let ExprView::Call { callee, args: inner_args, .. } = tree.expr(*inner) else {
            return Err("`readOnly` of something that is not a call has no method form".to_string());
        };
        let Some(inner_name) = called_name(tree, callee, locals, namespace, &None) else {
            return Err("`readOnly` of a value from outside this module has no method form"
                .to_string());
        };
        let Some((_, r)) = FUNCTIONS.iter().find(|(n, _)| *n == inner_name) else {
            return Err(format!("`readOnly({inner_name}(..))` has no method form"));
        };
        if r.ctor != "fs" {
            return Err(format!("`readOnly` of a `{}` is not a filesystem", r.ctor));
        }
        let inner_text = rewrite_call(tree, &inner_name, inner_args, locals, namespace, ctors)?;
        return Ok(format!("{inner_text}.readOnly()"));
    }
    let Some((_, r)) = FUNCTIONS.iter().find(|(n, _)| *n == name) else {
        return Err(format!("`{name}` has no `core/host/testing` twin; left alone"));
    };
    if args.len() != r.methods.len() {
        return Err(format!(
            "`{name}` called with {} arguments, not {}; left alone",
            args.len(),
            r.methods.len()
        ));
    }
    ctors.insert(r.ctor);
    let mut out = format!("{}()", r.ctor);
    for (method, arg) in r.methods.iter().zip(args) {
        out.push_str(&format!(".{method}({})", tree.source()[range(tree.span(*arg))].trim()));
    }
    Ok(out)
}

/// The local names bound to a value this migration rewrites to a constructor.
///
/// `let errors = captureErr();` puts `errors -> stderr` in the map, which is
/// what lets `errors.capturedErr()` become `errors.captured()` and nothing
/// else's `capturedErr` become anything.
fn receivers_of(
    module: &Module,
    tree: &Tree,
    locals: &BTreeMap<String, String>,
    namespace: Option<&str>,
) -> BTreeMap<String, &'static str> {
    let mut out = BTreeMap::new();
    for block in blocks_of(module, tree) {
        let data = tree.block(block);
        for stmt in tree.stmts_at(data.stmts_start, data.stmts_len) {
            if stmt.kind != StmtKind::Let || stmt.is_ctx {
                continue;
            }
            let (Some(pattern), Some(value)) = (tree.opt_pat(stmt.pattern), tree.opt(stmt.value))
            else {
                continue;
            };
            let ExprView::Call { callee, .. } = tree.expr(value) else { continue };
            let Some(name) = called_name(tree, callee, locals, namespace, &None) else { continue };
            let Some((_, r)) = FUNCTIONS.iter().find(|(n, _)| *n == name) else { continue };
            let bound = tree.source()[range(tree.pspan(pattern))].trim().to_string();
            if bound.chars().all(|c| c.is_alphanumeric() || c == '_') {
                out.insert(bound, r.ctor);
            }
        }
    }
    out
}

/// Every block in a file.
///
/// A declaration's own body is one the expression walk never yields — it starts
/// *inside* it — so it is added here, and the nested ones come from the walk.
fn blocks_of(module: &Module, tree: &Tree) -> Vec<BlockId> {
    let mut blocks: Vec<BlockId> = Vec::new();
    for item in &module.items {
        match item {
            Item::Fn(d) => blocks.extend(d.body),
            Item::Test(d) => blocks.push(d.body),
            Item::Impl(d) => blocks.extend(d.methods.iter().filter_map(|m| m.body)),
            Item::Trait(d) => blocks.extend(d.methods.iter().filter_map(|m| m.body)),
            _ => {}
        }
        walk_item(tree, item, &mut |tree, id| {
            if let ExprView::Block { block, .. } = tree.expr(id) {
                blocks.push(block);
            }
        });
    }
    blocks
}

/// The first watched name this file also binds for itself.
fn shadowing(module: &Module, watched: &BTreeSet<&str>) -> Option<String> {
    let tree = &module.tree;
    let mut declared: Vec<String> = Vec::new();
    for item in &module.items {
        match item {
            Item::Fn(d) => {
                declared.push(tree.name(d.name).to_string());
                declared.extend(d.params.iter().map(|p| tree.name(p.name).to_string()));
            }
            Item::Struct(d) => declared.push(tree.name(d.name).to_string()),
            Item::Enum(d) => declared.push(tree.name(d.name).to_string()),
            Item::TypeAlias(d) => declared.push(tree.name(d.name).to_string()),
            Item::Let(d) => declared.push(tree.name(d.name).to_string()),
            Item::Trait(d) => declared.push(tree.name(d.name).to_string()),
            Item::Context(d) => declared.push(tree.name(d.name).to_string()),
            Item::Impl(d) => {
                declared.extend(d.methods.iter().map(|m| tree.name(m.name).to_string()));
            }
            _ => {}
        }
    }
    // Every `let`, wherever it is written, and every lambda parameter.
    for block in blocks_of(module, tree) {
        let data = tree.block(block);
        for stmt in tree.stmts_at(data.stmts_start, data.stmts_len) {
            if let Some(p) = tree.opt_pat(stmt.pattern) {
                declared.push(tree.source()[range(tree.pspan(p))].trim().to_string());
            }
        }
    }
    for item in &module.items {
        walk_item(tree, item, &mut |tree, id| {
            if let ExprView::Lambda { params, .. } = tree.expr(id) {
                for p in params {
                    declared.push(tree.text(p.name).to_string());
                }
            }
        });
    }
    declared.into_iter().find(|d| watched.contains(d.as_str()))
}

// ---------------------------------------------------------------------------
// Writing a file
// ---------------------------------------------------------------------------

impl Plan {
    /// Whether this file has anything to do.
    pub fn touches_anything(&self) -> bool {
        self.refused.is_none() && (!self.edits.is_empty() || self.old_import.is_some())
    }

    /// The migrated text, in one of the two styles.
    ///
    /// Also answers, for [`Style::Probe`], where each of the file's top-level
    /// declarations landed: a byte range in the *rendered* text, which is what
    /// the fixpoint turns a diagnostic's line number into a site with.
    pub fn render(&self, style: Style) -> (String, Vec<(usize, usize)>) {
        if self.refused.is_some() {
            let items = self.items.iter().map(|s| (s.start as usize, s.end as usize)).collect();
            return (self.original.clone(), items);
        }

        // The two import lines, computed first: what they replace is a span
        // like any other edit, and rendering them here keeps the splice below
        // to one loop.
        let mut spliced: Vec<(Span, String)> = Vec::new();
        if let Some(span) = self.old_import {
            spliced.push((span, self.import_lines(style)));
        }
        if let Some((span, names)) = &self.effect_import {
            let mut merged: Vec<String> = names.clone();
            for effect in self.effects_named(style) {
                if !merged.iter().any(|n| n == effect) {
                    merged.push(effect.to_string());
                }
            }
            merged.sort_by_key(|n| effect_order(n));
            spliced.push((
                *span,
                format!("from \"{EFFECT_MODULE}\" import {{ {} }};\n", merged.join(", ")),
            ));
        }
        for edit in &self.edits {
            let text = match edit {
                Edit::Text(_, t) => t.clone(),
                Edit::Context(span, n) => self.context_text(*n, style, self.indent_of(*span)),
            };
            spliced.push((edit.span(), text));
        }
        spliced.sort_by_key(|(s, _)| (s.start, s.end));

        let mut out = String::with_capacity(self.original.len() + 512);
        // Where each original offset lands in the output, so the item ranges
        // below are ranges in what the compiler will actually read.
        let mut moved: Vec<(usize, isize)> = vec![(0, 0)];
        let mut at = 0usize;
        for (span, text) in &spliced {
            let start = span.start as usize;
            let end = span.end as usize;
            out.push_str(&self.original[at..start]);
            out.push_str(text);
            at = end;
            moved.push((end, out.len() as isize - end as isize));
        }
        out.push_str(&self.original[at..]);

        let map = |offset: usize| -> usize {
            let delta = moved.iter().rev().find(|(o, _)| *o <= offset).map_or(0, |(_, d)| *d);
            (offset as isize + delta).max(0) as usize
        };
        let items = self.items.iter().map(|s| (map(s.start as usize), map(s.end as usize))).collect();
        (out, items)
    }

    /// The import line, or lines, that replace the `core/testing/context` one.
    fn import_lines(&self, style: Style) -> String {
        let mut out = String::new();
        // The old module stays imported for as long as something still needs
        // it: a blocked site is still spelled `Hermetic()`. In the probe form
        // it always stays, because whether a site is blocked is one of the
        // things the fixpoint is still finding out, and the line count must
        // not move while it does.
        let keep_old = style == Style::Probe
            || !self.old_names_kept.is_empty()
            || self.sites.iter().any(|s| s.blocked.is_some());
        if keep_old {
            let mut names = self.old_names_kept.clone();
            if self.sites.iter().any(|s| s.blocked.is_some()) || style == Style::Probe {
                names.insert(0, "Hermetic".to_string());
            }
            names.dedup();
            out.push_str(&format!("from \"{OLD_MODULE}\" import {{ {} }};\n", names.join(", ")));
        }
        let ctors = self.ctors_named(style);
        if !ctors.is_empty() {
            out.push_str(&format!(
                "from \"{NEW_MODULE}\" import {{ {} }};\n",
                ctors.join(", ")
            ));
        }
        // A file with no `core/effect` import of its own gets one here, beside
        // the constructors, because the effect names in a context have to be
        // in scope and this is where the reader is already looking.
        if self.effect_import.is_none() {
            let effects = self.effects_named(style);
            if !effects.is_empty() {
                out.push_str(&format!(
                    "from \"{EFFECT_MODULE}\" import {{ {} }};\n",
                    effects.join(", ")
                ));
            }
        }
        out
    }

    /// The effect names the rendered file needs in scope.
    fn effects_named(&self, style: Style) -> Vec<&'static str> {
        if style == Style::Probe {
            return EFFECTS.iter().filter(|(_, c)| c.is_some()).map(|(e, _)| *e).collect();
        }
        let mut set: BTreeSet<&'static str> = BTreeSet::new();
        for site in &self.sites {
            if site.blocked.is_none() {
                set.extend(site.effects.iter().copied());
            }
        }
        let mut names: Vec<&'static str> = set.into_iter().collect();
        names.sort_by_key(|n| effect_order(n));
        names
    }

    /// The constructor names the rendered file calls.
    fn ctors_named(&self, style: Style) -> Vec<&'static str> {
        if style == Style::Probe {
            return EFFECTS.iter().filter_map(|(_, c)| *c).collect();
        }
        let mut set: BTreeSet<&'static str> = self.ctors_used.clone();
        for site in &self.sites {
            if site.blocked.is_some() {
                continue;
            }
            for effect in &site.effects {
                if let Some(c) = constructor_for(effect) {
                    set.insert(c);
                }
            }
        }
        let mut names: Vec<&'static str> = set.into_iter().collect();
        names.sort_by_key(|n| ctor_order(n));
        names
    }

    /// The `context { ... }` for one site.
    fn context_text(&self, n: usize, style: Style, indent: String) -> String {
        let Some(site) = self.sites.get(n) else { return "Hermetic()".to_string() };
        if site.blocked.is_some() {
            return "Hermetic()".to_string();
        }
        let mut effects: Vec<&&str> = site.effects.iter().collect();
        effects.sort_by_key(|n| effect_order(n));
        let bindings: Vec<String> = effects
            .iter()
            .map(|e| format!("{e}: {}()", constructor_for(e).unwrap_or("alloc")))
            .collect();
        if bindings.is_empty() {
            return "context { }".to_string();
        }
        let one_line = format!("context {{ {} }}", bindings.join(", "));
        if style == Style::Probe || indent.len() + one_line.len() + "let ctx = ;".len() <= WIDTH {
            return one_line;
        }
        let mut out = String::from("context {\n");
        for binding in &bindings {
            out.push_str(&format!("{indent}  {binding},\n"));
        }
        out.push_str(&format!("{indent}}}"));
        out
    }

    /// The whitespace at the start of the line a span is written on.
    fn indent_of(&self, span: Span) -> String {
        let start = self.original[..span.start as usize].rfind('\n').map_or(0, |i| i + 1);
        self.original[start..span.start as usize]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect()
    }
}

fn effect_order(name: &str) -> usize {
    EFFECTS.iter().position(|(e, _)| *e == name).unwrap_or(EFFECTS.len())
}

fn ctor_order(name: &str) -> usize {
    EFFECTS.iter().position(|(_, c)| *c == Some(name)).unwrap_or(EFFECTS.len())
}

/// A span grown to whole lines, including the newline that ends the last one.
///
/// An import is replaced line by line rather than token by token: the
/// replacement is sometimes two imports where there was one, and a span that
/// stopped at the semicolon would leave the second on the first's line.
fn whole_lines(text: &str, span: Span) -> Span {
    let start = text[..span.start as usize].rfind('\n').map_or(0, |i| i + 1);
    let end = text[span.end as usize..]
        .find('\n')
        .map_or(text.len(), |i| span.end as usize + i + 1);
    Span::new(span.file, start, end)
}

fn range(span: Span) -> std::ops::Range<usize> {
    span.start as usize..span.end as usize
}

// ---------------------------------------------------------------------------
// Walking the tree
// ---------------------------------------------------------------------------

/// Every expression under one declaration, each exactly once.
fn walk_item(tree: &Tree, item: &Item, f: &mut impl FnMut(&Tree, ExprId)) {
    match item {
        Item::Fn(d) => {
            if let Some(body) = d.body {
                walk_block(tree, body, f);
            }
        }
        Item::Test(d) => walk_block(tree, d.body, f),
        Item::Let(d) => walk_expr(tree, d.value, f),
        Item::Impl(d) => {
            for m in &d.methods {
                if let Some(body) = m.body {
                    walk_block(tree, body, f);
                }
            }
        }
        Item::Trait(d) => {
            for m in &d.methods {
                if let Some(body) = m.body {
                    walk_block(tree, body, f);
                }
            }
        }
        Item::Context(d) => walk_ctx_body(tree, d.body, f),
        _ => {}
    }
}

fn walk_block(tree: &Tree, block: BlockId, f: &mut impl FnMut(&Tree, ExprId)) {
    let data = tree.block(block);
    for stmt in tree.stmts_at(data.stmts_start, data.stmts_len).to_vec() {
        if let Some(value) = tree.opt(stmt.value) {
            walk_expr(tree, value, f);
        }
    }
    if let Some(tail) = tree.opt(data.tail) {
        walk_expr(tree, tail, f);
    }
}

fn walk_ctx_body(tree: &Tree, body: CtxBodyId, f: &mut impl FnMut(&Tree, ExprId)) {
    let data = tree.ctx_body(body);
    if let Some(spread) = tree.opt(data.spread) {
        walk_expr(tree, spread, f);
    }
    for bind in tree.bindings_at(data.bind_start, data.bind_len).to_vec() {
        if let Some(value) = tree.opt(bind.value) {
            walk_expr(tree, value, f);
        }
    }
}

/// The recursive half. Every variant of [`ExprView`] is named, so a new one is
/// a compile error here rather than a site this quietly stops visiting.
fn walk_expr(tree: &Tree, id: ExprId, f: &mut impl FnMut(&Tree, ExprId)) {
    f(tree, id);
    match tree.expr(id) {
        ExprView::Int { .. }
        | ExprView::Float { .. }
        | ExprView::Str { .. }
        | ExprView::Char { .. }
        | ExprView::Bool { .. }
        | ExprView::Ident { .. }
        | ExprView::SelfValue { .. }
        | ExprView::Ctx { .. }
        | ExprView::DotVariant { .. }
        | ExprView::Unit { .. }
        | ExprView::Error { .. } => {}
        ExprView::Template { parts, .. } => {
            for part in parts.iter().copied() {
                if let PartView::Hole(e) = tree.part(part) {
                    walk_expr(tree, e, f);
                }
            }
        }
        ExprView::Array { elems, .. } | ExprView::Tuple { elems, .. } => {
            for e in elems.iter().copied() {
                walk_expr(tree, e, f);
            }
        }
        ExprView::Block { block, .. } => walk_block(tree, block, f),
        ExprView::If { cond, then, else_, .. } => {
            walk_expr(tree, cond, f);
            walk_block(tree, then, f);
            if else_ != ExprId(NONE) {
                walk_expr(tree, else_, f);
            }
        }
        ExprView::Match { scrutinee, arms, .. } => {
            walk_expr(tree, scrutinee, f);
            for arm in arms.iter().copied() {
                if let Some(guard) = tree.opt(arm.guard) {
                    walk_expr(tree, guard, f);
                }
                if let Some(body) = tree.opt(arm.body) {
                    walk_expr(tree, body, f);
                }
            }
        }
        ExprView::ContextExpr { body, .. } => walk_ctx_body(tree, body, f),
        ExprView::Lambda { body, .. } => walk_expr(tree, body, f),
        ExprView::Unary { operand, .. } => walk_expr(tree, operand, f),
        ExprView::Binary { lhs, rhs, .. } => {
            walk_expr(tree, lhs, f);
            walk_expr(tree, rhs, f);
        }
        ExprView::Field { base, .. }
        | ExprView::TupleIndex { base, .. }
        | ExprView::Try { base, .. }
        | ExprView::Generic { base, .. } => walk_expr(tree, base, f),
        ExprView::Call { callee, args, .. } => {
            walk_expr(tree, callee, f);
            for a in args.iter().copied() {
                walk_expr(tree, a, f);
            }
        }
        ExprView::Index { base, index, .. } => {
            walk_expr(tree, base, f);
            walk_expr(tree, index, f);
        }
        ExprView::StructLit { head, spread, fields, .. } => {
            if let Some(h) = head {
                walk_expr(tree, h, f);
            }
            if let Some(s) = spread {
                walk_expr(tree, s, f);
            }
            for field in fields.iter().copied() {
                if let Some(v) = tree.opt(field.value) {
                    walk_expr(tree, v, f);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Asking the compiler what each context has to bind
// ---------------------------------------------------------------------------

/// What one migration run did.
pub struct Report {
    /// Every file the run planned, whether or not it changed.
    pub plans: Vec<Plan>,
    /// Sites whose bindings the compiler named, one effect at a time.
    pub derived: usize,
    /// Sites given the whole of [`over_approximation`] because the compiler
    /// could not be made to name them: more than one `Hermetic()` in one
    /// declaration, so a diagnostic cannot say which of them is short.
    pub approximated: usize,
    /// Sites left spelled `Hermetic()`, with the reason.
    /// `file:line`, and why.
    pub unmigrated: Vec<(String, String)>,
    /// How many compile rounds the fixpoint took.
    pub rounds: usize,
}

/// One diagnostic, as `--error-format=json` prints it.
struct Reported {
    code: String,
    message: String,
    file: String,
    line: usize,
    severity: String,
}

/// Runs the fixpoint: plan every file, then compile the packages over and over
/// until no context is missing a binding.
///
/// `run` is how the caller compiles: it is handed the whole rendered tree, one
/// `(relative path, text)` per file, and answers the diagnostics. Keeping the
/// compiler behind a closure is what lets the unit tests exercise the fixpoint
/// without a repository on disk.
pub fn migrate(
    files: &[(String, String)],
    mut run: impl FnMut(&[(String, String)]) -> String,
) -> Report {
    let mut plans: Vec<Plan> = files.iter().map(|(rel, text)| plan(rel, text)).collect();

    // Round zero: every context empty, every name imported, every context on
    // one line. The item ranges come from this rendering and stay true for
    // every round after it, because nothing that follows moves a line.
    let mut rounds = 0;
    let mut converged = false;
    // A round can only ever add to a site, and a site holds at most one
    // binding per effect, so the fixpoint cannot run longer than that plus the
    // round that finds nothing left to add.
    for _ in 0..EFFECTS.len() + 2 {
        rounds += 1;
        let drawn: Vec<(String, Vec<(usize, usize)>)> =
            plans.iter().map(|p| p.render(Style::Probe)).collect();
        let rendered: Vec<(String, String)> = plans
            .iter()
            .zip(&drawn)
            .map(|(p, (text, _))| (p.rel.clone(), text.clone()))
            .collect();
        let items: Vec<&Vec<(usize, usize)>> = drawn.iter().map(|(_, i)| i).collect();
        let output = run(&rendered);
        let reported = parse_diagnostics(&output);

        let mut learned = false;
        let mut unattributed: Vec<String> = Vec::new();
        for d in &reported {
            if d.severity != "error" {
                continue;
            }
            // `unsatisfied-bound` names the effect the context is short of.
            // `unbound-effect` — the monomorphiser's, for a method called
            // straight on a context — does not name one, so a site that draws
            // it takes the whole approximation. Anything else is not this
            // migration's business and is a failure, loudly.
            let effect = missing_effect(&d.code, &d.message);
            if effect.is_none() && d.code != "unbound-effect" {
                unattributed.push(format!("{}:{} [{}] {}", d.file, d.line, d.code, d.message));
                continue;
            }
            let Some(index) = plans.iter().position(|p| p.rel == d.file) else {
                unattributed.push(format!("{} is not a file this run planned", d.file));
                continue;
            };
            let offset = line_offset(&rendered[index].1, d.line);
            let owning: Vec<usize> = (0..plans[index].sites.len())
                .filter(|n| {
                    let item = plans[index].sites[*n].item;
                    items[index].get(item).is_some_and(|(s, e)| offset >= *s && offset <= *e)
                })
                .collect();
            match (owning.as_slice(), effect) {
                ([], _) => unattributed.push(format!(
                    "{}:{} {} — no `Hermetic()` in the declaration it is reported in",
                    d.file, d.line, d.message
                )),
                ([one], Some(effect)) if constructor_for(effect).is_some() => {
                    learned |= plans[index].sites[*one].effects.insert(effect);
                }
                ([one], Some(effect)) => {
                    // An effect `core/host/testing` cannot build yet. The site
                    // stays spelled `Hermetic()` and the report names it.
                    let site = &mut plans[index].sites[*one];
                    if site.blocked.is_none() {
                        site.blocked = Some(format!(
                            "needs `{effect}`, which `core/host/testing` has no constructor for \
                             yet"
                        ));
                        learned = true;
                    }
                }
                (many, _) => {
                    // Either the diagnostic did not name an effect, or more
                    // than one `Hermetic()` is written in the declaration it
                    // points into and the compiler cannot say which of them is
                    // short. Every candidate takes the whole approximation,
                    // and the report counts it.
                    for n in many {
                        let site = &mut plans[index].sites[*n];
                        if !site.approximated {
                            site.approximated = true;
                            site.effects = over_approximation();
                            learned = true;
                        }
                    }
                }
            }
        }

        assert!(
            unattributed.is_empty(),
            "the migration cannot place these diagnostics:\n  {}",
            unattributed.join("\n  ")
        );
        let errors = reported.iter().filter(|d| d.severity == "error").count();
        if errors == 0 {
            converged = true;
            break;
        }
        assert!(
            learned,
            "the migration stopped learning with {errors} errors left:\n{output}"
        );
    }
    assert!(converged, "the migration did not reach a fixpoint in {rounds} rounds");

    let mut report = Report {
        derived: 0,
        approximated: 0,
        unmigrated: Vec::new(),
        rounds,
        plans: Vec::new(),
    };
    for plan in &plans {
        for site in &plan.sites {
            match (&site.blocked, site.approximated) {
                (Some(why), _) => {
                    let line = plan.original[..site.span.start as usize].lines().count();
                    report.unmigrated.push((format!("{}:{line}", plan.rel), why.clone()));
                }
                (None, true) => report.approximated += 1,
                (None, false) => report.derived += 1,
            }
        }
    }
    report.plans = plans;
    report
}

/// The effect a diagnostic says a context does not bind.
///
/// `unsatisfied-bound` names it — the message is built from the trait, and a
/// context that does not have the effect is exactly what makes `satisfies`
/// answer no. `unbound-effect` does not name one, so a site that draws it
/// takes the whole approximation instead; it is reported here as `None` and
/// handled by the caller.
fn missing_effect(code: &str, message: &str) -> Option<&'static str> {
    if code != "unsatisfied-bound" || !message.starts_with("`a context` does not satisfy ") {
        return None;
    }
    let named = message.rsplit('`').nth(1)?;
    EFFECTS.iter().find(|(e, _)| *e == named).map(|(e, _)| *e)
}

/// The byte offset of the start of a one-based line.
fn line_offset(text: &str, line: usize) -> usize {
    let mut at = 0;
    for _ in 1..line {
        match text[at..].find('\n') {
            Some(i) => at += i + 1,
            None => return text.len(),
        }
    }
    at
}

/// `--error-format=json`, one object per line, read with the toolchain's own
/// JSON reader rather than a second one.
fn parse_diagnostics(output: &str) -> Vec<Reported> {
    let mut out = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(value) = buri::json::parse(line) else { continue };
        let text = |path: &str| {
            value.at(path).and_then(|v| v.as_str()).unwrap_or_default().to_string()
        };
        out.push(Reported {
            code: text("code"),
            message: text("message"),
            severity: text("severity"),
            file: text("location.file"),
            line: value.at("location.line").and_then(|v| v.as_u32()).unwrap_or_default() as usize,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Driving it over a repository on disk
// ---------------------------------------------------------------------------

/// The manifests that are `.buri` files without being Buri sources.
const MANIFESTS: &[&str] = &["BUILD.buri", "REPO.buri"];

/// Every Buri source under `dir`, repository-relative, sorted.
pub fn sources_under(root: &Path, dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect(&root.join(dir), root, &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "buri")
            && !path.file_name().is_some_and(|n| MANIFESTS.iter().any(|m| n == *m))
        {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.display().to_string());
            }
        }
    }
}
