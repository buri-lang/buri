//! Splitting a program into an artifact and the chunks it fetches.
//!
//! `core/lazy`'s `load(f)` answers `f`. What it also does, on a backend that
//! has files to split into, is move `f` and everything only `f` reaches out of
//! the artifact and into a chunk beside it, fetched when the `load` runs.
//!
//! # Two rewrites, and a backend sees only one of them
//!
//! `load` is a bodyless declaration, so monomorphization gives it the
//! intrinsic key [`intrinsic_keys::LAZY_LOAD`]. **No code generator implements
//! that key.** This pass rewrites every call to it before one is reached:
//!
//! * where nothing can be split — a native build, a test binary, or a `load`
//!   the split cannot help — the call is replaced by its own argument, so
//!   `load` costs the program nothing at all and [`dce::run`] then drops the
//!   declaration;
//! * otherwise it becomes an [`intrinsic_keys::lazy_chunk_key`] node, which the
//!   JavaScript backend turns into the fetch.
//!
//! The chunk node keeps the reference to the function it fetches as its one
//! argument. That is not decoration: `middle::rc` decides which functions are
//! printed `async` by looking at the function *values* a program builds, and a
//! chunk node that hid its function behind a string would hide it from that
//! analysis too.
//!
//! # What lands in a chunk
//!
//! `reachable({f}) \ reachable({entry})`, with the second walk stopping at
//! every chunk node ([`dce::Chunks::Deferred`]). So a helper the rest of the
//! program calls stays in the artifact and the chunk reads it from there, and a
//! helper two chunks share is in both — one copy per chunk rather than a third
//! file nobody asked for. Splitting pays off where the split code is code
//! nothing else touches, which is the case it exists for.
//!
//! A `load` of a function the artifact reaches anyway splits nothing, so it
//! becomes the identity: there would be no members to move, and a chunk file
//! holding one re-export is a network round trip for nothing.

use crate::compiler::backend::intrinsic_keys;
use crate::compiler::middle::dce;
use crate::compiler::middle::monomorphize::{Chunk, Program, ProgramRoots};
use crate::compiler::semantics::typed::{self, Callee, Expr, ExprKind};

