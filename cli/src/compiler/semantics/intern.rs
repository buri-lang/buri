//! Interned types.
//!
//! A [`Ty`] is a reference to one entry of a process-wide table that holds
//! each distinct type once. Two types are equal exactly when they are the same
//! entry, so equality is a pointer compare, and a `Ty` is eight bytes and
//! `Copy`. Copying one is free, where the owned tree it replaces was 40 bytes
//! plus a heap allocation per `Box` and `Vec` inside it.
//!
//! What a type *is* comes from [`Ty::kind`], which reads the entry:
//!
//! ```text
//! match ty.kind() {
//!     TyKind::Con(id, args) => …,      // `args: &'static [Ty]`
//!     TyKind::Array(elem) => …,        // `elem: &Ty`
//!     …
//! }
//! ```
//!
//! and a type is made by the constructors on [`Ty`] (`Ty::con`, `Ty::array`,
//! `Ty::tuple`, `Ty::func`, …), which look the shape up and add it if it is
//! new.
//!
//! # Why one table for the whole process
//!
//! Front ends and back ends run on a thread pool, and a type made by one is
//! read by another: monomorphization reads the checker's types, a backend reads
//! monomorphization's. A table per analysis would have to be threaded through
//! every function that reads a type, and a type could not be read without it.
//! A shared table lets `ty.kind()` work anywhere.
//!
//! What it costs is that entries are never freed. They are bounded by the
//! number of distinct shapes, not by the number of times a shape is made, and
//! every id inside a shape — a type constructor, an inference variable, a
//! context type — is a small dense number reused by every analysis. A
//! long-running language server re-checking the same program finds its types
//! already there.
//!
//! The table is sharded by hash, 64 ways, each shard behind its own mutex, so
//! two threads making types contend only when both make a type in the same
//! shard at once. Reading a type takes no lock at all.
//!
//! # Why output can't depend on which thread came first
//!
//! The address of an entry depends on which thread made the shape first, so
//! nothing may observe it. Nothing does:
//!
//! * `Hash` writes a hash of the *structure*, computed once when the entry is
//!   made, so a hash map keyed by types iterates in the same order whatever
//!   order the types were interned in.
//! * `Debug` prints the structure, exactly as the owned tree's derived `Debug`
//!   did.
//! * There is no `Ord`.

use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::sync::{Mutex, PoisonError};

use crate::compiler::semantics::types::{CtxTypeId, TyConId, TyVarId};
use crate::hash::{FxHasher, Map};

/// The structure of a type, with every child already interned.
///
/// Generic over the lifetime of its lists only so that a lookup can describe a
/// shape whose lists live on the caller's stack; every stored shape is a
/// [`TyKind`], whose lists live as long as the process.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TyKindIn<'a> {
    /// An inference variable. Local to one function body — no inference
    /// crosses a function boundary (guides/compile-speed.md).
    Var(TyVarId),
    /// A rigid generic parameter, by index into the item's generic list. A
    /// generic body is checked once, polymorphically (guides/compile-speed.md).
    Param(u32),
    /// A nominal type: a primitive, a struct, or an enum.
    Con(TyConId, &'a [Ty]),
    Array(Ty),
    Tuple(&'a [Ty]),
    Fn(&'a [Ty], Ty),
    Unit,
    /// The generated type of a `context { ... }` value. It has no name and is
    /// never written down (SPEC 11.3).
    Ctx(CtxTypeId),
    /// `Self` inside a trait or impl body.
    SelfTy,
    /// Poison, so one type error does not produce ten. There is deliberately
    /// no bottom type: every branch produces a real value, which is what makes
    /// "all cases are handled" mean what it says.
    Error,
}

/// The structure of a stored type.
pub type TyKind = TyKindIn<'static>;

/// One entry of the table.
pub struct TyData {
    kind: TyKind,
    /// The hash of the structure. What `Ty`'s `Hash` writes, so that it is the
    /// same in every run.
    hash: u64,
    /// The `HAS_*` bits of this type and everything inside it.
    flags: u8,
}

const HAS_VAR: u8 = 1;
const HAS_PARAM: u8 = 2;
const HAS_SELF: u8 = 4;
const HAS_ERROR: u8 = 8;

/// A type: a reference to its entry in the process-wide table.
#[derive(Clone, Copy)]
pub struct Ty(&'static TyData);

static UNIT: TyData = TyData { kind: TyKindIn::Unit, hash: 0x9e37_79b9_7f4a_7c15, flags: 0 };
static ERROR: TyData =
    TyData { kind: TyKindIn::Error, hash: 0xc2b2_ae3d_27d4_eb4f, flags: HAS_ERROR };
static SELF: TyData =
    TyData { kind: TyKindIn::SelfTy, hash: 0x1656_67b1_9e37_79f9, flags: HAS_SELF };

impl PartialEq for Ty {
    fn eq(&self, other: &Ty) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for Ty {}

impl Hash for Ty {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.0.hash);
    }
}

impl std::fmt::Debug for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.kind.fmt(f)
    }
}

