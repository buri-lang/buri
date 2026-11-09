//! Each codegen unit's **fingerprint**: a hash of everything `rc`'s scan and
//! `lower` read to build the unit's code, taken before either runs.
//!
//! Two units with the same fingerprint, over the same `Tables`, lower to the
//! same IR, so the build can reuse the `codegen` key it computed for one and
//! skip lowering the other. `build::actions` keeps the keys it computed, by
//! fingerprint, with the program's `Tables`.
//!
//! A function's code is a function of:
//!
//! - the function itself: every field of its post-middle `monomorphize::Func`,
//!   its body to the last literal, type and span
//! - its `rc` answers: its ownership row, purity and whether it can abort
//! - what lowering reads of each function it names: its symbol, its kind, its
//!   parameter and return types, how many loop entries it has, whether it
//!   takes an environment, and its `rc` answers ([`summary`])
//! - `Tables`, which the memo is scoped to
//!
//! A type is hashed by its interned identity ([`Ty::identity`]), and an id
//! into `Tables` by its index, which is why a fingerprint means nothing
//! against other tables.

use crate::compiler::middle::monomorphize::{Func, FuncKind, Program};
use crate::compiler::middle::{ir, rc};
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::typed::{
    ArrayRest, Expr, ExprKind, Local, NumberLit, PatKind, Pattern, Stmt, TemplatePart,
};
use crate::compiler::semantics::types::{FuncIdx, Ty};
use std::hash::Hash as _;

/// A streaming 128-bit hash: two 64-bit lanes, each a folded multiply.
///
/// Not cryptographic, and it needn't be: nobody chooses the inputs to collide,
/// and a chance collision is about 2^-64 per pair of units.
struct Hasher {
    a: u64,
    b: u64,
}

const K0: u64 = 0x9e37_79b9_7f4a_7c15;
const K1: u64 = 0xbf58_476d_1ce4_e5b9;
const K2: u64 = 0x94d0_49bb_1331_11eb;
const K3: u64 = 0x2545_f491_4f6c_dd1d;

/// The high and low halves of `x * y`, folded together.
fn fold(x: u64, y: u64) -> u64 {
    let p = u128::from(x).wrapping_mul(u128::from(y));
    (p as u64) ^ ((p >> 64) as u64)
}

impl Hasher {
    fn new() -> Hasher {
        Hasher { a: K0, b: K1 }
    }

    fn word(&mut self, w: u64) {
        self.a = fold(self.a ^ w, K2);
        self.b = fold(self.b ^ w.rotate_left(32), K3);
    }

    fn ty(&mut self, t: Ty) {
        self.word(t.identity() as u64);
    }

    fn tys(&mut self, ts: &[Ty]) {
        self.word(ts.len() as u64);
        ts.iter().for_each(|t| self.ty(*t));
    }

    fn str(&mut self, s: &str) {
        std::hash::Hasher::write(self, s.as_bytes());
    }

    fn finish128(&self) -> u128 {
        let a = fold(self.a ^ self.b, K1);
        let b = fold(self.b ^ a, K0);
        (u128::from(a) << 64) | u128::from(b)
    }
}

/// For the leaves that already say how they hash: a span, a primitive, a name.
impl std::hash::Hasher for Hasher {
    fn write(&mut self, bytes: &[u8]) {
        self.word(bytes.len() as u64);
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            self.word(u64::from_le_bytes(*w));
        }
        let mut w = [0; 8];
        for (to, from) in w.iter_mut().zip(rest) {
            *to = *from;
        }
        self.word(u64::from_le_bytes(w));
    }

    fn write_u8(&mut self, n: u8) {
        self.word(u64::from(n));
    }

    fn write_u32(&mut self, n: u32) {
        self.word(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.word(n);
    }

    fn write_usize(&mut self, n: usize) {
        self.word(n as u64);
    }

    fn finish(&self) -> u64 {
        self.a
    }
}

