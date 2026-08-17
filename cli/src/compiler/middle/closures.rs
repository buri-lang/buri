//! Closure conversion: a lambda becomes a code pointer and an environment.
//!
//! `ExprKind::Lambda { captures }` already carries the capture list, and the
//! JavaScript backend never reads it — it relies on JavaScript's own lexical
//! scoping, which is exactly right for JavaScript and available on neither
//! native backend. Conversion turns a lambda into a top-level function taking
//! an environment as its first parameter, plus a construction of that
//! environment at the point the lambda was written.
//!
//! It is sound without any analysis because SPEC 10.6 forbids capturing an
//! effect-carrying value, so an environment is always plain immutable data.
//!
//! **This runs on the native branch only** (`middle::native`). Closure
//! conversion is a pessimisation in JavaScript, where an arrow function closing
//! over its scope is what the engine wants, so the JS backend is handed the
//! tree before this pass and would only undo it.
//!
//! # What comes out
//!
//! A lambda that captures nothing becomes an `FnRef`. VALUE-MODEL.md §7 says a
//! function value is `{ code, env }` with a null `env` in that case, and
//! `FnRef` is already exactly it — so the empty environment is not built, not
//! passed, and not a special case anywhere downstream.
//!
//! Everything else becomes `ExprKind::Closure { func, env }`, where `env` is
//! the captured locals in capture order and the lifted function reads them back
//! out of a tuple parameter appended to its own. The type of the whole
//! expression does not change: a closure is not a different type from a
//! function, only a different way of filling one in.
//!
//! # The lifted function's locals
//!
//! A lifted body still holds the `LocalId`s of the function it came out of —
//! its own parameters, its own bindings, and the captures it reads — and a
//! `LocalId` is an index into one function's table. So the lifted function is
//! given its parent's whole table, plus the environment parameter at the end,
//! and nothing is renumbered.
//!
//! The alternative is to build a minimal table and remap, which means rewriting
//! every pattern that binds as well as every read, for a saving of some entries
//! in a `Vec` that hold a name and a type. A local nothing binds and nothing
//! reads produces no value in `lower`, so the entries cost nothing past this
//! module — and not renumbering is one fewer way to produce a body that reads
//! the wrong slot.
//!
//! Design: `design/native/ARCHITECTURE.md` §2.2, §2.3, `VALUE-MODEL.md` §7.

use crate::compiler::middle::inline::children_mut;
use crate::compiler::middle::monomorphize::{Func, FuncKind, Program};
use crate::compiler::semantics::typed::{
    self, Callee, Expr, ExprKind, PatKind, Pattern, Stmt,
};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty};

/// Lifts every lambda to a top-level function over an explicit environment.
pub fn run(program: &mut Program) {
    // Only the functions the program came in with: what this pass appends is
    // already converted, because a lambda nested inside another is lifted
    // while it is still part of the outer lambda's body.
    for i in 0..program.funcs.len() {
        let Some(func) = program.funcs.get_mut(i) else { continue };
        let Some(mut body) = func.take_body() else { continue };
        let parent = Parent {
            locals: func.locals.clone(),
            symbol: func.symbol.clone(),
            debug_name: func.debug_name.clone(),
        };
        let mut lifted = Vec::new();
        convert(&mut body, &parent, program.funcs.len(), &mut lifted);
        if let Some(func) = program.funcs.get_mut(i) {
            func.set_body(body);
        }
        program.funcs.extend(lifted);
    }
}

/// What a lifted function inherits from the function it was written inside.
struct Parent {
    locals: Vec<typed::Local>,
    symbol: String,
    debug_name: String,
}

