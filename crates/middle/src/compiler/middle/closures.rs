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
//! A lambda that captures nothing becomes an `FuncRef`. VALUE-MODEL.md §7 says a
//! function value is `{ code, env }` with a null `env` in that case, and
//! `FuncRef` is already exactly it — so the empty environment is not built, not
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
//! `LocalId` is an index into one function's table. So the lifted function
//! gets a table of just the locals its body names, in their old order, with
//! the environment parameter at the end, and every read and binding is
//! renumbered into it ([`each_local`]).
//!
//! Handing it the parent's whole table instead made a function with `n`
//! lambdas cost `n²`: every lifted function's table, and every per-function
//! pass sized by it, held all of the parent's locals.
//!
//! Design: `design/native/ARCHITECTURE.md` §2.2, §2.3, `VALUE-MODEL.md` §7.

use crate::compiler::middle::monomorphize::{Func, FuncKind, Program};
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::name::Name;
use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Pattern, Stmt};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty};
use crate::hash::Map as HashMap;

/// Lifts every lambda to a top-level function over an explicit environment.
pub fn run(program: &mut Program) {
    // Only the functions the program came in with: what this pass appends is
    // already converted, because a lambda nested inside another is lifted
    // while it is still part of the outer lambda's body.
    let base = program.funcs.len();
    for i in program.funcs.ids() {
        let func = program.funcs.get_mut(i);
        let Some(mut body) = func.take_body() else {
            continue;
        };
        let parent = Parent {
            locals: func.locals.clone(),
            symbol: func.symbol.clone(),
            debug_name: func.debug_name.clone(),
            base,
        };
        convert(&mut body, &parent, &mut program.funcs);
        program.funcs.get_mut(i).set_body(body);
    }
}

/// What a lifted function inherits from the function it was written inside.
struct Parent {
    locals: typed::Locals,
    symbol: String,
    debug_name: String,
    /// Where the lifted functions start in the program's table.
    base: usize,
}

/// Depth first, so a lambda inside a lambda is converted while it is still in
/// the outer one's body — which is what makes one pass enough, and what makes
/// the inner one's captures resolvable: a nested lambda's parameters are
/// locals of the same table as everything else it mentions.
fn convert(e: &mut Expr, parent: &Parent, funcs: &mut Dense<FuncIdx, Func>) {
    typed::children_mut(e, &mut |child| convert(child, parent, funcs));
    let ExprKind::Lambda {
        params,
        body,
        captures,
    } = &mut e.kind
    else {
        return;
    };

    let mut params = std::mem::take(params);
    let captures = std::mem::take(captures);
    let mut body = std::mem::replace(body, Box::new(Expr::new(ExprKind::Unit, Ty::UNIT, e.span)));

    let parent_locals = &parent.locals;
    let env_ty = Ty::tuple(captures.iter().map(|c| parent_locals.get(*c).ty));

    // The lifted function's own table: the environment, then the locals its
    // parameters and body name, in the order they are first named. Sized by
    // the lambda rather than by its parent, which may hold thousands of locals
    // and as many lambdas.
    let mut locals = typed::Locals::new();
    let env = locals.push(typed::Local {
        name: Name::new("env"),
        ty: env_ty,
        span: e.span,
    });
    let mut moved: HashMap<LocalId, LocalId> = HashMap::default();
    let mut renumber = |l: &mut LocalId| {
        *l = *moved
            .entry(*l)
            .or_insert_with(|| locals.push(parent_locals.get(*l).clone()));
    };
    params.iter_mut().for_each(&mut renumber);
    each_local(&mut body, &mut renumber);
    let inner_captures: Vec<LocalId> = captures
        .iter()
        .map(|c| {
            let mut c = *c;
            renumber(&mut c);
            c
        })
        .collect();

    // Each capture is bound out of the environment, so the body below reads
    // what it always read.
    let prologue: Vec<Stmt> = inner_captures
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let ty = locals.get(*c).ty;
            Stmt::Let {
                pattern: Pattern {
                    kind: PatKind::Bind {
                        local: *c,
                        sub: None,
                    },
                    ty,
                    span: e.span,
                },
                value: Expr::new(
                    ExprKind::TupleIndex {
                        base: Box::new(Expr::new(ExprKind::Local(env), env_ty, e.span)),
                        index: i,
                    },
                    ty,
                    e.span,
                ),
                span: e.span,
            }
        })
        .collect();

    let ret = body.ty;
    let inner = if prologue.is_empty() {
        *body
    } else {
        Expr::new(
            ExprKind::Block {
                stmts: prologue,
                tail: Some(body),
            },
            ret,
            e.span,
        )
    };

    let n = funcs.len().saturating_sub(parent.base);
    let mut lifted_params = vec![env];
    lifted_params.extend(params);
    let func = funcs.push(Func {
        symbol: format!("{}$fn{n}", parent.symbol),
        debug_name: format!("{} lambda {n}", parent.debug_name),
        params: lifted_params,
        locals,
        kind: FuncKind::Body(inner),
        ret,
        desc: None,
        span: e.span,
    });

    // Nothing captured means nothing to carry, which is the `FuncRef` a
    // top-level function already is.
    e.kind = if captures.is_empty() {
        ExprKind::FuncRef(func)
    } else {
        ExprKind::Closure {
            func,
            env: captures
                .iter()
                .map(|c| Expr::new(ExprKind::Local(*c), parent_locals.get(*c).ty, e.span))
                .collect(),
        }
    };
}

