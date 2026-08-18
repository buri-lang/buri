//! Exhaustiveness and reachability.
//!
//! Every `match` must cover its scrutinee's type, and no arm may be
//! unreachable — both are errors, not warnings (SPEC 6.5, 7.3). The checker
//! reasons about enum variants, `Bool`, tuples, structs, and array lengths. It
//! does not attempt exhaustiveness over integer or string ranges; those need a
//! `_` arm.
//!
//! This is the usefulness algorithm: a pattern vector is *useful* against a
//! matrix when it matches some value the matrix does not. An arm is
//! unreachable when it is not useful against the arms before it, and a match
//! is non-exhaustive when a wildcard row is still useful against all of them.
//! It is Maranget's, from *Warnings for pattern matching* (JFP 2007), vendored
//! at `reference/maranget-warnings-for-pattern-matching.pdf`.

use crate::compiler::semantics::inference::Infer;
use crate::compiler::semantics::typed::{self, PatKind, Pattern};
use crate::compiler::semantics::types::{Prim, Ty, TyConId, TyDef};
use crate::diagnostics::{Diagnostic, Span};

/// The head constructor of a pattern.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Ctor {
    Variant(TyConId, usize),
    /// Structs, tuples and unit: exactly one constructor.
    Single,
    Bool(bool),
    /// A fixed-length array pattern.
    Array(usize),
    /// `[a, b, ..rest]` — matches any length at or above `n`.
    ArrayRest(usize),
    /// A literal drawn from a set too large to enumerate — an integer, a
    /// string, a char, a float. Two different literals are two different
    /// constructors, and no finite set of them ever completes a match.
    Lit(LitValue),
}

/// A literal pattern's value, for the "same constructor?" test the usefulness
/// algorithm runs on it.
///
/// This was a `String` built with `format!` and a one-character type tag, so
/// "the same constructor" meant string equality on a rendering: `-0.0` and
/// `0.0` formatted to `"f-0"` and `"f0"` and were treated as two distinct
/// constructors even though they are the same value, and an integer and a
/// float were kept apart only by a prefix the producer had to remember.
#[derive(Clone, PartialEq, Eq, Debug)]
enum LitValue {
    /// Magnitude and sign, as the pattern spells them.
    Int(u128, bool),
    /// The bit pattern, so equality is total and `-0.0` and `0.0` agree the
    /// way the emitted comparison does.
    Float(u64),
    Str(String),
    Char(char),
}

impl Ctor {
    /// How many sub-patterns this constructor holds, for a given type.
    fn arity(&self, tables: &crate::compiler::semantics::types::Tables, ty: &Ty) -> usize {
        match self {
            Ctor::Variant(con, v) => {
                tables.tycon(*con).variants().get(*v).map_or(0, |x| x.fields.len())
            }
            Ctor::Single => match ty {
                Ty::Tuple(ts) => ts.len(),
                Ty::Con(con, _) => tables.tycon(*con).fields().len(),
                _ => 0,
            },
            Ctor::Bool(_) | Ctor::Lit(_) => 0,
            Ctor::Array(n) => *n,
            Ctor::ArrayRest(n) => *n,
        }
    }

    fn field_types(&self, tables: &crate::compiler::semantics::types::Tables, ty: &Ty) -> Vec<Ty> {
        match self {
            Ctor::Variant(con, v) => {
                let args = match ty {
                    Ty::Con(_, a) => a.clone(),
                    _ => Vec::new(),
                };
                let Some(variant) = tables.tycon(*con).variants().get(*v) else {
                    return Vec::new();
                };
                variant
                    .fields
                    .iter()
                    .map(|f| crate::compiler::semantics::types::substitute(&f.ty, &args, None))
                    .collect()
            }
            Ctor::Single => match ty {
                Ty::Tuple(ts) => ts.clone(),
                Ty::Con(con, args) => tables
                    .tycon(*con)
                    .fields()
                    .iter()
                    .map(|f| crate::compiler::semantics::types::substitute(&f.ty, args, None))
                    .collect(),
                _ => Vec::new(),
            },
            Ctor::Array(n) | Ctor::ArrayRest(n) => {
                let elem = match ty {
                    Ty::Array(e) => (**e).clone(),
                    _ => Ty::Error,
                };
                vec![elem; *n]
            }
            _ => Vec::new(),
        }
    }
}

/// A pattern as this algorithm sees it: either a wildcard or a constructor
/// applied to sub-patterns.
#[derive(Clone, Debug)]
enum Pat {
    Wild,
    Ctor(Ctor, Vec<Pat>),
    /// An or-pattern is expanded into several rows rather than handled here.
    Or(Vec<Pat>),
}