/// Each unit's fingerprint, in the order of `units`.
///
/// `answers` is the program's `rc` analysis, before its scan. A unit's
/// fingerprint covers its name and each member's, in order.
pub fn units(program: &Program, answers: &rc::Answers, units: &[ir::Unit]) -> Vec<u128> {
    let funcs: Vec<(FuncIdx, &Func)> = program.funcs.entries().collect();
    let summaries: Dense<FuncIdx, u128> =
        crate::parallel::map_items(&funcs, |_, &(i, f)| summary(answers, i, f))
            .into_iter()
            .collect();
    crate::parallel::map_items(units, |_, unit| {
        let mut h = Hasher::new();
        h.str(&unit.name);
        for &i in &unit.funcs {
            func(&mut h, &summaries, i, program.funcs.get(i));
        }
        h.finish128()
    })
}

/// What lowering a function reads of a function it names, and the function's
/// own `rc` answers.
fn summary(answers: &rc::Answers, i: FuncIdx, f: &Func) -> u128 {
    let mut h = Hasher::new();
    h.str(&f.symbol);
    match &f.kind {
        FuncKind::Unbuilt => h.word(0),
        FuncKind::Body(_) => h.word(1),
        FuncKind::Intrinsic(key) => {
            h.word(2);
            h.str(key);
        }
    }
    h.word(f.params.len() as u64);
    for p in &f.params {
        h.ty(f.locals.get(*p).ty);
    }
    h.ty(f.ret);
    h.word(u64::from(f.env));
    h.word(crate::compiler::middle::lower::loop_entries(f.body()) as u64);
    let params = answers.params(i);
    h.word(params.len() as u64);
    for p in params {
        h.word(*p as u64);
    }
    h.word(answers.purity(i) as u64);
    h.word(u64::from(answers.can_abort(i)));
    h.finish128()
}

/// One member of a unit: its summary, then all of it.
fn func(h: &mut Hasher, summaries: &Dense<FuncIdx, u128>, i: FuncIdx, f: &Func) {
    let Func {
        symbol: _,
        debug_name,
        params,
        locals,
        kind,
        ret: _,
        desc,
        env: _,
        span,
    } = f;
    // The symbol, the return type and `env` are the summary's.
    callee(h, summaries, i);
    h.str(debug_name);
    h.word(params.len() as u64);
    params.iter().for_each(|p| h.word(p.index() as u64));
    // `lower` drops the descriptor (its module docs say why), and its index
    // is the program's, so only whether there is one is hashed.
    h.word(u64::from(desc.is_some()));
    span.hash(h);
    match kind {
        FuncKind::Body(body) => {
            h.word(locals.len() as u64);
            locals.iter().for_each(|l| local(h, l));
            expr(h, summaries, body);
        }
        // Lowered from its parameters alone. Two programs can leave a
        // function they don't build with different locals.
        FuncKind::Unbuilt | FuncKind::Intrinsic(_) => {
            params.iter().for_each(|p| local(h, locals.get(*p)));
        }
    }
}

fn local(h: &mut Hasher, l: &Local) {
    l.name.hash(h);
    h.ty(l.ty);
    l.span.hash(h);
}

fn callee(h: &mut Hasher, summaries: &Dense<FuncIdx, u128>, f: FuncIdx) {
    let s = *summaries.get(f);
    h.word(s as u64);
    h.word((s >> 64) as u64);
}

fn exprs(h: &mut Hasher, summaries: &Dense<FuncIdx, u128>, es: &[Expr]) {
    h.word(es.len() as u64);
    es.iter().for_each(|e| expr(h, summaries, e));
}

