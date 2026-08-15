//! The JavaScript backend.
//!
//! Turns the monomorphized HIR into a JavaScript AST. Evaluation order is
//! fully specified (SPEC 8.2), and this is where that matters most: an
//! expression that needs statements to compute is hoisted into the enclosing
//! statement list rather than wrapped in a closure, except where hoisting
//! would move work across a branch that might not be taken.

use crate::hir::{self, ExprKind, PatKind, PrimOp};
use crate::js::{self, BinOp, Expr, Stmt, UnOp, VarKind};
use crate::mono::{Desc, Program};
use crate::tco;
use crate::types::{LocalId, Prim, Ty, TyDef};
use std::collections::HashMap;

pub struct Options {
    pub pretty: bool,
    /// Emitted so an abort names the function it came from.
    pub debug_names: bool,
    /// Whether a match keeps a test on its last arm, and an abort behind it,
    /// even though `exhaust.rs` has already proved one of the arms runs.
    ///
    /// On in debug, off in release. It is the backend's own belt to the
    /// checker's braces, and `release_and_debug_agree` is what says the two
    /// still compute the same answers.
    pub defensive_aborts: bool,
}

pub struct Output {
    pub stmts: Vec<Stmt>,
    /// Names the minifier must not rename.
    pub roots: Vec<String>,
    pub missing_intrinsics: Vec<String>,
}

pub struct Gen<'a> {
    pub(crate) program: &'a Program,
    pub(crate) tables: &'a crate::types::Tables,
    /// The current function's local names.
    names: HashMap<LocalId, String>,
    temp: usize,
    pub(crate) missing: Vec<String>,
    runtime: Vec<String>,
    plan: tco::Plan,
    /// How the function being emitted returns: plainly, by looping, or by
    /// dispatching within a merged group.
    mode: TailMode,
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
}

/// Tail-call elimination is a property of how a body is emitted, so it lives
/// here rather than as a rewrite of the tree.
#[derive(Clone)]
enum TailMode {
    Return,
    /// A function that tail-calls itself becomes a loop with parameter
    /// rebinding, which costs nothing.
    SelfLoop { self_index: usize, params: Vec<String> },
    /// A group merged into one function with a dispatch switch, which costs
    /// one branch per bounce.
    Group { members: Vec<usize>, which: String, slots: Vec<String> },
}