/// Depth first, so a lambda inside a lambda is converted while it is still in
/// the outer one's body — which is what makes one pass enough, and what makes
/// the inner one's captures resolvable: a nested lambda's parameters are
/// locals of the same table as everything else it mentions.
fn convert(e: &mut Expr, parent: &Parent, base: usize, lifted: &mut Vec<Func>) {
    for child in children_mut(e) {
        convert(child, parent, base, lifted);
    }
    let ExprKind::Lambda { params, body, captures } = &mut e.kind else { return };

    let func = FuncIdx(base.saturating_add(lifted.len()) as u32);
    let params = std::mem::take(params);
    let captures = std::mem::take(captures);
    let body = std::mem::replace(
        body,
        Box::new(Expr::new(ExprKind::Unit, Ty::Unit, e.span)),
    );

    let mut locals = parent.locals.clone();
    let env = LocalId(locals.len() as u32);
    let env_ty = Ty::Tuple(
        captures
            .iter()
            .filter_map(|c| locals.get(c.index()).map(|l| l.ty.clone()))
            .collect(),
    );
    locals.push(typed::Local { name: "env".to_string(), ty: env_ty.clone(), span: e.span });

    // Each capture is bound back to the id it had, out of the environment, so
    // the body below reads what it always read.
    let prologue: Vec<Stmt> = captures
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let ty = locals.get(c.index())?.ty.clone();
            Some(Stmt::Let {
                pattern: Pattern {
                    kind: PatKind::Bind { local: *c, sub: None },
                    ty: ty.clone(),
                    span: e.span,
                },
                value: Expr::new(
                    ExprKind::TupleIndex {
                        base: Box::new(Expr::new(
                            ExprKind::Local(env),
                            env_ty.clone(),
                            e.span,
                        )),
                        index: i,
                    },
                    ty,
                    e.span,
                ),
                span: e.span,
            })
        })
        .collect();

    let ret = body.ty.clone();
    let inner = if prologue.is_empty() {
        *body
    } else {
        Expr::new(ExprKind::Block { stmts: prologue, tail: Some(body) }, ret.clone(), e.span)
    };

    let n = lifted.len();
    let mut lifted_params = vec![env];
    lifted_params.extend(params);
    lifted.push(Func {
        symbol: format!("{}$fn{n}", parent.symbol),
        debug_name: format!("{} lambda {n}", parent.debug_name),
        params: lifted_params,
        locals,
        kind: FuncKind::Body(inner),
        ret,
        desc: None,
        span: e.span,
    });

    // Nothing captured means nothing to carry, which is the `FnRef` a
    // top-level function already is.
    e.kind = if captures.is_empty() {
        ExprKind::FnRef(Callee::Func(func))
    } else {
        ExprKind::Closure {
            func,
            env: captures
                .iter()
                .filter_map(|c| {
                    let l = parent.locals.get(c.index())?;
                    Some(Expr::new(ExprKind::Local(*c), l.ty.clone(), e.span))
                })
                .collect(),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::compiler::middle::monomorphize::{Func, FuncKind, Program, ProgramRoots};
    use crate::compiler::semantics::typed::{Expr, ExprKind, Local};
    use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty};
    use crate::diagnostics::Span;
    use crate::hash::Map as HashMap;

    fn local(name: &str) -> Local {
        Local { name: name.to_string(), ty: Ty::Unit, span: Span::default() }
    }

    fn e(kind: ExprKind) -> Expr {
        Expr::new(kind, Ty::Unit, Span::default())
    }

    fn program(locals: Vec<Local>, params: Vec<u32>, body: Expr) -> Program {
        Program {
            funcs: vec![Func {
                symbol: "host".to_string(),
                debug_name: "host".to_string(),
                params: params.into_iter().map(LocalId).collect(),
                locals,
                kind: FuncKind::Body(body),
                ret: Ty::Unit,
                desc: None,
                span: Span::default(),
            }],
            roots: ProgramRoots::Main(FuncIdx(0)),
            descriptors: Vec::new(),
            desc_index: HashMap::default(),
            ctx_layouts: HashMap::default(),
        }
    }

    /// The environment is built where the lambda was written, and the lifted
    /// function takes it as its first parameter.
    #[test]
    fn a_capturing_lambda_becomes_a_closure_over_an_environment() {
        let lambda = e(ExprKind::Lambda {
            params: vec![LocalId(1)],
            body: Box::new(e(ExprKind::Local(LocalId(0)))),
            captures: vec![LocalId(0)],
        });
        let mut p = program(vec![local("n"), local("x")], vec![0], lambda);
        run(&mut p);

        let ExprKind::Closure { func, env } = &p.funcs[0].body().unwrap().kind else {
            panic!("a capturing lambda is a closure");
        };
        assert_eq!(env.len(), 1);
        let lifted = &p.funcs[func.index()];
        assert_eq!(lifted.params.len(), 2);
        // The environment parameter comes first and is a fresh local.
        assert_eq!(lifted.params[0], LocalId(2));
        assert_eq!(lifted.params[1], LocalId(1));
    }

    /// A lambda over nothing is a top-level function, and a null environment is
    /// what `FnRef` already means (VALUE-MODEL.md §7).
    #[test]
    fn a_lambda_that_captures_nothing_is_a_reference() {
        let lambda = e(ExprKind::Lambda {
            params: vec![LocalId(0)],
            body: Box::new(e(ExprKind::Local(LocalId(0)))),
            captures: Vec::new(),
        });
        let mut p = program(vec![local("x")], Vec::new(), lambda);
        run(&mut p);
        assert!(matches!(p.funcs[0].body().unwrap().kind, ExprKind::FnRef(_)));
        assert_eq!(p.funcs.len(), 2);
        assert!(p.funcs[1].body().is_some());
    }

    /// One pass, however deep: the inner lambda is lifted while it is still
    /// inside the outer one's body, so the outer one carries a `Closure` rather
    /// than a `Lambda` by the time it is lifted itself.
    #[test]
    fn a_lambda_inside_a_lambda_is_lifted_too() {
        let inner = e(ExprKind::Lambda {
            params: vec![LocalId(2)],
            body: Box::new(e(ExprKind::Local(LocalId(1)))),
            captures: vec![LocalId(1)],
        });
        let outer = e(ExprKind::Lambda {
            params: vec![LocalId(1)],
            body: Box::new(inner),
            captures: Vec::new(),
        });
        let mut p = program(vec![local("f"), local("a"), local("b")], Vec::new(), outer);
        run(&mut p);
        assert_eq!(p.funcs.len(), 3);
        let mut lambdas = 0;
        for f in &p.funcs {
            if let Some(b) = f.body() {
                crate::compiler::semantics::typed::walk(b, &mut |x| {
                    if matches!(x.kind, ExprKind::Lambda { .. }) {
                        lambdas += 1;
                    }
                });
            }
        }
        assert_eq!(lambdas, 0);
    }
}