fn expr(h: &mut Hasher, s: &Dense<FuncIdx, u128>, e: &Expr) {
    std::mem::discriminant(&e.kind).hash(h);
    h.ty(e.ty);
    e.span.hash(h);
    match &e.kind {
        ExprKind::Int(m, negative) => {
            m.hash(h);
            h.word(u64::from(*negative));
        }
        ExprKind::Float(x) => h.word(x.to_bits()),
        ExprKind::Str(text) => h.str(text),
        ExprKind::Char(c) => h.word(u64::from(*c)),
        ExprKind::Bool(b) => h.word(u64::from(*b)),
        ExprKind::Unit | ExprKind::Error => {}
        ExprKind::Local(l) => h.word(l.index() as u64),
        ExprKind::Const(c) => h.word(c.index() as u64),
        ExprKind::FnRef(c) => {
            h.word(c.id.index() as u64);
            h.tys(&c.targs);
        }
        ExprKind::FuncRef(f) => callee(h, s, *f),
        ExprKind::CallValue { callee: c, args } => {
            expr(h, s, c);
            exprs(h, s, args);
        }
        ExprKind::CallFn { func, args } => {
            h.word(func.id.index() as u64);
            h.tys(&func.targs);
            exprs(h, s, args);
        }
        ExprKind::CallFunc { func, args } => {
            callee(h, s, *func);
            exprs(h, s, args);
        }
        ExprKind::CallTrait {
            trait_id,
            method,
            recv,
            targs,
            args,
        } => {
            h.word(trait_id.index() as u64);
            h.word(method.index() as u64);
            h.ty(*recv);
            h.tys(targs);
            exprs(h, s, args);
        }
        ExprKind::StructLit { con, targs, fields } => {
            h.word(con.index() as u64);
            h.tys(targs);
            exprs(h, s, fields);
        }
        ExprKind::StructUpdate { con, base, updates } => {
            h.word(con.index() as u64);
            expr(h, s, base);
            h.word(updates.len() as u64);
            for (field, value) in updates {
                h.word(field.index() as u64);
                expr(h, s, value);
            }
        }
        ExprKind::EnumLit {
            con,
            targs,
            variant,
            args,
        } => {
            h.word(con.index() as u64);
            h.tys(targs);
            h.word(variant.index() as u64);
            exprs(h, s, args);
        }
        ExprKind::Tuple(es) | ExprKind::Loop { entries: es } => exprs(h, s, es),
        ExprKind::Array { elem, elems } => {
            h.ty(*elem);
            exprs(h, s, elems);
        }
        ExprKind::Field { base, index } | ExprKind::TupleIndex { base, index } => {
            expr(h, s, base);
            h.word(index.index() as u64);
        }
        ExprKind::Index { base, index, elem } => {
            expr(h, s, base);
            expr(h, s, index);
            h.ty(*elem);
        }
        ExprKind::Block { stmts, tail } => {
            h.word(stmts.len() as u64);
            for stmt in stmts {
                match stmt {
                    Stmt::Let {
                        pattern,
                        value,
                        span,
                    } => {
                        h.word(0);
                        pat(h, pattern);
                        expr(h, s, value);
                        span.hash(h);
                    }
                    Stmt::Expr(e) => {
                        h.word(1);
                        expr(h, s, e);
                    }
                }
            }
            optional(h, s, tail.as_deref());
        }
        ExprKind::If { cond, then, else_ } => {
            expr(h, s, cond);
            expr(h, s, then);
            expr(h, s, else_);
        }
        ExprKind::Match { scrutinee, arms } => {
            expr(h, s, scrutinee);
            h.word(arms.len() as u64);
            for arm in arms {
                pat(h, &arm.pattern);
                optional(h, s, arm.guard.as_ref());
                expr(h, s, &arm.body);
                arm.span.hash(h);
            }
        }
        ExprKind::Lambda {
            params,
            body,
            captures,
        } => {
            h.word(params.len() as u64);
            params.iter().for_each(|p| h.word(p.index() as u64));
            expr(h, s, body);
            h.word(captures.len() as u64);
            captures.iter().for_each(|c| h.word(c.index() as u64));
        }
        ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            expr(h, s, lhs);
            expr(h, s, rhs);
        }
        ExprKind::Try { base, kind } => {
            expr(h, s, base);
            h.word(*kind as u64);
        }
        ExprKind::Continue { func, entry, args } => {
            match func {
                Some(f) => {
                    h.word(1);
                    callee(h, s, *f);
                }
                None => h.word(0),
            }
            h.word(*entry as u64);
            exprs(h, s, args);
        }
        ExprKind::Closure { func, env } => {
            callee(h, s, *func);
            exprs(h, s, env);
        }
        ExprKind::Prim { op, prim, args } => {
            h.word(*op as u64);
            prim.hash(h);
            exprs(h, s, args.as_slice());
        }
        ExprKind::Unary { op, prim, arg } => {
            h.word(*op as u64);
            prim.hash(h);
            expr(h, s, arg);
        }
        ExprKind::Template { parts } => {
            h.word(parts.len() as u64);
            for part in parts {
                match part {
                    TemplatePart::Text(text) => {
                        h.word(0);
                        h.str(text);
                    }
                    TemplatePart::Hole(value) => {
                        h.word(1);
                        expr(h, s, value);
                    }
                    TemplatePart::Shown { prim, value } => {
                        h.word(2);
                        prim.hash(h);
                        expr(h, s, value);
                    }
                }
            }
        }
        ExprKind::CtxLit { bindings, ctx } => {
            h.word(bindings.len() as u64);
            for (t, value) in bindings {
                h.word(t.index() as u64);
                expr(h, s, value);
            }
            h.word(ctx.index() as u64);
        }
        ExprKind::CtxGet {
            base,
            trait_id,
            field,
        } => {
            expr(h, s, base);
            h.word(trait_id.index() as u64);
            h.word(field.index() as u64);
        }
        ExprKind::CtxCall { decl, ctor } => {
            h.word(decl.index() as u64);
            h.word(ctor.index() as u64);
        }
        ExprKind::Intrinsic { name, targs, args } => {
            h.str(name);
            h.tys(targs);
            exprs(h, s, args);
        }
    }
}

