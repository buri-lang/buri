//! Match arms into a decision tree.
//!
//! `generate::arm_chain` tested arms in order, which is O(arms) comparisons to
//! reach the last one, and offered a release-mode shortcut that only helped the
//! final arm. A decision tree over the scrutinee's discriminants is O(1) for an
//! enum match and is the shape a `switch` wants in JavaScript, a jump table
//! wants in a machine backend, and a `switch` wants in LLVM. One pass, three
//! beneficiaries — which is the whole argument for it being here rather than in
//! an emitter.
//!
//! A linear arm chain is not a JavaScript decision. It was in the JavaScript
//! file because there was only one file to put it in.
//!
//! The background is Maranget's *Compiling Pattern Matching to Good Decision
//! Trees* and Jacobs' *How to compile pattern matching*, both in `reference/`.
//!
//! # The tree is the `Match`, restructured
//!
//! There is no new node for a decision tree, and that is deliberate. The tree
//! *is* a `Match` whose arms have been rearranged until the emitter's job is
//! mechanical:
//!
//!  * every arm tests one constructor, and no two arms test the same one, so
//!    the arms are mutually exclusive and the order they are emitted in cannot
//!    change which one runs;
//!  * an arm's own pattern binds and never tests below the head — anything an
//!    arm still had to look at moved into a `Match` on that field, so a shared
//!    test is asked once instead of once per arm;
//!  * at most one arm is irrefutable, and it is last, so it is the default.
//!
//! What a backend does with that is a backend's business — a `switch` where
//! the representation has a discriminant to switch on, a chain where it does
//! not (JavaScript's `Option` is a value or `undefined`, which is a test rather
//! than a table). The *decision* is taken once, here; the spelling is not a
//! decision.
//!
//! # Preserving first-match-wins
//!
//! Grouping arms by constructor reorders the tests, and a match with guards can
//! notice: a guarded arm that fails falls through to *later* arms, so a group
//! that moved past one would run the wrong body. Two rules make the rewrite
//! sound, and the pass gives up rather than bending either:
//!
//!  1. **Rows keep their order within a group.** All the arms whose head is
//!     `C` become one arm testing `C` whose body matches on `C`'s field, with
//!     those arms in their original order. A value whose head is `C` can only
//!     ever match an arm whose head is `C` or one that matches anything, so
//!     skipping the arms in between changes nothing — including when one of
//!     them is guarded, since a guard on an arm the value cannot match is not
//!     reached either way.
//!  2. **Every group must be total.** If the last row of a group can fail — a
//!     guard, or a field pattern that still tests something — then a value with
//!     that head could need to fall through to the match's default arm, and
//!     with the arms grouped there is no path from inside a group back out to
//!     it. Rather than duplicate the default into every group, whose cost is
//!     the exponential blow-up that makes naive pattern compilation famous,
//!     the whole match stays a chain.
//!
//! Anything else the pass does not understand — an or-pattern head, a tuple or
//! array or struct head, a binding with a sub-pattern, an irrefutable arm that
//! is not last — leaves the match exactly as it was. The published
//! measurements this comes from put a decision tree at 1.75× and name the
//! correctness risk as the reason not to land one casually;
//! being conservative in the shapes above is what makes it landable at all.
//!
//! Design: `design/native/ARCHITECTURE.md` §1, §2.2.

use crate::compiler::middle::monomorphize::{Func, FuncKind, Program};
use crate::compiler::semantics::typed::{
    self, Arm, Expr, ExprKind, FieldPat, PatKind, Pattern,
};
use crate::compiler::semantics::types::{LocalId, TyConId};
use crate::hash::Map;

/// Rewrites every `Match` whose arms discriminate on a scrutinee into a tree
/// of discriminant tests.
pub fn run(program: &mut Program) {
    for f in &mut program.funcs {
        let Func { locals, kind, .. } = f;
        let FuncKind::Body(body) = kind else { continue };
        rewrite(locals, body);
    }
}