impl Ty {
    /// `()`.
    pub const UNIT: Ty = Ty(&UNIT);
    /// The poison type.
    pub const ERROR: Ty = Ty(&ERROR);
    /// `Self` inside a trait or impl body.
    pub const SELF: Ty = Ty(&SELF);

    /// What this type is.
    #[inline]
    pub fn kind(self) -> &'static TyKind {
        &self.0.kind
    }

    pub fn var(id: TyVarId) -> Ty {
        intern(TyKindIn::Var(id))
    }

    pub fn param(index: u32) -> Ty {
        intern(TyKindIn::Param(index))
    }

    pub fn ctx(id: CtxTypeId) -> Ty {
        intern(TyKindIn::Ctx(id))
    }

    pub fn array(elem: Ty) -> Ty {
        intern(TyKindIn::Array(elem))
    }

    pub fn con(id: TyConId, args: impl IntoIterator<Item = Ty>) -> Ty {
        with_slice(args, |args| intern(TyKindIn::Con(id, args)))
    }

    pub fn tuple(elems: impl IntoIterator<Item = Ty>) -> Ty {
        with_slice(elems, |elems| intern(TyKindIn::Tuple(elems)))
    }

    pub fn func(params: impl IntoIterator<Item = Ty>, ret: Ty) -> Ty {
        with_slice(params, |params| intern(TyKindIn::Fn(params, ret)))
    }

    pub fn is_error(self) -> bool {
        self == Ty::ERROR
    }

    /// The head type constructor, which is all method resolution needs
    /// (guides/compile-speed.md).
    pub fn head(self) -> Option<TyConId> {
        match self.kind() {
            TyKindIn::Con(id, _) => Some(*id),
            _ => None,
        }
    }

    /// Whether an inference variable occurs anywhere in this type.
    #[inline]
    pub fn has_vars(self) -> bool {
        self.0.flags & HAS_VAR != 0
    }

    /// Whether a generic parameter or `Self` occurs anywhere in this type:
    /// whether substituting into it can change it.
    #[inline]
    pub fn has_params(self) -> bool {
        self.0.flags & (HAS_PARAM | HAS_SELF) != 0
    }

    /// Whether the poison type occurs anywhere in this type.
    #[inline]
    pub fn has_error(self) -> bool {
        self.0.flags & HAS_ERROR != 0
    }
}

/// Calls `f` with the items of `items` as a slice, on the stack where there
/// are few enough of them. Almost every list of types is a handful long, and
/// collecting one into a `Vec` only to look it up was an allocation per type
/// made.
fn with_slice<R>(items: impl IntoIterator<Item = Ty>, f: impl FnOnce(&[Ty]) -> R) -> R {
    const STACK: usize = 8;
    let mut iter = items.into_iter();
    let mut buffer = [Ty::UNIT; STACK];
    for (len, slot) in buffer.iter_mut().enumerate() {
        match iter.next() {
            Some(t) => *slot = t,
            None => return f(buffer.get(..len).unwrap_or(&[])),
        }
    }
    match iter.next() {
        None => f(&buffer),
        Some(next) => {
            let mut all: Vec<Ty> = buffer.to_vec();
            all.push(next);
            all.extend(iter);
            f(&all)
        }
    }
}

/// How many ways the table is split, as a power of two: a shard is picked by
/// the top bits of the hash.
const SHARD_BITS: u32 = 6;
const SHARDS: usize = 1 << SHARD_BITS;

/// One shard: structural hash -> the entries with that hash.
type Shard = Map<u64, Vec<Ty>>;

static TABLE: [Mutex<Shard>; SHARDS] =
    [const { Mutex::new(Map::with_hasher(BuildHasherDefault::new())) }; SHARDS];

