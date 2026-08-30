//! The JavaScript emitter.
//!
//! Turns the middle end's tree into a JavaScript AST. Evaluation order is
//! fully specified (SPEC 8.2), and this is where that matters most: an
//! expression that needs statements to compute is hoisted into the enclosing
//! statement list rather than wrapped in a closure, except where hoisting
//! would move work across a branch that might not be taken.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "every operation here counts positions in a tree already held in \
              memory — a slot within an aggregate, an argument within a call, \
              the next temporary's number — so no operand exceeds that tree's \
              size and every result is one step past a position that exists. \
              The one subtraction, narrowing a folded literal to its type's \
              width, runs only after the width has been bounded"
)]

use crate::compiler::backend::Profile;
use crate::compiler::backend::js::javascript::{self, BinOp, Expr, Stmt, UnOp, VarKind};
use crate::compiler::semantics::typed::{self, ExprKind, PatKind, PrimOp};
use crate::compiler::semantics::types::{LocalId, Prim, Tables, Ty, TyDef};
use crate::compiler::middle::monomorphize::{self, Desc, FuncKind, Program, ProgramRoots};
use crate::compiler::middle::rc;
use crate::diagnostics::Invariant as _;
use crate::hash::{Map as HashMap, Set as HashSet};

pub struct Output {
    pub stmts: Vec<Stmt>,
    /// Names the minifier must not rename.
    pub roots: Vec<String>,
    pub missing_intrinsics: Vec<String>,
}

/// The state that belongs to the *one function* being emitted.
///
/// These were three fields on `Gen` beside the whole-program ones, cleared by
/// three hand-written lines at each of the two places a new function starts —
/// and `merged_group` cleared two of the three. Entering a function now
/// replaces the whole value, so there is no partially-reset state: a leftover
/// `names` map would emit another function's variable names.
struct FnState {
    /// The current function's local names.
    names: HashMap<LocalId, String>,
    temp: usize,
    /// What a `Continue` in this function's body rebinds.
    loops: LoopSlots,
    /// Where this body's sharing marks go. See [`Marks`].
    marks: Marks,
}

/// Where one function body's sharing marks go, keyed by the **address** of the
/// node they belong to.
///
/// `middle::rc` numbers a body's nodes in pre-order and keys its plan by that
/// number; this emitter walks the same tree by reference and restructures it
/// as it goes, so a second copy of the numbering here would be a second copy
/// of a rule that has to agree exactly. One pre-order pass records where each
/// number's node *is*, and every lookup after that is the node the emitter is
/// already holding. The tree does not move while a function is emitted, so the
/// address is a name for the node and nothing else.
#[derive(Default)]
struct Marks {
    /// Mark the value this node produces: `None` unconditionally, `Some(l)`
    /// only if `l` — the parent this projection reads out of — is itself
    /// marked.
    value: HashMap<usize, Option<LocalId>>,
    /// Locals to mark before this node's own code runs.
    locals: HashMap<usize, Vec<LocalId>>,
    /// Nodes whose `locals` have already been emitted, so that a node reached
    /// through `tail` and then again through `expr` marks once.
    done: HashSet<usize>,
}

/// A node's address, which is what [`Marks`] is keyed by.
fn node_key(e: &typed::Expr) -> usize {
    std::ptr::from_ref(e) as usize
}

impl FnState {
    /// The state for a function whose locals are named positionally.
    fn for_locals(locals: &[typed::Local]) -> FnState {
        let mut names = HashMap::default();
        for (li, l) in locals.iter().enumerate() {
            names.insert(LocalId(li as u32), local_name(li, &l.name));
        }
        FnState { names, temp: 0, loops: LoopSlots::default(), marks: Marks::default() }
    }
}

/// The parameters a `Continue` assigns, and the name of the dispatch
/// parameter a multi-entry loop selects its entry with.
///
/// `middle::tail_calls` decided that this function loops and what its loop
/// rebinds; all that is left here is the two names, and the dispatch parameter
/// is the one thing the tree deliberately does not carry — an entry is a
/// control index rather than a value of any Buri type, so the backend spells
/// it (`typed::ExprKind::Continue`).
#[derive(Default)]
struct LoopSlots {
    targets: Vec<String>,
    which: Option<String>,
}

pub struct Gen<'a> {
    pub(crate) program: &'a Program,
    pub(crate) tables: &'a Tables,
    /// State for the function currently being emitted.
    func: FnState,
    pub(crate) missing: Vec<String>,
    runtime: Vec<String>,
    defensive_aborts: bool,
    /// Declarations a helper needed to emit alongside what it returned.
    extra: Vec<Stmt>,
    /// Constant aggregates, shared rather than rebuilt. See `Gen::intern`.
    consts: Vec<Expr>,
    /// Printed form of each interned aggregate, so an identical one anywhere
    /// in the program reaches the same declaration.
    const_index: HashMap<String, usize>,
    /// Set while emitting a `context { .. }`, where nothing may be shared.
    in_context: bool,
    /// Where a second reference to a value comes into existence, from
    /// `middle::rc` run with `Options::sharing`. Empty for a `Gen` that is only
    /// being asked which intrinsics exist.
    sharing: rc::Plan,
    /// One row per `Program::funcs` slot: whether it is printed `async`. See
    /// [`waiting_functions`]. Empty for a `Gen` that is only being asked which
    /// intrinsics exist, where nothing is emitted and so nothing is awaited.
    waits: Vec<bool>,
}