/// Depth first, so an arm's body is already a tree by the time the match
/// holding it becomes one — and so the nested matches this pass *creates* are
/// visited by the explicit call in [`group`] rather than being missed.
fn rewrite(locals: &mut Vec<typed::Local>, e: &mut Expr) {
    typed::children_mut(e, &mut |child| rewrite(locals, child));
    let ty = e.ty.clone();
    if let ExprKind::Match { arms, .. } = &mut e.kind {
        if let Some(grouped) = group(locals, arms, &ty) {
            *arms = grouped;
        }
    }
}

/// What an arm's pattern tests before it looks at anything below it.
///
/// Two arms with equal heads are two arms one value can reach, in the order
/// they were written; two arms with different constructor heads are two arms no
/// value can both reach.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Head {
    /// A variant of an enum, by its index and the type it belongs to.
    Tag(TyConId, usize),
    Int(u128, bool),
    Str(String),
    Char(char),
    Bool(bool),
    /// Matches whatever it is given: a wildcard, or a plain binding.
    Any,
}

/// The head of a pattern, or `None` for a shape this pass does not rewrite.
fn head(p: &Pattern) -> Option<Head> {
    match &p.kind {
        PatKind::Wild | PatKind::Unit => Some(Head::Any),
        // A binding *with* a sub-pattern tests through the binding, and
        // grouping it would have to carry the outer name into the group's own
        // arm. Rare enough to be worth a chain rather than a special case.
        PatKind::Bind { sub: None, .. } => Some(Head::Any),
        PatKind::Variant { con, variant, .. } => Some(Head::Tag(*con, *variant)),
        PatKind::Int(v, neg) => Some(Head::Int(*v, *neg)),
        PatKind::Str(s) => Some(Head::Str(s.clone())),
        PatKind::Char(c) => Some(Head::Char(*c)),
        PatKind::Bool(b) => Some(Head::Bool(*b)),
        _ => None,
    }
}

/// Whether this pattern matches every value of its type, decided without the
/// type tables the middle end's shared half does not carry.
///
/// Conservative in the one direction that is safe: a single-variant enum's
/// pattern is called refutable here, which costs a match its rewrite and
/// cannot cost it its meaning.
fn always_matches(p: &Pattern) -> bool {
    match &p.kind {
        PatKind::Wild | PatKind::Unit | PatKind::Error => true,
        PatKind::Bind { sub, .. } => sub.as_ref().is_none_or(|s| always_matches(s)),
        PatKind::Tuple(ps) => ps.iter().all(always_matches),
        PatKind::Struct { fields, .. } => fields.iter().all(|f| always_matches(&f.pattern)),
        _ => false,
    }
}

/// The arms of one match, grouped by head — or `None` where grouping would
/// have to guess.
fn group(
    locals: &mut Vec<typed::Local>,
    arms: &[Arm],
    ty: &crate::compiler::semantics::types::Ty,
) -> Option<Vec<Arm>> {
    if arms.len() < 2 {
        return None;
    }
    let heads: Vec<Head> = arms.iter().map(|a| head(&a.pattern)).collect::<Option<_>>()?;

    // An arm that matches anything ends the match: nothing after it is
    // reachable. Anywhere but last it is a shape this pass declines rather
    // than one it reasons about.
    let default = match heads.split_last() {
        Some((Head::Any, rest)) if !rest.contains(&Head::Any) => true,
        Some((_, rest)) if !rest.contains(&Head::Any) => false,
        _ => return None,
    };
    // A guard on the default arm makes it not a default.
    if default && arms.last().is_some_and(|a| a.guard.is_some()) {
        return None;
    }

    // First-appearance order, so the emitted tests are in the order they were
    // written wherever the writer's order was already a partition — which
    // keeps the diff of this pass readable and the output stable.
    //
    // One pass, with the group each head names looked up rather than scanned
    // for. A scan per head is quadratic in the arm count, and a match is as
    // wide as its enum: `wide-match/20k` is one 10,000-arm match, and this was
    // 100 M head comparisons in a pass whose output is linear.
    let tested = heads.len().checked_sub(usize::from(default))?;
    let mut order: Vec<usize> = Vec::new();
    let mut group_of: Map<&Head, usize> = Map::default();
    let mut groups: Vec<Vec<&Arm>> = Vec::new();
    for (arm, h) in arms.iter().zip(&heads).take(tested) {
        let slot = match group_of.get(h) {
            Some(slot) => *slot,
            None => {
                let slot = groups.len();
                group_of.insert(h, slot);
                groups.push(Vec::new());
                order.push(slot);
                slot
            }
        };
        if let Some(rows) = groups.get_mut(slot) {
            rows.push(arm);
        }
    }
    // Nothing to hoist: one test is one test however it is emitted.
    if order.len() < 2 {
        return None;
    }

    let mut out: Vec<Arm> = Vec::new();
    for rows in &groups {
        out.push(match rows.as_slice() {
            // One arm tests this constructor, so it is already the group.
            // Whether it can fail still matters: a value that reaches it and
            // fails has nowhere left to go.
            [only] if total(only) => (*only).clone(),
            [] | [_] => return None,
            many => collapse(locals, many, ty)?,
        });
    }
    if default {
        out.push(arms.last()?.clone());
    }
    Some(out)
}

