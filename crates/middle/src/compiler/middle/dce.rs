//! Reachability, and dropping what nothing reaches.
//!
//! Monomorphization already gives reachability-based dead-code elimination for
//! free: it builds a function only when something asks for it. Inlining then
//! creates *new* dead functions — a body inlined at its single call site leaves
//! the original unreachable — and those used to be left for
//! `javascript::eliminate_dead` to drop by name.
//!
//! Dropping by name is a JavaScript minifier's job. A native backend needs them
//! dropped by index, before layout and codegen spend time on them, which is
//! `design/native/CODEGEN-LLVM.md` §0's first instruction ("do dead code
//! elimination before it reaches LLVM IR"). So it happens here, over the tree, for every backend.
//!
//! The minifier's own pass stays, because it drops things this one cannot see:
//! a hand-written runtime declaration (`Stmt::RawDecl`) is not a function in
//! this program, and the tree-shaking of `runtime.js` is exactly what it is
//! for. What it no longer has to be is the *only* place a dead function dies.
//!
//! # Indices must not move
//!
//! `Program::roots`, `CallFunc` and `FuncRef` are all `FuncIdx`, and
//! `inline.rs` states the invariant that nothing renumbers them. So a dropped
//! function is replaced by `FuncKind::Unbuilt` **in place** rather than
//! removed: renumbering would mean rewriting every call in the program to save
//! a `Vec` slot holding a name and no body, and the one thing every pass after
//! this relies on is that the index it read is still the index it needs.
//!
//! `FuncKind::Unbuilt` is what monomorphization already uses for a function
//! requested and never built, and it means the same thing here: reaching one at
//! run time is a compiler bug, so a backend compiles it to an abort. The
//! difference is only in how it got that way.
//!
//! Design: `design/native/ARCHITECTURE.md` §2.2.

use crate::compiler::middle::monomorphize::{Desc, FuncKind, Program, ProgramRoots};
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::typed::{self, ExprKind};
use crate::compiler::semantics::types::FuncIdx;

/// Which edges a walk takes at a chunk node.
///
/// A `middle::chunks` node holds a reference to the function it fetches, so
/// that every pass after it — parkability, sharing, this one — sees a real
/// function reference rather than a name in a string. Two questions are then
/// asked of the same graph and they want different answers: *what does this
/// program contain* follows the edge, and *what would this program contain if
/// the chunk were not there* does not. The second is how a chunk's own members
/// are found.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Chunks {
    /// Follow a chunk node into what it fetches.
    Followed,
    /// Stop at a chunk node. What only a chunk reaches is then unreached.
    Deferred,
}

/// Drops every function no root reaches.
///
/// Dropping is [`FuncKind::Unbuilt`] in place, and an `Unbuilt` is a body — an
/// abort — the backends emit under the function's symbol. That is fine while
/// each symbol names one such body, which the program upheld before this pass
/// (`monomorphize::one_symbol_per_function`). The pass can break it, though:
/// **several functions may share one symbol as long as they are intrinsics**,
/// because an intrinsic defines nothing and that check excludes them — the
/// standard library's `Str.compare` is reached both inherently and through a
/// derived `Ordered`, and a `--filter`ed batch keeps one intrinsic copy per
/// suite (buri-lang/buri#186). Turning two of those into `Unbuilt` would give
/// one symbol two bodies. So [`disambiguate_unreached_symbols`] runs after the
/// drop: an unreached function is never called — a call resolves by `FuncIdx`,
/// not by name — so renaming the symbol of a dropped duplicate is invisible to
/// the program and leaves each emitted body a name of its own.
pub fn run(program: &mut Program) {
    let reached = reachable(program, &program_roots(program), Chunks::Followed);
    for (func, seen) in program.funcs.iter_mut().zip(&reached) {
        if !*seen {
            func.kind = FuncKind::Unbuilt;
        }
    }
    disambiguate_unreached_symbols(program, &reached);
}

/// Gives a dropped function a symbol of its own where it shared one.
///
/// Only an unreached function is touched, and only where its symbol is not
/// unique in the program — the sole way this pass can leave two bodies under
/// one name (see [`run`]). The rename is by slot index, so it is deterministic
/// and cannot collide with a second rename or with any live symbol.
fn disambiguate_unreached_symbols(program: &mut Program, reached: &Dense<FuncIdx, bool>) {
    // The symbols worn by two functions or more — the only ones a rename has to
    // reach. A second sighting of a symbol records it, so no count and no
    // arithmetic: `insert` returning `false` is the collision.
    let mut seen: crate::hash::Set<&str> = crate::hash::Set::default();
    seen.reserve(program.funcs.len());
    let mut shared: crate::hash::Set<String> = crate::hash::Set::default();
    for f in &program.funcs {
        if !seen.insert(&f.symbol) {
            shared.insert(f.symbol.clone());
        }
    }
    if shared.is_empty() {
        return;
    }
    for ((i, f), seen) in program.funcs.entries_mut().zip(reached) {
        if !*seen && shared.contains(&f.symbol) {
            f.symbol = format!("{}$unreached${}", f.symbol, i.index());
        }
    }
}

