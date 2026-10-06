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
//! It is Maranget's, from *Warnings for pattern matching* (JFP 2007), linked
//! from `reference/README.md`.
//!
//! The question is asked of each **alternative** of an or-pattern rather than
//! of the arm as a whole, which is the refinement §4.2 of that paper calls for.
//! `A | B` is two rows, and asking only whether *either* is useful lets a dead
//! `A` ride in on a live `B`:
//!
//! ```text
//! match (h) {
//!   Hello.Now(_) => "one",
//!   Hello.Now(_) | Hello.World => "two",   // `Hello.Now(_)` can never run
//! }
//! ```
//!
//! So each alternative is checked against everything before it — the arms
//! above, *and* the alternatives to its left in its own arm, which is what
//! catches `A | A`. An arm all of whose alternatives are dead is still one
//! `unreachable-arm`: the finer code is for the case the coarser one cannot
//! see, and the two never fire on the same arm.


use crate::compiler::semantics::inference::Infer;
use crate::compiler::semantics::typed::{self, PatKind, Pattern};
use crate::compiler::semantics::types::{Prim, Ty, TyKind, TyConId, TyDef};
use crate::diagnostics::{Diagnostic, Span};
use crate::hash::{Map as HashMap, Set as HashSet};

/// The head constructor of a pattern.
///
/// `Hash` as well as `Equal` because the matrix below is indexed by it and the
/// set of constructors a column mentions is a set: with `Vec::contains` in
/// their place, a `match` over N variants spent N²/2 comparisons deciding
/// whether the column was complete.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
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
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
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
            Ctor::Single => match ty.kind() {
                TyKind::Tuple(ts) => ts.len(),
                TyKind::Con(con, _) => tables.tycon(*con).fields().len(),
                _ => 0,
            },
            Ctor::Bool(_) | Ctor::Lit(_) => 0,
            Ctor::Array(n) => *n,
            Ctor::ArrayRest(n) => *n,
        }
    }

    /// The types of this constructor's fields at `ty`, appended to `out`.
    fn field_types_into(
        &self,
        tables: &crate::compiler::semantics::types::Tables,
        ty: &Ty,
        out: &mut Vec<Ty>,
    ) {
        use crate::compiler::semantics::types::substitute;
        match self {
            Ctor::Variant(con, v) => {
                let args: &[Ty] = match ty.kind() {
                    TyKind::Con(_, a) => a,
                    _ => &[],
                };
                if let Some(variant) = tables.tycon(*con).variants().get(*v) {
                    out.extend(variant.fields.iter().map(|f| substitute(&f.ty, args, None)));
                }
            }
            Ctor::Single => match ty.kind() {
                TyKind::Tuple(ts) => out.extend_from_slice(ts),
                TyKind::Con(con, args) => out.extend(
                    tables.tycon(*con).fields().iter().map(|f| substitute(&f.ty, args, None)),
                ),
                _ => {}
            },
            Ctor::Array(n) | Ctor::ArrayRest(n) => {
                let elem = match ty.kind() {
                    TyKind::Array(e) => *e,
                    _ => Ty::ERROR,
                };
                out.resize(out.len().saturating_add(*n), elem);
            }
            _ => {}
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

/// The wildcard a row is padded with where no pattern was written.
static WILD: Pat = Pat::Wild;

/// The type a column has when the caller supplied none.
static UNTYPED: Ty = Ty::ERROR;

/// One row of the matrix: the patterns it holds, borrowed.
///
/// Every operation below builds new rows out of the patterns of old ones —
/// `specialize` splices a constructor's sub-patterns in, `default_matrix`
/// drops a column, an or-pattern is distributed into a row per alternative —
/// and none of them changes a pattern. Owned rows made each of those a deep
/// copy of every pattern in the row; borrowed ones make it a pointer copy. The
/// patterns themselves are lowered once per `match`, and outlive every matrix
/// built from them.
type Row<'p> = Vec<&'p Pat>;

/// A matrix's rows, end to end in one list.
///
/// Every row of a matrix has the same width, so a row is a slice of `cells`
/// and building a matrix is one allocation rather than one per row.
#[derive(Default, Clone)]
struct Rows<'p> {
    cells: Vec<&'p Pat>,
    width: usize,
    len: usize,
}

impl<'p> Rows<'p> {
    fn len(&self) -> usize {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn get(&self, at: usize) -> Option<&[&'p Pat]> {
        if at >= self.len {
            return None;
        }
        let start = at.checked_mul(self.width)?;
        self.cells.get(start..start.checked_add(self.width)?)
    }

    fn iter(&self) -> impl Iterator<Item = &[&'p Pat]> {
        (0..self.len).filter_map(move |at| self.get(at))
    }

    /// Appends the row `fill` writes onto the end of the cells. The first row
    /// sets the width.
    fn push_with(&mut self, fill: impl FnOnce(&mut Vec<&'p Pat>)) {
        let before = self.cells.len();
        fill(&mut self.cells);
        let width = self.cells.len().saturating_sub(before);
        if self.len == 0 {
            self.width = width;
        }
        debug_assert_eq!(width, self.width, "every row of a matrix is as wide as the first");
        self.len = self.len.saturating_add(1);
    }

    fn push(&mut self, row: &[&'p Pat]) {
        self.push_with(|cells| cells.extend_from_slice(row));
    }

    fn truncate(&mut self, len: usize) {
        if len < self.len {
            self.cells.truncate(len.saturating_mul(self.width));
            self.len = len;
        }
    }

    /// The first `k` rows, as rows of their own.
    fn prefix(&self, k: usize) -> Rows<'p> {
        let len = k.min(self.len);
        let cells = self.cells.get(..len.saturating_mul(self.width)).unwrap_or_default().to_vec();
        Rows { cells, width: self.width, len }
    }
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
        PatKind::Int(v, neg) => Pat::Ctor(Ctor::Lit(LitValue::Int(v.get(), *neg)), Vec::new()),
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
fn distribute<'p>(alts: &'p [Pat], rest: &[&'p Pat]) -> Vec<Row<'p>> {
    alts.iter()
        .map(|a| {
            let mut row = Vec::with_capacity(rest.len().saturating_add(1));
            row.push(a);
            row.extend_from_slice(rest);
            row
        })
        .collect()
}

/// The constructors a row's head can start with. An or-pattern contributes
/// every constructor any of its alternatives does, so that a column covered by
/// `true | false` counts as complete.
fn collect_head_ctors(p: &Pat, out: &mut HashSet<Ctor>) {
    match p {
        Pat::Wild => {}
        Pat::Ctor(c, _) => {
            if !out.contains(c) {
                out.insert(c.clone());
            }
        }
        Pat::Or(alts) => {
            for a in alts {
                collect_head_ctors(a, out);
            }
        }
    }
}

/// Past this many rows a matrix carries a head-constructor index. Below it the
/// scan the index replaces is faster than building one, and almost every
/// `match` in real source is a handful of arms.
const INDEX_THRESHOLD: usize = 16;

/// The pattern matrix the usefulness algorithm works over.
///
/// Its two operations both keep a subset of the rows chosen by the head of
/// each: `specialize` keeps the rows headed by one constructor, plus the
/// wildcards and or-patterns, and `default_matrix` keeps only the latter. Done
/// by scanning, each visits every row — so a `match` over N variants, which
/// has N rows and asks about N constructors, does N² row visits, and that is
/// the whole of why a wide `match` was quadratic.
///
/// The index says which rows a constructor can reach without looking at the
/// others. Row numbers are kept ascending in each bucket and merged ascending
/// on the way out, so both operations produce their rows in exactly the order
/// the scan did — the witness a non-exhaustive `match` names and the order
/// unreachable arms are reported in both depend on it.
#[derive(Default)]
struct Matrix<'p> {
    rows: Rows<'p>,
    index: Option<Index>,
    /// Each constructor's specialization so far, and how many rows it has
    /// seen, for the matrix the reachability loop grows. That loop asks the
    /// same question with one more row each time, so a column every row
    /// shares, a tuple's, was `n²` rows copied over `n` arms.
    memo: Option<std::cell::RefCell<HashMap<Ctor, (usize, Matrix<'p>)>>>,
}

#[derive(Default)]
struct Index {
    /// Rows headed by each constructor, ascending.
    by_ctor: HashMap<Ctor, Vec<usize>>,
    /// Rows headed by a wildcard or an or-pattern, ascending. Every
    /// specialization visits these, and they are the whole default matrix.
    open: Vec<usize>,
    /// Every constructor any row's head can start with.
    heads: HashSet<Ctor>,
}

impl Index {
    /// The rows from `from` on that `specialize` must visit for `ctor`,
    /// ascending. Two sorted lists merged rather than concatenated and sorted,
    /// so this allocates nothing.
    fn rows_for_from<'i>(&'i self, ctor: &Ctor, from: usize) -> Merge<'i> {
        let a = self.by_ctor.get(ctor).map_or(&[][..], Vec::as_slice);
        let b = self.open.as_slice();
        Merge {
            a: a.get(a.partition_point(|&r| r < from)..).unwrap_or_default(),
            b: b.get(b.partition_point(|&r| r < from)..).unwrap_or_default(),
        }
    }
}

/// The ascending merge of two ascending row lists.
struct Merge<'i> {
    a: &'i [usize],
    b: &'i [usize],
}

impl Iterator for Merge<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        match (self.a.split_first(), self.b.split_first()) {
            (Some((&x, rest_a)), Some((&y, rest_b))) => {
                if x <= y {
                    self.a = rest_a;
                    Some(x)
                } else {
                    self.b = rest_b;
                    Some(y)
                }
            }
            (Some((&x, rest_a)), None) => {
                self.a = rest_a;
                Some(x)
            }
            (None, Some((&y, rest_b))) => {
                self.b = rest_b;
                Some(y)
            }
            (None, None) => None,
        }
    }
}