/// The runtime's exports, so a missing one is a build error rather than a
/// `ReferenceError` at run time.
fn runtime_names() -> Vec<String> {
    let mut out = Vec::new();
    let src = crate::compiler::backend::js::runtime_source();
    let bytes = src.as_bytes();
    // A byte scan: the runtime is mostly ASCII but its comments are not, so
    // this never slices the string at a position it has not checked.
    for i in 0..bytes.len() {
        let Some(rest) = bytes.get(i..) else { continue };
        for kw in [b"function $".as_slice(), b"const $".as_slice(), b"let $".as_slice()] {
            if rest.starts_with(kw) {
                // The keyword ends with the `$` that begins the name.
                let start = i.saturating_add(kw.len()).saturating_sub(1);
                let mut j = start;
                while bytes
                    .get(j)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
                {
                    j = j.saturating_add(1);
                }
                if let Some(Ok(name)) = bytes.get(start..j).map(std::str::from_utf8) {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}

impl<'a> Gen<'a> {
    /// A generator over one program.
    ///
    /// Split out of `generate` so that the intrinsic probe can have one without
    /// emitting anything — asking "does this backend implement this key" has to
    /// go through the same `Gen::intrinsic` the emission does, or the answer is
    /// a second implementation of the question and the two drift.
    fn over(program: &'a Program, tables: &'a Tables, profile: Profile) -> Gen<'a> {
        Gen {
            program,
            tables,
            func: FnState {
                names: HashMap::default(),
                temp: 0,
                loops: LoopSlots::default(),
                marks: Marks::default(),
            },
            missing: Vec::new(),
            runtime: runtime_names(),
            defensive_aborts: profile.defensive_aborts(),
            extra: Vec::new(),
            consts: Vec::new(),
            const_index: HashMap::default(),
            in_context: false,
            sharing: rc::Plan::default(),
            waits: Vec::new(),
        }
    }

    /// Whether the function in slot `index` is one this backend prints
    /// `async`, and so whether a call to it has to be awaited.
    ///
    /// "Is this printed `async`" and "is this call awaited" are the same
    /// question asked at the two ends of one call, so both read this. See
    /// [`waiting_functions`] for where the answer comes from.
    fn parks(&self, index: usize) -> bool {
        self.waits.get(index).copied().unwrap_or(false)
    }

    /// The marks for one body, from the plan's node numbers and one pre-order
    /// pass over the tree.
    ///
    /// Only the increments are read. A release is what a garbage collector
    /// does, and this backend emits none — the plan's `DecRef` sites, its
    /// reuse pairings and its ownership column are all for the native branch.
    /// The marks for one body: the plan's node numbers, resolved to nodes by
    /// one pre-order pass.
    ///
    /// **Which types carry a mark is not decided here.** `rc::sharing` runs its
    /// classifier over the whole program and emits a site only where a value
    /// can reach a list; a second opinion in the backend would be a second
    /// rule to keep in step, and the direction it could get wrong is an
    /// aliased list nobody copied.
    fn marks_for(&self, f: &monomorphize::Func, plan: &rc::FuncPlan) -> Marks {
        let mut marks = Marks::default();
        let Some(body) = f.body() else { return marks };
        // Whether each node is a local read, which is what decides between the
        // two ways a `Target::Local` is spelled.
        let mut nodes: Vec<(usize, bool)> = Vec::new();
        rc::preorder(body, &mut |_, e| {
            nodes.push((node_key(e), matches!(e.kind, ExprKind::Local(_))));
        });
        for site in &plan.sites {
            if site.op != rc::RcOp::IncRef {
                continue;
            }
            let Some(&(key, is_local_read)) = nodes.get(site.node.index()) else { continue };
            match site.target {
                // A projection: the value this node produces is a second
                // reference to something a parent still holds.
                rc::Target::Node(_) => {
                    let parent =
                        plan.inherits.iter().find(|(n, _)| *n == site.node).map(|(_, l)| *l);
                    marks.value.insert(key, parent);
                }
                // A local read that keeps the local alive past it — and, where
                // the node is not the read itself, a lambda's capture or an
                // arm's payload, which are marked as statements before it.
                rc::Target::Local(l) if is_local_read => {
                    marks.value.entry(key).or_insert(None);
                }
                rc::Target::Local(l) => marks.locals.entry(key).or_default().push(l),
            }
        }
        marks
    }
}

/// The intrinsic keys this backend has no body for, asked of the program
/// rather than accumulated as a side effect of a failed emission.
///
/// `Gen::intrinsic` is the one place that knows, so this calls it — with the
/// arguments the emitter would have passed, because several intrinsics decide
/// on arity. Nothing it builds is kept.
pub fn unimplemented_intrinsics(program: &Program, tables: &Tables) -> Vec<String> {
    let mut g = Gen::over(program, tables, Profile::Debug);
    let mut out = Vec::new();
    for f in &program.funcs {
        let FuncKind::Intrinsic(key) = &f.kind else { continue };
        g.func = FnState::for_locals(&f.locals);
        let args: Vec<Expr> =
            f.params.iter().map(|p| Expr::ident(g.local_name_of(p))).collect();
        if g.intrinsic(key, &args, f).is_none() {
            out.push(key.clone());
        }
    }
    out
}

pub fn generate(program: &Program, tables: &Tables, profile: Profile) -> Output {
    let mut g = Gen::over(program, tables, profile);
    // The ownership half of `middle::rc`, which this branch of the pipeline
    // runs for its increments alone. MEMORY.md §5.5.
    g.sharing = rc::sharing(program);
    g.waits = waiting_functions(program);

    let mut stmts = Vec::new();
    // A program that reaches a node module by name needs `require`, which an
    // ES module does not have. Emitted only when it is needed, so a browser
    // artifact never names `node:module` — and *guarded*, because a program
    // may reach `Stdout.writeBytes`, which every platform grants including
    // `WEB`, where there is no such module to resolve. A static `import` is
    // resolved before a line of the artifact runs and would fail the whole
    // page; a dynamic one behind `typeof process` is never reached there, and
    // `$fs`/`$fsp` answer the absence with a refusal that names it.
    //
    // The `await` is module top level, where it is available because every
    // artifact this backend writes is an ES module.
    if needs_require(program) {
        stmts.push(Stmt::Raw(
            "const $require=typeof process===\"undefined\"?undefined:\
             (await import(\"node:module\")).createRequire(import.meta.url);"
                .to_string(),
        ));
    }
    // The runtime, one declaration at a time so that dead code elimination can
    // drop what a program does not reach. It is hand-written JavaScript, so it
    // is compacted by the tokenizer in `javascript::strip` rather than by the AST
    // printer.
    for (name, src) in javascript::split_declarations(crate::compiler::backend::js::runtime_source()) {
        let src = if profile.pretty() { src } else { javascript::strip(&src) };
        stmts.push(Stmt::RawDecl { name, src });
    }
    // Where the shared constants go, once the bodies below have said which
    // ones they need.
    let runtime_end = stmts.len();

    // The stylesheet, as one string the compiler wrote. `mount` puts it in the
    // document; `ui/testing` reads it back. It is an assignment rather than a
    // declaration so that the runtime owns the binding — and a statement that
    // is not a declaration is a dead-code root, which is what keeps the
    // binding alive for the two readers above.
    //
    // Emitted only when there is a sheet, so a program that styles nothing
    // carries no trace of the styling machinery at all.
    if !program.stylesheet.is_empty() {
        stmts.push(Stmt::Expr(Expr::Assign {
            target: Box::new(Expr::ident("$ui_sheet")),
            value: Box::new(Expr::Str(program.stylesheet.clone())),
        }));
    }

    // The two halves of the user-interface runtime that nothing in the program
    // *names*, filled in here or left out entirely.
    //
    // Both are reached through a hole in `runtime.js` rather than by name, and
    // the assignment below is the only thing that ever fills one. A statement
    // that is not a declaration is a dead-code root, so the assignment is what
    // keeps the machinery alive — and its absence is what lets `eliminate_dead`
    // take the machinery out. The alternative is what was there before: a call
    // by name, which dead-code elimination cannot argue with, so a program
    // whose styles are all static shipped the 3.5 KB inline lowering and one
    // with no design tokens shipped the 1.7 KB of theme resolution.
    //
    // Asked of the program rather than of the platform: a `JS` test binary
    // rendering a headless tree is as much a user interface as a `WEB` output,
    // and a `WEB` output with no computed style is as free of the tier as any
    // other program.
    for (flag, hole, filling) in [
        (program.inline_styles, "$tree_declare_hook", "$tree_declare"),
        (program.themes, "$ui_theme_hook", "$ui_theme_install"),
    ] {
        if flag {
            stmts.push(Stmt::Expr(Expr::Assign {
                target: Box::new(Expr::ident(hole)),
                value: Box::new(Expr::ident(filling)),
            }));
        }
    }

    // Type descriptors, for the structural operations `derive` stands for.
    // Declared empty first and filled afterwards, because a recursive type's
    // descriptor names itself and a mutually recursive pair names each other.
    for i in 0..program.descriptors.len() {
        stmts.push(Stmt::Var {
            kind: VarKind::Const,
            name: descriptor_name(i),
            init: Some(Expr::Array(Vec::new())),
        });
    }
    for (i, d) in program.descriptors.iter().enumerate() {
        let Expr::Array(items) = g.descriptor(d) else { continue };
        stmts.push(Stmt::Expr(Expr::call(
            Expr::member(Expr::ident(descriptor_name(i)), "push"),
            items,
        )));
    }

    // One equality per type, in place of the runtime walker.
    //
    // `$eq` is generic: it asks `typeof`/`Array.isArray` of every element it
    // reaches, and because every type in the program shares that one call site
    // it is the only megamorphic code in an artifact. Compiled at the type,
    // a two-field struct is `a[0]===b[0]&&a[1]===b[1]` — no dispatch left at
    // all. `eliminate_dead` drops the ones nothing reaches.
    for i in 0..program.descriptors.len() {
        if let Some(decl) = g.eq_decl(i) {
            stmts.push(decl);
        }
    }

    for (fi, f) in program.funcs.iter().enumerate() {
        g.func = FnState::for_locals(&f.locals);
        // `async` iff this instantiation waits. A suspending intrinsic's own
        // wrapper is included: no `await` lands in it, but it returns what
        // the host hands back, and an `async` function returns that through
        // the promise its caller is already awaiting.
        let is_async = g.parks(fi);
        debug_assert!(
            !is_async || g.sharing.funcs.get(fi).is_none_or(|p| p.can_park),
            "{} is printed `async` but `rc`'s `can_park` column says it cannot \
             park; the column is the over-approximation and this set is inside \
             it, never the other way round",
            f.debug_name
        );
        if let Some(marks) = g.sharing.funcs.get(fi).map(|plan| g.marks_for(f, plan)) {
            g.func.marks = marks;
        }
        let mut params: Vec<String> =
            f.params.iter().map(|p| g.local_name_of(p)).collect();

        // A body the middle end turned into a loop takes one more parameter
        // than the tree says: the entry a caller enters at. Nothing else about
        // emitting it is special, which is the point of the rewrite.
        if let Some(entries) = loop_entries(f) {
            g.func.loops.targets = params.clone();
            if entries > 1 {
                g.func.loops.which = Some(DISPATCH.to_string());
                params.insert(0, DISPATCH.to_string());
            }
        }

        let body = match &f.kind {
            FuncKind::Body(e) => {
                let mut out = Vec::new();
                g.tail(e, &mut out);
                out
            }
            FuncKind::Intrinsic(key) => {
                let args: Vec<Expr> = params.iter().map(|p| Expr::ident(p.clone())).collect();
                match g.intrinsic(key, &args, f) {
                    Some(e) => vec![Stmt::Return(Some(e))],
                    None => {
                        g.missing.push(key.clone());
                        vec![Stmt::Throw(Expr::call(
                            Expr::ident("$abort"),
                            vec![Expr::Str(format!("missing intrinsic {key}"))],
                        ))]
                    }
                }
            }
            // Requested and never built. Calling one is a compiler bug; it
            // used to `return 0`, which is a value of whatever type the
            // caller expected.
            FuncKind::Unbuilt => vec![Stmt::Throw(Expr::call(
                Expr::ident("$abort"),
                vec![Expr::Str(format!("{} was never built", f.debug_name))],
            ))],
        };
        stmts.push(Stmt::Func { name: f.symbol.clone(), params, body, is_async });
    }

    // The shared constants the bodies above reached for, spliced in ahead of
    // them. Function declarations hoist, so their order relative to these does
    // not matter; the entry epilogue is a statement and does run, which is why
    // they are inserted rather than appended.
    let consts: Vec<Stmt> = g
        .consts
        .iter()
        .enumerate()
        .map(|(i, e)| Stmt::Var {
            kind: VarKind::Const,
            name: const_name(i),
            init: Some(e.clone()),
        })
        .collect();
    stmts.splice(runtime_end..runtime_end, consts);

    let mut roots = Vec::new();
    if let ProgramRoots::Main(entry) = &program.roots {
        let entry = entry.index();
        // `.Ok(())` exits 0. `.Err(msg)` prints `msg` to stderr and exits 1.
        let sym = program
            .funcs
            .get(entry)
            .or_ice("the entry point is one of the functions monomorphization emitted")
            .symbol
            .clone();
        roots.push(sym.clone());
        // Awaited only when the entry itself parks, so an artifact whose
        // `main` never waits is the same bytes it was before this transform.
        // The epilogue is module top level, where `await` is available
        // because every artifact this backend writes is an ES module.
        let wait = if g.parks(entry) { "await " } else { "" };
        stmts.push(Stmt::Raw(format!(
            "try{{const r={wait}{sym}();$host.flush();\
             if(r[0]!==0){{$write(2,$str(r[1])+\"\\n\");\
             if(typeof process!==\"undefined\")process.exit(1);}}}}\
             catch(e){{$host.flush();\
             $write(2,(e&&e.message?e.message:String(e))+\"\\n\");\
             if(e&&e.stack)$write(2,e.stack+\"\\n\");\
             if(typeof process!==\"undefined\")process.exit(1);}}"
        )));
    }

    if !program.roots.tests().is_empty() {
        let harness = g.test_harness();
        stmts.append(&mut g.extra);
        stmts.push(harness);
        // The runner appends its own epilogue after minification, so what that
        // epilogue names has to survive dead code elimination.
        for name in ["$run", "$write", "$str", "$t", "$host"] {
            roots.push(name.into());
        }
    }

    Output { stmts, roots, missing_intrinsics: g.missing }
}

/// The functions this backend prints `async`: those that reach a host call
/// that blocks, through calls it can name.
///
/// `middle::rc`'s `can_park` column asks a neighbouring question — *may this
/// function wait* — and answers `true` at an indirect call, because a code
/// pointer has no name and the conservative direction is the safe one. That
/// direction is free on the native backends, where the column decides how
/// much stack a call may need. It is not free here. `async` is not a property
/// a caller may ignore: an `async` function returns a promise whether or not
/// it ever waits, and this artifact hands function values to JavaScript that
/// cannot await one — a `view` given to `mount`, the row callbacks inside
/// `ui.each`, the callback of `$list_mapCtx`, a sort comparator. Printing
/// `Prop.read` `async` because one of its three arms calls a function value
/// is what turns `ui.each`'s row count into a promise and its duplicate-key
/// check into a no-op.
///
/// So the set is computed over the same call graph, from the same seed —
/// `rc::suspends`, the one list of blocking host keys — with the one arm that
/// cannot be honoured left out: an indirect call contributes nothing.
///
/// What that gives up is a *function value* that parks, called through a
/// position this pass cannot resolve. Nothing in the language builds one
/// today: a lambda may not capture a context (`middle::closures`), so it can
/// park only through a context *parameter*, which only the `*Ctx` combinators
/// supply, and no callback handed to one in this tree reaches a host call
/// that waits. Track B's precision slice is where `can_park` learns to name
/// the targets a `Ty::Fn` position can hold; on the day it lands, the column
/// *is* this set, and this walk goes away in favour of reading it.
///
/// The two are checked against each other where the flag is used: this set is
/// inside `can_park`, always.
fn waiting_functions(program: &Program) -> Vec<bool> {
    let mut waits: Vec<bool> = program
        .funcs
        .iter()
        .map(|f| match &f.kind {
            FuncKind::Intrinsic(key) => rc::suspends(key),
            // An unbuilt body is lowered to a throw, which is the one thing
            // that certainly does not wait.
            FuncKind::Unbuilt | FuncKind::Body(_) => false,
        })
        .collect();
    // The same fixpoint `rc::infer_effects` runs, over the same arms, minus
    // the indirect ones. Monotone — a row only ever climbs — so a row already
    // `true` is skipped and the loop terminates in at most one pass per edge.
    let mut changed = true;
    while changed {
        changed = false;
        for (i, f) in program.funcs.iter().enumerate() {
            if waits.get(i).copied() != Some(false) {
                continue;
            }
            let Some(body) = f.body() else { continue };
            let mut k = false;
            typed::walk(body, &mut |e| match &e.kind {
                ExprKind::CallFn { func, .. } => {
                    if let Some(c) = func.func() {
                        k = k || waits.get(c.index()).copied().unwrap_or(false);
                    }
                }
                // A jump into another function's loop is a call.
                ExprKind::Continue { func: Some(c), .. } => {
                    k = k || waits.get(c.index()).copied().unwrap_or(false);
                }
                // A host call spelled as an inline node rather than reached
                // through an intrinsic *function*; the same seed answers both.
                ExprKind::Intrinsic { name, .. } => k = k || rc::suspends(name),
                _ => {}
            });
            if k {
                if let Some(slot) = waits.get_mut(i) {
                    *slot = true;
                }
                changed = true;
            }
        }
    }
    waits
}

/// Whether any reachable intrinsic reaches a node module by name.
///
/// Two do, and they are not the same module. `runtime.js`'s `$fsp` is
/// `node:fs/promises`, which every `Fs` method waits on; `$fs` is the
/// synchronous `fs`, and the one caller left for it is `$writeRaw`, behind
/// `Stdout.writeBytes` — a write that must land before the next request is
/// read, so it does not wait and cannot use the other one.
///
/// `Stdin` is no longer on this list: it reads `process.stdin`, which is a
/// global rather than a module.
fn needs_require(program: &Program) -> bool {
    program.funcs.iter().any(|f| {
        f.intrinsic_key().is_some_and(|k| {
            k.starts_with("host.HostFs.") || k == "host.HostStdout.writeBytes"
        })
    })
}

/// The dispatch parameter of a multi-entry loop. Every local a body can name
/// is `name_index` or `vN` (`local_name`), and every temporary is `$tN`, so
/// this collides with neither.
const DISPATCH: &str = "$w";

/// Whether this expression's own tail position holds a jump back to the top of
/// the enclosing loop.
///
/// A jump is statements — assignments and a `continue` — so an operator whose
/// right operand holds one cannot stay an expression. A jump into *another*
/// function's loop is an ordinary call and does not count.
fn has_tail_continue(e: &typed::Expr) -> bool {
    match &e.kind {
        ExprKind::Continue { func, .. } => func.is_none(),
        ExprKind::Block { tail: Some(t), .. } => has_tail_continue(t),
        ExprKind::If { then, else_, .. } => has_tail_continue(then) || has_tail_continue(else_),
        ExprKind::Match { arms, .. } => arms.iter().any(|a| has_tail_continue(&a.body)),
        ExprKind::And { rhs, .. } | ExprKind::Or { rhs, .. } | ExprKind::Coalesce { rhs, .. } => {
            has_tail_continue(rhs)
        }
        _ => false,
    }
}

/// How many entries this function's body loops over, if it loops at all.
///
/// A `Loop` is always the whole body: `middle::tail_calls` puts it there and
/// nothing else produces one, so this is a look at the root rather than a
/// search.
fn loop_entries(f: &monomorphize::Func) -> Option<usize> {
    match &f.body()?.kind {
        ExprKind::Loop { entries } => Some(entries.len()),
        _ => None,
    }
}

/// The descriptor a generated call passes to a runtime function.
pub fn descriptor_name(i: usize) -> String {
    format!("$D{i}")
}

fn eq_name(i: usize) -> String {
    format!("$eqD{i}")
}

/// `a == b` at a float: `===` widened by the one pair it denies.
///
/// SPEC 7.2 rules `NaN == NaN`, and SPEC 6.2 keeps `-0.0 == 0.0`. `Object.is`
/// answers the first and gets the second wrong, so the test is spelled out.
///
/// Inline only where both operands may be written twice, which is a name, a
/// literal, or a projection of one — the shapes a compiled comparison reads
/// out of an aggregate. `$feq` is the same test for everything else, and is
/// tree-shaken away in a program that never needs it.
/// `BigInt.asIntN(bits, v)`: the low `bits` of a `BigInt`, read as signed.
pub(crate) fn as_int_n(bits: u32, v: Expr) -> Expr {
    Expr::call(
        Expr::member(Expr::ident("BigInt"), "asIntN"),
        vec![Expr::Num(f64::from(bits)), v],
    )
}

/// The unsigned half of [`as_int_n`].
pub(crate) fn as_uint_n(bits: u32, v: Expr) -> Expr {
    Expr::call(
        Expr::member(Expr::ident("BigInt"), "asUintN"),
        vec![Expr::Num(f64::from(bits)), v],
    )
}

pub(crate) fn float_eq(a: Expr, b: Expr) -> Expr {
    if !a.is_duplicable() || !b.is_duplicable() {
        return Expr::call(Expr::ident("$feq"), vec![a, b]);
    }
    Expr::bin(
        BinOp::Or,
        Expr::bin(BinOp::StrictEq, a.clone(), b.clone()),
        Expr::bin(
            BinOp::And,
            Expr::bin(BinOp::StrictNe, a.clone(), a),
            Expr::bin(BinOp::StrictNe, b.clone(), b),
        ),
    )
}

/// How a value of a described type is compared.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EqKind {
    /// `a === b` says it: an integer, a string, a boolean, `()`, or an enum
    /// whose every variant is payload-free and so is a bare number.
    Identity,
    /// A float, where `===` is wrong in exactly one pair: SPEC 7.2 rules
    /// `NaN == NaN` and `===` denies it.
    Float,
    /// An aggregate whose shape is known, compiled into its own function.
    Compiled,
    /// `Option` — whose nesting the runtime boxes — and anything with no
    /// structural description. Both stay with the generic walker, which is
    /// already exactly right for them.
    Generic,
}

fn local_name(i: usize, original: &str) -> String {
    // Distinct per local even when a name is shadowed, which Buri allows both
    // in nested scopes and within one block.
    let clean: String = original
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if clean.is_empty() || clean.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("v{i}")
    } else {
        format!("{clean}_{i}")
    }
}

impl<'a> Gen<'a> {
    pub(crate) fn runtime_has(&self, name: &str) -> bool {
        self.runtime.iter().any(|n| n == name)
    }

    pub(crate) fn prim_of(&self, ty: &Ty) -> Option<Prim> {
        self.tables.as_prim(ty)
    }

    pub(crate) fn prim_op_pub(&mut self, op: PrimOp, prim: Prim, args: Vec<Expr>) -> Expr {
        self.prim_op(op, prim, args)
    }

    /// A callee that still names a declaration rather than a concrete function
    /// means this tree never went through monomorphization. Unreachable in the
    /// real pipeline; emitted rather than panicked so a caller assembling a
    /// program by hand gets a located failure instead of a compiler crash.
    /// Runs `f` with sharing disabled, restoring the previous setting however
    /// `f` returns. A hand-written save/restore pair is one early exit away
    /// from leaving every later constant in the program unshared.
    fn inside_context<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let was = std::mem::replace(&mut self.in_context, true);
        let out = f(self);
        self.in_context = was;
        out
    }

    fn abort_unmonomorphized(&mut self) -> Expr {
        Expr::call(
            Expr::ident("$abort"),
            vec![Expr::Str("call was never monomorphized".to_string())],
        )
    }

    fn fresh(&mut self) -> String {
        self.func.temp = self.func.temp.saturating_add(1);
        format!("$t{}", self.func.temp)
    }

    /// The JavaScript name of a local.
    ///
    /// `FnState::for_locals` names every local the checker recorded for the
    /// function being emitted, and every `LocalId` in that function's body
    /// indexes the same list.
    fn local_name_of(&self, local: &LocalId) -> String {
        self.func
            .names
            .get(local)
            .or_ice("every local in a body was named from that function's own local list")
            .clone()
    }

    /// Shares a constant aggregate instead of rebuilding it.
    ///
    /// `Shape.Empty` is `[0]`, and writing it in a loop allocated an array per
    /// iteration for a value with no contents. Every value in the language is
    /// immutable and nothing in the runtime writes through one, so two
    /// occurrences of the same constant can be the same object: no expression
    /// can tell. The runtime already does this by hand for `None`.
    ///
    /// Only aggregates built entirely from literals qualify, and one built
    /// inside a `context { .. }` never does — a context is a live effect
    /// handle, and the runtime *does* write through those.
    fn intern(&mut self, e: Expr) -> Expr {
        if self.in_context {
            return e;
        }
        let Expr::Array(items) = &e else { return e };
        if items.is_empty() || !items.iter().all(shareable) {
            return e;
        }
        let key = javascript::print(&[Stmt::Expr(e.clone())], false);
        if let Some(i) = self.const_index.get(&key) {
            return Expr::ident(const_name(*i));
        }
        let i = self.consts.len();
        self.consts.push(e);
        self.const_index.insert(key, i);
        Expr::ident(const_name(i))
    }

    // -----------------------------------------------------------------------
    // Inlining the standard library's bodiless declarations
    // -----------------------------------------------------------------------

    /// The expansion of a call to an intrinsic, in place of a call to the
    /// one-line function that forwards to it.
    ///
    /// Most of `core/*` is declared without a body, and each such declaration
    /// becomes its own top-level function whose whole content is
    /// `return $str_trim(self);`. Every use then costs a real call frame for a
    /// call it is about to make anyway. Expanding at the call site removes
    /// both, and the wrapper itself falls out of the artifact once nothing
    /// names it — except where something still does, through `FnRef`, which is
    /// why the declaration is still emitted.
    ///
    /// Three things have to hold before an expansion may replace a call, and
    /// they are all about the arguments, because a call binds them and an
    /// expansion pastes them:
    ///
    ///  * no argument may be used twice, or the work would run twice;
    ///  * no argument may land under a `?:` branch, or it might not run at all;
    ///  * arguments must appear in the order they were written, because
    ///    evaluation order is specified (SPEC 8.2).
    ///
    /// A literal argument is exempt from all three: duplicating, skipping or
    /// reordering one is unobservable.
    fn inline_intrinsic(&mut self, index: usize, args: &[Expr]) -> Option<Expr> {
        // Copied out of `self` so the callee's borrow does not outlive the
        // `&mut self` the expansion needs.
        let program = self.program;
        let callee = program
            .funcs
            .get(index)
            .or_ice("a call's callee index was minted by monomorphization's own function table");
        if callee.body().is_some() {
            return None;
        }
        let key = callee.intrinsic_key()?.to_string();

        // Built once against placeholders, purely to see what the expansion
        // does with each argument.
        let probe: Vec<Expr> =
            (0..args.len()).map(|i| Expr::ident(format!("$$arg{i}"))).collect();
        let shape = self.intrinsic(&key, &probe, callee)?;
        if js_size(&shape) > MAX_INLINE_INTRINSIC {
            return None;
        }

        let mut seen = ArgUse::new(args.len());
        survey_args(&shape, false, &mut seen);
        let mut last = 0usize;
        for i in seen.order.iter().copied() {
            let (Some(arg), Some(facts)) = (args.get(i), seen.args.get(i)) else {
                return None;
            };
            if arg.is_pure_literal() {
                continue;
            }
            if facts.count > 1 || facts.conditional || i < last {
                return None;
            }
            last = i;
        }

        self.intrinsic(&key, args, callee)
    }
}