fn structural_hash(kind: &TyKindIn<'_>) -> u64 {
    let mut h = FxHasher::default();
    kind.hash(&mut h);
    h.finish()
}

fn flags_of(kind: &TyKindIn<'_>) -> u8 {
    let children = |ts: &[Ty]| ts.iter().fold(0, |acc, t| acc | t.0.flags);
    match kind {
        TyKindIn::Var(_) => HAS_VAR,
        TyKindIn::Param(_) => HAS_PARAM,
        TyKindIn::SelfTy => HAS_SELF,
        TyKindIn::Error => HAS_ERROR,
        TyKindIn::Unit | TyKindIn::Ctx(_) => 0,
        TyKindIn::Con(_, args) | TyKindIn::Tuple(args) => children(args),
        TyKindIn::Array(elem) => elem.0.flags,
        TyKindIn::Fn(params, ret) => children(params) | ret.0.flags,
    }
}

/// A list that lives as long as the process.
fn leak(ts: &[Ty]) -> &'static [Ty] {
    if ts.is_empty() {
        return &[];
    }
    Box::leak(ts.to_vec().into_boxed_slice())
}

/// The one entry for this shape, made if there is none yet.
fn intern(probe: TyKindIn<'_>) -> Ty {
    match probe {
        TyKindIn::Unit => return Ty::UNIT,
        TyKindIn::Error => return Ty::ERROR,
        TyKindIn::SelfTy => return Ty::SELF,
        _ => {}
    }
    let hash = structural_hash(&probe);
    let shard = (hash >> (u64::BITS - SHARD_BITS)) as usize;
    let Some(shard) = TABLE.get(shard) else { return Ty::ERROR };
    let mut shard = shard.lock().unwrap_or_else(PoisonError::into_inner);
    let bucket = shard.entry(hash).or_default();
    if let Some(found) = bucket.iter().find(|t| t.0.kind == probe) {
        return *found;
    }
    let kind: TyKind = match probe {
        TyKindIn::Con(id, args) => TyKindIn::Con(id, leak(args)),
        TyKindIn::Tuple(elems) => TyKindIn::Tuple(leak(elems)),
        TyKindIn::Fn(params, ret) => TyKindIn::Fn(leak(params), ret),
        TyKindIn::Var(id) => TyKindIn::Var(id),
        TyKindIn::Param(i) => TyKindIn::Param(i),
        TyKindIn::Array(elem) => TyKindIn::Array(elem),
        TyKindIn::Ctx(id) => TyKindIn::Ctx(id),
        TyKindIn::Unit => TyKindIn::Unit,
        TyKindIn::SelfTy => TyKindIn::SelfTy,
        TyKindIn::Error => TyKindIn::Error,
    };
    let flags = flags_of(&kind);
    let ty = Ty(Box::leak(Box::new(TyData { kind, hash, flags })));
    bucket.push(ty);
    ty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shape_is_one_entry() {
        let a = Ty::con(TyConId(7), [Ty::UNIT, Ty::param(0)]);
        let b = Ty::con(TyConId(7), vec![Ty::UNIT, Ty::param(0)]);
        assert_eq!(a, b);
        assert!(std::ptr::eq(a.kind(), b.kind()));
        assert_ne!(a, Ty::con(TyConId(7), [Ty::param(0), Ty::UNIT]));
    }

    #[test]
    fn long_lists_intern_like_short_ones() {
        let elems: Vec<Ty> = (0..20).map(Ty::param).collect();
        let a = Ty::tuple(elems.iter().copied());
        let b = Ty::tuple(elems.clone());
        assert_eq!(a, b);
        assert!(matches!(a.kind(), TyKindIn::Tuple(es) if es.len() == 20));
    }

    #[test]
    fn flags_cover_every_child() {
        let f = Ty::func([Ty::var(TyVarId(3))], Ty::UNIT);
        assert!(f.has_vars() && !f.has_params());
        let g = Ty::array(Ty::tuple([Ty::SELF]));
        assert!(g.has_params() && !g.has_vars());
        assert!(Ty::con(TyConId(1), [Ty::ERROR]).has_error());
    }

    #[test]
    fn debug_prints_the_structure() {
        let t = Ty::array(Ty::con(TyConId(2), []));
        assert_eq!(format!("{t:?}"), "Array(Con(TyConId(2), []))");
    }
}
