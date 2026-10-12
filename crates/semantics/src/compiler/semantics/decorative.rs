//! A decorative stack has nothing to announce, so it takes no role.
//!
//! `ui/node`'s `stack` hides its subtree from assistive technology when its
//! `isDecorative` is `true`, and a `role` is something announced. The two
//! together say opposite things about one element, so a `stack` literal naming
//! both is `decorative-with-role`.
//!
//! The pass reads the two fields the way `icons` reads an image's: folded
//! where the `Stack` is written, which is a literal or an update anywhere, not
//! only one written in the call. A pair the compiler can't read is refused
//! too, because it can't be shown to be one or the other.

use crate::compiler::modules::Loaded;
use crate::compiler::semantics::consteval::{Env, Folder, Value};
use crate::compiler::semantics::layered::Layered;
use crate::compiler::semantics::resolve::{own_type, BodyMap, ConstMap, ModuleScope, Walked};
use crate::compiler::semantics::typed::{self, ExprKind};
use crate::compiler::semantics::types::{Tables, TyConId};
use crate::diagnostics::{Diagnostic, Diagnostics};

/// `Stack<C>`'s fields, in declaration order — which is the order a struct
/// literal stores them in, whatever order the call site wrote them.
const STACK_ROLE: usize = 2;
const STACK_DECORATIVE: usize = 4;

/// `Option::None`'s variant.
const OPTION_NONE: usize = 1;

/// Refuses every `Stack` literal or update that is decorative and has a role.
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
    let Some(stack) = own_type(loaded, scopes, "ui/node", "Stack") else { return };
    let mut each = |e: &typed::Expr| {
        typed::walk(e, &mut |e| check(e, stack, tables, bodies, consts, diags));
    };
    for (_, init) in walked.constants(tables, consts) {
        each(init);
    }
    for (_, body) in walked.functions(tables, bodies) {
        each(&body.expr);
    }
}

/// One `Stack` value as written: refused unless its role is provably left out
/// or its `isDecorative` provably isn't `true`. An update that sets neither
/// field keeps its base's pair, which was read where the base was written.
fn check(
    e: &typed::Expr,
    stack: TyConId,
    tables: &Tables,
    bodies: &BodyMap,
    consts: &ConstMap,
    diags: &mut Diagnostics,
) {
    match &e.kind {
        ExprKind::StructLit { con, .. } if *con == stack => {}
        ExprKind::StructUpdate { con, updates, .. }
            if *con == stack && updates.iter().any(|(i, _)| [STACK_ROLE, STACK_DECORATIVE].contains(i)) => {}
        _ => return,
    }
    let mut folder = Folder::new(tables, bodies, consts);
    let mut fold = |index| e.written_field(index).and_then(|f| folder.eval(f, &Env::default()));
    let role_value = fold(STACK_ROLE);
    let without_role = role_value.as_ref().and_then(Value::as_variant).is_some_and(|(v, _)| v == OPTION_NONE);
    if without_role {
        return;
    }
    let hidden = fold(STACK_DECORATIVE);
    let shown = match hidden.as_ref().and_then(Value::as_variant) {
        Some((OPTION_NONE, _)) => true,
        Some((_, args)) => args.first().and_then(Value::as_bool) == Some(false),
        None => false,
    };
    if !shown {
        let span = e.written_field(STACK_DECORATIVE).map_or(e.span, |f| f.span);
        diags.items.push(Diagnostic::templated("decorative-with-role", span));
    }
}