/// What makes a primitive operation's operands there to take: the checker
/// resolved the operator to this `PrimOp` from the operands it had.
const BINARY_ARITY: &str = "a binary primitive operation reaches the backend with two operands";
const UNARY_ARITY: &str = "a unary primitive operation reaches the backend with one operand";

/// A call worth replacing is small; anything larger is cheaper as a call.
/// `signum` and the checked-arithmetic expansions sit above this deliberately.
const MAX_INLINE_INTRINSIC: usize = 8;

/// How many fields a functional update will write out in full before falling
/// back to copying the base and patching it.
const MAX_SPELLED_UPDATE: usize = 8;

/// Whether a divisor is written down and is not zero.
fn nonzero_literal(e: Option<&Expr>) -> bool {
    match e {
        Some(Expr::Num(n)) => *n != 0.0,
        Some(Expr::BigInt(s)) => s != "0",
        _ => false,
    }
}

/// Whether an element is a constant, so an aggregate holding it is one too.
/// A name qualifies only when it is another shared constant.
fn shareable(e: &Expr) -> bool {
    match e {
        Expr::Num(_) | Expr::BigInt(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null
        | Expr::Undefined => true,
        Expr::Ident(n) => n.starts_with("$k"),
        Expr::Array(xs) => xs.iter().all(shareable),
        _ => false,
    }
}

fn const_name(i: usize) -> String {
    format!("$k{i}")
}

/// Where one placeholder argument ended up in an expansion.
#[derive(Clone, Copy, Default)]
struct ArgFacts {
    count: usize,
    /// Whether the argument appears only on one side of a conditional, where
    /// it might not be evaluated.
    conditional: bool,
}

/// Records where an expansion put each placeholder argument.
///
/// `counts` and `conditional` were two `Vec`s that had to be the same length,
/// with a `#[derive(Default)]` producing an all-empty value that is invalid
/// for any non-zero arity; `survey_args` bounds-checked one and then indexed
/// the other. One row per argument, and one constructor that takes the arity.
struct ArgUse {
    args: Vec<ArgFacts>,
    /// The order the arguments are reached in.
    order: Vec<usize>,
}

impl ArgUse {
    fn new(arity: usize) -> ArgUse {
        ArgUse { args: vec![ArgFacts::default(); arity], order: Vec::new() }
    }
}

/// Records where an expansion put each placeholder argument.
fn survey_args(e: &Expr, under_cond: bool, out: &mut ArgUse) {
    if let Expr::Ident(name) = e {
        if let Some(n) = name.strip_prefix("$$arg") {
            if let Ok(i) = n.parse::<usize>() {
                if let Some(a) = out.args.get_mut(i) {
                    a.count = a.count.saturating_add(1);
                    a.conditional |= under_cond;
                    out.order.push(i);
                }
            }
        }
        return;
    }
    match e {
        Expr::Array(xs) | Expr::Seq(xs) => {
            xs.iter().for_each(|x| survey_args(x, under_cond, out))
        }
        Expr::Object(fs) => fs.iter().for_each(|(_, v)| survey_args(v, under_cond, out)),
        Expr::Member { obj, .. } => survey_args(obj, under_cond, out),
        Expr::Index { obj, index } => {
            survey_args(obj, under_cond, out);
            survey_args(index, under_cond, out);
        }
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            survey_args(callee, under_cond, out);
            args.iter().for_each(|a| survey_args(a, under_cond, out));
        }
        Expr::Unary { operand, .. } => survey_args(operand, under_cond, out),
        Expr::Binary { op, lhs, rhs } => {
            survey_args(lhs, under_cond, out);
            // The right side of `&&` and `||` is itself conditional.
            let short = matches!(op, BinOp::And | BinOp::Or | BinOp::Coalesce);
            survey_args(rhs, under_cond || short, out);
        }
        Expr::Cond { test, cons, alt } => {
            survey_args(test, under_cond, out);
            survey_args(cons, true, out);
            survey_args(alt, true, out);
        }
        Expr::Assign { target, value } => {
            survey_args(target, under_cond, out);
            survey_args(value, under_cond, out);
        }
        Expr::Arrow { body, .. } => survey_args(body, true, out),
        Expr::ArrowBlock { .. } => {}
        Expr::Await(x) => survey_args(x, under_cond, out),
        Expr::Spread(x) => survey_args(x, under_cond, out),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Closures created inside a tail loop
// ---------------------------------------------------------------------------
//
// A tail loop rebinds its parameters in place, which is exact for everything
// that reads them within the iteration that wrote them. A closure is the one
// thing that does not: it holds the *slot*, and by the time it runs the loop
// has moved on, so it reads the value the loop stopped at rather than the one
// its iteration had.
//
// Elm generates the same shape and has had precisely this silent miscompile
// open since 2016 (elm/compiler#2268). ReScript and Gleam avoid it by giving
// every iteration its own binding for the parameters: the parameter itself
// becomes a write-only carrier, and the loop body opens with `const p = $pN;`,
// re-executed on each `continue`. Reads then see the iteration's value and a
// closure captures a binding no later iteration can reach.
//
// That fix used to live here, over the emitted statements, and it is now
// `middle::tail_calls::snapshot_captures` over the tree — because it is a
// property of the *rewrite* that introduced the loop, and a backend that
// forgot it would have the bug back. What arrives here is already a body whose
// captured parameters are read out of a per-iteration `let`, so there is
// nothing left for this file to do about it.

/// The bitwise operators at a known integer width, where the answer follows
/// from the operands alone.
///
/// Two literals fold outright; and `x | 0`, `x ^ 0`, `x & 0`, `x & x`, `x | x`
/// and `x ^ x` are answers without an operation. Dropping an operand is only
/// legal when it is pure, because the drop would take its effect with it —
/// `x | 0` needs no such condition, since it keeps `x` and evaluates it once.
fn fold_bitwise(op: PrimOp, p: Prim, args: &mut [Expr]) -> Option<Expr> {
    let narrow = |v: i128| -> Option<Expr> {
        let bits = p.bits();
        // The folder computes in an `i128`, so a width it cannot hold every
        // value of stays an operation rather than becoming a wrong literal.
        if bits == 0 || bits > 64 {
            return None;
        }
        let masked = v & ((1i128 << bits) - 1);
        let signed = if p.is_signed() && masked >= (1i128 << (bits - 1)) {
            masked - (1i128 << bits)
        } else {
            masked
        };
        if p.is_bigint() {
            return Some(Expr::BigInt(signed.to_string()));
        }
        let f = signed as f64;
        if f as i128 != signed {
            return None;
        }
        Some(Expr::Num(f))
    };
    let lit = |e: &Expr| -> Option<i128> {
        match e {
            Expr::Num(n) if n.fract() == 0.0 && n.is_finite() => Some(*n as i128),
            Expr::BigInt(s) => s.parse::<i128>().ok(),
            _ => None,
        }
    };
    let zero_of_p = || {
        if p.is_bigint() { Expr::BigInt("0".into()) } else { Expr::Num(0.0) }
    };

    if op == PrimOp::BitNot {
        return narrow(!lit(args.first()?)?);
    }
    if !matches!(op, PrimOp::BitAnd | PrimOp::BitOr | PrimOp::BitXor) {
        return None;
    }
    let [a, b] = args else { return None };
    if let (Some(x), Some(y)) = (lit(a), lit(b)) {
        return narrow(match op {
            PrimOp::BitAnd => x & y,
            PrimOp::BitOr => x | y,
            _ => x ^ y,
        });
    }
    // A zero on either side. `&` answers zero and needs the other operand
    // gone, so it has to be pure; `|` and `^` answer the other operand and
    // keep it exactly where it was.
    for (zero, other) in [(&*a, &*b), (&*b, &*a)] {
        if lit(zero) != Some(0) {
            continue;
        }
        return match op {
            PrimOp::BitAnd if other.is_pure() => Some(zero_of_p()),
            PrimOp::BitOr | PrimOp::BitXor => Some(other.clone()),
            _ => None,
        };
    }
    // Both sides the same value, read twice — so folding drops one read.
    if a.same_as(b) && a.is_pure() {
        return match op {
            PrimOp::BitAnd | PrimOp::BitOr => Some(a.clone()),
            _ => Some(zero_of_p()),
        };
    }
    None
}

/// Whether an expression reads `name`.
fn reads_ident(e: &Expr, name: &str) -> bool {
    match e {
        Expr::Ident(n) => n == name,
        Expr::Array(xs) | Expr::Seq(xs) => xs.iter().any(|x| reads_ident(x, name)),
        Expr::Object(fs) => fs.iter().any(|(_, v)| reads_ident(v, name)),
        Expr::Member { obj, .. } => reads_ident(obj, name),
        Expr::Index { obj, index } => reads_ident(obj, name) || reads_ident(index, name),
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            reads_ident(callee, name) || args.iter().any(|a| reads_ident(a, name))
        }
        Expr::Unary { operand, .. } => reads_ident(operand, name),
        Expr::Binary { lhs, rhs, .. } => reads_ident(lhs, name) || reads_ident(rhs, name),
        Expr::Cond { test, cons, alt } => {
            reads_ident(test, name) || reads_ident(cons, name) || reads_ident(alt, name)
        }
        Expr::Assign { target, value } => {
            reads_ident(target, name) || reads_ident(value, name)
        }
        Expr::Arrow { body, .. } => reads_ident(body, name),
        // A closure body is not walked, so a capture is assumed to read
        // everything: the conservative answer is the safe one here.
        Expr::ArrowBlock { .. } => true,
        Expr::Await(x) => reads_ident(x, name),
        Expr::Spread(x) => reads_ident(x, name),
        _ => false,
    }
}

/// Node count, for the size gate.
fn js_size(e: &Expr) -> usize {
    1 + match e {
        Expr::Array(xs) | Expr::Seq(xs) => xs.iter().map(js_size).sum::<usize>(),
        Expr::Object(fs) => fs.iter().map(|(_, v)| js_size(v)).sum(),
        Expr::Member { obj, .. } => js_size(obj),
        Expr::Index { obj, index } => js_size(obj) + js_size(index),
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            js_size(callee) + args.iter().map(js_size).sum::<usize>()
        }
        Expr::Unary { operand, .. } => js_size(operand),
        Expr::Binary { lhs, rhs, .. } => js_size(lhs) + js_size(rhs),
        Expr::Cond { test, cons, alt } => js_size(test) + js_size(cons) + js_size(alt),
        Expr::Assign { target, value } => js_size(target) + js_size(value),
        Expr::Arrow { body, .. } => js_size(body),
        Expr::Await(x) => js_size(x),
        Expr::Spread(x) => js_size(x),
        _ => 0,
    }
}