fn lower(p: &Pattern) -> Pat {
    match &p.kind {
        PatKind::Wild | PatKind::Error => Pat::Wild,
        // A binding matches everything its sub-pattern does.
        PatKind::Bind { sub, .. } => match sub {
            Some(s) => lower(s),
            None => Pat::Wild,
        },
        PatKind::Unit => Pat::Ctor(Ctor::Single, Vec::new()),
        PatKind::Bool(b) => Pat::Ctor(Ctor::Bool(*b), Vec::new()),
        PatKind::Int(v, neg) => Pat::Ctor(Ctor::Lit(LitValue::Int(*v, *neg)), Vec::new()),
        // `+0.0 == -0.0`, so they are one constructor rather than two.
        PatKind::Float(v) => {
            Pat::Ctor(Ctor::Lit(LitValue::Float((v + 0.0).to_bits())), Vec::new())
        }
        PatKind::Str(v) => Pat::Ctor(Ctor::Lit(LitValue::Str(v.clone())), Vec::new()),
        PatKind::Char(v) => Pat::Ctor(Ctor::Lit(LitValue::Char(*v)), Vec::new()),
        PatKind::Tuple(ps) => Pat::Ctor(Ctor::Single, ps.iter().map(lower).collect()),
        PatKind::Struct { con, fields } => {
            let n = fields.iter().map(|f| f.index.saturating_add(1)).max().unwrap_or(0);
            let total = n.max(fields.len());
            let mut subs = vec![Pat::Wild; total];
            for f in fields {
                if let Some(slot) = subs.get_mut(f.index) {
                    *slot = lower(&f.pattern);
                }
            }
            let _ = con;
            Pat::Ctor(Ctor::Single, subs)
        }
        PatKind::Variant { con, variant, fields } => {
            let total = fields.iter().map(|f| f.index.saturating_add(1)).max().unwrap_or(0);
            let mut subs = vec![Pat::Wild; total];
            for f in fields {
                if let Some(slot) = subs.get_mut(f.index) {
                    *slot = lower(&f.pattern);
                }
            }
            Pat::Ctor(Ctor::Variant(*con, *variant), subs)
        }
        PatKind::Array { elems, rest } => {
            let subs: Vec<Pat> = elems.iter().map(lower).collect();
            let ctor =
                if rest.is_open() { Ctor::ArrayRest(subs.len()) } else { Ctor::Array(subs.len()) };
            Pat::Ctor(ctor, subs)
        }
        PatKind::Or(alts) => Pat::Or(alts.iter().map(lower).collect()),
    }
}

/// The longest array length any pattern in the match distinguishes, plus one.
/// Beyond it every value behaves the same, so `[a, ..rest]` can be expanded
/// into the fixed lengths `n ..= limit` and arrays become an ordinary
/// enumerable type.
fn length_limit(p: &Pat) -> usize {
    match p {
        Pat::Wild => 0,
        Pat::Or(alts) => alts.iter().map(length_limit).max().unwrap_or(0),
        Pat::Ctor(c, subs) => {
            let here = match c {
                Ctor::Array(n) | Ctor::ArrayRest(n) => *n,
                _ => 0,
            };
            here.max(subs.iter().map(length_limit).max().unwrap_or(0))
        }
    }
}

/// Rewrites `[a, ..rest]` as `[a] | [a, _] | ... | [a, _, ..]` up to `limit`.
fn expand_lengths(p: Pat, limit: usize) -> Pat {
    match p {
        Pat::Wild => Pat::Wild,
        Pat::Or(alts) => {
            Pat::Or(alts.into_iter().map(|a| expand_lengths(a, limit)).collect())
        }
        Pat::Ctor(Ctor::ArrayRest(n), subs) => {
            let subs: Vec<Pat> =
                subs.into_iter().map(|s| expand_lengths(s, limit)).collect();
            let alts: Vec<Pat> = (n..=limit.max(n))
                .map(|len| {
                    let mut fields = subs.clone();
                    while fields.len() < len {
                        fields.push(Pat::Wild);
                    }
                    Pat::Ctor(Ctor::Array(len), fields)
                })
                .collect();
            // One length is not an alternation.
            match <[Pat; 1]>::try_from(alts) {
                Ok([only]) => only,
                Err(alts) => Pat::Or(alts),
            }
        }
        Pat::Ctor(c, subs) => {
            Pat::Ctor(c, subs.into_iter().map(|s| expand_lengths(s, limit)).collect())
        }
    }
}

