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
    /// Emitted so a crash names the function it came from.
    pub debug_names: bool,
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
                            Expr::ident("$crash"),
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
             if(e&&e.stack&&!e.$buri)$write(2,e.stack+\"\\n\");\
             if(typeof process!==\"undefined\")process.exit(1);}}"
        )));
    }

    if !program.tests.is_empty() {
        stmts.push(g.test_harness());
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

    pub(crate) fn checked_pub(&self, v: Expr, p: Prim) -> Expr {
        self.checked(v, p)
    }

    pub(crate) fn prim_op_pub(&mut self, op: PrimOp, prim: Option<Prim>, args: Vec<Expr>) -> Expr {
        self.prim_op(op, prim, args)
    }

    fn fresh(&mut self) -> String {
        self.temp += 1;
        format!("$t{}", self.temp)
    }

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
            ExprKind::Crash { message } => {
                let m = self.expr(message, out);
                out.push(Stmt::Expr(Expr::call(Expr::ident("$crash"), vec![m])));
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
        let mut temps = Vec::new();
        for v in values {
            let t = self.fresh();
            out.push(Stmt::Var { kind: VarKind::Const, name: t.clone(), init: Some(v) });
            temps.push(t);
        }
        for (slot, t) in targets.iter().zip(&temps) {
            out.push(assign(slot, Expr::ident(t.clone())));
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
            cases.push((Some(Expr::Num(index as f64)), body));
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
            Expr::ident("$crash"),
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
            // Exhaustiveness is checked, so this is unreachable — but a crash
            // here is cheaper than a silently wrong value if it ever is not.
            out.push(Stmt::Expr(Expr::call(
                Expr::ident("$crash"),
                vec![Expr::Str("no arm matched".into())],
            )));
            return;
        };
        let mut body = Vec::new();
        self.bind(&arm.pattern, subject, &mut body);
        self.arm_body(arm, &mut body, target);

        match self.test(&arm.pattern, subject) {
            // The last arm, or an irrefutable one, needs no test.
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
            // An or-pattern's bindings are assigned inside its test.
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
            ExprKind::Float(v) => Expr::Num(*v),
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
                Expr::call(Expr::ident(self.symbol(func.index())), args)
            }
            ExprKind::CallValue { callee, args } => {
                let c = self.expr(callee, out);
                let args = self.exprs(args, out);
                Expr::call(c, args)
            }
            ExprKind::CallTrait { .. } => Expr::Num(0.0),
            ExprKind::StructLit { fields, .. } => Expr::Array(self.exprs(fields, out)),
            ExprKind::StructUpdate { base, updates, .. } => {
                // Functional update: copy, then replace the named fields. The
                // runtime is free to do this in place when the value is
                // provably unshared; that is never observable.
                let b = self.expr(base, out);
                let name = self.fresh();
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
                Expr::ident(name)
            }
            ExprKind::EnumLit { con, variant, args, .. } => {
                if self.payloadless(*con) {
                    Expr::Num(*variant as f64)
                } else {
                    let mut items = vec![Expr::Num(*variant as f64)];
                    items.extend(self.exprs(args, out));
                    Expr::Array(items)
                }
            }
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => Expr::Array(self.exprs(xs, out)),
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
                let mut items = Vec::new();
                for p in parts {
                    if let Some(t) = &p.text {
                        items.push(Expr::Str(t.clone()));
                    }
                    if let Some(h) = &p.hole {
                        let v = self.expr(h, out);
                        items.push(v);
                    }
                }
                Expr::Array(items)
            }
            ExprKind::Crash { message } => {
                let m = self.expr(message, out);
                Expr::call(Expr::ident("$crash"), vec![m])
            }
            ExprKind::CtxLit { bindings } => {
                let items: Vec<Expr> =
                    bindings.iter().map(|(_, v)| self.expr(v, out)).collect();
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

    fn exprs(&mut self, xs: &[hir::Expr], out: &mut Vec<Stmt>) -> Vec<Expr> {
        // Arguments are evaluated left to right before the call.
        xs.iter().map(|x| self.expr(x, out)).collect()
    }

    fn symbol(&self, i: usize) -> String {
        self.program
            .funcs
            .get(i)
            .map(|f| f.symbol.clone())
            .unwrap_or_else(|| format!("$missing{i}"))
    }

    fn int_literal(&self, v: u128, neg: bool, ty: &Ty) -> Expr {
        let big = self.tables.as_prim(ty).is_some_and(|p| p.is_bigint());
        let is_float = self.tables.as_prim(ty).is_some_and(|p| p.is_float());
        if big {
            Expr::BigInt(if neg { format!("-{v}") } else { v.to_string() })
        } else {
            let n = v as f64;
            let _ = is_float;
            Expr::Num(if neg { -n } else { n })
        }
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
                if float {
                    v
                } else {
                    self.checked(v, p)
                }
            }
            PrimOp::Add | PrimOp::Sub | PrimOp::Mul => {
                let jsop = match op {
                    PrimOp::Add => BinOp::Add,
                    PrimOp::Sub => BinOp::Sub,
                    _ => BinOp::Mul,
                };
                let v = two(jsop, &mut args);
                // Overflow of any signed or unsigned integer operation is a
                // crash. Silent wrapping is a correctness bug in almost all
                // code, and the little of it that wants wrapping says so.
                if float {
                    v
                } else {
                    self.checked(v, p)
                }
            }
            PrimOp::Div => {
                if float {
                    two(BinOp::Div, &mut args)
                } else {
                    let f = if big { "$divb" } else { "$divi" };
                    let v = Expr::call(Expr::ident(f), args);
                    self.checked(v, p)
                }
            }
            PrimOp::Rem => {
                if float {
                    two(BinOp::Rem, &mut args)
                } else {
                    let f = if big { "$remb" } else { "$remi" };
                    Expr::call(Expr::ident(f), args)
                }
            }
        }
    }

    /// Wraps a value in the range check that turns overflow into a crash.
    fn checked(&self, v: Expr, p: Prim) -> Expr {
        let Some((lo, hi)) = p.int_range() else { return v };
        let (lo, hi) = if p.is_bigint() {
            (Expr::BigInt(lo.to_string()), Expr::BigInt(hi.to_string()))
        } else {
            (Expr::Num(lo as f64), Expr::Num(hi as f64))
        };
        Expr::call(Expr::ident("$ovf"), vec![v, lo, hi])
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
        Stmt::Raw(format!(
            "const $cases={};\
             function $run(filter){{const out=[];for(const[n,m,f]of $cases){{\
             if(filter&&!n.includes(filter))continue;\
             const started=Date.now();try{{f();out.push({{name:n,module:m,ok:true,ms:Date.now()-started}});}}\
             catch(e){{out.push({{name:n,module:m,ok:false,ms:Date.now()-started,\
             error:e&&e.$assert?e.$assert:{{message:String(e&&e.message||e)}},\
             stack:e&&e.stack||\"\"}});}}}}\
             return out;}}",
            js::print(&[Stmt::Expr(Expr::Array(cases))], false)
                .trim_end_matches(';')
                .to_string()
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