fn optional(h: &mut Hasher, s: &Dense<FuncIdx, u128>, e: Option<&Expr>) {
    match e {
        Some(e) => {
            h.word(1);
            expr(h, s, e);
        }
        None => h.word(0),
    }
}

fn pats(h: &mut Hasher, ps: &[Pattern]) {
    h.word(ps.len() as u64);
    ps.iter().for_each(|p| pat(h, p));
}

fn pat(h: &mut Hasher, p: &Pattern) {
    std::mem::discriminant(&p.kind).hash(h);
    h.ty(p.ty);
    p.span.hash(h);
    match &p.kind {
        PatKind::Wild | PatKind::Unit | PatKind::Error => {}
        PatKind::Bind { local, sub } => {
            h.word(local.index() as u64);
            match sub {
                Some(sub) => {
                    h.word(1);
                    pat(h, sub);
                }
                None => h.word(0),
            }
        }
        PatKind::Int(m, negative) => {
            m.hash(h);
            h.word(u64::from(*negative));
        }
        PatKind::Float(x) => h.word(x.to_bits()),
        PatKind::Number { prim, value } => {
            prim.hash(h);
            match value {
                NumberLit::Int(m, negative) => {
                    h.word(0);
                    m.hash(h);
                    h.word(u64::from(*negative));
                }
                NumberLit::Float(x) => {
                    h.word(1);
                    h.word(x.to_bits());
                }
            }
        }
        PatKind::Str(text) => h.str(text),
        PatKind::Char(c) => h.word(u64::from(*c)),
        PatKind::Bool(b) => h.word(u64::from(*b)),
        PatKind::Tuple(ps) | PatKind::Or(ps) => pats(h, ps),
        PatKind::Struct { con, fields } => {
            h.word(con.index() as u64);
            h.word(fields.len() as u64);
            for f in fields {
                h.word(f.index.index() as u64);
                pat(h, &f.pattern);
            }
        }
        PatKind::Variant {
            con,
            variant,
            fields,
        } => {
            h.word(con.index() as u64);
            h.word(variant.index() as u64);
            h.word(fields.len() as u64);
            for f in fields {
                h.word(f.index.index() as u64);
                pat(h, &f.pattern);
            }
        }
        PatKind::Array { elem, elems, rest } => {
            h.ty(*elem);
            pats(h, elems);
            match rest {
                ArrayRest::None => h.word(0),
                ArrayRest::Ignored => h.word(1),
                ArrayRest::Bound(l) => {
                    h.word(2);
                    h.word(l.index() as u64);
                }
            }
        }
    }
}
