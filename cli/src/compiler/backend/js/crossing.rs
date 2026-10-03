//! The crossing table, written out: the conversion a value takes on its way
//! to a repository platform's `js` file, and on its way back.
//!
//! What crosses and how is [`crate::compiler::semantics::crossing`]'s answer,
//! which the checker has already held every signature to. This turns each
//! answer into an expression over the backend's value representation
//! (`runtime.js`'s header): a struct is an array of its fields, `None` is
//! `undefined`, `()` is `0`, and a `Result` is `[0, value]` or `[1, error]`.

use crate::compiler::backend::js::generate::Gen;
use crate::compiler::backend::js::javascript::{BinOp, Expr, Stmt, VarKind};
use crate::compiler::middle::monomorphize::Func;
use crate::compiler::semantics::crossing::{classify, Crossing, Known};
use crate::compiler::semantics::types::Ty;

/// The name of the arrow parameter a conversion at this depth binds. Each
/// nested conversion binds its own, so an inner one never shadows the outer
/// one it reads.
fn bound(depth: usize) -> String {
    format!("$x{depth}")
}

/// `(p => body)(e)`, or `await (async p => body)(e)` where the body waits.
fn applied(e: Expr, depth: usize, waits: bool, body: impl FnOnce(Expr) -> Expr) -> Expr {
    let p = bound(depth);
    let arrow = Expr::Arrow { params: vec![p.clone()], body: Box::new(body(Expr::ident(p))), is_async: waits };
    let call = Expr::call(arrow, vec![e]);
    if waits { Expr::Await(Box::new(call)) } else { call }
}

/// A Buri value, as the `js` file receives it.
pub fn to_js(c: &Crossing, e: Expr, depth: usize) -> Expr {
    if c.is_same() {
        return e;
    }
    match c {
        Crossing::Same => e,
        Crossing::Widened => Expr::call(Expr::ident("BigInt"), vec![e]),
        Crossing::Bytes => Expr::call(Expr::member(Expr::ident("Uint8Array"), "from"), vec![e]),
        Crossing::List(inner) => {
            let p = bound(depth);
            let each = to_js(inner, Expr::ident(p.clone()), depth.saturating_add(1));
            let arrow = Expr::Arrow { params: vec![p], body: Box::new(each), is_async: false };
            Expr::call(Expr::member(e, "map"), vec![arrow])
        }
        Crossing::Tuple(items) => applied(e, depth, false, |t| {
            Expr::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, c)| to_js(c, Expr::index(t.clone(), Expr::Num(i as f64)), depth.saturating_add(1)))
                    .collect(),
            )
        }),
        Crossing::Optional(inner) => applied(e, depth, false, |v| Expr::Cond {
            test: Box::new(Expr::bin(BinOp::StrictEq, v.clone(), Expr::Undefined)),
            cons: Box::new(Expr::Undefined),
            alt: Box::new(to_js(inner, v, depth.saturating_add(1))),
        }),
        Crossing::Unit => Expr::Undefined,
        Crossing::Result(inner) => {
            to_js(inner, Expr::call(Expr::ident("$crossThrow"), vec![e]), depth)
        }
        Crossing::Record(fields) => applied(e, depth, false, |r| {
            Expr::Object(
                fields
                    .iter()
                    .enumerate()
                    .map(|(i, (name, c))| {
                        let field = Expr::index(r.clone(), Expr::Num(i as f64));
                        (name.clone(), to_js(c, field, depth.saturating_add(1)))
                    })
                    .collect(),
            )
        }),
        Crossing::Request => Expr::call(Expr::ident("$crossRequestOut"), vec![e]),
        Crossing::Response => Expr::call(Expr::ident("$crossResponseOut"), vec![e]),
    }
}

/// A value the `js` file handed back, as a Buri value. `Result` is answered
/// by [`answer_from_js`], since it crosses only as a whole answer.
pub fn from_js(c: &Crossing, e: Expr, depth: usize) -> Expr {
    if c.is_same() {
        return e;
    }
    let next = depth.saturating_add(1);
    match c {
        Crossing::Same => e,
        Crossing::Widened => Expr::call(Expr::ident("Number"), vec![e]),
        Crossing::Bytes => Expr::call(Expr::member(Expr::ident("Array"), "from"), vec![e]),
        Crossing::List(inner) => {
            let p = bound(depth);
            let waits = inner.waits();
            let each = from_js(inner, Expr::ident(p.clone()), next);
            let arrow = Expr::Arrow { params: vec![p], body: Box::new(each), is_async: waits };
            let mapped = Expr::call(Expr::member(Expr::ident("Array"), "from"), vec![e, arrow]);
            if waits {
                let all = Expr::call(Expr::member(Expr::ident("Promise"), "all"), vec![mapped]);
                Expr::Await(Box::new(all))
            } else {
                mapped
            }
        }
        Crossing::Tuple(items) => applied(e, depth, c.waits(), |t| {
            Expr::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, c)| from_js(c, Expr::index(t.clone(), Expr::Num(i as f64)), next))
                    .collect(),
            )
        }),
        Crossing::Optional(inner) => applied(e, depth, c.waits(), |v| {
            let absent = Expr::bin(
                BinOp::Or,
                Expr::bin(BinOp::StrictEq, v.clone(), Expr::Undefined),
                Expr::bin(BinOp::StrictEq, v.clone(), Expr::Null),
            );
            Expr::Cond {
                test: Box::new(absent),
                cons: Box::new(Expr::Undefined),
                alt: Box::new(from_js(inner, v, next)),
            }
        }),
        Crossing::Unit => Expr::Num(0.0),
        Crossing::Result(inner) => from_js(inner, e, depth),
        Crossing::Record(fields) => applied(e, depth, c.waits(), |r| {
            Expr::Array(
                fields
                    .iter()
                    .map(|(name, c)| from_js(c, Expr::member(r.clone(), name), next))
                    .collect(),
            )
        }),
        Crossing::Request => {
            Expr::Await(Box::new(Expr::call(Expr::ident("$crossRequestIn"), vec![e])))
        }
        Crossing::Response => {
            Expr::Await(Box::new(Expr::call(Expr::ident("$crossResponseIn"), vec![e])))
        }
    }
}