impl<'a> Gen<'a> {

    // -----------------------------------------------------------------------
    // Statement position
    // -----------------------------------------------------------------------

    /// Emits `e` in tail position: the statements end with a `return`.
    fn tail(&mut self, e: &typed::Expr, out: &mut Vec<Stmt>) {
        self.mark_locals(e, out);
        match &e.kind {
            ExprKind::Block { stmts, tail } => {
                for s in stmts {
                    self.stmt(s, out);
                }
                match tail {
                    Some(t) => self.tail(t, out),
                    None => out.push(Stmt::Return(Some(Expr::Num(0.0)))),
                }
            }
            ExprKind::If { cond, then, else_ } => {
                let c = self.expr(cond, out);
                let mut t = Vec::new();
                self.tail(then, &mut t);
                let mut f = Vec::new();
                self.tail(else_, &mut f);
                out.push(Stmt::If { cond: c, then: t, else_: f });
            }
            ExprKind::Match { scrutinee, arms } => {
                let s = self.expr(scrutinee, out);
                self.match_stmts(s, arms, out, None);
            }
            // The loop `middle::tail_calls` put here. One entry is a plain
            // `while (true)`; more than one is a group merged into this
            // function, and the entry a caller asked for selects whose body
            // runs first.
            ExprKind::Loop { entries } => {
                let body = match entries.as_slice() {
                    [only] => {
                        let mut inner = Vec::new();
                        self.tail(only, &mut inner);
                        inner
                    }
                    many => {
                        let mut cases = Vec::new();
                        for (i, entry) in many.iter().enumerate() {
                            let mut inner = Vec::new();
                            self.tail(entry, &mut inner);
                            // A block, because the cases of a `switch` share
                            // one lexical scope: two entries declaring a local
                            // at the same index would otherwise collide — and
                            // `javascript::rename_scope` clones its map per
                            // case, so the collision would appear only once
                            // names are shortened, in release, with debug
                            // passing.
                            cases.push((Some(Expr::Num(i as f64)), vec![Stmt::Block(inner)]));
                        }
                        vec![Stmt::Switch { disc: Expr::ident(DISPATCH), cases }]
                    }
                };
                out.push(Stmt::While { cond: Expr::Bool(true), body });
            }
            // A jump back to the top of this function's own loop: rebind and
            // continue. A jump into *another* function's loop is a call, and
            // falls through to the expression form below.
            ExprKind::Continue { func: None, entry, args } => {
                let values = self.exprs(args, out);
                self.rebind(*entry, values, out);
            }
            // `a && f(x)` *is* `f(x)` when `a` holds, so a tail call there was
            // a tail call, and `tail_calls` rewrote it to a `Continue` — which
            // is statements rather than a value. Splitting the operator into a
            // branch is what gives those statements somewhere to go.
            //
            // Only worth doing when the right operand really holds one:
            // otherwise the one-expression form below is shorter and says the
            // same thing.
            ExprKind::And { lhs, rhs } if has_tail_continue(rhs) => {
                let c = self.expr(lhs, out);
                let mut then = Vec::new();
                self.tail(rhs, &mut then);
                out.push(Stmt::If {
                    cond: c,
                    then,
                    else_: vec![Stmt::Return(Some(Expr::Bool(false)))],
                });
            }
            ExprKind::Or { lhs, rhs } if has_tail_continue(rhs) => {
                let c = self.expr(lhs, out);
                let mut else_ = Vec::new();
                self.tail(rhs, &mut else_);
                out.push(Stmt::If {
                    cond: c,
                    then: vec![Stmt::Return(Some(Expr::Bool(true)))],
                    else_,
                });
            }
            ExprKind::Coalesce { lhs, rhs, .. } if has_tail_continue(rhs) => {
                // The same test and held value `expr` computes, off the same
                // temporary, so the left operand is reached exactly once.
                let (ok, held) = self.coalesce_split(lhs, out);
                let mut else_ = Vec::new();
                self.tail(rhs, &mut else_);
                out.push(Stmt::If { cond: ok, then: vec![Stmt::Return(Some(held))], else_ });
            }
            _ => {
                let v = self.expr(e, out);
                out.push(Stmt::Return(Some(v)));
            }
        }
    }

    /// The `??` test and the value the left operand holds, both reading one
    /// temporary. Shared by the value and the tail forms so the two cannot
    /// drift apart over what `option_nesting` means.
    fn coalesce_split(&mut self, lhs: &typed::Expr, out: &mut Vec<Stmt>) -> (Expr, Expr) {
        let l = self.expr(lhs, out);
        let name = self.fresh();
        out.push(Stmt::Var { kind: VarKind::Const, name: name.clone(), init: Some(l) });
        let option = self.option_nesting(&lhs.ty);
        let ok = match option {
            Some(_) => Expr::bin(BinOp::StrictNe, Expr::ident(name.clone()), Expr::Undefined),
            None => Expr::bin(
                BinOp::StrictEq,
                Expr::index(Expr::ident(name.clone()), Expr::Num(0.0)),
                Expr::Num(0.0),
            ),
        };
        let held = match option {
            Some(nested) => self.option_value(Expr::ident(name.clone()), nested),
            None => Expr::index(Expr::ident(name), Expr::Num(1.0)),
        };
        (ok, held)
    }

    /// Assigns the new arguments and continues. The values are computed into
    /// temporaries first, because an argument may name a parameter the
    /// rebinding is about to overwrite.
    ///
    /// *Whether* to jump is not decided here — `middle::tail_calls` decided it,
    /// and this is handed a `Continue`. What is left is the one thing that is
    /// genuinely a JavaScript question: a parallel move onto named slots, where
    /// a native backend would pass block arguments and need none of it.
    fn rebind(&mut self, entry: usize, values: Vec<Expr>, out: &mut Vec<Stmt>) {
        let targets = self.func.loops.targets.clone();
        let which = self.func.loops.which.clone().map(|w| (w, entry));
        // Rebinding the parameters is a parallel move: every new value is
        // computed from the old ones, and then all the slots change at once.
        // A temporary per parameter is always correct and usually
        // unnecessary.
        //
        // The values are still unevaluated expressions here, so they are
        // reached in written order whatever this does; what varies is *when
        // each slot is written*. A slot may be written as soon as its value
        // has been reached, unless a value still to come reads it — in which
        // case the value waits in a temporary and the write happens once
        // everything has been read.
        //
        // `f(n - 1, acc + n)` then needs one temporary and `f(a, b)` none,
        // where binding every parameter through one needs as many as there are
        // parameters.
        //
        // A parameter whose new value is *itself* does not move at all. Those
        // go first, which also lifts the constraint they would otherwise place
        // on the others: a slot never written cannot be read too late.
        let values: Vec<Option<Expr>> = values
            .into_iter()
            .zip(targets.iter())
            .map(|(v, target)| match &v {
                Expr::Ident(n) if n == target => None,
                _ => Some(v),
            })
            .collect();

        let mut deferred: Vec<(String, Expr)> = Vec::new();
        for (i, target) in targets.iter().enumerate() {
            let Some(Some(v)) = values.get(i).cloned() else { continue };
            let read_later = values
                .get(i.saturating_add(1)..)
                .unwrap_or_default()
                .iter()
                .flatten()
                .any(|later| reads_ident(later, target));
            if read_later {
                let t = self.fresh();
                out.push(Stmt::Var { kind: VarKind::Const, name: t.clone(), init: Some(v) });
                deferred.push((target.clone(), Expr::ident(t)));
            } else {
                out.push(assign(target, v));
            }
        }
        // Everything parked above, now that every read has happened.
        for (target, v) in deferred {
            out.push(assign(&target, v));
        }
        if let Some((w, index)) = which {
            out.push(assign(&w, Expr::Num(index as f64)));
        }
        out.push(Stmt::Continue);
    }

    fn stmt(&mut self, s: &typed::Stmt, out: &mut Vec<Stmt>) {
        match s {
            typed::Stmt::Let { pattern, value, .. } => {
                let v = self.expr(value, out);
                // The pattern is irrefutable, so no test is needed — only the
                // bindings. `let _ = io.println(ctx, "hi")` binds nothing, and
                // since there are no expression statements it is the only way
                // to perform an effect for its own sake: the value still has
                // to be evaluated.
                self.hoist_or_declarations(pattern, out);
                let mut bound = Vec::new();
                pattern.binds(&mut bound);
                if bound.is_empty() {
                    if !v.is_pure_literal() {
                        out.push(Stmt::Expr(v));
                    }
                } else {
                    self.bind(pattern, &v, out);
                }
            }
            typed::Stmt::Expr(e) => {
                let v = self.expr(e, out);
                if !v.is_pure_literal() {
                    out.push(Stmt::Expr(v));
                }
            }
        }
    }

    /// A `match` in statement position: each arm's body is emitted in tail
    /// position, or assigned to `target` when the match produces a value.
    fn match_stmts(
        &mut self,
        subject: Expr,
        arms: &[typed::Arm],
        out: &mut Vec<Stmt>,
        target: Option<&str>,
    ) {
        // Bind the scrutinee once: it is evaluated before the arms are tried.
        let s = self.fresh();
        out.push(Stmt::Var { kind: VarKind::Const, name: s.clone(), init: Some(subject) });
        let subject = Expr::ident(s);

        // What is tested, and in what order, was decided by
        // `middle::decision` before this file saw the arms: they arrive
        // mutually exclusive, with everything an arm still had to look at
        // moved into a match on the field it looks at. All that is left here
        // is to spell them.
        //
        // The spelling is a chain, and that is not this backend declining the
        // table. `javascript::to_switch` turns a chain of literal tests over
        // one discriminant into a `switch` (`javascript.rs:1380-1389`) — after
        // folding, so a match on a value already known folds away instead of
        // becoming a dispatch on a constant; with equal bodies merged, so four
        // variants handled alike are four labels and one body; and only past
        // the arm count where a `switch` prints shorter than an `if`. A switch
        // emitted here would arrive too early for all three. Measured on this
        // corpus, emitting one here cost 4.6% of the generated bytes.
        let guarded = arms.iter().any(|a| a.guard.is_some());
        if !guarded {
            let mut chain: Vec<Stmt> = Vec::new();
            self.arm_chain(&subject, arms, 0, &mut chain, target);
            out.extend(chain);
            return;
        }

        // With guards, an arm that matches may still fall through, so the arms
        // run inside a loop the first success breaks out of.
        let mut body: Vec<Stmt> = Vec::new();
        for arm in arms {
            self.hoist_or_declarations(&arm.pattern, &mut body);
            let mut inner: Vec<Stmt> = Vec::new();
            self.bind(&arm.pattern, &subject, &mut inner);
            let mut taken: Vec<Stmt> = Vec::new();
            self.arm_body(arm, &mut taken, target);
            if target.is_some() {
                taken.push(Stmt::Break);
            }
            match &arm.guard {
                Some(g) => {
                    let mut gout = Vec::new();
                    let gv = self.expr(g, &mut gout);
                    inner.extend(gout);
                    inner.push(Stmt::If { cond: gv, then: taken, else_: Vec::new() });
                }
                None => inner.extend(taken),
            }
            match self.test(&arm.pattern, &subject) {
                Some(cond) => body.push(Stmt::If { cond, then: inner, else_: Vec::new() }),
                None => body.extend(inner),
            }
        }
        body.push(Stmt::Expr(Expr::call(
            Expr::ident("$abort"),
            vec![Expr::Str("no arm matched".into())],
        )));
        out.push(Stmt::While { cond: Expr::Bool(true), body });
    }