/// The runtime's exports, so a missing one is a build error rather than a
/// `ReferenceError` at run time.
fn runtime_names() -> Vec<String> {
    let mut out = Vec::new();
    let src = crate::runtime_source();
    let bytes = src.as_bytes();
    // A byte scan: the runtime is mostly ASCII but its comments are not, so
    // this never slices the string at a position it has not checked.
    for i in 0..bytes.len() {
        for kw in [b"function $".as_slice(), b"const $".as_slice(), b"let $".as_slice()] {
            if bytes[i..].starts_with(kw) {
                let start = i + kw.len() - 1;
                let mut j = start;
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'$')
                {
                    j += 1;
                }
                if let Ok(name) = std::str::from_utf8(&bytes[start..j]) {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}

pub fn generate(program: &Program, tables: &crate::types::Tables, opts: &Options) -> Output {
    let mut g = Gen {
        program,
        tables,
        names: HashMap::new(),
        temp: 0,
        missing: Vec::new(),
        runtime: runtime_names(),
        plan: tco::analyze(program),
        mode: TailMode::Return,
        defensive_aborts: opts.defensive_aborts,
        extra: Vec::new(),
        consts: Vec::new(),
        const_index: HashMap::new(),
        in_context: false,
    };

    let mut stmts = Vec::new();
    // A program that reaches the filesystem or standard input needs node's
    // `require`, which an ES module does not have. Emitted only when it is
    // needed, so a browser artifact never names `node:module`.
    if needs_require(program) {
        stmts.push(Stmt::Raw(
            "import{createRequire as $createRequire}from\"node:module\";\n\
             const $require=$createRequire(import.meta.url);"
                .to_string(),
        ));
    }
    // The runtime, one declaration at a time so that dead code elimination can
    // drop what a program does not reach. It is hand-written JavaScript, so it
    // is compacted by the tokenizer in `js::strip` rather than by the AST
    // printer.
    for (name, src) in js::split_declarations(crate::runtime_source()) {
        let src = if opts.pretty { src } else { js::strip(&src) };
        stmts.push(Stmt::RawDecl { name, src });
    }
    // Where the shared constants go, once the bodies below have said which
    // ones they need.
    let runtime_end = stmts.len();

    // Type descriptors, for the structural operations `derive` stands for.
    // Declared empty first and filled afterwards, because a recursive type's
    // descriptor names itself and a mutually recursive pair names each other.
    for i in 0..program.descriptors.len() {
        stmts.push(Stmt::Var {
            kind: VarKind::Const,
            name: desc_name(i),
            init: Some(Expr::Array(Vec::new())),
        });
    }
    for (i, d) in program.descriptors.iter().enumerate() {
        let Expr::Array(items) = g.descriptor(d) else { continue };
        stmts.push(Stmt::Expr(Expr::call(
            Expr::member(Expr::ident(desc_name(i)), "push"),
            items,
        )));
    }

    for gi in 0..g.plan.groups.len() {
        let merged = g.merged_group(gi);
        stmts.push(merged);
    }

    for (i, f) in program.funcs.iter().enumerate() {
        g.names.clear();
        g.temp = 0;
        g.mode = TailMode::Return;
        for (li, l) in f.locals.iter().enumerate() {
            g.names.insert(LocalId(li as u32), local_name(li, &l.name));
        }
        let params: Vec<String> =
            f.params.iter().map(|p| g.names[p].clone()).collect();

        // A member of a merged group keeps its name and forwards, so a
        // reference to it from outside the group still works.
        if let Some((group, index)) = g.plan.group_of(i) {
            let call = Expr::call(
                Expr::ident(group_name(group)),
                std::iter::once(Expr::Num(index as f64))
                    .chain(params.iter().map(|p| Expr::ident(p.clone())))
                    .collect(),
            );
            stmts.push(Stmt::Func {
                name: f.symbol.clone(),
                params,
                body: vec![Stmt::Return(Some(call))],
            });
            continue;
        }
        if g.plan.self_loop.contains(&i) {
            g.mode = TailMode::SelfLoop { self_index: i, params: params.clone() };
        }

        let body = match (&f.body, &f.intrinsic) {
            (Some(e), _) => {
                let mut out = Vec::new();
                g.tail(e, &mut out);
                g.wrap_loop(out)
            }
            (None, Some(key)) => {
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
            (None, None) => vec![Stmt::Return(Some(Expr::Num(0.0)))],
        };
        stmts.push(Stmt::Func { name: f.symbol.clone(), params, body });
        let _ = i;
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
    if let Some(entry) = program.entry {
        // `.Ok(())` exits 0. `.Err(msg)` prints `msg` to stderr and exits 1.
        let sym = program.funcs[entry].symbol.clone();
        roots.push(sym.clone());
        stmts.push(Stmt::Raw(format!(
            "try{{const r={sym}();$host.flush();\
             if(r[0]!==0){{$write(2,$str(r[1])+\"\\n\");\
             if(typeof process!==\"undefined\")process.exit(1);}}}}\
             catch(e){{$host.flush();\
             $write(2,(e&&e.message?e.message:String(e))+\"\\n\");\
             if(e&&e.stack)$write(2,e.stack+\"\\n\");\
             if(typeof process!==\"undefined\")process.exit(1);}}"
        )));
    }

    if !program.tests.is_empty() {
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

/// Whether any reachable intrinsic touches the host filesystem.
fn needs_require(program: &Program) -> bool {
    program.funcs.iter().any(|f| {
        f.intrinsic.as_ref().is_some_and(|k| {
            k.starts_with("host.HostFs.") || k.starts_with("host.HostStdin.")
        })
    })
}

fn group_name(i: usize) -> String {
    format!("$tc{i}")
}

fn desc_name(i: usize) -> String {
    format!("$D{i}")
}

/// The descriptor a generated call passes to a runtime function.
pub fn descriptor_name(i: usize) -> String {
    desc_name(i)
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

    pub(crate) fn prim_op_pub(&mut self, op: PrimOp, prim: Option<Prim>, args: Vec<Expr>) -> Expr {
        self.prim_op(op, prim, args)
    }

    fn fresh(&mut self) -> String {
        self.temp += 1;
        format!("$t{}", self.temp)
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
        let key = js::print(&[Stmt::Expr(e.clone())], false);
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
        let callee = &program.funcs[index];
        if callee.body.is_some() {
            return None;
        }
        let key = callee.intrinsic.clone()?;

        // Built once against placeholders, purely to see what the expansion
        // does with each argument.
        let probe: Vec<Expr> =
            (0..args.len()).map(|i| Expr::ident(format!("$$arg{i}"))).collect();
        let shape = self.intrinsic(&key, &probe, callee)?;
        if js_size(&shape) > MAX_INLINE_INTRINSIC {
            return None;
        }

        let mut seen = ArgUse { counts: vec![0; args.len()], conditional: vec![false; args.len()], order: Vec::new() };
        survey_args(&shape, false, &mut seen);
        let mut last = 0usize;
        for i in seen.order.iter().copied() {
            if args[i].is_pure_literal() {
                continue;
            }
            if seen.counts[i] > 1 || seen.conditional[i] || i < last {
                return None;
            }
            last = i;
        }

        self.intrinsic(&key, args, callee)
    }
}

/// A call worth replacing is small; anything larger is cheaper as a call.
/// `signum` and the checked-arithmetic expansions sit above this deliberately.
const MAX_INLINE_INTRINSIC: usize = 8;

/// How many fields a functional update will write out in full before falling
/// back to copying the base and patching it.
const MAX_SPELLED_UPDATE: usize = 8;

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

#[derive(Default)]
struct ArgUse {
    counts: Vec<usize>,
    /// Whether the argument appears only on one side of a conditional, where
    /// it might not be evaluated.
    conditional: Vec<bool>,
    /// The order the arguments are reached in.
    order: Vec<usize>,
}

/// Records where an expansion put each placeholder argument.
fn survey_args(e: &Expr, under_cond: bool, out: &mut ArgUse) {
    if let Expr::Ident(name) = e {
        if let Some(n) = name.strip_prefix("$$arg") {
            if let Ok(i) = n.parse::<usize>() {
                if i < out.counts.len() {
                    out.counts[i] += 1;
                    out.conditional[i] |= under_cond;
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
        Expr::Spread(x) => survey_args(x, under_cond, out),
        _ => {}
    }
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
        Expr::Spread(x) => js_size(x),
        _ => 0,
    }
}

impl<'a> Gen<'a> {

    // -----------------------------------------------------------------------
    // Statement position
    // -----------------------------------------------------------------------

    /// Emits `e` in tail position: the statements end with a `return`.
    fn tail(&mut self, e: &hir::Expr, out: &mut Vec<Stmt>) {
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
            // A tail call the plan asked us to eliminate becomes a rebinding
            // of the parameters and a jump back to the top of the loop.
            ExprKind::CallFn { func, args, .. } if self.is_eliminable(func.index()) => {
                let values = self.exprs(args, out);
                self.rebind(func.index(), values, out);
            }
            _ => {
                let v = self.expr(e, out);
                out.push(Stmt::Return(Some(v)));
            }
        }
    }

    fn is_eliminable(&self, callee: usize) -> bool {
        match &self.mode {
            TailMode::Return => false,
            TailMode::SelfLoop { self_index, .. } => callee == *self_index,
            TailMode::Group { members, .. } => members.contains(&callee),
        }
    }

    /// Assigns the new arguments and continues. The values are computed into
    /// temporaries first, because an argument may name a parameter the
    /// rebinding is about to overwrite.
    fn rebind(&mut self, callee: usize, values: Vec<Expr>, out: &mut Vec<Stmt>) {
        let (targets, which): (Vec<String>, Option<(String, usize)>) = match &self.mode {
            TailMode::SelfLoop { params, .. } => (params.clone(), None),
            TailMode::Group { members, which, slots } => {
                let index = members.iter().position(|m| *m == callee).unwrap_or(0);
                (slots.clone(), Some((which.clone(), index)))
            }
            TailMode::Return => return,
        };
        // Rebinding the parameters is a parallel move: every new value is
        // computed from the old ones, and then all the slots change at once.
        // A temporary per parameter is always correct and usually
        // unnecessary.
        let remaining = values.clone();
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
        let mut deferred: Vec<(usize, Expr)> = Vec::new();
        for (i, v) in values.into_iter().enumerate() {
            // `values` was consumed by the loop, so what is still to come is
            // asked of the original expressions, kept alongside.
            if remaining[i + 1..].iter().any(|later| reads_ident(later, &targets[i])) {
                let t = self.fresh();
                out.push(Stmt::Var { kind: VarKind::Const, name: t.clone(), init: Some(v) });
                deferred.push((i, Expr::ident(t)));
            } else {
                out.push(assign(&targets[i], v));
            }
        }
        // Everything parked above, now that every read has happened.
        for (i, v) in deferred {
            out.push(assign(&targets[i], v));
        }
        if let Some((w, index)) = which {
            out.push(assign(&w, Expr::Num(index as f64)));
        }
        out.push(Stmt::Continue);
    }

    fn wrap_loop(&mut self, body: Vec<Stmt>) -> Vec<Stmt> {
        match &self.mode {
            TailMode::SelfLoop { .. } => {
                vec![Stmt::While { cond: Expr::Bool(true), body }]
            }
            _ => body,
        }
    }

    /// One function per mutually tail-recursive group: the members' parameters
    /// share a set of slots, and a switch selects whose body runs.
    fn merged_group(&mut self, group: usize) -> Stmt {
        let members = self.program.funcs.len();
        let _ = members;
        let group_members = self.plan.groups[group].clone();
        let arity = tco::max_arity(self.program, &group_members);
        let which = "$w".to_string();
        let slots: Vec<String> = (0..arity).map(|i| format!("$a{i}")).collect();

        let mut cases = Vec::new();
        for (index, f) in group_members.iter().enumerate() {
            let func = &self.program.funcs[*f];
            self.names.clear();
            self.temp = 0;
            for (li, l) in func.locals.iter().enumerate() {
                self.names.insert(LocalId(li as u32), local_name(li, &l.name));
            }
            // The parameters read from the shared slots.
            for (pi, p) in func.params.iter().enumerate() {
                self.names.insert(*p, slots[pi].clone());
            }
            self.mode = TailMode::Group {
                members: group_members.clone(),
                which: which.clone(),
                slots: slots.clone(),
            };
            let mut body = Vec::new();
            if let Some(e) = &func.body {
                self.tail(e, &mut body);
            }
            // A block, because the cases of a `switch` share one lexical
            // scope: two members declaring a local at the same index would
            // otherwise collide — and `js::rename_scope` clones its map per
            // case, so the collision would appear only once names are
            // shortened, in release, with debug passing.
            cases.push((Some(Expr::Num(index as f64)), vec![Stmt::Block(body)]));
        }
        self.mode = TailMode::Return;

        let mut params = vec![which.clone()];
        params.extend(slots.iter().cloned());
        // The slots are reassigned on every bounce, so they cannot be `const`.
        let switch = Stmt::Switch { disc: Expr::ident(which), cases };
        Stmt::Func {
            name: group_name(group),
            params,
            body: vec![Stmt::While { cond: Expr::Bool(true), body: vec![switch] }],
        }
    }

    fn stmt(&mut self, s: &hir::Stmt, out: &mut Vec<Stmt>) {
        match s {
            hir::Stmt::Let { pattern, value, .. } => {
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
            hir::Stmt::Expr(e) => {
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
        arms: &[hir::Arm],
        out: &mut Vec<Stmt>,
        target: Option<&str>,
    ) {
        // Bind the scrutinee once: it is evaluated before the arms are tried.
        let s = self.fresh();
        out.push(Stmt::Var { kind: VarKind::Const, name: s.clone(), init: Some(subject) });
        let subject = Expr::ident(s);

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

    fn arm_chain(
        &mut self,
        subject: &Expr,
        arms: &[hir::Arm],
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
        // already known — a six-variant enum performs six comparisons to reach
        // its sixth. `exhaust.rs` proves exhaustiveness at compile time, and a
        // release build takes that proof at its word. A debug build keeps the
        // test and the abort behind it.
        let last = !self.defensive_aborts && i + 1 == arms.len();
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

    fn arm_body(&mut self, arm: &hir::Arm, out: &mut Vec<Stmt>, target: Option<&str>) {
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
    fn test(&mut self, pattern: &hir::Pattern, subject: &Expr) -> Option<Expr> {
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
                let mut cond = if rest.is_some() {
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
        parts: impl Iterator<Item = (&'p hir::Pattern, Expr)>,
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
    fn bind(&mut self, pattern: &hir::Pattern, subject: &Expr, out: &mut Vec<Stmt>) {
        match &pattern.kind {
            PatKind::Bind { local, sub } => {
                let name = self.names[local].clone();
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
                if let Some(Some(local)) = rest {
                    let name = self.names[local].clone();
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
    fn hoist_or_declarations(&mut self, pattern: &hir::Pattern, out: &mut Vec<Stmt>) {
        match &pattern.kind {
            PatKind::Or(alts) => {
                let mut declared = Vec::new();
                if let Some(first) = alts.first() {
                    first.binds(&mut declared);
                }
                for local in declared {
                    let name = self.names[&local].clone();
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

    /// The assignment form, for bindings inside an or-pattern's test.
    fn bind_assignments(&mut self, pattern: &hir::Pattern, subject: &Expr, out: &mut Vec<Expr>) {
        match &pattern.kind {
            PatKind::Bind { local, sub } => {
                let name = self.names[local].clone();
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
                if let Some(Some(local)) = rest {
                    let name = self.names[local].clone();
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

    fn payloadless(&self, con: crate::types::TyConId) -> bool {
        match &self.tables.tycon(con).def {
            TyDef::Enum { variants } => variants.iter().all(|v| v.fields.is_empty()),
            _ => false,
        }
    }

    // -----------------------------------------------------------------------
    // Expression position
    // -----------------------------------------------------------------------

    fn expr(&mut self, e: &hir::Expr, out: &mut Vec<Stmt>) -> Expr {
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
                self.names.get(l).cloned().unwrap_or_else(|| format!("v{}", l.0)),
            ),
            ExprKind::Const(_) => Expr::Num(0.0),
            ExprKind::FnRef(f, _) => Expr::ident(self.symbol(f.index())),
            ExprKind::CallFn { func, args, .. } => {
                let args = self.exprs(args, out);
                if let Some(e) = self.inline_intrinsic(func.index(), &args) {
                    return e;
                }
                Expr::call(Expr::ident(self.symbol(func.index())), args)
            }
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
            ExprKind::Lambda { params, body, .. } => {
                let names: Vec<String> =
                    params.iter().map(|p| self.names[p].clone()).collect();
                let mut inner = Vec::new();
                self.tail(body, &mut inner);
                Expr::ArrowBlock { params: names, body: inner }
            }
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
                let l = self.expr(lhs, out);
                let name = self.fresh();
                out.push(Stmt::Var {
                    kind: VarKind::Const,
                    name: name.clone(),
                    init: Some(l),
                });
                let ok = Expr::bin(
                    BinOp::StrictEq,
                    Expr::index(Expr::ident(name.clone()), Expr::Num(0.0)),
                    Expr::Num(0.0),
                );
                let _ = kind;
                let mut r_stmts = Vec::new();
                let r = self.expr(rhs, &mut r_stmts);
                if r_stmts.is_empty() {
                    return Expr::cond(
                        ok,
                        Expr::index(Expr::ident(name.clone()), Expr::Num(1.0)),
                        r,
                    );
                }
                let result = self.fresh();
                out.push(Stmt::Var { kind: VarKind::Let, name: result.clone(), init: None });
                r_stmts.push(assign(&result, r));
                out.push(Stmt::If {
                    cond: ok,
                    then: vec![assign(
                        &result,
                        Expr::index(Expr::ident(name), Expr::Num(1.0)),
                    )],
                    else_: r_stmts,
                });
                Expr::ident(result)
            }
            ExprKind::Try { base, .. } => {
                // `?` is the only early exit in the language. `.Err(e)` and
                // `.None` are both the value itself, so the failure case
                // returns what it matched.
                let b = self.expr(base, out);
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
                let a = self.exprs(args, out);
                let call = Expr::call(Expr::ident("$eq"), a);
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
                    if let Some(t) = &p.text {
                        items.push(Expr::Str(t.clone()));
                    }
                    if let Some(h) = &p.hole {
                        let v = self.expr(h, out);
                        items.push(self.render_hole(v, &h.ty));
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
                let was = std::mem::replace(&mut self.in_context, true);
                let items: Vec<Expr> =
                    bindings.iter().map(|(_, v)| self.expr(v, out)).collect();
                self.in_context = was;
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
                match name.as_str() {
                    "structuralEq" => {
                        let mut a = a;
                        a.pop();
                        Expr::call(Expr::ident("$eq"), a)
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
                    "structuralShow" => {
                        let desc = a[1].clone();
                        let d = match desc {
                            Expr::Num(n) => Expr::ident(desc_name(n as usize)),
                            other => other,
                        };
                        Expr::call(Expr::ident("$show"), vec![a[0].clone(), d])
                    }
                    other => {
                        self.missing.push(other.to_string());
                        Expr::Num(0.0)
                    }
                }
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
    fn exprs(&mut self, xs: &[hir::Expr], out: &mut Vec<Stmt>) -> Vec<Expr> {
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

    fn int_literal(&self, v: u128, neg: bool, ty: &Ty) -> Expr {
        let _ = ty;
        let n = v as f64;
        Expr::Num(if neg { -n } else { n })
    }

    // -----------------------------------------------------------------------
    // Primitive operations
    // -----------------------------------------------------------------------

    fn prim_op(&mut self, op: PrimOp, prim: Option<Prim>, mut args: Vec<Expr>) -> Expr {
        let p = prim.unwrap_or(Prim::I64);
        let big = p.is_bigint();
        let float = p.is_float();
        let two = |op: BinOp, args: &mut Vec<Expr>| {
            let b = args.pop().unwrap();
            let a = args.pop().unwrap();
            Expr::bin(op, a, b)
        };
        match op {
            PrimOp::Not => Expr::un(UnOp::Not, args.pop().unwrap()),
            PrimOp::Eq => two(BinOp::StrictEq, &mut args),
            PrimOp::Ne => two(BinOp::StrictNe, &mut args),
            PrimOp::Lt => two(BinOp::Lt, &mut args),
            PrimOp::Le => two(BinOp::Le, &mut args),
            PrimOp::Gt => two(BinOp::Gt, &mut args),
            PrimOp::Ge => two(BinOp::Ge, &mut args),
            PrimOp::BitAnd => two(BinOp::BitAnd, &mut args),
            PrimOp::BitOr => two(BinOp::BitOr, &mut args),
            PrimOp::BitXor => two(BinOp::BitXor, &mut args),
            PrimOp::BitNot => Expr::un(UnOp::BitNot, args.pop().unwrap()),
            PrimOp::Neg => {
                let v = Expr::un(UnOp::Neg, args.pop().unwrap());
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
            PrimOp::Div => {
                if float {
                    let v = two(BinOp::Div, &mut args);
                    self.rounded(v, p)
                } else {
                    let _ = big;
                    Expr::call(Expr::ident("$divi"), args)
                }
            }
            PrimOp::Rem => {
                if float {
                    let v = two(BinOp::Rem, &mut args);
                    self.rounded(v, p)
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
                let tag = match p {
                    Prim::Str => "s",
                    Prim::Char => "c",
                    Prim::F32 | Prim::F64 => "f",
                    Prim::Bool => "b",
                    _ => "i",
                };
                Expr::Array(vec![Expr::Num(0.0), Expr::Str(tag.into())])
            }
            Desc::Unit => Expr::Array(vec![Expr::Num(1.0)]),
            Desc::Struct { name, record, fields, types } => Expr::Array(vec![
                Expr::Num(2.0),
                Expr::Str(name.clone()),
                Expr::Bool(*record),
                Expr::Array(fields.iter().map(|f| Expr::Str(f.clone())).collect()),
                Expr::Array(types.iter().map(|t| Expr::ident(desc_name(*t))).collect()),
            ]),
            Desc::Enum { name, variants, payloadless } => Expr::Array(vec![
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
                                    v.fields.iter().map(|f| Expr::Str(f.clone())).collect(),
                                ),
                                Expr::Array(
                                    v.types.iter().map(|t| Expr::ident(desc_name(*t))).collect(),
                                ),
                            ])
                        })
                        .collect(),
                ),
                Expr::Bool(*payloadless),
            ]),
            Desc::Array(inner) => {
                Expr::Array(vec![Expr::Num(4.0), Expr::ident(desc_name(*inner))])
            }
            Desc::Tuple(items) => Expr::Array(vec![
                Expr::Num(5.0),
                Expr::Array(items.iter().map(|t| Expr::ident(desc_name(*t))).collect()),
            ]),
            Desc::Opaque(_) => Expr::Array(vec![Expr::Num(6.0)]),
        }
    }

    fn test_harness(&mut self) -> Stmt {
        let cases: Vec<Expr> = self
            .program
            .tests
            .iter()
            .map(|t| {
                Expr::Array(vec![
                    Expr::Str(t.name.clone()),
                    Expr::Str(t.module.clone()),
                    Expr::ident(self.symbol(t.func)),
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
        Stmt::Raw(format!(
            "{}function $run(filter){{const out=[];for(const[n,m,f]of $cases){{\
             if(filter&&!n.includes(filter))continue;\
             const started=Date.now();try{{f();out.push({{name:n,module:m,ok:true,ms:Date.now()-started}});}}\
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
        let mut out =
            ArgUse { counts: vec![0; n], conditional: vec![false; n], order: Vec::new() };
        survey_args(e, false, &mut out);
        out
    }

    /// The shape every runtime forwarder has: each argument once, in order,
    /// unconditionally. This is the case inlining exists for.
    #[test]
    fn a_plain_forwarder_uses_each_argument_once_in_order() {
        let e = Expr::call(Expr::ident("$str_split"), vec![arg(0), arg(1), arg(2)]);
        let u = survey(&e, 3);
        assert_eq!(u.counts, vec![1, 1, 1]);
        assert_eq!(u.conditional, vec![false, false, false]);
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
        assert!(u.counts[0] > 1);
    }

    /// An argument under a `?:` branch might not run at all.
    #[test]
    fn an_argument_under_a_branch_is_conditional() {
        let e = Expr::cond(Expr::ident("c"), arg(0), Expr::Num(0.0));
        assert!(survey(&e, 1).conditional[0]);
    }

    /// So is the right operand of a short circuit.
    #[test]
    fn an_argument_after_a_short_circuit_is_conditional() {
        let e = Expr::bin(BinOp::And, Expr::ident("c"), arg(0));
        assert!(survey(&e, 1).conditional[0]);
        let e = Expr::bin(BinOp::And, arg(0), Expr::ident("c"));
        assert!(!survey(&e, 1).conditional[0]);
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