/// Expands or-patterns so each row holds no alternation *at the top of a
/// column*. An alternation nested inside a constructor stays where it is; it
/// surfaces later, when `specialize` peels that constructor off, and the matrix
/// operations below distribute over it there.
fn expand(row: Vec<Pat>) -> Vec<Vec<Pat>> {
    let Some(pos) = row.iter().position(|p| matches!(p, Pat::Or(_))) else {
        return vec![row];
    };
    let Some(Pat::Or(alts)) = row.get(pos).cloned() else { return vec![row] };
    let mut out = Vec::new();
    for alt in alts {
        let mut next = row.clone();
        if let Some(slot) = next.get_mut(pos) {
            *slot = alt;
        }
        out.extend(expand(next));
    }
    out
}

/// One row per alternative of an or-pattern that sits at the head of a column,
/// each carrying the rest of the original row along with it.
fn distribute(alts: &[Pat], rest: &[Pat]) -> Vec<Vec<Pat>> {
    alts.iter()
        .map(|a| {
            let mut row = Vec::with_capacity(rest.len().saturating_add(1));
            row.push(a.clone());
            row.extend_from_slice(rest);
            row
        })
        .collect()
}

/// The constructors a row's head can start with. An or-pattern contributes
/// every constructor any of its alternatives does, so that a column covered by
/// `true | false` counts as complete.
fn collect_head_ctors(p: &Pat, out: &mut Vec<Ctor>) {
    match p {
        Pat::Wild => {}
        Pat::Ctor(c, _) => {
            if !out.contains(c) {
                out.push(c.clone());
            }
        }
        Pat::Or(alts) => {
            for a in alts {
                collect_head_ctors(a, out);
            }
        }
    }
}

struct Ctx<'a> {
    tables: &'a crate::compiler::semantics::types::Tables,
    /// The largest array length the match distinguishes.
    limit: usize,
}

impl<'a> Ctx<'a> {
    /// The complete set of constructors for a type, or `None` when the type
    /// has too many to enumerate.
    fn all_ctors(&self, ty: &Ty) -> Option<Vec<Ctor>> {
        match ty {
            Ty::Con(con, _) => match &self.tables.tycon(*con).def {
                TyDef::Enum { variants } => {
                    Some((0..variants.len()).map(|i| Ctor::Variant(*con, i)).collect())
                }
                TyDef::Struct { .. } => Some(vec![Ctor::Single]),
                TyDef::Prim(Prim::Bool) => Some(vec![Ctor::Bool(false), Ctor::Bool(true)]),
                // Integers, strings, chars and floats need a `_` arm.
                TyDef::Prim(_) => None,
            },
            Ty::Tuple(_) | Ty::Unit => Some(vec![Ctor::Single]),
            // Finite, because rest patterns were expanded into fixed lengths
            // and nothing distinguishes anything longer than `limit`.
            Ty::Array(_) => Some((0..=self.limit).map(Ctor::Array).collect()),
            _ => None,
        }
    }

    /// Rows of the matrix whose first pattern is `ctor`, with that pattern's
    /// sub-patterns spliced in.
    fn specialize(&self, matrix: &[Vec<Pat>], ctor: &Ctor, arity: usize) -> Vec<Vec<Pat>> {
        let mut out = Vec::new();
        for row in matrix {
            let Some((head, rest)) = row.split_first() else { continue };
            match head {
                Pat::Wild => {
                    let mut next = vec![Pat::Wild; arity];
                    next.extend_from_slice(rest);
                    out.push(next);
                }
                Pat::Ctor(c, subs) if c == ctor => {
                    let mut next = subs.clone();
                    while next.len() < arity {
                        next.push(Pat::Wild);
                    }
                    next.truncate(arity);
                    next.extend_from_slice(rest);
                    out.push(next);
                }
                // An alternation `expand` left nested, now exposed by peeling
                // its constructor off. Distribute: each alternative is a row of
                // its own, and the coverage of all of them is the row's.
                // Dropping the row instead would lose that coverage and make a
                // wildcard look useful — the false rejection of `.Some(true |
                // false)`.
                Pat::Or(alts) => {
                    out.extend(self.specialize(&distribute(alts, rest), ctor, arity));
                }
                _ => {}
            }
        }
        out
    }