/// The slots this program starts at: its entry, or every one of its tests.
pub fn program_roots(program: &Program) -> Vec<FuncIdx> {
    let mut roots: Vec<FuncIdx> = match &program.roots {
        ProgramRoots::Main(entry) => vec![*entry],
        ProgramRoots::Tests(tests) => tests.iter().map(|t| t.func).collect(),
    };
    // A reactive cell's comparison is named by no call: a write is handed it.
    let mut cells: Vec<FuncIdx> = program.cell_equal.values().copied().collect();
    cells.sort_unstable();
    roots.extend(cells);
    // Nor is an `IndexSet`'s pair: a walk over its descriptor calls them.
    for d in &program.descriptors {
        if let Desc::Members { to, from, .. } = d {
            roots.extend([*to, *from]);
        }
    }
    roots
}

/// Every function `roots` reaches, one row per [`Program::funcs`] slot.
///
/// The walk `run` drops by, and the walk `middle::chunks` splits by. One
/// function rather than two, because a chunk holding a function the artifact
/// also holds, or missing one it needs, is the same defect as a dropped live
/// function — and it would be found by a different piece of code.
pub fn reachable(program: &Program, roots: &[FuncIdx], chunks: Chunks) -> Dense<FuncIdx, bool> {
    let mut reached = program.funcs.map(|_| false);
    let mut work: Vec<FuncIdx> = roots.to_vec();
    for f in &work {
        *reached.get_mut(*f) = true;
    }

    while let Some(f) = work.pop() {
        let Some(body) = program.funcs.get(f).body() else {
            continue;
        };
        let mut callees = Vec::new();
        references(body, chunks, &mut callees);
        for c in callees {
            let seen = reached.get_mut(c);
            if !*seen {
                *seen = true;
                work.push(c);
            }
        }
    }
    reached
}

/// The slots one function's body names directly: the edges [`reachable`]
/// walks from it, chunks followed.
///
/// For a caller that walks the same graph from many sets of roots, such as
/// `buri test` dividing a batch of suites between binaries. Asking
/// [`reachable`] once per set would walk every shared body once per set.
pub fn callees(program: &Program, func: FuncIdx) -> Vec<FuncIdx> {
    let mut out = Vec::new();
    if let Some(body) = program.funcs.get(func).body() {
        references(body, Chunks::Followed, &mut out);
    }
    out
}

/// Every function this body can reach.
///
/// A call is not the only way to name one: `FuncRef` is a function used as a
/// value, which is how a lambda-free callback and every `map(f)` reaches its
/// callee, and dropping a function only *referenced* would be a
/// `ReferenceError` at run time rather than a smaller artifact. `Continue`
/// names the function a merged tail-recursive group became, which cannot be
/// dead while a member is live — but this pass runs before that rewrite and
/// after it in a second run, and it costs one arm to be right in both.
fn references(e: &typed::Expr, chunks: Chunks, out: &mut Vec<FuncIdx>) {
    // A chunk node under `Deferred` is a leaf: its one child is the reference
    // to the function the chunk holds, and following it is exactly what this
    // walk is being asked not to do.
    if chunks == Chunks::Deferred && is_chunk(e) {
        return;
    }
    match &e.kind {
        ExprKind::CallFunc { func, .. } | ExprKind::FuncRef(func) => {
            out.push(*func);
        }
        ExprKind::Continue { func: Some(f), .. } => out.push(*f),
        _ => {}
    }
    typed::children(e, &mut |c| references(c, chunks, out));
}