impl<'p> Matrix<'p> {
    fn new(rows: Rows<'p>) -> Self {
        let mut m = Matrix { rows, index: None, memo: None };
        if m.rows.len() >= INDEX_THRESHOLD {
            m.build_index();
        }
        m
    }

    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Appends a row, keeping the index in step. The reachability loop grows
    /// one matrix arm by arm, so rebuilding the index per arm would put the
    /// square back.
    fn push(&mut self, row: &[&'p Pat]) {
        let at = self.rows.len();
        self.rows.push(row);
        if self.index.is_some() {
            self.index_row(at);
        } else if self.rows.len() >= INDEX_THRESHOLD {
            self.build_index();
        }
    }

    /// Drops every row from `len` on.
    ///
    /// A guarded arm covers nothing, so its rows come out again once its own
    /// alternatives have been asked about each other. Rebuilding the index is
    /// the whole cost, and only a guarded arm with an alternation pays it.
    fn truncate(&mut self, len: usize) {
        if len >= self.rows.len() {
            return;
        }
        self.rows.truncate(len);
        if self.index.is_some() {
            self.build_index();
        }
        if let Some(memo) = &self.memo {
            memo.borrow_mut().clear();
        }
    }

    fn build_index(&mut self) {
        self.index = Some(Index::default());
        for at in 0..self.rows.len() {
            self.index_row(at);
        }
    }

    fn index_row(&mut self, at: usize) {
        let Some(row) = self.rows.get(at) else { return };
        let Some(&head) = row.first() else { return };
        let Some(ix) = self.index.as_mut() else { return };
        match head {
            Pat::Wild | Pat::Or(_) => ix.open.push(at),
            Pat::Ctor(c, _) => ix.by_ctor.entry(c.clone()).or_default().push(at),
        }
        collect_head_ctors(head, &mut ix.heads);
    }

    /// Whether the first column mentions `c`. Below the index threshold a scan
    /// of the heads beats building the set the index holds.
    fn mentions(&self, c: &Ctor) -> bool {
        match self.index.as_ref() {
            Some(ix) => ix.heads.contains(c),
            None => self.rows.iter().any(|row| row.first().is_some_and(|&head| head_mentions(head, c))),
        }
    }
}

/// Whether a row's head can start with `c`, by the rule of
/// [`collect_head_ctors`].
fn head_mentions(p: &Pat, c: &Ctor) -> bool {
    match p {
        Pat::Wild => false,
        Pat::Ctor(d, _) => d == c,
        Pat::Or(alts) => alts.iter().any(|a| head_mentions(a, c)),
    }
}

/// A specialization: built for the asking, or kept by the matrix it came from.
enum Specialized<'m, 'p> {
    Owned(Matrix<'p>),
    Kept(std::cell::Ref<'m, Matrix<'p>>),
}

impl<'p> std::ops::Deref for Specialized<'_, 'p> {
    type Target = Matrix<'p>;