    /// Rows whose first pattern is a wildcard, with that column dropped.
    fn default_matrix(&self, matrix: &[Vec<Pat>]) -> Vec<Vec<Pat>> {
        let mut out = Vec::new();
        for row in matrix {
            let Some((head, rest)) = row.split_first() else { continue };
            match head {
                Pat::Wild => out.push(rest.to_vec()),
                // Distribute, for the same reason `specialize` does. An
                // alternative that is a wildcard makes the whole row a default
                // row.
                Pat::Or(alts) => out.extend(self.default_matrix(&distribute(alts, rest))),
                Pat::Ctor(..) => {}
            }
        }
        out
    }

    fn head_ctors(&self, matrix: &[Vec<Pat>]) -> Vec<Ctor> {
        let mut out: Vec<Ctor> = Vec::new();
        for row in matrix {
            if let Some(head) = row.first() {
                collect_head_ctors(head, &mut out);
            }
        }
        out
    }

    /// Whether `v` matches a value the matrix does not. Returns a witness when
    /// it does, so a diagnostic can name the missing case.
    fn useful(&self, matrix: &[Vec<Pat>], v: &[Pat], types: &[Ty]) -> Option<Vec<Witness>> {
        let Some((head, tail)) = v.split_first() else {
            return matrix.is_empty().then(Vec::new);
        };
        // A row and its type list are built together, but the type list is the
        // one the caller supplied, so a shorter one leaves the columns past it
        // untyped rather than out of bounds.
        let (head_ty, rest_types) = match types.split_first() {
            Some((t, rest)) => (t.clone(), rest),
            None => (Ty::Error, &[][..]),
        };

        match head {
            Pat::Or(alts) => {
                for alt in alts {
                    let mut next = vec![alt.clone()];
                    next.extend_from_slice(tail);
                    if let Some(w) = self.useful(matrix, &next, types) {
                        return Some(w);
                    }
                }
                None
            }
            Pat::Ctor(c, subs) => {
                let arity = c.arity(self.tables, &head_ty);
                let specialized = self.specialize(matrix, c, arity);
                let mut next: Vec<Pat> = subs.clone();
                while next.len() < arity {
                    next.push(Pat::Wild);
                }
                next.extend_from_slice(tail);
                let mut next_types = c.field_types(self.tables, &head_ty);
                next_types.extend_from_slice(rest_types);
                self.useful(&specialized, &next, &next_types).map(|w| {
                    let (taken, rest) = w.split_at(arity.min(w.len()));
                    let mut out =
                        vec![Witness::Ctor(c.clone(), head_ty.clone(), taken.to_vec())];
                    out.extend_from_slice(rest);
                    out
                })
            }
            Pat::Wild => {
                let used = self.head_ctors(matrix);
                let complete = match self.all_ctors(&head_ty) {
                    Some(all) => all.iter().all(|c| used.contains(c)),
                    None => false,
                };
                if complete {
                    let all = self.all_ctors(&head_ty).unwrap_or_else(|| used.clone());
                    for c in all {
                        let arity = c.arity(self.tables, &head_ty);
                        let specialized = self.specialize(matrix, &c, arity);
                        let mut next = vec![Pat::Wild; arity];
                        next.extend_from_slice(tail);
                        let mut next_types = c.field_types(self.tables, &head_ty);
                        next_types.extend_from_slice(rest_types);
                        if let Some(w) = self.useful(&specialized, &next, &next_types) {
                            let (taken, rest) = w.split_at(arity.min(w.len()));
                            let mut out =
                                vec![Witness::Ctor(c.clone(), head_ty.clone(), taken.to_vec())];
                            out.extend_from_slice(rest);
                            return Some(out);
                        }
                    }
                    None
                } else {
                    let default = self.default_matrix(matrix);
                    self.useful(&default, tail, rest_types).map(|w| {
                        // Name a constructor the match does not mention, when
                        // there is one to name.
                        let missing = self
                            .all_ctors(&head_ty)
                            .and_then(|all| all.into_iter().find(|c| !used.contains(c)))
                            .map(|c| {
                                let arity = c.arity(self.tables, &head_ty);
                                Witness::Ctor(
                                    c,
                                    head_ty.clone(),
                                    vec![Witness::Wild; arity],
                                )
                            })
                            .unwrap_or(Witness::Wild);
                        let mut out = vec![missing];
                        out.extend(w);
                        out
                    })
                }
            }
        }
    }

}

/// A value the match does not cover, rendered into the diagnostic.
#[derive(Clone, Debug)]
enum Witness {
    Wild,
    Ctor(Ctor, Ty, Vec<Witness>),
}