    /// The arms, in the order they arrived, as an `if`/`else` chain.
    ///
    /// This used to be the strategy — the reason a six-variant enum performed
    /// six comparisons to reach its sixth arm. It is not one any more:
    /// `middle::decision` groups the arms, hoists the test they share, and puts
    /// the fallback last, and this walks what it produced. A match that pass
    /// declined comes through here too, unchanged, which is exactly what
    /// declining is for.
    fn arm_chain(
        &mut self,
        subject: &Expr,
        arms: &[typed::Arm],
        i: usize,
        out: &mut Vec<Stmt>,
        target: Option<&str>,
    ) {
        let Some(arm) = arms.get(i) else {
            // Exhaustiveness is checked, so this is unreachable — but an
            // abort here is cheaper than a silently wrong value if it ever is
            // not. There is no way to write this in the source language: it is
            // the backend's own belt to the checker's braces.
            out.push(Stmt::Expr(Expr::call(
                Expr::ident("$abort"),
                vec![Expr::Str("no arm matched".into())],
            )));
            return;
        };
        self.hoist_or_declarations(&arm.pattern, out);
        let mut body = Vec::new();
        self.bind(&arm.pattern, subject, &mut body);
        self.arm_body(arm, &mut body, target);

        // The last arm of an exhaustive match is the one that runs when no
        // earlier one did, so testing it is asking a question whose answer is
        // already known. `exhaustiveness.rs` proves exhaustiveness at compile
        // time, and a release build takes that proof at its word; a debug build
        // keeps the test and the abort behind it, which is why this is a
        // profile decision and stayed here when the rest of the strategy left.
        //
        // *Which* arm is the fallback is not decided here any more —
        // `middle::decision` is what puts it last, and what guarantees that the
        // arms before it are mutually exclusive so that "last" means "when
        // nothing else matched" rather than "written last".
        //
        // An or-pattern is the exception: which alternative matched is decided
        // by running the test, and the test is where the alternative's
        // bindings are assigned — so dropping it would leave the body reading
        // names nothing ever wrote.
        let last = !self.defensive_aborts
            && i + 1 == arms.len()
            && !Self::test_assigns(&arm.pattern);
        match self.test(&arm.pattern, subject) {
            // The last arm, or an irrefutable one, needs no test.
            _ if last => out.extend(body),
            None => out.extend(body),
            Some(cond) => {
                let mut else_ = Vec::new();
                self.arm_chain(subject, arms, i + 1, &mut else_, target);
                out.push(Stmt::If { cond, then: body, else_ });
            }
        }
    }