    fn deref(&self) -> &Matrix<'p> {
        match self {
            Specialized::Owned(m) => m,
            Specialized::Kept(m) => m,
        }
    }
}

struct Ctx<'a> {
    tables: &'a crate::compiler::semantics::types::Tables,
    /// The largest array length the match distinguishes.
    limit: usize,
    /// Whether `useful` builds the witness it answers with. Only the
    /// exhaustiveness question names one; reachability asks only whether
    /// there is one, and building it cost a copy of a type per constructor on
    /// the way back up from every live arm.
    witnesses: bool,
}

impl<'a> Ctx<'a> {
    /// The complete set of constructors for a type, or `None` when the type
    /// has too many to enumerate.
    fn all_ctors(&self, ty: &Ty) -> Option<Vec<Ctor>> {
        match ty.kind() {
            TyKind::Con(con, _) => match &self.tables.tycon(*con).def {
                TyDef::Enum { variants } => {
                    Some((0..variants.len()).map(|i| Ctor::Variant(*con, i)).collect())
                }
                TyDef::Struct { .. } => Some(vec![Ctor::Single]),
                TyDef::Prim(Prim::Bool) => Some(vec![Ctor::Bool(false), Ctor::Bool(true)]),
                // Integers, strings, chars and floats need a `_` arm.
                TyDef::Prim(_) => None,
            },
            TyKind::Tuple(_) | TyKind::Unit => Some(vec![Ctor::Single]),
            // Finite, because rest patterns were expanded into fixed lengths
            // and nothing distinguishes anything longer than `limit`.
            TyKind::Array(_) => Some((0..=self.limit).map(Ctor::Array).collect()),
            _ => None,
        }
    }