/// Splits, or takes `load` back out.
///
/// `split` is the backend's answer: a native artifact is one file and has
/// nowhere to put a chunk. A test binary is refused here rather than by a
/// caller, because `buri test` runs the same sources on every backend and a
/// suite whose answers depended on which files the harness copied would not be
/// the reference run any more.
pub fn run(program: &mut Program, split: bool) {
    let loads = load_slots(program);
    if loads.is_empty() {
        return;
    }
    if !split || !matches!(program.roots, ProgramRoots::Main(_)) {
        for f in 0..program.funcs.len() {
            with_body(program, f, &mut |e| unwrap_load(e, &loads));
        }
        // The declaration is now called by nothing, and an unreached intrinsic
        // key is one a backend would still be asked to implement.
        dce::run(program);
        return;
    }

    // Numbered by the order the roots appear in, so two builds of one tree
    // write the same chunk to the same file.
    let mut roots: Vec<usize> = Vec::new();
    for f in 0..program.funcs.len() {
        let mut found: Vec<usize> = Vec::new();
        if let Some(body) = program.funcs.get(f).and_then(|f| f.body()) {
            typed::walk(body, &mut |e| {
                if let Some(root) = load_target(e, &loads) {
                    found.push(root);
                }
            });
        }
        for root in found {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }

    // Provisionally every root is its own chunk, numbered by its slot so that
    // the walk below can tell them apart. The final numbers are dense and are
    // assigned once the artifact's own reachability has said which roots there
    // is anything to move.
    for f in 0..program.funcs.len() {
        with_body(program, f, &mut |e| {
            let Some(root) = load_target(e, &loads) else { return };
            let arg = take_arg(e);
            e.kind = ExprKind::Intrinsic {
                name: intrinsic_keys::lazy_chunk_key(root),
                targs: Vec::new(),
                args: vec![arg],
            };
        });
    }

    let in_artifact = dce::reachable(program, &dce::program_roots(program), dce::Chunks::Deferred);
    let split_roots: Vec<usize> =
        roots.into_iter().filter(|r| in_artifact.get(*r) != Some(&true)).collect();

    for f in 0..program.funcs.len() {
        with_body(program, f, &mut |e| {
            let ExprKind::Intrinsic { name, .. } = &e.kind else { return };
            let Some(root) = intrinsic_keys::lazy_chunk_of(name) else { return };
            match split_roots.iter().position(|r| *r == root) {
                Some(n) => {
                    if let ExprKind::Intrinsic { name, .. } = &mut e.kind {
                        *name = intrinsic_keys::lazy_chunk_key(n);
                    }
                }
                // Nothing to move: the artifact reaches this function anyway.
                None => {
                    let arg = take_arg(e);
                    *e = arg;
                }
            }
        });
        // A `load` whose argument was not a function reference is still a call
        // to a key no backend implements.
        with_body(program, f, &mut |e| unwrap_load(e, &loads));
    }
    // `load` itself is called by nothing now. The chunk nodes keep what they
    // fetch reachable, so this drops the declaration and nothing else.
    dce::run(program);

    program.chunks = split_roots
        .iter()
        .map(|root| {
            let reached = dce::reachable(program, &[*root], dce::Chunks::Followed);
            let members = reached
                .iter()
                .enumerate()
                .filter(|(i, seen)| **seen && in_artifact.get(*i) != Some(&true))
                .map(|(i, _)| i)
                .collect();
            Chunk { root: *root, members }
        })
        .collect();
}

/// The `Program::funcs` slots holding `core/lazy`'s `load`.
///
/// A list rather than one index: `load` is generic, so a program that loads two
/// differently typed functions monomorphizes two of it.
fn load_slots(program: &Program) -> Vec<usize> {
    program
        .funcs
        .iter()
        .enumerate()
        .filter(|(_, f)| f.intrinsic_key() == Some(intrinsic_keys::LAZY_LOAD))
        .map(|(i, _)| i)
        .collect()
}

/// The function a `load` call names, where this node is one.
///
/// `None` for anything else, and for a `load` whose argument monomorphization
/// could not resolve to a function — which the front end's
/// `lazy-not-a-function` has already refused, and which is answered here by
/// splitting nothing rather than by an assertion.
fn load_target(e: &Expr, loads: &[usize]) -> Option<usize> {
    let ExprKind::CallFn { func, args } = &e.kind else { return None };
    let called = func.func()?.index();
    if !loads.contains(&called) {
        return None;
    }
    match args.first().map(|a| &a.kind) {
        Some(ExprKind::FnRef(Callee::Func(i))) => Some(i.index()),
        _ => None,
    }
}

/// Replaces a `load` call with its own argument.
fn unwrap_load(e: &mut Expr, loads: &[usize]) {
    let ExprKind::CallFn { func, .. } = &e.kind else { return };
    if !func.func().is_some_and(|i| loads.contains(&i.index())) {
        return;
    }
    let arg = take_arg(e);
    *e = arg;
}

/// The one argument out of a call or a chunk node, leaving the node behind.
///
/// A `load` and a chunk node both take exactly one, so a node with none is a
/// compiler bug rather than a program the user can write; it answers a unit
/// expression, which the type checker has already made unreachable.
fn take_arg(e: &mut Expr) -> Expr {
    let args = match &mut e.kind {
        ExprKind::CallFn { args, .. } | ExprKind::Intrinsic { args, .. } => args,
        _ => return Expr::new(ExprKind::Unit, e.ty.clone(), e.span),
    };
    match args.drain(..).next() {
        Some(a) => a,
        None => Expr::new(ExprKind::Unit, e.ty.clone(), e.span),
    }
}

/// Applies `f` to every node of one function's body, children first.
///
/// Bottom-up, because a `load` whose argument is itself rewritten must see the
/// rewritten one — and because replacing a node in place while standing on its
/// parent is what `children_mut` is for.
fn with_body(program: &mut Program, f: usize, rewrite: &mut impl FnMut(&mut Expr)) {
    let Some(func) = program.funcs.get_mut(f) else { return };
    let Some(body) = func.body_mut() else { return };
    walk_mut(body, rewrite);
}

fn walk_mut(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    typed::children_mut(e, &mut |c| walk_mut(c, f));
    f(e);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::middle::monomorphize::{Func, FuncKind};
    use crate::compiler::semantics::types::{FuncIdx, Ty};
    use crate::diagnostics::Span;
    use crate::hash::Map as HashMap;

    fn func(symbol: &str, kind: FuncKind) -> Func {
        Func {
            symbol: symbol.to_string(),
            debug_name: symbol.to_string(),
            params: Vec::new(),
            locals: Vec::new(),
            kind,
            ret: Ty::Unit,
            desc: None,
            span: Span::default(),
        }
    }

    fn unit() -> Expr {
        Expr::new(ExprKind::Unit, Ty::Unit, Span::default())
    }

    fn call(to: u32, args: Vec<Expr>) -> Expr {
        Expr::new(
            ExprKind::CallFn { func: Callee::Func(FuncIdx(to)), args },
            Ty::Unit,
            Span::default(),
        )
    }

    fn fn_ref(to: u32) -> Expr {
        Expr::new(ExprKind::FnRef(Callee::Func(FuncIdx(to))), Ty::Unit, Span::default())
    }

    /// slot 0 `main`, slot 1 `lazy.load`, slot 2 the loaded function, slot 3
    /// what only it reaches, slot 4 what everybody reaches.
    fn program(main: Expr) -> Program {
        Program {
            funcs: vec![
                func("main", FuncKind::Body(main)),
                func("load", FuncKind::Intrinsic(intrinsic_keys::LAZY_LOAD.into())),
                func("page", FuncKind::Body(call(3, Vec::new()))),
                func("only_the_page", FuncKind::Body(unit())),
                func("shared", FuncKind::Body(unit())),
            ],
            roots: ProgramRoots::Main(FuncIdx(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: HashMap::default(),
            cell_equal: HashMap::default(),
            ctx_layouts: HashMap::default(),
            shapes: Default::default(),
            chunks: Vec::new(),
            stylesheet: String::new(),
            inline_styles: false,
            themes: false,
        }
    }

    fn chunk_name(e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Intrinsic { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    #[test]
    fn what_only_the_loaded_function_reaches_is_the_chunk() {
        let mut p = program(call(1, vec![fn_ref(2)]));
        run(&mut p, true);
        assert_eq!(p.chunks.len(), 1);
        assert_eq!(p.chunks[0].root, 2);
        assert_eq!(p.chunks[0].members, vec![2, 3]);
        assert_eq!(chunk_name(p.funcs[0].body().unwrap()).as_deref(), Some("lazy.chunk.0"));
    }

    /// A function the artifact reaches anyway has nothing to move out of it, so
    /// the call goes back to being the identity it started as.
    #[test]
    fn loading_something_the_artifact_already_holds_splits_nothing() {
        let body = Expr::new(
            ExprKind::Block {
                stmts: vec![typed::Stmt::Expr(call(2, Vec::new()))],
                tail: Some(Box::new(call(1, vec![fn_ref(2)]))),
            },
            Ty::Unit,
            Span::default(),
        );
        let mut p = program(body);
        run(&mut p, true);
        assert!(p.chunks.is_empty());
        assert!(p.funcs[2].body().is_some(), "and the function is still in the artifact");
    }

    /// The native answer: `load` disappears, and so does its declaration.
    #[test]
    fn a_backend_with_nowhere_to_put_a_chunk_gets_the_argument_back() {
        let mut p = program(call(1, vec![fn_ref(2)]));
        run(&mut p, false);
        assert!(p.chunks.is_empty());
        assert!(
            matches!(p.funcs[0].body().unwrap().kind, ExprKind::FnRef(_)),
            "the call is replaced by what it was handed"
        );
        assert!(p.funcs[1].body().is_none(), "and nothing asks for the intrinsic any more");
        assert!(p.funcs[2].body().is_some(), "while what it named is still reached");
    }
}