/// What a `js` file's method answered, as a Buri value: `call` is the
/// unawaited call. A `Result` answer is `.Ok` of the value, or `.Err` of the
/// message of whatever the method threw.
pub fn answer_from_js(c: &Crossing, call: Expr) -> Expr {
    match c {
        Crossing::Result(inner) => {
            let value = from_js(inner, Expr::Await(Box::new(call)), 0);
            let thunk = Expr::Arrow { params: Vec::new(), body: Box::new(value), is_async: true };
            Expr::Await(Box::new(Expr::call(Expr::ident("$crossCatch"), vec![thunk])))
        }
        other => from_js(other, Expr::Await(Box::new(call)), 0),
    }
}

/// The binding an artifact hands a `js` file its entries in. The bundle the
/// build writes reads it, so it keeps its name through minification.
pub const HOSTED_PROGRAM: &str = "$buri$program";

/// The function a method a `js` file implements is looked up through,
/// answering the file's exports. The bundle defines it, after the artifact.
pub const HOST_LOOKUP: &str = "$buri$host";

impl Gen<'_> {
    fn known(&self) -> Known {
        Known { request: self.program.hosted.request, response: self.program.hosted.response }
    }

    /// How a value of this type crosses. The checker refused every type that
    /// cannot, so a refusal here is a value that is already the same on both
    /// sides.
    fn crossing(&self, ty: &Ty, answer: bool) -> Crossing {
        classify(self.tables, self.known(), ty, answer).unwrap_or(Crossing::Same)
    }

    /// The body of a method a platform's `js` file implements: the export
    /// named after its struct, called with its arguments crossed, its answer
    /// awaited and crossed back.
    ///
    /// `self` is a production struct with no fields, so it crosses as `{}`.
    pub(crate) fn js_implemented(&mut self, key: &str, args: &[Expr], f: &Func) -> Option<Expr> {
        let (owner, method) = key.rsplit_once('.')?;
        let (_, strukt) = owner.rsplit_once('.')?;
        let mut crossed = Vec::with_capacity(args.len());
        for (i, (arg, param)) in args.iter().zip(&f.params).enumerate() {
            if i == 0 {
                crossed.push(Expr::Object(Vec::new()));
                continue;
            }
            let ty = f.locals.get(param.index()).map(|l| l.ty.clone()).unwrap_or(Ty::Error);
            crossed.push(to_js(&self.crossing(&ty, false), arg.clone(), 0));
        }
        let implementation = Expr::member(Expr::call(Expr::ident(HOST_LOOKUP), Vec::new()), strukt);
        let call = Expr::call(Expr::member(implementation, method), crossed);
        Some(answer_from_js(&self.crossing(&f.ret, true), call))
    }

    /// `const $buri$program = { fetch: async (...) => ... }`: the entry, as
    /// the `js` file imports it from `buri:program`. Its arguments arrive from
    /// JavaScript and its answer leaves for it, both by the crossing table.
    pub(crate) fn hosted_export(&mut self, name: &str, symbol: &str, f: &Func) -> Stmt {
        let params: Vec<String> = (0..f.params.len()).map(|i| format!("$a{i}")).collect();
        let args: Vec<Expr> = f
            .params
            .iter()
            .zip(&params)
            .map(|(p, name)| {
                let ty = f.locals.get(p.index()).map(|l| l.ty.clone()).unwrap_or(Ty::Error);
                from_js(&self.crossing(&ty, false), Expr::ident(name.clone()), 0)
            })
            .collect();
        let answer = Expr::Await(Box::new(Expr::call(Expr::ident(symbol), args)));
        let body = vec![
            Stmt::Var { kind: VarKind::Const, name: String::from("$r"), init: Some(answer) },
            Stmt::Return(Some(to_js(&self.crossing(&f.ret, true), Expr::ident("$r"), 0))),
        ];
        let entry = Expr::ArrowBlock { params, body, is_async: true };
        Stmt::Var {
            kind: VarKind::Const,
            name: String::from(HOSTED_PROGRAM),
            init: Some(Expr::Object(vec![(name.to_string(), entry)])),
        }
    }
}