/// Whether this node fetches a chunk.
fn is_chunk(e: &typed::Expr) -> bool {
    match &e.kind {
        ExprKind::Intrinsic { name, .. } => {
            crate::compiler::backend::intrinsic_keys::lazy_chunk_of(name).is_some()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::compiler::middle::func_id;
    use crate::compiler::middle::monomorphize::{Func, FuncKind, Program, ProgramRoots};
    use crate::compiler::semantics::typed::{Expr, ExprKind};
    use crate::compiler::semantics::types::Ty;
    use crate::diagnostics::Span;
    use crate::hash::Map as HashMap;

    fn func(symbol: &str, body: Option<Expr>) -> Func {
        Func {
            symbol: symbol.to_string(),
            debug_name: symbol.to_string(),
            params: Vec::new(),
            locals: Default::default(),
            kind: match body {
                Some(e) => FuncKind::Body(e),
                None => FuncKind::Unbuilt,
            },
            ret: Ty::UNIT,
            desc: None,
            env: false,
            span: Span::default(),
        }
    }

    fn intrinsic(symbol: &str, key: &str) -> Func {
        let mut f = func(symbol, None);
        f.kind = FuncKind::Intrinsic(key.to_string());
        f
    }

    fn call(to: u32) -> Expr {
        Expr::new(
            ExprKind::CallFunc {
                func: func_id(to),
                args: Vec::new(),
            },
            Ty::UNIT,
            Span::default(),
        )
    }

    fn program(funcs: Vec<Func>) -> Program {
        Program {
            funcs: funcs.into_iter().collect(),
            roots: ProgramRoots::Main(func_id(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: HashMap::default(),
            cell_equal: HashMap::default(),
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

    #[test]
    fn what_the_entry_point_reaches_survives_and_the_rest_does_not() {
        let mut p = program(vec![
            func("main", Some(call(1))),
            func(
                "live",
                Some(Expr::new(ExprKind::Unit, Ty::UNIT, Span::default())),
            ),
            func(
                "dead",
                Some(Expr::new(ExprKind::Unit, Ty::UNIT, Span::default())),
            ),
        ]);
        run(&mut p);
        assert!(p.funcs.as_slice()[0].body().is_some());
        assert!(p.funcs.as_slice()[1].body().is_some());
        assert!(p.funcs.as_slice()[2].body().is_none());
    }

    /// Indices are what every other pass holds, so nothing may move.
    #[test]
    fn a_dropped_function_keeps_its_slot() {
        let mut p = program(vec![
            func("main", Some(call(2))),
            func(
                "dead",
                Some(Expr::new(ExprKind::Unit, Ty::UNIT, Span::default())),
            ),
            func(
                "live",
                Some(Expr::new(ExprKind::Unit, Ty::UNIT, Span::default())),
            ),
        ]);
        run(&mut p);
        assert_eq!(p.funcs.len(), 3);
        assert_eq!(p.funcs.as_slice()[2].symbol, "live");
        assert!(p.funcs.as_slice()[2].body().is_some());
    }

    /// A cycle nothing enters is still dead, which a plain "is it called
    /// anywhere" count would keep alive for ever.
    #[test]
    fn a_dead_cycle_does_not_keep_itself_alive() {
        let mut p = program(vec![
            func(
                "main",
                Some(Expr::new(ExprKind::Unit, Ty::UNIT, Span::default())),
            ),
            func("a", Some(call(2))),
            func("b", Some(call(1))),
        ]);
        run(&mut p);
        assert!(p.funcs.as_slice()[1].body().is_none());
        assert!(p.funcs.as_slice()[2].body().is_none());
    }

    /// A function used as a value is reached, and a pass that only counted
    /// calls would drop it and leave a name with nothing behind it.
    #[test]
    fn a_function_referenced_as_a_value_is_reached() {
        let body = Expr::new(ExprKind::FuncRef(func_id(1)), Ty::UNIT, Span::default());
        let mut p = program(vec![
            func("main", Some(body)),
            func(
                "passed_around",
                Some(Expr::new(ExprKind::Unit, Ty::UNIT, Span::default())),
            ),
        ]);
        run(&mut p);
        assert!(p.funcs.as_slice()[1].body().is_some());
    }

    /// Two dropped copies of one intrinsic do not leave two bodies under one
    /// symbol.
    ///
    /// A batched, `--filter`ed build can hold several intrinsic functions under
    /// one symbol — the standard library's `Str.compare`, reached inherently
    /// and through a derived `Ordered`, one copy per suite. `one_symbol_per_function`
    /// excludes intrinsics, so they coexist; but a filter that leaves two of
    /// them unreached would, before the fix, turn both into `Unbuilt` abort
    /// bodies sharing the symbol and trip the check when it is asked again after
    /// `derives::run` (buri-lang/buri#186). The drop must give each dropped copy
    /// a name of its own.
    #[test]
    fn two_dropped_copies_of_one_intrinsic_do_not_share_a_symbol() {
        let mut p = program(vec![
            func(
                "main",
                Some(Expr::new(ExprKind::Unit, Ty::UNIT, Span::default())),
            ),
            intrinsic("core_str$Str_compare", "str.compare"),
            intrinsic("core_str$Str_compare", "str.compare"),
        ]);
        run(&mut p);
        // Both were dropped, and the shared symbol is no longer worn by two
        // bodies — the invariant `monomorphize::one_symbol_per_function` states.
        assert!(p.funcs.as_slice()[1].body().is_none());
        assert!(p.funcs.as_slice()[2].body().is_none());
        assert_ne!(
            p.funcs.as_slice()[1].symbol,
            p.funcs.as_slice()[2].symbol,
            "two dropped copies of one intrinsic kept one symbol between two bodies"
        );
        crate::compiler::middle::monomorphize::assert_one_symbol_per_function(
            p.funcs.as_slice(),
            "`dce::run`",
        );
    }

    /// A dropped intrinsic whose symbol was unique keeps that symbol.
    ///
    /// The rename is only for a shared symbol; the ordinary case — one dead
    /// intrinsic, its own name — must be untouched, so a build with no
    /// collision keeps every symbol a golden already records.
    #[test]
    fn a_lone_dropped_intrinsic_keeps_its_symbol() {
        let mut p = program(vec![
            func(
                "main",
                Some(Expr::new(ExprKind::Unit, Ty::UNIT, Span::default())),
            ),
            intrinsic("core_lazy$load", "lazy.load"),
        ]);
        run(&mut p);
        assert!(p.funcs.as_slice()[1].body().is_none());
        assert_eq!(p.funcs.as_slice()[1].symbol, "core_lazy$load");
    }
}