    fn arm_body(&mut self, arm: &typed::Arm, out: &mut Vec<Stmt>, target: Option<&str>) {
        match target {
            None => self.tail(&arm.body, out),
            Some(name) => {
                let v = self.expr(&arm.body, out);
                out.push(Stmt::Expr(Expr::Assign {
                    target: Box::new(Expr::ident(name.to_string())),
                    value: Box::new(v),
                }));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Patterns
    // -----------------------------------------------------------------------

    /// The condition under which `pattern` matches `subject`, or `None` when
    /// it always does.
    fn test(&mut self, pattern: &typed::Pattern, subject: &Expr) -> Option<Expr> {
        match &pattern.kind {
            PatKind::Wild | PatKind::Unit | PatKind::Error => None,
            PatKind::Bind { sub, .. } => sub.as_ref().and_then(|s| self.test(s, subject)),
            PatKind::Bool(b) => Some(if *b {
                subject.clone()
            } else {
                Expr::un(UnOp::Not, subject.clone())
            }),
            PatKind::Int(v, neg) => {
                let lit = self.int_literal(*v, *neg, &pattern.ty);
                Some(Expr::bin(BinOp::StrictEq, subject.clone(), lit))
            }
            // `===` and not `float_eq`: the grammar has no `NaN` literal, so
            // the right operand is never one, and the extra test SPEC 7.2 asks
            // for could only ever answer false here. A `NaN` subject fails this
            // test on every backend, which is `float_eq` at a non-`NaN`
            // literal.
            PatKind::Float(v) => {
                Some(Expr::bin(BinOp::StrictEq, subject.clone(), Expr::Num(*v)))
            }
            PatKind::Str(s) => {
                Some(Expr::bin(BinOp::StrictEq, subject.clone(), Expr::Str(s.clone())))
            }
            PatKind::Char(c) => Some(Expr::bin(
                BinOp::StrictEq,
                subject.clone(),
                Expr::Str(c.to_string()),
            )),
            PatKind::Tuple(ps) => self.all_tests(
                ps.iter().enumerate().map(|(i, p)| (p, Expr::index(subject.clone(), Expr::Num(i as f64)))),
            ),
            PatKind::Struct { fields, .. } => self.all_tests(
                fields
                    .iter()
                    .map(|f| (&f.pattern, Expr::index(subject.clone(), Expr::Num(f.index as f64)))),
            ),
            PatKind::Variant { con, variant, fields } => {
                if let Some(nested) = self.option_nesting(&pattern.ty) {
                    // `Some` is anything but absence; `None` is absence.
                    let present = Expr::bin(
                        if *variant == 1 { BinOp::StrictEq } else { BinOp::StrictNe },
                        subject.clone(),
                        Expr::Undefined,
                    );
                    if *variant == 1 {
                        return Some(present);
                    }
                    let inner = self.option_value(subject.clone(), nested);
                    return match self
                        .all_tests(fields.iter().map(|f| (&f.pattern, inner.clone())))
                    {
                        Some(t) => Some(Expr::bin(BinOp::And, present, t)),
                        None => Some(present),
                    };
                }
                let flat = self.payloadless(*con);
                let tag = if flat {
                    subject.clone()
                } else {
                    Expr::index(subject.clone(), Expr::Num(0.0))
                };
                let mut cond =
                    Expr::bin(BinOp::StrictEq, tag, Expr::Num(*variant as f64));
                // A single-variant enum needs no tag test.
                if self.tables.tycon(*con).variants().len() == 1 {
                    match self.all_tests(fields.iter().map(|f| {
                        (&f.pattern, Expr::index(subject.clone(), Expr::Num((f.index + 1) as f64)))
                    })) {
                        Some(inner) => return Some(inner),
                        None => return None,
                    }
                }
                if let Some(inner) = self.all_tests(fields.iter().map(|f| {
                    (&f.pattern, Expr::index(subject.clone(), Expr::Num((f.index + 1) as f64)))
                })) {
                    cond = Expr::bin(BinOp::And, cond, inner);
                }
                Some(cond)
            }
            PatKind::Array { elems, rest } => {
                let len = Expr::member(subject.clone(), "length");
                let mut cond = if rest.is_open() {
                    Expr::bin(BinOp::Ge, len, Expr::Num(elems.len() as f64))
                } else {
                    Expr::bin(BinOp::StrictEq, len, Expr::Num(elems.len() as f64))
                };
                if let Some(inner) = self.all_tests(
                    elems.iter().enumerate().map(|(i, p)| {
                        (p, Expr::index(subject.clone(), Expr::Num(i as f64)))
                    }),
                ) {
                    cond = Expr::bin(BinOp::And, cond, inner);
                }
                Some(cond)
            }
            PatKind::Or(alts) => {
                // Alternatives bind identical names, so the assignments are
                // folded into the test and the winning alternative's run.
                let mut cond: Option<Expr> = None;
                for alt in alts {
                    let mut binds = Vec::new();
                    self.bind_assignments(alt, subject, &mut binds);
                    let t = self.test(alt, subject);
                    let branch = match (t, binds.is_empty()) {
                        (None, true) => return None,
                        (None, false) => {
                            let mut seq = binds;
                            seq.push(Expr::Bool(true));
                            Expr::Seq(seq)
                        }
                        (Some(t), true) => t,
                        (Some(t), false) => {
                            let mut seq = binds;
                            seq.push(Expr::Bool(true));
                            Expr::bin(BinOp::And, t, Expr::Seq(seq))
                        }
                    };
                    cond = Some(match cond {
                        None => branch,
                        Some(prev) => Expr::bin(BinOp::Or, prev, branch),
                    });
                }
                cond
            }
        }
    }

    fn all_tests<'p>(
        &mut self,
        parts: impl Iterator<Item = (&'p typed::Pattern, Expr)>,
    ) -> Option<Expr> {
        let mut acc: Option<Expr> = None;
        for (p, subject) in parts {
            if let Some(t) = self.test(p, &subject) {
                acc = Some(match acc {
                    None => t,
                    Some(prev) => Expr::bin(BinOp::And, prev, t),
                });
            }
        }
        acc
    }

    /// Emits `const` declarations for everything a pattern binds.
    fn bind(&mut self, pattern: &typed::Pattern, subject: &Expr, out: &mut Vec<Stmt>) {
        match &pattern.kind {
            PatKind::Bind { local, sub } => {
                let name = self.local_name_of(local);
                out.push(Stmt::Var {
                    kind: VarKind::Const,
                    name,
                    init: Some(subject.clone()),
                });
                if let Some(s) = sub {
                    self.bind(s, subject, out);
                }
            }
            PatKind::Tuple(ps) => {
                for (i, p) in ps.iter().enumerate() {
                    self.bind(p, &Expr::index(subject.clone(), Expr::Num(i as f64)), out);
                }
            }
            PatKind::Struct { fields, .. } => {
                for f in fields {
                    self.bind(
                        &f.pattern,
                        &Expr::index(subject.clone(), Expr::Num(f.index as f64)),
                        out,
                    );
                }
            }
            PatKind::Variant { fields, .. } => {
                if let Some(nested) = self.option_nesting(&pattern.ty) {
                    let inner = self.option_value(subject.clone(), nested);
                    for f in fields {
                        self.bind(&f.pattern, &inner, out);
                    }
                    return;
                }
                for f in fields {
                    self.bind(
                        &f.pattern,
                        &Expr::index(subject.clone(), Expr::Num((f.index + 1) as f64)),
                        out,
                    );
                }
            }
            PatKind::Array { elems, rest } => {
                for (i, p) in elems.iter().enumerate() {
                    self.bind(p, &Expr::index(subject.clone(), Expr::Num(i as f64)), out);
                }
                if let typed::ArrayRest::Bound(local) = rest {
                    let name = self.local_name_of(local);
                    out.push(Stmt::Var {
                        kind: VarKind::Const,
                        name,
                        init: Some(Expr::call(
                            Expr::member(subject.clone(), "slice"),
                            vec![Expr::Num(elems.len() as f64)],
                        )),
                    });
                }
            }
            // An or-pattern's bindings are assigned inside its test, so they
            // are declared by `hoist_or_declarations` before the `if` rather
            // than here — a declaration in the body would come after the
            // assignment and shadow it.
            PatKind::Or(_) => {}
            _ => {}
        }
    }

    /// Declares, in the statement list that will hold the `if`, every local an
    /// or-pattern anywhere in this pattern binds. The test assigns them as it
    /// decides which alternative matched, so they must already exist.
    fn hoist_or_declarations(&mut self, pattern: &typed::Pattern, out: &mut Vec<Stmt>) {
        match &pattern.kind {
            PatKind::Or(alts) => {
                let mut declared = Vec::new();
                if let Some(first) = alts.first() {
                    first.binds(&mut declared);
                }
                for local in declared {
                    let name = self.local_name_of(&local);
                    out.push(Stmt::Var { kind: VarKind::Let, name, init: None });
                }
            }
            PatKind::Bind { sub: Some(s), .. } => self.hoist_or_declarations(s, out),
            PatKind::Tuple(ps) => {
                for p in ps {
                    self.hoist_or_declarations(p, out);
                }
            }
            PatKind::Struct { fields, .. } | PatKind::Variant { fields, .. } => {
                for f in fields {
                    self.hoist_or_declarations(&f.pattern, out);
                }
            }
            PatKind::Array { elems, .. } => {
                for p in elems {
                    self.hoist_or_declarations(p, out);
                }
            }
            _ => {}
        }
    }

    /// Whether this pattern's test writes the bindings the body reads.
    ///
    /// Only an or-pattern does: the alternative that matched is the one that
    /// assigns, so its test is not a question whose answer can be assumed even
    /// when exhaustiveness says it is.
    fn test_assigns(pattern: &typed::Pattern) -> bool {
        match &pattern.kind {
            PatKind::Or(alts) => {
                let mut bound = Vec::new();
                if let Some(first) = alts.first() {
                    first.binds(&mut bound);
                }
                !bound.is_empty()
            }
            PatKind::Bind { sub: Some(s), .. } => Self::test_assigns(s),
            PatKind::Tuple(ps) => ps.iter().any(Self::test_assigns),
            PatKind::Struct { fields, .. } | PatKind::Variant { fields, .. } => {
                fields.iter().any(|f| Self::test_assigns(&f.pattern))
            }
            PatKind::Array { elems, .. } => elems.iter().any(Self::test_assigns),
            _ => false,
        }
    }

    /// The assignment form, for bindings inside an or-pattern's test.
    fn bind_assignments(&mut self, pattern: &typed::Pattern, subject: &Expr, out: &mut Vec<Expr>) {
        match &pattern.kind {
            PatKind::Bind { local, sub } => {
                let name = self.local_name_of(local);
                out.push(Expr::Assign {
                    target: Box::new(Expr::ident(name)),
                    value: Box::new(subject.clone()),
                });
                if let Some(s) = sub {
                    self.bind_assignments(s, subject, out);
                }
            }
            PatKind::Tuple(ps) => {
                for (i, p) in ps.iter().enumerate() {
                    self.bind_assignments(p, &Expr::index(subject.clone(), Expr::Num(i as f64)), out);
                }
            }
            PatKind::Struct { fields, .. } => {
                for f in fields {
                    self.bind_assignments(
                        &f.pattern,
                        &Expr::index(subject.clone(), Expr::Num(f.index as f64)),
                        out,
                    );
                }
            }
            PatKind::Variant { fields, .. } => {
                if let Some(nested) = self.option_nesting(&pattern.ty) {
                    let inner = self.option_value(subject.clone(), nested);
                    for f in fields {
                        self.bind_assignments(&f.pattern, &inner, out);
                    }
                    return;
                }
                for f in fields {
                    self.bind_assignments(
                        &f.pattern,
                        &Expr::index(subject.clone(), Expr::Num((f.index + 1) as f64)),
                        out,
                    );
                }
            }
            PatKind::Array { elems, rest } => {
                for (i, p) in elems.iter().enumerate() {
                    self.bind_assignments(
                        p,
                        &Expr::index(subject.clone(), Expr::Num(i as f64)),
                        out,
                    );
                }
                if let typed::ArrayRest::Bound(local) = rest {
                    let name = self.local_name_of(local);
                    out.push(Expr::Assign {
                        target: Box::new(Expr::ident(name)),
                        value: Box::new(Expr::call(
                            Expr::member(subject.clone(), "slice"),
                            vec![Expr::Num(elems.len() as f64)],
                        )),
                    });
                }
            }
            // A nested or-pattern's own alternatives are handled by its test.
            _ => {}
        }
    }

    /// How an `Option` value is spelled, given the whole `Option<T>` type.
    ///
    /// `None` is `undefined` and `Some(x)` is `x`. Absence is the only thing
    /// in the value representation that is ever `undefined` (runtime.js), so
    /// the two are told apart by that alone — no tag, and no array to build.
    ///
    /// A nested `Option<Option<U>>` is the one case where they collide, since
    /// `Some(None)` would be `undefined` too. There the payload is wrapped, by
    /// `$some` going in and `$val` coming out.
    fn option_nesting(&self, ty: &Ty) -> Option<bool> {
        let payload = self.tables.option_payload(ty)?;
        Some(self.tables.is_option_ty(payload))
    }

    /// The payload of a `Some`, read out of the value that *is* the payload.
    fn option_value(&self, subject: Expr, nested: bool) -> Expr {
        if nested {
            Expr::call(Expr::ident("$val"), vec![subject])
        } else {
            subject
        }
    }

    fn payloadless(&self, con: crate::compiler::semantics::types::TyConId) -> bool {
        match &self.tables.tycon(con).def {
            TyDef::Enum { variants } => variants.iter().all(|v| v.fields.is_empty()),
            _ => false,
        }
    }

    // -----------------------------------------------------------------------
    // Expression position
    // -----------------------------------------------------------------------

    /// One expression, with its sharing marks around it.
    ///
    /// The marks are a wrapper rather than a case inside the emitter because
    /// they apply to *whatever* the node produced: a projection, a local read
    /// and a capture all reach the same two lines.
    fn expr(&mut self, e: &typed::Expr, out: &mut Vec<Stmt>) -> Expr {
        self.mark_locals(e, out);
        let value = self.emit(e, out);
        self.mark_value(e, value)
    }

    /// The locals this node marks before it runs: a lambda's captures, and an
    /// arm's payload bindings.
    fn mark_locals(&mut self, e: &typed::Expr, out: &mut Vec<Stmt>) {
        let key = node_key(e);
        if !self.func.marks.done.insert(key) {
            return;
        }
        let Some(locals) = self.func.marks.locals.get(&key).cloned() else { return };
        for l in locals {
            let name = self.local_name_of(&l);
            out.push(Stmt::Expr(Expr::call(Expr::ident("$share"), vec![Expr::ident(name)])));
        }
    }

    /// The mark on the value this node produced, where it has one.
    fn mark_value(&mut self, e: &typed::Expr, value: Expr) -> Expr {
        let Some(parent) = self.func.marks.value.get(&node_key(e)).copied() else {
            return value;
        };
        match parent {
            None => Expr::call(Expr::ident("$share"), vec![value]),
            Some(l) => {
                let name = self.local_name_of(&l);
                Expr::call(Expr::ident("$fromShared"), vec![Expr::ident(name), value])
            }
        }
    }

    fn emit(&mut self, e: &typed::Expr, out: &mut Vec<Stmt>) -> Expr {
        match &e.kind {
            ExprKind::Int(v, neg) => self.int_literal(*v, *neg, &e.ty),
            ExprKind::Float(v) => {
                // A literal takes its type from context, so an `F32` one is
                // stored as the binary32 value it denotes.
                match self.tables.as_prim(&e.ty) {
                    Some(Prim::F32) => Expr::Num(*v as f32 as f64),
                    _ => Expr::Num(*v),
                }
            }
            ExprKind::Str(s) => Expr::Str(s.clone()),
            ExprKind::Char(c) => Expr::Str(c.to_string()),
            ExprKind::Bool(b) => Expr::Bool(*b),
            ExprKind::Unit => Expr::Num(0.0),
            ExprKind::Local(l) => Expr::ident(
                self.func.names.get(l).cloned().unwrap_or_else(|| format!("v{}", l.0)),
            ),
            ExprKind::Const(_) => Expr::Num(0.0),
            // Both of these name a `Program::funcs` slot, which is what
            // `Callee::func` answers; a callee still naming a declaration
            // means monomorphization did not run over this tree.
            ExprKind::FnRef(f) => match f.func() {
                Some(i) => Expr::ident(self.symbol(i.index())),
                None => self.abort_unmonomorphized(),
            },
            ExprKind::CallFn { func, args } => {
                let Some(idx) = func.func() else {
                    return self.abort_unmonomorphized();
                };
                let args = self.exprs(args, out);
                let parks = self.parks(idx.index());
                let call = match self.inline_intrinsic(idx.index(), &args) {
                    // An inlined intrinsic is the same callee spelled at the
                    // call site, so it is awaited on the same answer.
                    Some(x) => x,
                    None => Expr::call(Expr::ident(self.symbol(idx.index())), args),
                };
                if parks { Expr::awaited(call) } else { call }
            }
            // Not awaited: the callee is a function value, and
            // [`waiting_functions`] is the argument for why no function value
            // this backend emits is `async`.
            ExprKind::CallValue { callee, args } => {
                let c = self.expr(callee, out);
                let args = self.exprs(args, out);
                Expr::call(c, args)
            }
            ExprKind::CallTrait { .. } => Expr::Num(0.0),
            ExprKind::StructLit { fields, .. } => {
                let a = Expr::Array(self.exprs(fields, out));
                self.intern(a)
            }
            ExprKind::StructUpdate { con, base, updates } => {
                // A functional update names the fields it changes; the type
                // says what the rest are. So the result is written out in
                // full, reading the untouched fields from the base — no copy,
                // no mutation, and one expression rather than a statement per
                // field.
                //
                // `..base` is evaluated first and once, whether or not any
                // field survives it, because the source says so.
                let b = self.expr(base, out);
                let arity = self.tables.tycon(*con).fields().len();
                let name = self.fresh();

                // Past a certain width, naming every field costs more than a
                // bulk copy does, in output and in work — so a wide struct is
                // still copied and patched.
                if arity > MAX_SPELLED_UPDATE {
                    out.push(Stmt::Var {
                        kind: VarKind::Const,
                        name: name.clone(),
                        init: Some(Expr::call(Expr::member(b, "slice"), vec![])),
                    });
                    for (i, v) in updates {
                        let value = self.expr(v, out);
                        out.push(Stmt::Expr(Expr::Assign {
                            target: Box::new(Expr::index(
                                Expr::ident(name.clone()),
                                Expr::Num(*i as f64),
                            )),
                            value: Box::new(value),
                        }));
                    }
                    return Expr::ident(name);
                }

                out.push(Stmt::Var {
                    kind: VarKind::Const,
                    name: name.clone(),
                    init: Some(b),
                });

                // Left to right over the fields, so a replacement expression
                // runs in field order rather than in the order it was written
                // — which is what `StructLit` does, and what the update is
                // shorthand for.
                let mut fields: Vec<Expr> = Vec::with_capacity(arity);
                for i in 0..arity {
                    match updates.iter().find(|(j, _)| *j == i) {
                        Some((_, v)) => {
                            let value = self.expr(v, out);
                            fields.push(value);
                        }
                        None => fields.push(Expr::index(
                            Expr::ident(name.clone()),
                            Expr::Num(i as f64),
                        )),
                    }
                }
                Expr::Array(fields)
            }
            ExprKind::EnumLit { con, variant, args, .. } => {
                if let Some(nested) = self.option_nesting(&e.ty) {
                    // Variant 0 is `Some`, 1 is `None`; `Tables::is_option`
                    // checks that.
                    if *variant == 1 {
                        return Expr::Undefined;
                    }
                    let arg = args
                        .first()
                        .or_ice("`Some` is declared with one field, so the checker gives it one argument");
                    let v = self.expr(arg, out);
                    return if nested {
                        Expr::call(Expr::ident("$some"), vec![v])
                    } else {
                        v
                    };
                }
                if self.payloadless(*con) {
                    Expr::Num(*variant as f64)
                } else {
                    let mut items = vec![Expr::Num(*variant as f64)];
                    items.extend(self.exprs(args, out));
                    let a = Expr::Array(items);
                    self.intern(a)
                }
            }
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
                let a = Expr::Array(self.exprs(xs, out));
                self.intern(a)
            }
            ExprKind::Field { base, index } | ExprKind::TupleIndex { base, index } => {
                let b = self.expr(base, out);
                Expr::index(b, Expr::Num(*index as f64))
            }
            ExprKind::Index { base, index, .. } => {
                let b = self.expr(base, out);
                let i = self.expr(index, out);
                Expr::call(Expr::ident("$list_get"), vec![b, i])
            }
            ExprKind::Block { stmts, tail } => {
                for s in stmts {
                    self.stmt(s, out);
                }
                match tail {
                    Some(t) => self.expr(t, out),
                    None => Expr::Num(0.0),
                }
            }
            ExprKind::If { cond, then, else_ } => {
                let c = self.expr(cond, out);
                // A ternary where both branches are expressions; otherwise a
                // temporary, because hoisting work out of a branch would run
                // it when the branch is not taken.
                let mut t_stmts = Vec::new();
                let t = self.expr(then, &mut t_stmts);
                let mut f_stmts = Vec::new();
                let f = self.expr(else_, &mut f_stmts);
                if t_stmts.is_empty() && f_stmts.is_empty() {
                    return Expr::cond(c, t, f);
                }
                let name = self.fresh();
                out.push(Stmt::Var { kind: VarKind::Let, name: name.clone(), init: None });
                t_stmts.push(assign(&name, t));
                f_stmts.push(assign(&name, f));
                out.push(Stmt::If { cond: c, then: t_stmts, else_: f_stmts });
                Expr::ident(name)
            }
            ExprKind::Match { scrutinee, arms } => {
                let s = self.expr(scrutinee, out);
                let name = self.fresh();
                out.push(Stmt::Var { kind: VarKind::Let, name: name.clone(), init: None });
                let mut body = Vec::new();
                self.match_stmts(s, arms, &mut body, Some(&name));
                out.extend(body);
                Expr::ident(name)
            }
            // A lambda is not a `Program::funcs` slot on this branch — it is
            // an arrow printed where it stands — so it has no row in
            // [`waiting_functions`], and the emitted body is asked instead:
            // `async` exactly when an `await` landed in it.
            //
            // The function *holding* the lambda is `async` either way: that
            // walk descends into a lambda body, so a call that puts an
            // `await` here has already marked the function the lambda sits
            // in. And an arrow printed `async` is a promise handed to
            // whoever calls it — see [`waiting_functions`] for why no
            // program in this tree builds one.
            ExprKind::Lambda { params, body, .. } => {
                let names: Vec<String> =
                    params.iter().map(|p| self.local_name_of(p)).collect();
                let mut inner = Vec::new();
                self.tail(body, &mut inner);
                let is_async = javascript::has_await(&inner);
                Expr::ArrowBlock { params: names, body: inner, is_async }
            }
            // Entering the function a mutually tail-recursive group was merged
            // into: an ordinary call, plus the entry, which is what the
            // dispatch parameter of `middle::tail_calls` is here.
            ExprKind::Continue { func: Some(target), entry, args } => {
                let args = std::iter::once(Expr::Num(*entry as f64))
                    .chain(self.exprs(args, out))
                    .collect();
                let call = Expr::call(Expr::ident(self.symbol(target.index())), args);
                if self.parks(target.index()) { Expr::awaited(call) } else { call }
            }
            // A jump back to the top of a loop is statements, and `tail` is
            // the only place they can go; a loop is a whole function body, and
            // nothing nests one in an expression. A `Closure` is
            // `middle::closures`, which is on the native branch and never runs
            // before this backend — an arrow function closing over its scope is
            // what the engine wants, so JavaScript is handed the tree with its
            // lambdas still lambdas (`middle::mod.rs`). All three are reachable
            // only from a tree assembled by hand.
            ExprKind::Loop { .. }
            | ExprKind::Continue { func: None, .. }
            | ExprKind::Closure { .. } => Expr::call(
                Expr::ident("$abort"),
                vec![Expr::Str("a middle-end node reached the JavaScript backend".to_string())],
            ),
            ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
                let is_and = matches!(e.kind, ExprKind::And { .. });
                let l = self.expr(lhs, out);
                let mut r_stmts = Vec::new();
                let r = self.expr(rhs, &mut r_stmts);
                if r_stmts.is_empty() {
                    return Expr::bin(if is_and { BinOp::And } else { BinOp::Or }, l, r);
                }
                let name = self.fresh();
                out.push(Stmt::Var {
                    kind: VarKind::Let,
                    name: name.clone(),
                    init: Some(l),
                });
                r_stmts.push(assign(&name, r));
                let cond = if is_and {
                    Expr::ident(name.clone())
                } else {
                    Expr::un(UnOp::Not, Expr::ident(name.clone()))
                };
                out.push(Stmt::If { cond, then: r_stmts, else_: Vec::new() });
                Expr::ident(name)
            }
            ExprKind::Coalesce { lhs, rhs, kind } => {
                // The right operand is evaluated only when the left is
                // `None`/`Err`.
                let (ok, held) = self.coalesce_split(lhs, out);
                let _ = kind;
                let mut r_stmts = Vec::new();
                let r = self.expr(rhs, &mut r_stmts);
                if r_stmts.is_empty() {
                    return Expr::cond(ok, held, r);
                }
                let result = self.fresh();
                out.push(Stmt::Var { kind: VarKind::Let, name: result.clone(), init: None });
                r_stmts.push(assign(&result, r));
                out.push(Stmt::If {
                    cond: ok,
                    then: vec![assign(&result, held)],
                    else_: r_stmts,
                });
                Expr::ident(result)
            }
            ExprKind::Try { base, .. } => {
                // `?` is the only early exit in the language. `.Err(e)` and
                // `.None` are both the value itself, so the failure case
                // returns what it matched.
                let b = self.expr(base, out);
                if let Some(nested) = self.option_nesting(&base.ty) {
                    let name = self.fresh();
                    out.push(Stmt::Var {
                        kind: VarKind::Const,
                        name: name.clone(),
                        init: Some(b),
                    });
                    out.push(Stmt::If {
                        cond: Expr::bin(
                            BinOp::StrictEq,
                            Expr::ident(name.clone()),
                            Expr::Undefined,
                        ),
                        then: vec![Stmt::Return(Some(Expr::Undefined))],
                        else_: Vec::new(),
                    });
                    return self.option_value(Expr::ident(name), nested);
                }
                let name = self.fresh();
                out.push(Stmt::Var {
                    kind: VarKind::Const,
                    name: name.clone(),
                    init: Some(b),
                });
                out.push(Stmt::If {
                    cond: Expr::bin(
                        BinOp::StrictNe,
                        Expr::index(Expr::ident(name.clone()), Expr::Num(0.0)),
                        Expr::Num(0.0),
                    ),
                    then: vec![Stmt::Return(Some(Expr::ident(name.clone())))],
                    else_: Vec::new(),
                });
                Expr::index(Expr::ident(name), Expr::Num(1.0))
            }
            ExprKind::Prim { op, prim, args } => {
                let a = self.exprs(args, out);
                self.prim_op(*op, *prim, a)
            }
            ExprKind::StructuralEq { negate, args } => {
                // Compiled at the type where the shape is known — which for a
                // primitive is `===` and no call at all. `desc_index` misses
                // only if nothing built a descriptor for this type, and then
                // the generic walker is still exactly right.
                let desc = args
                    .first()
                    .and_then(|a| self.program.desc_index.get(&a.ty).copied());
                let a = self.exprs(args, out);
                let call = match desc {
                    // Anything other than a pair is the generic walker's job.
                    Some(d) => match <[Expr; 2]>::try_from(a) {
                        Ok([lhs, rhs]) => self.eq_call(d, lhs, rhs),
                        Err(a) => Expr::call(Expr::ident("$eq"), a),
                    },
                    None => Expr::call(Expr::ident("$eq"), a),
                };
                if *negate {
                    Expr::un(UnOp::Not, call)
                } else {
                    call
                }
            }
            ExprKind::StructuralCmp { args, .. } => {
                let a = self.exprs(args, out);
                Expr::call(Expr::ident("$cmp"), a)
            }
            ExprKind::Template { parts } => {
                // Every part is rendered to a string from its static type, so
                // the whole interpolation is a concatenation: no array to
                // allocate and nothing for the runtime to walk. `$fmt` takes a
                // string as readily as the parts, which is what lets this be a
                // choice made here rather than a change to every consumer.
                let mut items = Vec::new();
                for p in parts {
                    match p {
                        typed::TemplatePart::Text(t) => items.push(Expr::Str(t.clone())),
                        typed::TemplatePart::Hole(h) => {
                            let v = self.expr(h, out);
                            items.push(self.render_hole(v, &h.ty));
                        }
                    }
                }
                let mut it = items.into_iter();
                // An interpolation with no parts at all is the empty string.
                let first = it.next().unwrap_or(Expr::Str(String::new()));
                // `Expr::bin` merges adjacent literals, so a template with no
                // holes collapses to one string here.
                it.fold(first, |acc, x| Expr::bin(BinOp::Add, acc, x))
            }
            ExprKind::CtxLit { bindings } => {
                // A context is a live effect handle and the runtime writes
                // through one, so neither it nor anything built inside it may
                // be shared with another occurrence.
                let items = self.inside_context(|g| {
                    bindings.iter().map(|(_, v)| g.expr(v, out)).collect::<Vec<Expr>>()
                });
                Expr::Array(items)
            }
            ExprKind::CtxGet { base, trait_id } => {
                let b = self.expr(base, out);
                let slot = match &base.ty {
                    Ty::Ctx(id) => self
                        .program
                        .ctx_layouts
                        .get(id)
                        .and_then(|l| l.iter().position(|t| t == trait_id))
                        .unwrap_or(0),
                    _ => 0,
                };
                Expr::index(b, Expr::Num(slot as f64))
            }
            ExprKind::CtxCall { .. } => Expr::Num(0.0),
            ExprKind::Intrinsic { name, args, .. } => {
                let a = self.exprs(args, out);
                let e = match name.as_str() {
                    "structuralEq" => {
                        // The last argument is the type descriptor, which the
                        // generic walker would have to interpret at run time
                        // and which `eq_call` reads here instead.
                        let mut a = a;
                        let desc = a.pop();
                        match (desc, <[Expr; 2]>::try_from(a)) {
                            (Some(Expr::Num(d)), Ok([lhs, rhs])) => {
                                self.eq_call(d as usize, lhs, rhs)
                            }
                            (_, Ok(pair)) => Expr::call(Expr::ident("$eq"), Vec::from(pair)),
                            (_, Err(a)) => Expr::call(Expr::ident("$eq"), a),
                        }
                    }
                    "structuralCompare" => {
                        let mut a = a;
                        a.pop();
                        Expr::call(Expr::ident("$cmp"), a)
                    }
                    "structuralHash" => {
                        let mut a = a;
                        a.pop();
                        Expr::call(Expr::ident("$hash"), vec![a.remove(0)])
                    }
                    // Both are emitted with a value and the descriptor of its
                    // type, and with nothing else.
                    name @ ("structuralShow" | "structuralToJson") => {
                        let Ok([value, desc]) = <[Expr; 2]>::try_from(a) else {
                            crate::ice!("{name} is emitted with a value and a descriptor")
                        };
                        let d = match desc {
                            Expr::Num(n) => Expr::ident(descriptor_name(n as usize)),
                            other => other,
                        };
                        let helper =
                            if name == "structuralShow" { "$show" } else { "$json_of" };
                        Expr::call(Expr::ident(helper), vec![value, d])
                    }
                    // The runner's end-of-block hook, which
                    // `middle::monomorphize` emits after every `test` body so
                    // that one implementation serves all three backends. It is
                    // an inline node rather than an intrinsic *function*
                    // because no Buri declaration produces it, and it is here
                    // rather than in `$run` because `$run` is handed a compiled
                    // body and would not know the block's index.
                    "test.leave" => Expr::call(Expr::ident("$test_leave"), a),
                    other => {
                        self.missing.push(other.to_string());
                        Expr::Num(0.0)
                    }
                };
                // A host call spelled as an inline node rather than reached
                // through an intrinsic *function*. `rc::suspends` is the seed
                // list the column itself is built from, so the two spellings
                // cannot disagree about which of them waits.
                if rc::suspends(name) { Expr::awaited(e) } else { e }
            }
            ExprKind::Error => Expr::Num(0.0),
        }
    }

    /// A template hole is rendered by its static type. `I8` and `F64` are both
    /// JS numbers, and `5` must not come out as `5.0`.
    /// A hole, rendered to a string from its static type.
    ///
    /// Every arm has to produce a string, because the parts are joined with
    /// `+`: two boolean holes side by side would otherwise add to `1`. `Str`
    /// and `Char` are already strings — a `Char` is a one-scalar string — and
    /// everything else says so explicitly.
    fn render_hole(&self, v: Expr, ty: &Ty) -> Expr {
        match self.tables.as_prim(ty) {
            Some(p) if p.is_integer() => Expr::call(Expr::ident("String"), vec![v]),
            Some(p) if p.is_float() => Expr::call(Expr::ident("$f64"), vec![v]),
            Some(Prim::Str) | Some(Prim::Char) => v,
            _ => Expr::call(Expr::ident("$str"), vec![v]),
        }
    }

    /// Operands, in the order they are written.
    ///
    /// Evaluation order is specified as left to right (SPEC 8.2), and an
    /// operand that needs statements is the one place that is not free. Given
    /// `f(g(), match (x) { .. })`, the match becomes statements *before* the
    /// call, so `g()` — still sitting unevaluated in the argument list — would
    /// run after them. Anything computed before such an operand is therefore
    /// pinned to a binding placed ahead of those statements.
    ///
    /// A literal or a name is exempt: it denotes the same value wherever it is
    /// read, since the language has no assignment (SPEC 8.1).
    fn exprs(&mut self, xs: &[typed::Expr], out: &mut Vec<Stmt>) -> Vec<Expr> {
        let mut vals: Vec<Expr> = Vec::new();
        for x in xs {
            let before = out.len();
            let v = self.expr(x, out);
            if out.len() > before {
                let mut at = before;
                for prev in vals.iter_mut() {
                    if prev.is_pure_literal() {
                        continue;
                    }
                    let name = self.fresh();
                    let init = std::mem::replace(prev, Expr::ident(name.clone()));
                    out.insert(
                        at,
                        Stmt::Var { kind: VarKind::Const, name, init: Some(init) },
                    );
                    at += 1;
                }
            }
            vals.push(v);
        }
        vals
    }

    fn symbol(&self, i: usize) -> String {
        self.program
            .funcs
            .get(i)
            .map(|f| f.symbol.clone())
            .unwrap_or_else(|| format!("$missing{i}"))
    }

    /// A literal is written in the representation its type has: `5` at `I32`
    /// and `5n` at `I64`, which is the whole of why the wide widths are exact.
    fn int_literal(&self, v: u128, neg: bool, ty: &Ty) -> Expr {
        if self.prim_of(ty).is_some_and(Prim::is_bigint) {
            let digits = v.to_string();
            return Expr::BigInt(if neg { format!("-{digits}") } else { digits });
        }
        let n = v as f64;
        Expr::Num(if neg { -n } else { n })
    }

    // -----------------------------------------------------------------------
    // Primitive operations
    // -----------------------------------------------------------------------

    fn prim_op(&mut self, op: PrimOp, p: Prim, mut args: Vec<Expr>) -> Expr {
        let big = p.is_bigint();
        let float = p.is_float();
        let two = |op: BinOp, args: &mut Vec<Expr>| {
            let b = args.pop().or_ice(BINARY_ARITY);
            let a = args.pop().or_ice(BINARY_ARITY);
            Expr::bin(op, a, b)
        };
        // The bitwise operators, where the answer is already in hand.
        //
        // This belongs here and not in `javascript::simplify_bin`, because the
        // rewrites depend on the *width*: `x | 0` is the identity at 64 bits
        // and a truncation to 32 in JavaScript, so a folder that cannot see
        // the Buri type would be folding the wrong operator. Above 32 bits
        // each of these otherwise costs a call into the runtime.
        if p.is_integer() {
            if let Some(v) = fold_bitwise(op, p, &mut args) {
                return v;
            }
        }
        match op {
            PrimOp::Not => Expr::un(UnOp::Not, args.pop().or_ice(UNARY_ARITY)),
            // A float is the one primitive `===` gets wrong, and only at
            // `NaN`. The ordering operators below keep IEEE's unordered
            // answer, which SPEC 6.2 still pins.
            PrimOp::Eq | PrimOp::Ne if float => {
                let b = args.pop().or_ice(BINARY_ARITY);
                let a = args.pop().or_ice(BINARY_ARITY);
                let same = float_eq(a, b);
                if op == PrimOp::Eq { same } else { Expr::un(UnOp::Not, same) }
            }
            PrimOp::Eq => two(BinOp::StrictEq, &mut args),
            PrimOp::Ne => two(BinOp::StrictNe, &mut args),
            PrimOp::Lt => two(BinOp::Lt, &mut args),
            PrimOp::Le => two(BinOp::Le, &mut args),
            PrimOp::Gt => two(BinOp::Gt, &mut args),
            PrimOp::Ge => two(BinOp::Ge, &mut args),
            // A `BigInt` has the bitwise operators themselves, and two
            // operands inside the type's range give a result inside it — the
            // one exception being an unsigned complement, which is negative
            // and needs narrowing back. JavaScript's own `&` coerces to 32-bit
            // signed and would have discarded everything above bit 31.
            PrimOp::BitAnd | PrimOp::BitOr | PrimOp::BitXor | PrimOp::BitNot
                if big =>
            {
                let v = match op {
                    PrimOp::BitAnd => two(BinOp::BitAnd, &mut args),
                    PrimOp::BitOr => two(BinOp::BitOr, &mut args),
                    PrimOp::BitXor => two(BinOp::BitXor, &mut args),
                    _ => Expr::un(UnOp::BitNot, args.pop().or_ice(UNARY_ARITY)),
                };
                if p.is_signed() || op != PrimOp::BitNot {
                    v
                } else {
                    as_uint_n(p.bits(), v)
                }
            }
            // Unsigned and narrow. The native operators are exact here, but
            // their result is *signed* 32-bit — `0x80000000 | 0` came back
            // negative, and `~0` on a `U8` came back as `-1` rather than
            // `255`. Narrowing the result to the type's own width fixes the
            // representation without touching the operation.
            PrimOp::BitAnd | PrimOp::BitOr | PrimOp::BitXor | PrimOp::BitNot
                if p.is_integer() && !p.is_signed() =>
            {
                let v = match op {
                    PrimOp::BitAnd => two(BinOp::BitAnd, &mut args),
                    PrimOp::BitOr => two(BinOp::BitOr, &mut args),
                    PrimOp::BitXor => two(BinOp::BitXor, &mut args),
                    _ => Expr::un(UnOp::BitNot, args.pop().or_ice(UNARY_ARITY)),
                };
                Expr::call(Expr::ident("$umask"), vec![v, Expr::Num(p.bits() as f64)])
            }
            PrimOp::BitAnd => two(BinOp::BitAnd, &mut args),
            PrimOp::BitOr => two(BinOp::BitOr, &mut args),
            PrimOp::BitXor => two(BinOp::BitXor, &mut args),
            PrimOp::BitNot => Expr::un(UnOp::BitNot, args.pop().or_ice(UNARY_ARITY)),
            PrimOp::Neg => {
                let v = Expr::un(UnOp::Neg, args.pop().or_ice(UNARY_ARITY));
                self.rounded(v, p)
            }
            PrimOp::Add | PrimOp::Sub | PrimOp::Mul => {
                let jsop = match op {
                    PrimOp::Add => BinOp::Add,
                    PrimOp::Sub => BinOp::Sub,
                    _ => BinOp::Mul,
                };
                let v = two(jsop, &mut args);
                // Overflow and underflow are undefined, so the operation is
                // emitted and nothing else. Code that wants a defined answer
                // at the boundary says so: `checkedAdd`, `wrappingAdd`,
                // `saturatingAdd`.
                self.rounded(v, p)
            }
            // Integer division and remainder go through the runtime because
            // a zero divisor aborts — there is no answer to give. Divide by a
            // literal that is not zero and there is nothing to ask, so the
            // check and the call both go.
            PrimOp::Div => {
                if float {
                    let v = two(BinOp::Div, &mut args);
                    self.rounded(v, p)
                } else if nonzero_literal(args.get(1)) {
                    let v = two(BinOp::Div, &mut args);
                    // A `BigInt` quotient already truncates toward zero.
                    if big {
                        v
                    } else {
                        Expr::call(Expr::member(Expr::ident("Math"), "trunc"), vec![v])
                    }
                } else if big {
                    Expr::call(Expr::ident("$divb"), args)
                } else {
                    Expr::call(Expr::ident("$divi"), args)
                }
            }
            PrimOp::Rem => {
                if float {
                    let v = two(BinOp::Rem, &mut args);
                    self.rounded(v, p)
                } else if nonzero_literal(args.get(1)) {
                    two(BinOp::Rem, &mut args)
                } else if big {
                    Expr::call(Expr::ident("$remb"), args)
                } else {
                    Expr::call(Expr::ident("$remi"), args)
                }
            }
        }
    }

    /// `F32` is IEEE-754 binary32, and JavaScript has only binary64, so every
    /// `F32` result is rounded back. `F64` needs nothing.
    fn rounded(&self, v: Expr, p: Prim) -> Expr {
        if p == Prim::F32 {
            Expr::call(Expr::member(Expr::ident("Math"), "fround"), vec![v])
        } else {
            v
        }
    }

    // -----------------------------------------------------------------------
    // Descriptors and the test harness
    // -----------------------------------------------------------------------

    fn descriptor(&self, d: &Desc) -> Expr {
        match d {
            Desc::Prim(p) => {
                // "i" is an integer and "I" a `BigInt` one. The runtime walkers
                // need the two apart only where JSON is involved: a document
                // carries a double, so decoding into one builds a `BigInt`.
                let tag = match p {
                    Prim::Str => "s",
                    Prim::Char => "c",
                    Prim::F32 | Prim::F64 => "f",
                    Prim::Bool => "b",
                    p if p.is_bigint() => "I",
                    _ => "i",
                };
                Expr::Array(vec![Expr::Num(0.0), Expr::Str(tag.into())])
            }
            Desc::Unit => Expr::Array(vec![Expr::Num(1.0)]),
            Desc::Struct { name, record, fields } => Expr::Array(vec![
                Expr::Num(2.0),
                Expr::Str(name.clone()),
                Expr::Bool(*record),
                Expr::Array(fields.iter().map(|f| Expr::Str(f.name.clone())).collect()),
                Expr::Array(fields.iter().map(|f| Expr::ident(descriptor_name(f.ty))).collect()),
            ]),
            Desc::Enum { name, variants } => Expr::Array(vec![
                Expr::Num(3.0),
                Expr::Str(name.clone()),
                Expr::Array(
                    variants
                        .iter()
                        .map(|v| {
                            Expr::Array(vec![
                                Expr::Str(v.name.clone()),
                                Expr::Bool(v.record),
                                Expr::Array(
                                    v.fields.iter().map(|f| Expr::Str(f.name.clone())).collect(),
                                ),
                                Expr::Array(
                                    v.fields
                                        .iter()
                                        .map(|f| Expr::ident(descriptor_name(f.ty)))
                                        .collect(),
                                ),
                            ])
                        })
                        .collect(),
                ),
                Expr::Bool(d.payloadless()),
            ]),
            Desc::Array(inner) => {
                Expr::Array(vec![Expr::Num(4.0), Expr::ident(descriptor_name(*inner))])
            }
            Desc::Tuple(items) => Expr::Array(vec![
                Expr::Num(5.0),
                Expr::Array(items.iter().map(|t| Expr::ident(descriptor_name(*t))).collect()),
            ]),
            Desc::Option(inner) => {
                Expr::Array(vec![Expr::Num(7.0), Expr::ident(descriptor_name(*inner))])
            }
            Desc::Opaque(_) | Desc::Reserved => Expr::Array(vec![Expr::Num(6.0)]),
        }
    }

    // -----------------------------------------------------------------------
    // Structural equality, compiled at the type
    // -----------------------------------------------------------------------

    /// How values of the type descriptor `i` describes are compared.
    fn eq_kind(&self, i: usize) -> EqKind {
        match self.program.descriptors.get(i) {
            Some(Desc::Prim(p)) if p.is_float() => EqKind::Float,
            Some(Desc::Prim(_) | Desc::Unit) => EqKind::Identity,
            Some(e @ Desc::Enum { .. }) if e.payloadless() => EqKind::Identity,
            Some(Desc::Struct { .. } | Desc::Enum { .. } | Desc::Array(_) | Desc::Tuple(_)) => {
                EqKind::Compiled
            }
            // `Option`'s nesting is carried by a `{$n}` box that only the
            // runtime knows how to read, and an opaque type has no shape to
            // compile. Both answer correctly through `$eq` already.
            _ => EqKind::Generic,
        }
    }

    /// The expression that compares two values of the described type.
    fn eq_call(&self, i: usize, a: Expr, b: Expr) -> Expr {
        match self.eq_kind(i) {
            EqKind::Identity => Expr::bin(BinOp::StrictEq, a, b),
            EqKind::Float => float_eq(a, b),
            EqKind::Compiled => Expr::call(Expr::ident(eq_name(i)), vec![a, b]),
            EqKind::Generic => Expr::call(Expr::ident("$eq"), vec![a, b]),
        }
    }

    /// The comparison for descriptor `i`, as its own function — or `None` for
    /// the types that need none, because `===` or `$eq` already says it.
    ///
    /// Every one of these begins `if (a === b) return true;`. SPEC 7.2 makes
    /// `==` an equivalence relation, so a reference that is already known to
    /// be the same value is already known to be equal, and the walk below can
    /// only reach the same answer more slowly.
    fn eq_decl(&self, i: usize) -> Option<Stmt> {
        if self.eq_kind(i) != EqKind::Compiled {
            return None;
        }
        let a = || Expr::ident("a");
        let b = || Expr::ident("b");
        let at = |e: Expr, k: usize| Expr::index(e, Expr::Num(k as f64));
        let mut body = vec![Stmt::If {
            cond: Expr::bin(BinOp::StrictEq, a(), b()),
            then: vec![Stmt::Return(Some(Expr::Bool(true)))],
            else_: Vec::new(),
        }];

        // The conjunction comparing a run of components held at `offset..`.
        let fields = |types: &[usize], offset: usize| -> Expr {
            let mut it = types.iter().enumerate().map(|(k, t)| {
                self.eq_call(*t, at(a(), k + offset), at(b(), k + offset))
            });
            match it.next() {
                None => Expr::Bool(true),
                Some(first) => it.fold(first, |acc, x| Expr::bin(BinOp::And, acc, x)),
            }
        };

        match self.program.descriptors.get(i)? {
            Desc::Struct { fields: fs, .. } => {
                let types: Vec<usize> = fs.iter().map(|f| f.ty).collect();
                body.push(Stmt::Return(Some(fields(&types, 0))));
            }
            Desc::Tuple(types) => {
                body.push(Stmt::Return(Some(fields(types, 0))));
            }
            Desc::Array(inner) => {
                let len = |e: Expr| Expr::member(e, "length");
                let idx = || Expr::ident("$i");
                body.push(Stmt::If {
                    cond: Expr::bin(BinOp::StrictNe, len(a()), len(b())),
                    then: vec![Stmt::Return(Some(Expr::Bool(false)))],
                    else_: Vec::new(),
                });
                // An indexed loop, so nothing allocates an iterator per call.
                body.push(Stmt::Var {
                    kind: VarKind::Let,
                    name: "$i".into(),
                    init: Some(Expr::Num(0.0)),
                });
                let step = self.eq_call(
                    *inner,
                    Expr::index(a(), idx()),
                    Expr::index(b(), idx()),
                );
                body.push(Stmt::While {
                    cond: Expr::bin(BinOp::Lt, idx(), len(a())),
                    body: vec![
                        Stmt::If {
                            cond: Expr::un(UnOp::Not, step),
                            then: vec![Stmt::Return(Some(Expr::Bool(false)))],
                            else_: Vec::new(),
                        },
                        Stmt::Expr(Expr::Assign {
                            target: Box::new(idx()),
                            value: Box::new(Expr::bin(BinOp::Add, idx(), Expr::Num(1.0))),
                        }),
                    ],
                });
                body.push(Stmt::Return(Some(Expr::Bool(true))));
            }
            Desc::Enum { variants, .. } => {
                // The tag first: two different variants are never equal, and
                // once the tag agrees the payload is known.
                body.push(Stmt::If {
                    cond: Expr::bin(BinOp::StrictNe, at(a(), 0), at(b(), 0)),
                    then: vec![Stmt::Return(Some(Expr::Bool(false)))],
                    else_: Vec::new(),
                });
                let cases: Vec<(Option<Expr>, Vec<Stmt>)> = variants
                    .iter()
                    .enumerate()
                    .map(|(k, v)| {
                        // Payloads start at 1: slot 0 is the tag.
                        let types: Vec<usize> = v.fields.iter().map(|f| f.ty).collect();
                        (Some(Expr::Num(k as f64)), vec![Stmt::Return(Some(fields(&types, 1)))])
                    })
                    .chain(std::iter::once((None, vec![Stmt::Return(Some(Expr::Bool(false)))])))
                    .collect();
                body.push(Stmt::Switch { disc: at(a(), 0), cases });
            }
            _ => return None,
        }
        Some(Stmt::Func {
            name: eq_name(i),
            params: vec!["a".into(), "b".into()],
            body,
            // Structural equality is a walk over values the program already
            // holds; nothing in one waits.
            is_async: false,
        })
    }

    fn test_harness(&mut self) -> Stmt {
        let cases: Vec<Expr> = self
            .program
            .roots
            .tests()
            .iter()
            .map(|t| {
                Expr::Array(vec![
                    Expr::Str(t.name.clone()),
                    Expr::Str(t.module.clone()),
                    Expr::ident(self.symbol(t.func.index())),
                ])
            })
            .collect();
        // `$cases` is built as a tree rather than as text so that the names in
        // it are identifiers the later passes can rewrite. Two tests with the
        // same body are one function afterwards, and each still reports under
        // its own name.
        self.extra.push(Stmt::Var {
            kind: VarKind::Const,
            name: "$cases".into(),
            init: Some(Expr::Array(cases)),
        });
        // `async`, and each case awaited, because a test that reaches a
        // suspending host call is printed `async` and hands back a promise —
        // one that would otherwise be recorded as a pass before the test had
        // run. Awaiting a case that is not `async` costs a microtask and
        // changes nothing, which is why this is unconditional: the driver is
        // one function shared by every case in the artifact.
        //
        // `$t.from` is the handle table's length as the block starts, which is
        // what makes `$test_leave` a question about *this* block's doubles: the
        // table grows for the life of the process. `buri_rt_test_enter` marks
        // the same watermark natively, and this is the line that has to do it
        // here because JavaScript has no `enter`.
        Stmt::Raw(format!(
            "{}async function $run(filter){{const out=[];for(const[n,m,f]of $cases){{\
             if(filter&&!n.includes(filter))continue;\
             $t.from=$t.h.length;\
             const started=Date.now();try{{await f();out.push({{name:n,module:m,ok:true,ms:Date.now()-started}});}}\
             catch(e){{out.push({{name:n,module:m,ok:false,ms:Date.now()-started,\
             error:e&&e.$assert?e.$assert:{{message:String(e&&e.message||e)}},\
             stack:e&&e.stack||\"\"}});}}}}\
             return out;}}",
            ""
        ))
    }
}

fn assign(name: &str, value: Expr) -> Stmt {
    Stmt::Expr(Expr::Assign {
        target: Box::new(Expr::ident(name.to_string())),
        value: Box::new(value),
    })
}

/// Checks that every intrinsic the program reaches exists in the runtime.
pub fn check_intrinsics(missing: &[String]) -> Vec<String> {
    let known = runtime_names();
    missing
        .iter()
        .filter(|m| !known.contains(&format!("${}", m.replace('.', "_"))))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg(i: usize) -> Expr {
        Expr::ident(format!("$$arg{i}"))
    }

    fn survey(e: &Expr, n: usize) -> ArgUse {
        let mut out = ArgUse::new(n);
        survey_args(e, false, &mut out);
        out
    }

    /// The shape every runtime forwarder has: each argument once, in order,
    /// unconditionally. This is the case inlining exists for.
    #[test]
    fn a_plain_forwarder_uses_each_argument_once_in_order() {
        let e = Expr::call(Expr::ident("$str_split"), vec![arg(0), arg(1), arg(2)]);
        let u = survey(&e, 3);
        assert_eq!(u.args.iter().map(|a| a.count).collect::<Vec<_>>(), vec![1, 1, 1]);
        assert!(u.args.iter().all(|a| !a.conditional));
        assert_eq!(u.order, vec![0, 1, 2]);
    }

    /// `signum` names its argument three times. Pasting it at a call site
    /// would evaluate the argument three times.
    #[test]
    fn a_repeated_argument_is_seen() {
        let v = arg(0);
        let e = Expr::cond(
            Expr::bin(BinOp::Lt, v.clone(), Expr::Num(0.0)),
            Expr::Num(-1.0),
            Expr::cond(Expr::bin(BinOp::Gt, v.clone(), Expr::Num(0.0)), Expr::Num(1.0), v),
        );
        let u = survey(&e, 1);
        assert!(u.args[0].count > 1);
    }

    /// An argument under a `?:` branch might not run at all.
    #[test]
    fn an_argument_under_a_branch_is_conditional() {
        let e = Expr::cond(Expr::ident("c"), arg(0), Expr::Num(0.0));
        assert!(survey(&e, 1).args[0].conditional);
    }

    /// So is the right operand of a short circuit.
    #[test]
    fn an_argument_after_a_short_circuit_is_conditional() {
        let e = Expr::bin(BinOp::And, Expr::ident("c"), arg(0));
        assert!(survey(&e, 1).args[0].conditional);
        let e = Expr::bin(BinOp::And, arg(0), Expr::ident("c"));
        assert!(!survey(&e, 1).args[0].conditional);
    }

    /// Evaluation order is specified, so an expansion that reads its second
    /// argument first is not the call it replaces.
    #[test]
    fn arguments_out_of_order_are_seen() {
        let e = Expr::call(Expr::ident("$f"), vec![arg(1), arg(0)]);
        assert_eq!(survey(&e, 2).order, vec![1, 0]);
    }

    #[test]
    fn size_counts_every_node() {
        assert_eq!(js_size(&Expr::ident("x")), 1);
        assert_eq!(js_size(&Expr::call(Expr::ident("f"), vec![Expr::ident("x")])), 3);
    }
}