    /// Rows of the matrix whose first pattern is `ctor`, with that pattern's
    /// sub-patterns spliced in.
    fn specialize<'m, 'p>(
        &self,
        matrix: &'m Matrix<'p>,
        ctor: &Ctor,
        arity: usize,
    ) -> Specialized<'m, 'p> {
        let Some(memo) = &matrix.memo else {
            let mut out = Rows::default();
            self.specialize_from(matrix, ctor, arity, 0, &mut out);
            return Specialized::Owned(Matrix::new(out));
        };
        {
            let mut memo = memo.borrow_mut();
            let (seen, known) = memo.entry(ctor.clone()).or_default();
            if *seen < matrix.rows.len() {
                let mut out = Rows::default();
                self.specialize_from(matrix, ctor, arity, *seen, &mut out);
                for row in out.iter() {
                    known.push(row);
                }
                *seen = matrix.rows.len();
            }
        }
        match std::cell::Ref::filter_map(memo.borrow(), |m| m.get(ctor).map(|(_, known)| known)) {
            Ok(known) => Specialized::Kept(known),
            // The entry was made a few lines up.
            Err(_) => {
                let mut out = Rows::default();
                self.specialize_from(matrix, ctor, arity, 0, &mut out);
                Specialized::Owned(Matrix::new(out))
            }
        }
    }