fn render(tables: &crate::compiler::semantics::types::Tables, w: &Witness) -> String {
    match w {
        Witness::Wild => "_".into(),
        Witness::Ctor(c, ty, subs) => match c {
            Ctor::Variant(con, v) => {
                let Some(variant) = tables.tycon(*con).variants().get(*v) else {
                    return "_".into();
                };
                if subs.is_empty() {
                    format!(".{}", variant.name)
                } else if variant.record {
                    let fields: Vec<String> = variant
                        .fields
                        .iter()
                        .zip(subs)
                        .map(|(f, s)| format!("{}: {}", f.name, render(tables, s)))
                        .collect();
                    format!(".{} {{ {} }}", variant.name, fields.join(", "))
                } else {
                    let parts: Vec<String> = subs.iter().map(|s| render(tables, s)).collect();
                    format!(".{}({})", variant.name, parts.join(", "))
                }
            }
            Ctor::Bool(b) => b.to_string(),
            Ctor::Single => match ty {
                Ty::Tuple(_) => {
                    let parts: Vec<String> = subs.iter().map(|s| render(tables, s)).collect();
                    format!("({})", parts.join(", "))
                }
                Ty::Con(con, _) => {
                    let name = &tables.tycon(*con).name;
                    if subs.is_empty() {
                        name.clone()
                    } else {
                        let fields = tables.tycon(*con).fields();
                        let record = matches!(tables.tycon(*con).def, TyDef::Struct { record: true, .. });
                        if record {
                            let parts: Vec<String> = fields
                                .iter()
                                .zip(subs)
                                .map(|(f, s)| format!("{}: {}", f.name, render(tables, s)))
                                .collect();
                            format!("{name} {{ {} }}", parts.join(", "))
                        } else {
                            let parts: Vec<String> =
                                subs.iter().map(|s| render(tables, s)).collect();
                            format!("{name}({})", parts.join(", "))
                        }
                    }
                }
                _ => "()".into(),
            },
            Ctor::Array(n) | Ctor::ArrayRest(n) => {
                let parts: Vec<String> = subs.iter().map(|s| render(tables, s)).collect();
                let _ = n;
                format!("[{}]", parts.join(", "))
            }
            Ctor::Lit(_) => "_".into(),
        },
    }
}

pub fn check(inf: &mut Infer<'_, '_>, scrutinee: &Ty, arms: &[typed::Arm], span: Span) {
    if scrutinee.is_error() {
        return;
    }
    let lowered: Vec<Pat> = arms.iter().map(|a| lower(&a.pattern)).collect();
    let limit = lowered.iter().map(length_limit).max().unwrap_or(0).saturating_add(1);
    let ctx = Ctx { tables: &inf.c.tables, limit };
    let types = vec![scrutinee.clone()];

    // Arms are tried in order and the first matching arm wins, so an arm is
    // unreachable when the arms before it already cover it. A guarded arm
    // covers nothing, because its guard may fail.
    let mut covering: Vec<Vec<Pat>> = Vec::new();
    let mut reported = Vec::new();
    for (arm, low) in arms.iter().zip(&lowered) {
        let rows = expand(vec![expand_lengths(low.clone(), limit)]);
        let useful = rows.iter().any(|r| ctx.useful(&covering, r, &types).is_some());
        if !useful {
            reported.push(arm.span);
        }
        if arm.guard.is_none() {
            covering.extend(rows);
        }
    }
    for s in reported {
        inf.c.diags.push(
            Diagnostic::error(s, "this arm is unreachable").with_code("unreachable-arm")
                .with_note("the arms before it already cover everything it matches")
                .with_fix("delete it, or move it above the arm that subsumes it"),
        );
    }

    // A non-exhaustive match is a compile error that names a missing case.
    let ctx = Ctx { tables: &inf.c.tables, limit };
    if let Some(witness) = ctx.useful(&covering, &[Pat::Wild], &types) {
        let shown = witness
            .first()
            .map(|w| render(&inf.c.tables, w))
            .unwrap_or_else(|| "_".into());
        let mut d = Diagnostic::error(span, format!("this `match` does not cover `{shown}`")).with_code("match-not-exhaustive")
            .with_label("not covered");
        if ctx.all_ctors(scrutinee).is_none() {
            d = d
                .with_note("exhaustiveness is not attempted over integer or string ranges")
                .with_fix("add a `_` arm");
        } else {
            d = d
                .with_note("every `match` must cover its scrutinee's type")
                .with_fix(format!("add an arm for `{shown}`, or a `_` arm for everything left"));
        }
        inf.c.diags.push(d);
    }
}