/// Whether an arm, having matched its head, cannot then fail.
fn total(a: &Arm) -> bool {
    a.guard.is_none() && fields_of(&a.pattern).iter().all(|f| always_matches(&f.pattern))
}

fn fields_of(p: &Pattern) -> &[FieldPat] {
    match &p.kind {
        PatKind::Variant { fields, .. } => fields,
        _ => &[],
    }
}

/// Several arms testing one constructor, collapsed into one arm that tests it
/// and one `Match` on the field they disagree about.
///
/// The field is bound to a fresh local rather than re-projected per arm, which
/// is what "shared tests hoisted" means here: the head is tested once, the
/// projection happens once, and what is left is a smaller match the same rules
/// apply to.
fn collapse(
    locals: &mut Vec<typed::Local>,
    rows: &[&Arm],
    ty: &crate::compiler::semantics::types::Ty,
) -> Option<Arm> {
    let first = rows.first()?;
    let PatKind::Variant { con, variant, .. } = &first.pattern.kind else { return None };

    // One column. Two arms disagreeing about two fields of one constructor
    // would need a scrutinee that is both, and building a tuple to be one is
    // an allocation this pass would be adding rather than removing.
    let mut tested: Option<usize> = None;
    for row in rows {
        for f in fields_of(&row.pattern) {
            if always_matches(&f.pattern) {
                continue;
            }
            match tested {
                Some(c) if c == f.index => {}
                Some(_) => return None,
                None => tested = Some(f.index),
            }
        }
    }
    let column = match tested {
        Some(c) => c,
        // Nothing below the head is tested, so these arms differ by guard
        // alone; any field will do as the thing to write the inner match over.
        None => fields_of(&first.pattern).first().map(|f| f.index)?,
    };
    // The last row is what a value with this head falls back on, and it has to
    // catch everything: there is no way out of a group.
    if !total(rows.last()?) {
        return None;
    }

    // The fresh local takes its type from the pattern that was matching it,
    // which is the one place a type is available without the type tables.
    let sub = |row: &Arm| -> Pattern {
        fields_of(&row.pattern)
            .iter()
            .find(|f| f.index == column)
            .map(|f| f.pattern.clone())
            .unwrap_or(Pattern {
                kind: PatKind::Wild,
                ty: first.pattern.ty.clone(),
                span: row.pattern.span,
            })
    };
    let bound = sub(first);
    let held = LocalId(locals.len() as u32);
    locals.push(typed::Local {
        name: "col".to_string(),
        ty: bound.ty.clone(),
        span: bound.span,
    });

    let arms: Vec<Arm> = rows
        .iter()
        .map(|row| Arm {
            pattern: sub(row),
            guard: row.guard.clone(),
            body: row.body.clone(),
            span: row.span,
        })
        .collect();
    let mut body = Expr::new(
        ExprKind::Match {
            scrutinee: Box::new(Expr::new(
                ExprKind::Local(held),
                bound.ty.clone(),
                bound.span,
            )),
            arms,
        },
        ty.clone(),
        first.span,
    );
    // The match just built is a match like any other, and the column below it
    // may be a tree too.
    rewrite(locals, &mut body);

    Some(Arm {
        pattern: Pattern {
            kind: PatKind::Variant {
                con: *con,
                variant: *variant,
                fields: vec![FieldPat {
                    index: column,
                    pattern: Pattern {
                        kind: PatKind::Bind { local: held, sub: None },
                        ty: bound.ty,
                        span: bound.span,
                    },
                }],
            },
            ty: first.pattern.ty.clone(),
            span: first.pattern.span,
        },
        guard: None,
        body,
        span: first.span,
    })
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::compiler::middle::monomorphize::{Func, FuncKind, Program, ProgramRoots};
    use crate::compiler::semantics::typed::{
        Arm, Expr, ExprKind, FieldPat, Local, PatKind, Pattern,
    };
    use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty, TyConId};
    use crate::diagnostics::Span;
    use crate::hash::Map as HashMap;

    const CON: TyConId = TyConId(7);

    fn e(kind: ExprKind) -> Expr {
        Expr::new(kind, Ty::Unit, Span::default())
    }

    fn pat(kind: PatKind) -> Pattern {
        Pattern { kind, ty: Ty::Unit, span: Span::default() }
    }

    /// `.V(sub)`, the one-field variant every case here is written over.
    fn variant(v: usize, sub: PatKind) -> Pattern {
        pat(PatKind::Variant {
            con: CON,
            variant: v,
            fields: vec![FieldPat { index: 0, pattern: pat(sub) }],
        })
    }

    fn arm(pattern: Pattern, guard: Option<Expr>, body: u128) -> Arm {
        Arm { pattern, guard, body: e(ExprKind::Int(body, false)), span: Span::default() }
    }

    /// One function whose whole body is the match under test.
    fn matched(arms: Vec<Arm>) -> Program {
        let body = e(ExprKind::Match {
            scrutinee: Box::new(e(ExprKind::Local(LocalId(0)))),
            arms,
        });
        Program {
            funcs: vec![Func {
                symbol: "f".to_string(),
                debug_name: "f".to_string(),
                params: vec![LocalId(0)],
                locals: vec![Local {
                    name: "s".to_string(),
                    ty: Ty::Unit,
                    span: Span::default(),
                }],
                kind: FuncKind::Body(body),
                ret: Ty::Unit,
                desc: None,
                span: Span::default(),
            }],
            roots: ProgramRoots::Main(FuncIdx(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: HashMap::default(),
            ctx_layouts: HashMap::default(),
            shapes: Default::default(),
            stylesheet: String::new(),
            inline_styles: false,
            themes: false,
        }
    }

    fn arms_of(p: &Program) -> &[Arm] {
        match &p.funcs[0].body().unwrap().kind {
            ExprKind::Match { arms, .. } => arms,
            _ => panic!("the body is the match"),
        }
    }

    /// A match that already tests each constructor once is already a tree.
    #[test]
    fn a_partition_is_left_as_it_was() {
        let mut p = matched(vec![
            arm(variant(0, PatKind::Wild), None, 1),
            arm(variant(1, PatKind::Wild), None, 2),
        ]);
        run(&mut p);
        assert_eq!(arms_of(&p).len(), 2);
        assert!(matches!(arms_of(&p)[0].pattern.kind, PatKind::Variant { variant: 0, .. }));
    }

    /// Two arms on one constructor become one test of it and a match on the
    /// field they disagree about — the shared test asked once.
    #[test]
    fn arms_on_one_constructor_are_hoisted_into_one_test() {
        let mut p = matched(vec![
            arm(variant(0, PatKind::Int(1, false)), None, 10),
            arm(variant(0, PatKind::Wild), None, 20),
            arm(variant(1, PatKind::Wild), None, 30),
        ]);
        run(&mut p);
        let arms = arms_of(&p);
        assert_eq!(arms.len(), 2);
        // The group's own arm binds the column rather than testing it.
        let PatKind::Variant { variant: 0, fields, .. } = &arms[0].pattern.kind else {
            panic!("the group tests its constructor")
        };
        assert!(matches!(fields[0].pattern.kind, PatKind::Bind { sub: None, .. }));
        // And the two rows are inside, in the order they were written.
        let ExprKind::Match { arms: inner, .. } = &arms[0].body.kind else {
            panic!("the column is a match")
        };
        assert_eq!(inner.len(), 2);
        assert!(matches!(inner[0].pattern.kind, PatKind::Int(1, false)));
        assert!(matches!(inner[0].body.kind, ExprKind::Int(10, false)));
        assert!(matches!(inner[1].body.kind, ExprKind::Int(20, false)));
        // A fresh local holds the column.
        assert_eq!(p.funcs[0].locals.len(), 2);
    }

    /// A guarded arm may fail into a later arm, and grouping must keep it able
    /// to: the guard travels into the group, ahead of the row that catches it.
    #[test]
    fn a_guarded_arm_keeps_the_arm_it_falls_through_to() {
        let mut p = matched(vec![
            arm(variant(0, PatKind::Wild), Some(e(ExprKind::Bool(true))), 10),
            arm(variant(1, PatKind::Wild), None, 20),
            arm(variant(0, PatKind::Wild), None, 30),
        ]);
        run(&mut p);
        let arms = arms_of(&p);
        assert_eq!(arms.len(), 2);
        assert!(arms[0].guard.is_none(), "the group's own test is not guarded");
        let ExprKind::Match { arms: inner, .. } = &arms[0].body.kind else {
            panic!("the column is a match")
        };
        assert!(inner[0].guard.is_some());
        assert!(matches!(inner[1].body.kind, ExprKind::Int(30, false)));
    }

    /// A group whose last row can still fail would have to fall out of the
    /// group and into the default, and there is no path back out — so the
    /// match stays a chain rather than duplicating the default into it.
    #[test]
    fn a_group_that_can_fail_is_declined() {
        let mut p = matched(vec![
            arm(variant(0, PatKind::Int(1, false)), None, 10),
            arm(variant(1, PatKind::Wild), None, 20),
            arm(pat(PatKind::Wild), None, 30),
        ]);
        run(&mut p);
        assert_eq!(arms_of(&p).len(), 3);
        assert!(matches!(arms_of(&p)[0].pattern.kind, PatKind::Variant { .. }));
    }

    /// An arm that matches anything hides every arm after it, and this pass
    /// does not reason about that: it declines.
    #[test]
    fn an_irrefutable_arm_that_is_not_last_is_declined() {
        let mut p = matched(vec![
            arm(variant(0, PatKind::Wild), None, 10),
            arm(pat(PatKind::Wild), None, 20),
            arm(variant(1, PatKind::Wild), None, 30),
        ]);
        run(&mut p);
        assert_eq!(arms_of(&p).len(), 3);
        assert!(matches!(arms_of(&p)[1].pattern.kind, PatKind::Wild));
    }

    /// An or-pattern head reaches several constructors from one body, which is
    /// a row this pass would have to duplicate to group.
    #[test]
    fn an_or_pattern_head_is_declined() {
        let mut p = matched(vec![
            arm(
                pat(PatKind::Or(vec![variant(0, PatKind::Wild), variant(1, PatKind::Wild)])),
                None,
                10,
            ),
            arm(variant(2, PatKind::Wild), None, 20),
        ]);
        run(&mut p);
        assert!(matches!(arms_of(&p)[0].pattern.kind, PatKind::Or(_)));
    }

    /// Literal arms group too, and a trailing wildcard stays the default.
    #[test]
    fn literal_arms_keep_their_default() {
        let mut p = matched(vec![
            arm(pat(PatKind::Int(0, false)), None, 10),
            arm(pat(PatKind::Int(1, false)), None, 20),
            arm(pat(PatKind::Wild), None, 30),
        ]);
        run(&mut p);
        let arms = arms_of(&p);
        assert_eq!(arms.len(), 3);
        assert!(matches!(arms[2].pattern.kind, PatKind::Wild));
    }
}
