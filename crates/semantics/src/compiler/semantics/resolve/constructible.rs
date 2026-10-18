//! Every type can be built (design/static-rules.md rule 14).
//!
//! A type can't be when building one needs one already: `struct S { s: S }`,
//! `enum A { A(B) } enum B { B(A) }`. `enum Wrap { W(Option<Wrap>) }` has one,
//! `.W(.None)`: a generic argument matters only where the constructor's own
//! fields need it.

use super::*;

/// Why a declaration can never be built, for its diagnostic.
pub(super) enum Why {
    /// Every variant holds another of it.
    Variants,
    /// Every value holds another of it in this field.
    Field(String),
}

/// The declarations among `decls` that can never be built, and the ones of
/// those to report, each with what it was declared with and why.
///
/// One that can't be only because it holds another such type is not reported:
/// that type's own report is the mistake, and fixing it gives this one values
/// too. `refused` are the enums already reported for having no variants.
pub(super) fn unconstructible<P: Copy>(
    tables: &Tables,
    decls: &[(TyConId, P)],
    refused: &HashSet<TyConId>,
) -> (Vec<TyConId>, Vec<(TyConId, P, Why)>) {
    let unbuilt: Vec<(TyConId, P)> = decls
        .iter()
        .copied()
        .filter(|&(c, _)| Search::new(tables, refused.clone()).declared(c).is_err())
        .collect();
    let none: Vec<TyConId> = unbuilt.iter().map(|&(c, _)| c).collect();
    let mut out = Vec::new();
    for &(c, p) in &unbuilt {
        // Every unbuildable type outside this one's cycle counts as buildable,
        // so what is left is this one's own doing.
        let mut assumed = refused.clone();
        assumed.extend(
            none.iter()
                .copied()
                .filter(|&o| !(reaches(tables, &none, c, o) && reaches(tables, &none, o, c))),
        );
        if let Err(why) = Search::new(tables, assumed).declared(c) {
            out.push((c, p, why));
        }
    }
    (none, out)
}

/// Whether `to` is `from`, or a type `from` holds, among `among`.
fn reaches(tables: &Tables, among: &[TyConId], from: TyConId, to: TyConId) -> bool {
    let mut seen: HashSet<TyConId> = HashSet::default();
    let mut stack = vec![from];
    while let Some(c) = stack.pop() {
        if c == to {
            return true;
        }
        let con = tables.tycon(c);
        let fields = con
            .fields()
            .iter()
            .chain(con.variants().iter().flat_map(|v| v.fields.iter()));
        let mut held = Vec::new();
        for f in fields {
            mentioned(&f.ty, &mut held);
        }
        stack.extend(
            held.into_iter()
                .filter(|h| among.contains(h) && seen.insert(*h)),
        );
    }
    false
}

/// The named types `ty` stores, through generic arguments and tuples. A list
/// can be empty and a function needs no argument to exist, so neither counts.
fn mentioned(ty: &Ty, out: &mut Vec<TyConId>) {
    match ty.kind() {
        TyKind::Con(c, args) => {
            out.push(*c);
            args.iter().for_each(|a| mentioned(a, out));
        }
        TyKind::Tuple(es) => es.iter().for_each(|e| mentioned(e, out)),
        _ => {}
    }
}

/// A search for one value, over a constructor and which of its arguments have
/// none. That pair is finite, so a constructor that grows its own argument,
/// `Grow<Option<T>>` inside `Grow<T>`, still ends.
struct Search<'a> {
    tables: &'a Tables,
    /// Constructors taken to have values whatever their arguments.
    assumed: HashSet<TyConId>,
    /// The pairs being searched. Meeting one again finds nothing: a value
    /// built from itself is no value.
    open: Vec<(TyConId, Vec<u32>)>,
}

impl<'a> Search<'a> {
    fn new(tables: &'a Tables, assumed: HashSet<TyConId>) -> Search<'a> {
        Search {
            tables,
            assumed,
            open: Vec::new(),
        }
    }

    /// Whether a declaration has a value when each of its parameters does,
    /// and if not, why.
    fn declared(&mut self, c: TyConId) -> Result<(), Why> {
        if self.assumed.contains(&c) {
            return Ok(());
        }
        self.open.push((c, Vec::new()));
        let found = self.values(c, &[]);
        self.open.pop();
        found
    }

    /// Whether `c` has a value when the parameters in `lacking` have none.
    fn has(&mut self, c: TyConId, lacking: Vec<u32>) -> bool {
        if self.assumed.contains(&c) {
            return true;
        }
        if self.open.iter().any(|(o, l)| (o, l) == (&c, &lacking)) {
            return false;
        }
        self.open.push((c, lacking.clone()));
        let found = self.values(c, &lacking).is_ok();
        self.open.pop();
        found
    }

    /// Whether some value of `c` needs none of `lacking`, and if not, why: for
    /// a struct, the first field that can't be built.
    fn values(&mut self, c: TyConId, lacking: &[u32]) -> Result<(), Why> {
        let tables = self.tables;
        match &tables.tycon(c).def {
            TyDef::Prim(_) => Ok(()),
            TyDef::Struct { fields, .. } => fields
                .iter()
                .find(|f| !self.ty(&f.ty, lacking))
                .map_or(Ok(()), |f| Err(Why::Field(f.name.clone()))),
            TyDef::Enum { variants } => variants
                .iter()
                .any(|v| v.fields.iter().all(|f| self.ty(&f.ty, lacking)))
                .then_some(())
                .ok_or(Why::Variants),
        }
    }

    fn ty(&mut self, ty: &Ty, lacking: &[u32]) -> bool {
        match ty.kind() {
            TyKind::Param(i) => !lacking.contains(i),
            TyKind::Con(c, args) => {
                let lacking = (0u32..)
                    .zip(args.iter())
                    .filter(|(_, a)| !self.ty(a, lacking))
                    .map(|(j, _)| j)
                    .collect();
                self.has(*c, lacking)
            }
            TyKind::Tuple(es) => es.iter().all(|e| self.ty(e, lacking)),
            _ => true,
        }
    }
}