/// Hands `f` every `LocalId` in an expression: every read, every lambda
/// parameter and capture, and every name a pattern binds.
fn each_local(e: &mut Expr, f: &mut impl FnMut(&mut LocalId)) {
    match &mut e.kind {
        ExprKind::Local(l) => f(l),
        ExprKind::Lambda {
            params, captures, ..
        } => {
            params.iter_mut().for_each(&mut *f);
            captures.iter_mut().for_each(&mut *f);
        }
        ExprKind::Block { stmts, .. } => {
            for st in stmts.iter_mut() {
                if let Stmt::Let { pattern, .. } = st {
                    pattern_locals(pattern, f);
                }
            }
        }
        ExprKind::Match { arms, .. } => {
            for a in arms.iter_mut() {
                pattern_locals(&mut a.pattern, f);
            }
        }
        _ => {}
    }
    typed::children_mut(e, &mut |child| each_local(child, f));
}

fn pattern_locals(p: &mut Pattern, f: &mut impl FnMut(&mut LocalId)) {
    match &mut p.kind {
        PatKind::Bind { local, sub } => {
            f(local);
            if let Some(s) = sub {
                pattern_locals(s, f);
            }
        }
        PatKind::Tuple(ps) | PatKind::Or(ps) => ps.iter_mut().for_each(|p| pattern_locals(p, f)),
        PatKind::Struct { fields, .. } | PatKind::Variant { fields, .. } => fields
            .iter_mut()
            .for_each(|x| pattern_locals(&mut x.pattern, f)),
        PatKind::Array { elems, rest } => {
            elems.iter_mut().for_each(|p| pattern_locals(p, f));
            if let typed::ArrayRest::Bound(l) = rest {
                f(l);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::compiler::middle::func_id;
    use crate::compiler::middle::local_id;
    use crate::compiler::middle::monomorphize::{Func, FuncKind, Program, ProgramRoots};
    use crate::compiler::semantics::name::Name;
    use crate::compiler::semantics::typed::{Expr, ExprKind, Local};
    use crate::compiler::semantics::types::Ty;
    use crate::diagnostics::Span;
    use crate::hash::Map as HashMap;

    fn local(name: &str) -> Local {
        Local {
            name: Name::new(name),
            ty: Ty::UNIT,
            span: Span::default(),
        }
    }

    fn e(kind: ExprKind) -> Expr {
        Expr::new(kind, Ty::UNIT, Span::default())
    }

    fn program(locals: Vec<Local>, params: Vec<u32>, body: Expr) -> Program {
        Program {
            funcs: [Func {
                symbol: "host".to_string(),
                debug_name: "host".to_string(),
                params: params.into_iter().map(local_id).collect(),
                locals: locals.into_iter().collect(),
                kind: FuncKind::Body(body),
                ret: Ty::UNIT,
                desc: None,
                span: Span::default(),
            }]
            .into_iter()
            .collect(),
            roots: ProgramRoots::Main(func_id(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: HashMap::default(),
            cell_equal: HashMap::default(),
            ctx_layouts: HashMap::default(),
            shapes: Default::default(),
            stylesheet: String::new(),
            inline_styles: false,
            inline_animations: false,
            icons: false,
            tooltips: false,
            themes: false,
            chunks: Vec::new(),
            hosted: Default::default(),
            instances: Default::default(),
        }
    }

    /// The environment is built where the lambda was written, and the lifted
    /// function takes it as its first parameter.
    #[test]
    fn a_capturing_lambda_becomes_a_closure_over_an_environment() {
        let lambda = e(ExprKind::Lambda {
            params: vec![local_id(1)],
            body: Box::new(e(ExprKind::Local(local_id(0)))),
            captures: vec![local_id(0)],
        });
        let mut p = program(vec![local("n"), local("x")], vec![0], lambda);
        run(&mut p);

        let ExprKind::Closure { func, env } = &p.funcs.as_slice()[0].body().unwrap().kind else {
            panic!("a capturing lambda is a closure");
        };
        assert_eq!(env.len(), 1);
        let lifted = &p.funcs.as_slice()[func.index()];
        assert_eq!(lifted.params.len(), 2);
        // The environment parameter comes first, then the lambda's own.
        assert_eq!(lifted.locals.get(lifted.params[0]).name, "env");
        assert_eq!(lifted.locals.get(lifted.params[1]).name, "x");
    }

    /// A lifted function's table holds what its body names and its
    /// environment, not every local of the function it came out of: a table
    /// sizes every later per-function pass, and a function with many lambdas
    /// would hand each one all of them.
    #[test]
    fn a_lifted_table_holds_only_what_the_lambda_names() {
        let lambda = e(ExprKind::Lambda {
            params: vec![local_id(3)],
            body: Box::new(e(ExprKind::Tuple(vec![
                e(ExprKind::Local(local_id(1))),
                e(ExprKind::Local(local_id(3))),
            ]))),
            captures: vec![local_id(1)],
        });
        let names = ["a", "n", "b", "x", "c"];
        let mut p = program(names.iter().map(|n| local(n)).collect(), vec![0], lambda);
        run(&mut p);

        let lifted = &p.funcs.as_slice()[1];
        let table: Vec<&str> = lifted.locals.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(table, ["env", "x", "n"]);
        assert_eq!(lifted.params, [local_id(0), local_id(1)]);
        let mut read = Vec::new();
        crate::compiler::semantics::typed::walk(lifted.body().unwrap(), &mut |x| {
            if let ExprKind::Local(l) = x.kind {
                read.push(l.index());
            }
        });
        // The environment, then the capture and the parameter it binds and reads.
        assert_eq!(read, [0, 2, 1]);
    }

    /// A lambda over nothing is a top-level function, and a null environment is
    /// what `FuncRef` already means (VALUE-MODEL.md §7).
    #[test]
    fn a_lambda_that_captures_nothing_is_a_reference() {
        let lambda = e(ExprKind::Lambda {
            params: vec![local_id(0)],
            body: Box::new(e(ExprKind::Local(local_id(0)))),
            captures: Vec::new(),
        });
        let mut p = program(vec![local("x")], Vec::new(), lambda);
        run(&mut p);
        assert!(matches!(
            p.funcs.as_slice()[0].body().unwrap().kind,
            ExprKind::FuncRef(_)
        ));
        assert_eq!(p.funcs.len(), 2);
        assert!(p.funcs.as_slice()[1].body().is_some());
    }

    /// One pass, however deep: the inner lambda is lifted while it is still
    /// inside the outer one's body, so the outer one carries a `Closure` rather
    /// than a `Lambda` by the time it is lifted itself.
    #[test]
    fn a_lambda_inside_a_lambda_is_lifted_too() {
        let inner = e(ExprKind::Lambda {
            params: vec![local_id(2)],
            body: Box::new(e(ExprKind::Local(local_id(1)))),
            captures: vec![local_id(1)],
        });
        let outer = e(ExprKind::Lambda {
            params: vec![local_id(1)],
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
