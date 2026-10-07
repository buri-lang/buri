//! A decorative stack has nothing to announce, so it takes no role.
//!
//! `ui/node`'s `stack` hides its subtree from assistive technology when its
//! `isDecorative` is `true`, and a `role` is something announced. The two
//! together say opposite things about one element, so a `stack` literal naming
//! both is `decorative-with-role`.
//!
//! The pass reads the two fields the way `icons` reads an image's: folded at
//! the call site. A pair the compiler can't read is refused too, because it
//! can't be shown to be one or the other.

use crate::compiler::modules::Loaded;
use crate::compiler::semantics::consteval::{Env, Folder, Value};
use crate::compiler::semantics::layered::Layered;
use crate::compiler::semantics::resolve::{own_fn, BodyMap, ConstMap, ModuleScope, Walked};
use crate::compiler::semantics::typed::{self, ExprKind};
use crate::compiler::semantics::types::{FnId, Tables};
use crate::diagnostics::{Diagnostic, Diagnostics};

/// `Stack<C>`'s fields, in declaration order — which is the order a struct
/// literal stores them in, whatever order the call site wrote them.
const STACK_ROLE: usize = 2;
const STACK_DECORATIVE: usize = 4;

/// `Option::None`'s variant.
const OPTION_NONE: usize = 1;

/// Refuses every `stack` literal that is decorative and has a role.
///
/// A compilation that did not load `ui/node` returns immediately.
pub fn run(
    loaded: &Loaded,
    tables: &Tables,
    scopes: &Layered<ModuleScope>,
    bodies: &BodyMap,
    consts: &ConstMap,
    diags: &mut Diagnostics,
    walked: &Walked,
) {
    let Some(stack) = own_fn(loaded, scopes, "ui/node", "stack") else { return };
    for (_, init) in walked.constants(tables, consts) {
        walk(init, stack, tables, bodies, consts, diags);
    }
    for (_, body) in walked.functions(tables, bodies) {
        walk(&body.expr, stack, tables, bodies, consts, diags);
    }
}

fn walk(
    e: &typed::Expr,
    stack: FnId,
    tables: &Tables,
    bodies: &BodyMap,
    consts: &ConstMap,
    diags: &mut Diagnostics,
) {
    if let ExprKind::CallFn { func, args } = &e.kind {
        if func.decl() == Some(stack) {
            if let Some(arg) = args.first() {
                check(arg, tables, bodies, consts, diags);
            }
        }
    }
    typed::children(e, &mut |child| walk(child, stack, tables, bodies, consts, diags));
}

/// One `stack` literal: refused unless its role is provably left out or its
/// `isDecorative` provably isn't `true`.
fn check(arg: &typed::Expr, tables: &Tables, bodies: &BodyMap, consts: &ConstMap, diags: &mut Diagnostics) {
    let ExprKind::StructLit { fields, .. } = &arg.kind else { return };
    let (Some(role), Some(decorative)) = (fields.get(STACK_ROLE), fields.get(STACK_DECORATIVE)) else {
        return;
    };
    let mut folder = Folder::new(tables, bodies, consts);
    let role_value = folder.eval(role, &Env::default());
    let without_role = role_value.as_ref().and_then(Value::as_variant).is_some_and(|(v, _)| v == OPTION_NONE);
    if without_role {
        return;
    }
    let hidden = folder.eval(decorative, &Env::default());
    let shown = match hidden.as_ref().and_then(Value::as_variant) {
        Some((OPTION_NONE, _)) => true,
        Some((_, args)) => args.first().and_then(Value::as_bool) == Some(false),
        None => false,
    };
    if !shown {
        diags.items.push(Diagnostic::templated("decorative-with-role", decorative.span));
    }
}
