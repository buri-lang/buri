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
/// those to report, each with why.
///
/// One that can't be only because it holds another such type is not reported:
/// that type's own report is the mistake, and fixing it gives this one values
/// too. `refused` are the enums already reported for having no variants.
pub(super) fn unconstructible(
    tables: &Tables,
    decls: &[TyConId],
    refused: &HashSet<TyConId>,
) -> (Vec<TyConId>, Vec<(TyConId, Why)>) {
    let none: Vec<TyConId> = decls
        .iter()
        .copied()
        .filter(|&c| !Search::new(tables, refused.clone()).declared(c))
        .collect();
    let mut out = Vec::new();
    for &c in &none {
        // Every unbuildable type outside this one's cycle counts as buildable,
        // so what is left is this one's own doing.
        let mut assumed = refused.clone();
        assumed.extend(
            none.iter()
                .copied()
                .filter(|&o| !(reaches(tables, &none, c, o) && reaches(tables, &none, o, c))),
        );
        let mut search = Search::new(tables, assumed);
        if !search.declared(c) {
            out.push((c, search.why(c)));
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
/// values. That pair is finite, so a constructor that grows its own argument,
/// `Grow<Option<T>>` inside `Grow<T>`, still ends.
struct Search<'a> {
    tables: &'a Tables,
    /// Constructors taken to have values whatever their arguments.
    assumed: HashSet<TyConId>,
    /// The pairs being searched. Meeting one again finds nothing: a value
    /// built from itself is no value.
    open: Vec<(TyConId, Vec<bool>)>,
}

impl<'a> Search<'a> {
    fn new(tables: &'a Tables, assumed: HashSet<TyConId>) -> Search<'a> {
        Search {
            tables,
            assumed,
            open: Vec::new(),
        }
    }

    /// Whether a declaration has a value when each of its parameters does.
    fn declared(&mut self, c: TyConId) -> bool {
        self.has(c, vec![true; self.tables.tycon(c).arity()])
    }

    fn has(&mut self, c: TyConId, given: Vec<bool>) -> bool {
        if self.assumed.contains(&c) {
            return true;
        }
        let key = (c, given.clone());
        if self.open.contains(&key) {
            return false;
        }
        self.open.push(key);
        let tables = self.tables;
        let found = match &tables.tycon(c).def {
            TyDef::Prim(_) => true,
            TyDef::Struct { fields, .. } => fields.iter().all(|f| self.ty(&f.ty, &given)),
            TyDef::Enum { variants } => variants
                .iter()
                .any(|v| v.fields.iter().all(|f| self.ty(&f.ty, &given))),
        };
        self.open.pop();
        found
    }

    fn ty(&mut self, ty: &Ty, given: &[bool]) -> bool {
        match ty.kind() {
            TyKind::Param(i) => *given
                .get(*i as usize)
                .or_ice("a type parameter outside its declaration's arguments"),
            TyKind::Con(c, args) => {
                let args = args.iter().map(|a| self.ty(a, given)).collect();
                self.has(*c, args)
            }
            TyKind::Tuple(es) => es.iter().all(|e| self.ty(e, given)),
            _ => true,
        }
    }

    /// Why `c` can never be built: for a struct, the first field that can't be.
    fn why(&mut self, c: TyConId) -> Why {
        let tables = self.tables;
        let con = tables.tycon(c);
        let given = vec![true; con.arity()];
        match &con.def {
            TyDef::Struct { fields, .. } => Why::Field(
                fields
                    .iter()
                    .find(|f| !self.ty(&f.ty, &given))
                    .or_ice("a struct that can never be built has a field that can't be")
                    .name
                    .clone(),
            ),
            _ => Why::Variants,
        }
    }
}
