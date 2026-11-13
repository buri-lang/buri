//! A decorative stack has nothing to announce, so it takes no role, and a
//! label needs a role to name.
//!
//! `ui/node`'s `stack` hides its subtree from assistive technology when its
//! `isDecorative` is `true`, and a `role` is something announced. The two
//! together say opposite things about one element, so a `stack` literal naming
//! both is `decorative-with-role`.
//!
//! A `label` is the name a reader hears for what a stack is, so a stack with a
//! label and no role is `label-without-role`. Every `Role` can be named, and a
//! decorative stack takes no role, so this refuses a label on a decorative
//! stack too.
//!
//! The pass reads the fields the way `icons` reads an image's: folded where
//! the `Stack` is written, which is a literal or an update anywhere, not only
//! one written in the call. A field the compiler can't read is refused too,
//! because the stack can't be shown to be one or the other.

use crate::compiler::semantics::consteval::{Env, Folder, Value};
use crate::compiler::semantics::resolve::{BodyMap, ConstMap};
use crate::compiler::semantics::typed::{self, ExprKind};
use crate::compiler::semantics::types::{FieldId, Tables, TyConId};
use crate::diagnostics::Diagnostic;

/// `Stack<C>`'s fields, in declaration order — which is the order a struct
/// literal stores them in, whatever order the call site wrote them.
const STACK_ROLE: FieldId = FieldId::declared(2);
const STACK_LABEL: FieldId = FieldId::declared(3);
const STACK_DECORATIVE: FieldId = FieldId::declared(5);

/// `Option::None`'s variant.
const OPTION_NONE: usize = 1;

/// One `Stack` value as written: refused unless its role is provably left out
/// or its `isDecorative` provably isn't `true`, and unless its label is
/// provably left out or its role provably given. An update that sets none of
/// the fields a rule reads keeps its base's answer, which was read where the
/// base was written. `stack` is `ui/node`'s `Stack`, which a compilation that
/// loaded the module has.
pub(crate) fn check(
    e: &typed::Expr,
    stack: Option<TyConId>,
    tables: &Tables,
    bodies: &BodyMap,
    consts: &ConstMap,
    diags: &mut Vec<Diagnostic>,
) {
    let sets = |field: FieldId| match &e.kind {
        ExprKind::StructLit { con, .. } => Some(*con) == stack,
        ExprKind::StructUpdate { con, updates, .. } => {
            Some(*con) == stack && updates.iter().any(|(i, _)| *i == field)
        }
        _ => false,
    };
    let (role_set, label_set) = (sets(STACK_ROLE), sets(STACK_LABEL));
    let decorative_set = sets(STACK_DECORATIVE);
    if !role_set && !label_set && !decorative_set {
        return;
    }
    let mut folder = Folder::new(tables, bodies, consts);
    let mut fold = |index| {
        e.written_field(index)
            .and_then(|f| folder.eval(f, &Env::default()))
    };
    let variant = |v: Option<Value>| v.as_ref().and_then(Value::as_variant).map(|(i, _)| i);
    let role = variant(fold(STACK_ROLE));
    let unnamed = variant(fold(STACK_LABEL)) == Some(OPTION_NONE);
    if (role_set || label_set) && !unnamed && role.is_none_or(|v| v == OPTION_NONE) {
        let span = e.written_field(STACK_LABEL).map_or(e.span, |f| f.span);
        diags.push(Diagnostic::templated("label-without-role", span));
    }
    if !(role_set || decorative_set) || role == Some(OPTION_NONE) {
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
        diags.push(Diagnostic::templated("decorative-with-role", span));
    }
}
