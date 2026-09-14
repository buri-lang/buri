//! What a reactive builder may not do: `load`.
//!
//! `ui/node`'s `computed`, `each` and `rebuild` hand the renderer a closure it
//! calls to make a subtree. `core/lazy`'s `load` waits for a chunk to arrive,
//! so a builder that reaches it compiles to an `async` function — it answers a
//! promise rather than a node, and the renderer renders the promise: a blank
//! page, and every listener after the first failed render left unregistered
//! (buri-lang/buri#152).
//!
//! A handler is the opposite — `fn(C, Event) => ()` may wait, because it runs
//! to completion and writes signals when the wait is over — so `load` has a
//! home already. This pass refuses the one that cannot render rather than
//! letting the compiler emit a program that type-checks and comes up blank.
//!
//! **The rule is "the builder's own body is synchronous", not "nothing under it
//! ever loads".** A `load` inside a nested lambda the builder writes — a press
//! handler, or an inner reactive builder — is that closure's own business, and
//! a handler's `load` is exactly where it belongs. So the walk stops at every
//! lambda boundary: it reads the builder's synchronous body, and the body of a
//! function that body calls, but never the body of a lambda.

use crate::compiler::modules::Loaded;
use crate::compiler::semantics::resolve::{ModuleScope, Sym};
use crate::compiler::semantics::typed::{self, ExprKind};
use crate::compiler::semantics::types::{FnId, Tables};
use crate::diagnostics::{Diagnostic, Diagnostics, FileId, Span};
use crate::hash::{Map as HashMap, Set as HashSet};

/// Refuses a `load` reached synchronously from a reactive builder.
///
/// A compilation that loaded neither `core/lazy` nor `ui/node` returns at once:
/// with no `load` there is nothing to reach, and with no reactive constructor
/// there is nowhere it would be refused. That is every program without a lazily
/// split user interface.
pub fn run(
    loaded: &Loaded,
    tables: &Tables,
    scopes: &[ModuleScope],
    bodies: &HashMap<FnId, typed::Body>,
    diags: &mut Diagnostics,
    only: Option<&[FileId]>,
) {
    let Some(load) = fn_of(loaded, scopes, "core/lazy", "load") else { return };
    let builders: Vec<(FnId, &'static str)> = [
        ("computed", "computed"),
        ("each", "each"),
        ("rebuild", "rebuild"),
    ]
    .iter()
    .filter_map(|(name, label)| fn_of(loaded, scopes, "ui/node", name).map(|id| (id, *label)))
    .collect();
    if builders.is_empty() {
        return;
    }

    // The functions that, run, wait on a `load` — directly, or by calling one
    // that does. A lambda they write is not part of this: it is a separate
    // function value, called elsewhere, so `reaches` never crosses one.
    let waiting = waiting_set(bodies, load);

    let wanted = |file| only.is_none_or(|files: &[FileId]| files.contains(&file));
    let mut ids: Vec<FnId> = bodies.keys().copied().collect();
    ids.sort_by_key(|f| f.index());
    for id in ids {
        if wanted(tables.fn_info(id).span.file) {
            if let Some(body) = bodies.get(&id) {
                walk(&body.expr, &builders, load, &waiting, diags);
            }
        }
    }
}

/// A named member of a module in the loaded set, when this compilation has it.
fn fn_of(loaded: &Loaded, scopes: &[ModuleScope], path: &str, name: &str) -> Option<FnId> {
    let index = loaded.modules.iter().position(|m| m.path == path)?;
    match scopes.get(index)?.own.get(name)? {
        Sym::Fn(id) => Some(*id),
        _ => None,
    }
}

/// The fixpoint: a function waits if its synchronous body reaches `load`, or
/// calls a function that waits. Small, because `load` is rare — most rounds add
/// nothing and the loop settles in as many passes as the deepest chain of waits.
fn waiting_set(bodies: &HashMap<FnId, typed::Body>, load: FnId) -> HashSet<FnId> {
    let mut set: HashSet<FnId> = HashSet::default();
    loop {
        let mut changed = false;
        for (id, body) in bodies {
            if !set.contains(id) && reaches(&body.expr, load, &set).is_some() {
                set.insert(*id);
                changed = true;
            }
        }
        if !changed {
            return set;
        }
    }
}

/// The span of the first call, in this expression's *synchronous* extent, to
/// `load` or to a function that waits — or `None`. It does not enter a lambda:
/// a lambda is a function value whose body runs when it is called, not here.
fn reaches(e: &typed::Expr, load: FnId, waiting: &HashSet<FnId>) -> Option<Span> {
    if let ExprKind::Lambda { .. } = &e.kind {
        return None;
    }
    if let ExprKind::CallFn { func, .. } = &e.kind {
        if func.decl().is_some_and(|f| f == load || waiting.contains(&f)) {
            return Some(e.span);
        }
    }
    let mut found = None;
    typed::children(e, &mut |child| {
        if found.is_none() {
            found = reaches(child, load, waiting);
        }
    });
    found
}

/// Every call to a reactive constructor, refusing a builder argument whose body
/// waits.
fn walk(
    e: &typed::Expr,
    builders: &[(FnId, &'static str)],
    load: FnId,
    waiting: &HashSet<FnId>,
    diags: &mut Diagnostics,
) {
    if let ExprKind::CallFn { func, args } = &e.kind {
        if let Some(label) = func.decl().and_then(|f| builder_label(f, builders)) {
            for arg in args {
                if let ExprKind::Lambda { body, .. } = &arg.kind {
                    if let Some(span) = reaches(body, load, waiting) {
                        diags.items.push(
                            Diagnostic::templated("load-in-a-reactive-builder", span)
                                .with_bind("builder", label),
                        );
                    }
                }
            }
        }
    }
    typed::children(e, &mut |child| walk(child, builders, load, waiting, diags));
}

/// The name a refused builder is reported under, when `f` is one.
fn builder_label(f: FnId, builders: &[(FnId, &'static str)]) -> Option<&'static str> {
    builders.iter().find(|(id, _)| *id == f).map(|(_, label)| *label)
}