    /// [`Ctx::specialize`]'s rows from row `from` of `matrix` on, into `out`.
    fn specialize_from<'p>(
        &self,
        matrix: &Matrix<'p>,
        ctor: &Ctor,
        arity: usize,
        from: usize,
        out: &mut Rows<'p>,
    ) {
        match matrix.index.as_ref() {
            Some(ix) => {
                for at in ix.rows_for_from(ctor, from) {
                    if let Some(row) = matrix.rows.get(at) {
                        self.specialize_row(row, ctor, arity, out);
                    }
                }
            }
            None => {
                for row in matrix.rows.iter().skip(from) {
                    self.specialize_row(row, ctor, arity, out);
                }
            }
        }
    }

    fn specialize_row<'p>(
        &self,
        row: &[&'p Pat],
        ctor: &Ctor,
        arity: usize,
        out: &mut Rows<'p>,
    ) {
        let Some((&head, rest)) = row.split_first() else { return };
        match head {
            Pat::Wild => out.push_with(|cells| {
                cells.resize(cells.len().saturating_add(arity), &WILD);
                cells.extend_from_slice(rest);
            }),
            Pat::Ctor(c, subs) if c == ctor => out.push_with(|cells| {
                let end = cells.len().saturating_add(arity);
                cells.extend(subs.iter().take(arity));
                cells.resize(end, &WILD);
                cells.extend_from_slice(rest);
            }),
            // An alternation `expand` left nested, now exposed by peeling
            // its constructor off. Distribute: each alternative is a row of
            // its own, and the coverage of all of them is the row's.
            // Dropping the row instead would lose that coverage and make a
            // wildcard look useful — the false rejection of `.Some(true |
            // false)`.
            Pat::Or(alts) => {
                for next in distribute(alts, rest) {
                    self.specialize_row(&next, ctor, arity, out);
                }
            }
            Pat::Ctor(..) => {}
        }
    }

    /// Rows whose first pattern is a wildcard, with that column dropped.
    fn default_matrix<'p>(&self, matrix: &Matrix<'p>) -> Matrix<'p> {
        let mut out = Rows::default();
        match matrix.index.as_ref() {
            Some(ix) => {
                for &at in &ix.open {
                    if let Some(row) = matrix.rows.get(at) {
                        self.default_row(row, &mut out);
                    }
                }
            }
            None => {
                for row in matrix.rows.iter() {
                    self.default_row(row, &mut out);
                }
            }
        }
        Matrix::new(out)
    }

    fn default_row<'p>(&self, row: &[&'p Pat], out: &mut Rows<'p>) {
        let Some((&head, rest)) = row.split_first() else { return };
        match head {
            Pat::Wild => out.push(rest),
            // Distribute, for the same reason `specialize` does. An
            // alternative that is a wildcard makes the whole row a default
            // row.
            Pat::Or(alts) => {
                for next in distribute(alts, rest) {
                    self.default_row(&next, out);
                }
            }
            Pat::Ctor(..) => {}
        }
    }

    /// Whether `v` matches a value the matrix does not. Returns a witness when
    /// it does, so a diagnostic can name the missing case — when `witnesses`
    /// is on; otherwise the answer is only whether there is one.
    ///
    fn useful<'p>(
        &self,
        matrix: &Matrix<'p>,
        v: &[&'p Pat],
        types: &[Ty],
    ) -> Option<Vec<Witness>> {
        let Some((&head, tail)) = v.split_first() else {
            return matrix.is_empty().then(Vec::new);
        };
        // A row and its type list are built together, but the type list is the
        // one the caller supplied, so a shorter one leaves the columns past it
        // untyped rather than out of bounds.
        let (head_ty, rest_types): (&Ty, &[Ty]) = match types.split_first() {
            Some((t, rest)) => (t, rest),
            None => (&UNTYPED, &[]),
        };

        match head {
            Pat::Or(alts) => {
                for alt in alts {
                    let mut next = Vec::with_capacity(v.len());
                    next.push(alt);
                    next.extend_from_slice(tail);
                    if let Some(w) = self.useful(matrix, &next, types) {
                        return Some(w);
                    }
                }
                None
            }
            Pat::Ctor(c, subs) => {
                let arity = c.arity(self.tables, head_ty);
                let specialized = self.specialize(matrix, c, arity);
                let mut next: Row<'p> = subs.iter().collect();
                if next.len() < arity {
                    next.resize(arity, &WILD);
                }
                next.extend_from_slice(tail);
                let next_types = self.column_types(c, head_ty, rest_types);
                self.useful(&specialized, &next, &next_types)
                    .map(|w| self.wrap(c, head_ty, arity, w))
            }
            Pat::Wild => {
                // Once, not once per branch: it allocates one `Ctor` per
                // variant, and both branches below want the same list.
                let all_ctors = self.all_ctors(head_ty);
                let complete = match &all_ctors {
                    Some(all) => all.iter().all(|c| matrix.mentions(c)),
                    None => false,
                };
                if complete {
                    // `complete` implies the list is there.
                    let all = all_ctors.unwrap_or_default();
                    for c in all {
                        let arity = c.arity(self.tables, head_ty);
                        let specialized = self.specialize(matrix, &c, arity);
                        let mut next: Row<'p> = vec![&WILD; arity];
                        next.extend_from_slice(tail);
                        let next_types = self.column_types(&c, head_ty, rest_types);
                        if let Some(w) = self.useful(&specialized, &next, &next_types) {
                            return Some(self.wrap(&c, head_ty, arity, w));
                        }
                    }
                    None
                } else {
                    let default = self.default_matrix(matrix);
                    self.useful(&default, tail, rest_types).map(|w| {
                        if !self.witnesses {
                            return w;
                        }
                        // Name a constructor the match does not mention, when
                        // there is one to name.
                        let missing = all_ctors
                            .and_then(|all| all.into_iter().find(|c| !matrix.mentions(c)))
                            .map(|c| {
                                let arity = c.arity(self.tables, head_ty);
                                Witness::Ctor(
                                    c,
                                    *head_ty,
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

    /// The column types after `c` is peeled off a column of type `head_ty`:
    /// its fields' types, then the rest of the columns' as they were.
    fn column_types(&self, c: &Ctor, head_ty: &Ty, rest_types: &[Ty]) -> Vec<Ty> {
        let arity = c.arity(self.tables, head_ty);
        let mut out = Vec::with_capacity(arity.saturating_add(rest_types.len()));
        c.field_types_into(self.tables, head_ty, &mut out);
        out.extend_from_slice(rest_types);
        out
    }

    /// A witness for the specialized matrix, put back under the constructor
    /// it was specialized by: the first `arity` values are that constructor's
    /// fields, and the rest are the columns after it.
    fn wrap(&self, c: &Ctor, head_ty: &Ty, arity: usize, mut w: Vec<Witness>) -> Vec<Witness> {
        if !self.witnesses {
            return w;
        }
        let rest = w.split_off(arity.min(w.len()));
        let mut out = Vec::with_capacity(rest.len().saturating_add(1));
        out.push(Witness::Ctor(c.clone(), *head_ty, w));
        out.extend(rest);
        out
    }

    /// Which row of `rows` first made `alt` useless — the last row of the
    /// shortest prefix that already covers it.
    ///
    /// Coverage only grows as rows are added, so "is this still useful against
    /// the first *k* rows" is monotone in `k` and the boundary is found by
    /// bisection: `log₂(k)` usefulness runs rather than `k`. It is on the
    /// error path either way, and it is what lets the diagnostic point at the
    /// pattern that subsumes this one instead of waving at everything above.
    ///
    /// `None` when no prefix covers it, which the caller cannot reach: it asks
    /// only about an alternative it has already found useless.
    fn covered_by(
        &self,
        rows: &Rows<'_>,
        upto: usize,
        alt: &Rows<'_>,
        types: &[Ty],
    ) -> Option<usize> {
        let live = |k: usize| {
            let prefix = Matrix::new(rows.prefix(k));
            alt.iter().any(|r| self.useful(&prefix, r, types).is_some())
        };
        let (mut lo, mut hi) = (0usize, upto);
        while lo < hi {
            let mid = lo.saturating_add(hi.saturating_sub(lo) / 2);
            if live(mid) {
                lo = mid.saturating_add(1);
            } else {
                hi = mid;
            }
        }
        lo.checked_sub(1)
    }
}

/// The alternatives of an arm's pattern, left to right, each with the span the
/// diagnostic points at.
///
/// A pattern with no `|` at its top is one alternative — and its span is its
/// own rather than the arm's, because the caller only reaches for it when some
/// *other* alternative of the same arm is alive. A wholly dead arm is reported
/// against `Arm::span`, as it always was.
///
/// The recursion is through `Or` alone, and it is a recursion because
/// parentheses can nest one: the parser reads a run of `|` as a single node, so
/// the only way to get an `Or` inside an `Or` is to write `a | (b | c)`, and
/// that is three alternatives rather than two.
///
/// An alternation *inside a constructor* — `.Some(a | b)` — is not an
/// alternative of the arm. It stays where it is, counting toward coverage
/// where `specialize` distributes it, and is not reported branch by branch.
fn alternatives_of(p: &Pattern, out: &mut Vec<(Span, Pat)>) {
    match &p.kind {
        PatKind::Or(alts) if !alts.is_empty() => {
            for a in alts {
                alternatives_of(a, out);
            }
        }
        _ => out.push((p.span, lower(p))),
    }
}

/// Whether a match is the common shape, and has nothing to report: arms of
/// distinct variants or literals that bind at most names, then at most one
/// catch-all, covering the scrutinee. Every arm of that shape is reachable, so
/// the matrix below would say nothing. Any other match, including every one it
/// would report on, goes the long way.
fn plainly_covered(
    tables: &crate::compiler::semantics::types::Tables,
    scrutinee: &Ty,
    arms: &[typed::Arm],
) -> bool {
    #[derive(PartialEq, Eq, Hash)]
    enum Key<'a> {
        Variant(usize),
        Bool(bool),
        Int(u128, bool),
        Float(u64),
        Str(&'a str),
        Char(char),
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Of {
        Enum(TyConId, usize),
        Bool,
        Literal,
    }
    fn names_only(p: &Pattern) -> bool {
        match &p.kind {
            PatKind::Wild | PatKind::Bind { sub: None, .. } => true,
            PatKind::Bind { sub: Some(s), .. } => names_only(s),
            _ => false,
        }
    }
    // `None` is a catch-all; anything that is neither is the long way.
    fn head(p: &Pattern, of: Of) -> Option<Option<Key<'_>>> {
        let key = match (&p.kind, of) {
            (PatKind::Wild | PatKind::Bind { sub: None, .. }, _) => return Some(None),
            (PatKind::Bind { sub: Some(s), .. }, _) => return head(s, of),
            (PatKind::Variant { con, variant, fields }, Of::Enum(e, n))
                if *con == e && *variant < n && fields.iter().all(|f| names_only(&f.pattern)) =>
            {
                Key::Variant(*variant)
            }
            (PatKind::Bool(b), Of::Bool) => Key::Bool(*b),
            (PatKind::Int(v, neg), Of::Literal) => Key::Int(v.get(), *neg),
            // As `lower` keys it: `+0.0 == -0.0`.
            (PatKind::Float(v), Of::Literal) => Key::Float((v + 0.0).to_bits()),
            (PatKind::Str(s), Of::Literal) => Key::Str(s),
            (PatKind::Char(c), Of::Literal) => Key::Char(*c),
            _ => return None,
        };
        Some(Some(key))
    }

    let TyKind::Con(con, _) = scrutinee.kind() else { return false };
    let (of, finite) = match &tables.tycon(*con).def {
        TyDef::Enum { variants } => (Of::Enum(*con, variants.len()), Some(variants.len())),
        TyDef::Prim(Prim::Bool) => (Of::Bool, Some(2)),
        TyDef::Prim(_) => (Of::Literal, None),
        TyDef::Struct { .. } => return false,
    };
    let mut seen: HashSet<Key<'_>> = HashSet::default();
    let mut caught = false;
    for arm in arms {
        if caught || arm.guard.is_some() {
            return false;
        }
        match head(&arm.pattern, of) {
            None => return false,
            // A catch-all after every case is unreachable.
            Some(None) if finite == Some(seen.len()) => return false,
            Some(None) => caught = true,
            Some(Some(key)) => {
                if !seen.insert(key) {
                    return false;
                }
            }
        }
    }
    caught || finite == Some(seen.len())
}

/// Whether any part of this pattern is one the checker could not build.
///
/// `PatKind::Error` lowers to a wildcard, and a wildcard covers everything
/// after it — so on a file that already has an error, every alternative to the
/// right of the broken one looks dead. That is the cascade `Ty::Error` exists
/// to prevent, and the answer here is the same one: ask the finer question only
/// of a `match` the checker understood.
fn has_error(p: &Pattern) -> bool {
    match &p.kind {
        PatKind::Error => true,
        PatKind::Bind { sub, .. } => sub.as_ref().is_some_and(|s| has_error(s)),
        PatKind::Tuple(ps) => ps.iter().any(has_error),
        PatKind::Struct { fields, .. } | PatKind::Variant { fields, .. } => {
            fields.iter().any(|f| has_error(&f.pattern))
        }
        PatKind::Array { elems, .. } => elems.iter().any(has_error),
        PatKind::Or(alts) => alts.iter().any(has_error),
        _ => false,
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
            Ctor::Single => match ty.kind() {
                TyKind::Tuple(_) => {
                    let parts: Vec<String> = subs.iter().map(|s| render(tables, s)).collect();
                    format!("({})", parts.join(", "))
                }
                TyKind::Con(con, _) => {
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
    if scrutinee.is_error() || plainly_covered(&inf.c.tables, scrutinee, arms) {
        return;
    }
    let alternatives: Vec<Vec<(Span, Pat)>> = arms
        .iter()
        .map(|a| {
            let mut out = Vec::new();
            alternatives_of(&a.pattern, &mut out);
            out
        })
        .collect();
    let limit = alternatives
        .iter()
        .flatten()
        .map(|(_, p)| length_limit(p))
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    // Each alternative's rows, built once up front: the matrices below borrow
    // their patterns from here.
    let alternatives: Vec<Vec<(Span, Vec<Vec<Pat>>)>> = alternatives
        .into_iter()
        .map(|alts| {
            alts.into_iter()
                .map(|(at, low)| (at, expand(vec![expand_lengths(low, limit)])))
                .collect()
        })
        .collect();
    let ctx = Ctx { tables: &inf.c.tables, limit, witnesses: false };
    let types = [*scrutinee];
    let recovered = arms.iter().any(|a| has_error(&a.pattern));

    // Arms are tried in order and the first matching arm wins, so an arm is
    // unreachable when the arms before it already cover it. A guarded arm
    // covers nothing, because its guard may fail.
    //
    // `origin` runs alongside the matrix's rows: which alternative put each one
    // there, so that a dead alternative can be shown the pattern that subsumes
    // it rather than told to go and find it.
    let mut covering = Matrix { memo: Some(Default::default()), ..Matrix::default() };
    let mut origin: Vec<Span> = Vec::new();
    let mut reported = Vec::new();
    for (arm, alts) in arms.iter().zip(&alternatives) {
        let base = covering.rows.len();
        let mut alive = false;
        let mut dead: Vec<(Span, Rows<'_>, usize)> = Vec::new();
        // A guarded arm covers nothing below it, so its rows come back out at
        // the end — they go in first only because they do cover this arm's own
        // later alternatives. An arm with one alternative has no later one, so
        // it neither adds nor removes anything and the index is left alone.
        let hold = arm.guard.is_none() || alts.len() > 1;
        for (at, owned) in alts {
            let before = covering.rows.len();
            let mut rows = Rows::default();
            for r in owned {
                rows.push_with(|cells| cells.extend(r.iter()));
            }
            if rows.iter().any(|r| ctx.useful(&covering, r, &types).is_some()) {
                alive = true;
            } else {
                dead.push((*at, rows.clone(), before));
            }
            // Even a dead alternative goes in — it adds no coverage, and
            // leaving it out would make the next one's "before" a lie.
            if hold {
                for r in rows.iter() {
                    covering.push(r);
                    origin.push(*at);
                }
            }
        }
        if !alive {
            // Every alternative dead is the arm dead, which is the older and
            // coarser report. It says the same thing about the same text —
            // and, on a `match` that did not parse whole, it says it about
            // every arm below the broken one, because a pattern the checker
            // could not build covers everything after it. So this is asked
            // only of a `match` the checker understood, for the reason the
            // finer report beside it is.
            if !recovered {
                reported.push(Diagnostic::templated("unreachable-arm", arm.span));
            }
        } else if !recovered {
            for (at, rows, before) in dead {
                let culprit = ctx.covered_by(&covering.rows, before, &rows, &types);
                let same_arm = culprit.is_some_and(|i| i >= base);
                let by = if same_arm {
                    "an earlier alternative of this arm"
                } else {
                    "an arm above"
                };
                let mut d = Diagnostic::templated("unreachable-alternative", at)
                    .with_bind("covered_by", by);
                if let Some(span) = culprit.and_then(|i| origin.get(i)) {
                    d = d.with_secondary_span(*span, "already covered here");
                }
                reported.push(d);
            }
        }
        if arm.guard.is_some() && hold {
            covering.truncate(base);
            origin.truncate(base);
        }
    }
    for d in reported {
        inf.c.diags.push(d);
    }

    // A non-exhaustive match is a compile error that names a missing case.
    let ctx = Ctx { tables: &inf.c.tables, limit, witnesses: true };
    if let Some(witness) = ctx.useful(&covering, &[&WILD], &types) {
        let shown = witness
            .first()
            .map(|w| render(&inf.c.tables, w))
            .unwrap_or_else(|| "_".into());
        let mut d =
            Diagnostic::templated("match-not-exhaustive", span).with_bind("witness", shown.clone());
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
