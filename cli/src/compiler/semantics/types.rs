//! Types, the nominal tables, and unification.
//!
//! The type system is nominal throughout: every type has a declaration, and
//! trait conformance is declared rather than inferred from shape. That is what
//! makes checking `T: Ord` one lookup in one table keyed by `(trait, type)`
//! rather than a search (SPEC 5.12.1), and it is why nothing in this module
//! needs a fixpoint (SPEC 13.6).

use crate::diagnostics::{Invariant as _, Span};
use crate::hash::Map as HashMap;
use std::fmt::Write as _;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub u32);
        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id_type!(ModuleId);
id_type!(TyConId);
id_type!(FnId);
id_type!(TraitId);
id_type!(ConstId);
id_type!(ContextDeclId);
id_type!(CtxTypeId);
id_type!(TyVarId);
id_type!(LocalId);

// `FuncIdx` is an index into `Program::funcs`: one concrete function, after
// monomorphization. Deliberately not `FnId`, which indexes the *declaration*
// table. The two are different spaces, and while both were spelled `FnId` a
// backend pass could read `tables.fn_info(func)` on a monomorphized call and
// silently name a different function.
id_type!(FuncIdx);

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Prim {
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    Char,
    Str,
    Template,
}

impl Prim {
    pub fn name(self) -> &'static str {
        match self {
            Prim::Bool => "Bool",
            Prim::I8 => "I8",
            Prim::I16 => "I16",
            Prim::I32 => "I32",
            Prim::I64 => "I64",
            Prim::I128 => "I128",
            Prim::U8 => "U8",
            Prim::U16 => "U16",
            Prim::U32 => "U32",
            Prim::U64 => "U64",
            Prim::U128 => "U128",
            Prim::F32 => "F32",
            Prim::F64 => "F64",
            Prim::Char => "Char",
            Prim::Str => "Str",
            Prim::Template => "Template",
        }
    }

    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Prim::I8
                | Prim::I16
                | Prim::I32
                | Prim::I64
                | Prim::I128
                | Prim::U8
                | Prim::U16
                | Prim::U32
                | Prim::U64
                | Prim::U128
        )
    }

    pub fn is_signed(self) -> bool {
        matches!(self, Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64 | Prim::I128)
    }

    pub fn is_float(self) -> bool {
        matches!(self, Prim::F32 | Prim::F64)
    }

    pub fn bits(self) -> u32 {
        match self {
            Prim::I8 | Prim::U8 => 8,
            Prim::I16 | Prim::U16 => 16,
            Prim::I32 | Prim::U32 | Prim::F32 => 32,
            Prim::I64 | Prim::U64 | Prim::F64 => 64,
            Prim::I128 | Prim::U128 => 128,
            _ => 0,
        }
    }

    /// Whether this type is a JavaScript `BigInt` rather than a `number`.
    ///
    /// A double holds every integer up to 2^53 exactly, so every width up to
    /// 32 bits is a `number` and loses nothing. The four wider ones are
    /// `BigInt`s, which are exact at the type's own width — `Int` is `I64` and
    /// a nanosecond timestamp is past 2^53 today, and `I128` was never
    /// representable at all (SPEC 15, open question 8).
    pub fn is_bigint(self) -> bool {
        self.is_integer() && self.bits() >= 64
    }

    /// Inclusive range of representable integers.
    pub fn int_range(self) -> Option<(i128, u128)> {
        Some(match self {
            Prim::I8 => (i8::MIN as i128, i8::MAX as u128),
            Prim::I16 => (i16::MIN as i128, i16::MAX as u128),
            Prim::I32 => (i32::MIN as i128, i32::MAX as u128),
            Prim::I64 => (i64::MIN as i128, i64::MAX as u128),
            Prim::I128 => (i128::MIN, i128::MAX as u128),
            Prim::U8 => (0, u8::MAX as u128),
            Prim::U16 => (0, u16::MAX as u128),
            Prim::U32 => (0, u32::MAX as u128),
            Prim::U64 => (0, u64::MAX as u128),
            Prim::U128 => (0, u128::MAX),
            _ => return None,
        })
    }

    pub fn all() -> &'static [Prim] {
        &[
            Prim::Bool,
            Prim::I8,
            Prim::I16,
            Prim::I32,
            Prim::I64,
            Prim::I128,
            Prim::U8,
            Prim::U16,
            Prim::U32,
            Prim::U64,
            Prim::U128,
            Prim::F32,
            Prim::F64,
            Prim::Char,
            Prim::Str,
            Prim::Template,
        ]
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ty {
    /// An inference variable. Local to one function body — no inference
    /// crosses a function boundary (SPEC 13.3).
    Var(TyVarId),
    /// A rigid generic parameter, by index into the item's generic list. A
    /// generic body is checked once, polymorphically (SPEC 13.5).
    Param(u32),
    /// A nominal type: a primitive, a struct, or an enum.
    Con(TyConId, Vec<Ty>),
    Array(Box<Ty>),
    Tuple(Vec<Ty>),
    Fn(Vec<Ty>, Box<Ty>),
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

impl Ty {
    pub fn is_error(&self) -> bool {
        matches!(self, Ty::Error)
    }

    /// The head type constructor, which is all method resolution needs
    /// (SPEC 13.2).
    pub fn head(&self) -> Option<TyConId> {
        match self {
            Ty::Con(id, _) => Some(*id),
            _ => None,
        }
    }
}

/// What a numeric literal is constrained to before anything pins it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NumClass {
    Int,
    Float,
}

impl NumClass {
    /// The type a literal of this class takes when nothing pins it, in the
    /// everyday spelling (SPEC 5.1.1).
    pub fn default_name(self) -> &'static str {
        match self {
            NumClass::Int => "Int",
            NumClass::Float => "Float",
        }
    }

    /// The default name as a noun phrase, for a sentence that wants an article
    /// in front of it.
    pub fn default_noun_phrase(self) -> &'static str {
        match self {
            NumClass::Int => "an `Int`",
            NumClass::Float => "a `Float`",
        }
    }

    /// How a note names the literal itself, where the sentence is about the
    /// syntax rather than the type it stands for.
    pub fn literal_phrase(self) -> &'static str {
        match self {
            NumClass::Int => "an integer literal",
            NumClass::Float => "a float literal",
        }
    }
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Ty,
    pub exported: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct VariantInfo {
    pub name: String,
    pub fields: Vec<FieldInfo>,
    /// A record-like variant is matched and built by field name; a tuple-like
    /// one by position.
    pub record: bool,
    /// Copied from the enum. A variant has no visibility of its own.
    pub exported: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TyDef {
    Prim(Prim),
    /// `record` distinguishes `struct S { a: T }` from `struct S(T)`.
    Struct { fields: Vec<FieldInfo>, record: bool },
    Enum { variants: Vec<VariantInfo> },
}

#[derive(Clone, Debug)]
pub struct TyCon {
    pub name: String,
    /// The one module that declares it. A type has exactly one defining
    /// module, which is what makes method resolution a lookup.
    pub module: ModuleId,
    pub generics: Vec<GenericInfo>,
    pub def: TyDef,
    pub exported: bool,
    pub span: Span,
}

impl TyCon {
    pub fn arity(&self) -> usize {
        self.generics.len()
    }

    pub fn variants(&self) -> &[VariantInfo] {
        match &self.def {
            TyDef::Enum { variants } => variants,
            _ => &[],
        }
    }

    pub fn fields(&self) -> &[FieldInfo] {
        match &self.def {
            TyDef::Struct { fields, .. } => fields,
            _ => &[],
        }
    }

    pub fn variant_index(&self, name: &str) -> Option<usize> {
        self.variants().iter().position(|v| v.name == name)
    }

    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields().iter().position(|f| f.name == name)
    }
}

#[derive(Clone, Debug)]
pub struct GenericInfo {
    pub name: String,
    /// Traits the argument type must satisfy. Multiple bounds join with `+`.
    pub bounds: Vec<TraitId>,
    pub span: Span,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamRole {
    SelfParam,
    Ctx,
    Normal,
}

#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub name: String,
    pub ty: Ty,
    pub role: ParamRole,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnInfo {
    pub name: String,
    pub module: ModuleId,
    pub generics: Vec<GenericInfo>,
    pub params: Vec<ParamInfo>,
    pub ret: Ty,
    pub exported: bool,
    pub span: Span,
    /// `Some` when this is a method: the head constructor of its `self` type.
    pub self_ty: Option<TyConId>,
    /// `Some` when this function is a trait method supplied by an `impl`.
    pub impl_of: Option<(TraitId, usize)>,
    /// Index of the AST item this came from, for the checker to find the body.
    pub ast: AstRef,
    /// Backed by the runtime rather than by Buri source. Only the embedded
    /// standard library declares these.
    pub intrinsic: bool,
}

/// Where a declaration's syntax lives.
///
/// This was a struct of three numbers carrying two independent sentinels:
/// `module == u32::MAX` meant "no syntax at all" and `sub == u32::MAX` meant
/// "not a method". They were decoded by hand at every use, `item: 0` on the
/// `NONE` value was indistinguishable from a genuine first item, and a
/// declaration with no syntax but a real `sub` was representable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AstRef {
    /// Declared by the toolchain rather than by any source: the primitives,
    /// their methods, and the constructors generated for `context`.
    Builtin,
    /// A top-level item, by its index in its module.
    Item { module: ModuleId, item: u32 },
    /// A method inside an `impl`: the impl's item index, and the method's
    /// index within it.
    Method { module: ModuleId, item: u32, sub: u32 },
}

impl AstRef {
    /// The module and item index, for anything that has syntax.
    pub fn item(self) -> Option<(ModuleId, u32)> {
        match self {
            AstRef::Builtin => None,
            AstRef::Item { module, item } | AstRef::Method { module, item, .. } => {
                Some((module, item))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct TraitInfo {
    pub name: String,
    pub module: ModuleId,
    pub generics: Vec<GenericInfo>,
    pub methods: Vec<TraitMethod>,
    /// Declared with `effect`. Its implementors are effect-carrying, and so
    /// may be passed only as `self` or `ctx` (SPEC 10.1).
    pub is_effect: bool,
    pub exported: bool,
    pub span: Span,
}

impl TraitInfo {
    pub fn method_index(&self, name: &str) -> Option<usize> {
        self.methods.iter().position(|m| m.name == name)
    }
}

/// Whether a trait is one a program may only ever `derive`.
///
/// `core/json`'s two are: what a derived encoder stands for is the type's
/// shape, so a hand-written one would be obeyed where the type is encoded on
/// its own and ignored where a type holding it is. `semantics::resolve`
/// rejects the `impl`; this is here so that no diagnostic elsewhere offers
/// writing one as the fix.
pub fn is_derive_only(trait_name: &str) -> bool {
    matches!(trait_name, "ToJson" | "FromJson")
}

/// "add `derive T for X;` in that type's own module, or write `impl ...`" —
/// minus the half that is not available.
pub fn conformance_fix(trait_name: &str, shown: &str) -> String {
    if is_derive_only(trait_name) {
        format!("add `derive {trait_name} for {shown};` in that type's own module")
    } else {
        format!(
            "add `derive {trait_name} for {shown};` in that type's own module, or write \
             `impl {trait_name} for {shown} {{ ... }}` there"
        )
    }
}

#[derive(Clone, Debug)]
pub struct TraitMethod {
    pub name: String,
    pub generics: Vec<GenericInfo>,
    pub params: Vec<ParamInfo>,
    pub ret: Ty,
    pub span: Span,
}

/// What an `impl` supplies.
///
/// A `derive`d implementation has no methods at all and must never be indexed;
/// a hand-written one has exactly one slot per trait method. Both facts used
/// to be carried by a `derived: bool` sitting beside a `Vec<FnId>` that padded
/// its gaps with `FnId(u32::MAX)` — a sentinel decoded 200 lines away in
/// another module, and an out-of-bounds index into the function table for
/// anyone who forgot.
#[derive(Clone, Debug)]
pub enum ImplBody {
    /// Generated by `derive`: the implementation is the fold over the type's
    /// components, so there is no function to call.
    Derived,
    /// Written by hand. One slot per trait method, in the trait's declaration
    /// order; `None` where the `impl` did not supply one, which is always
    /// already diagnosed.
    Written(Vec<Option<FnId>>),
}

#[derive(Clone, Debug)]
pub struct ImplInfo {
    pub trait_id: TraitId,
    pub self_con: TyConId,
    /// The type written after `for`, elaborated in the `impl`'s own generic
    /// scope: `ReadOnly<C>` is `Con(ReadOnly, [Param(0)])`, `Pair<Int, Str>` is
    /// `Con(Pair, [Int, Str])`. `self_con` is only its head, which is all
    /// method lookup needs — but monomorphization has to read the `impl`'s
    /// generics *back off* a concrete receiver, and matching this against that
    /// receiver is the only thing that says which argument stands for which
    /// parameter. Without it the recovery is positional arithmetic that is
    /// right by coincidence.
    pub head: Ty,
    pub body: ImplBody,
    pub span: Span,
}

impl ImplInfo {
    pub fn is_derived(&self) -> bool {
        matches!(self.body, ImplBody::Derived)
    }

    /// The function supplying one trait method, by its slot in the trait's
    /// declaration order. `None` for a derived impl, an out-of-range slot, or
    /// a method the `impl` failed to supply — three cases the caller has to
    /// handle identically anyway.
    pub fn method(&self, slot: usize) -> Option<FnId> {
        match &self.body {
            ImplBody::Derived => None,
            ImplBody::Written(ms) => ms.get(slot).copied().flatten(),
        }
    }
}

/// A `context` declaration. It takes no parameters and is constructed by
/// calling it, so each use gets a fresh one.
#[derive(Clone, Debug)]
pub struct ContextDeclInfo {
    pub name: String,
    pub module: ModuleId,
    pub exported: bool,
    /// What checking the declaration produced, or `None` before it has been
    /// checked and where checking failed.
    ///
    /// The generated type and its constructor were two separate `Option`s,
    /// which made `(None, Some)` representable — and reachable, because a body
    /// that did not evaluate to a context still got a constructor. One field
    /// makes the two agree by construction.
    pub checked: Option<CheckedContext>,
    pub span: Span,
    pub ast: AstRef,
}

/// A `context` declaration once checked.
#[derive(Clone, Copy, Debug)]
pub struct CheckedContext {
    /// The generated type: exactly the effects bound, and nothing else.
    pub ty: CtxTypeId,
    /// The nullary function that builds a fresh one. The parentheses in
    /// a test context are not decoration: a test's `Fs` and its captured
    /// `Stdout` accumulate what the test does to them, so two tests sharing
    /// one value would share its state.
    pub ctor: FnId,
}

#[derive(Clone, Debug)]
pub struct ConstInfo {
    pub name: String,
    pub module: ModuleId,
    pub ty: Ty,
    pub exported: bool,
    pub span: Span,
    pub ast: AstRef,
}

/// The generated type of a context value: exactly the effects bound, and
/// nothing else.
#[derive(Clone, Debug, Default)]
pub struct CtxType {
    /// Effect trait -> the type of the value implementing it, in the order the
    /// bindings were written (a spread's bindings first).
    pub bindings: Vec<(TraitId, Ty)>,
}

impl CtxType {
    pub fn get(&self, t: TraitId) -> Option<&Ty> {
        self.bindings.iter().find(|(id, _)| *id == t).map(|(_, ty)| ty)
    }

    pub fn has(&self, t: TraitId) -> bool {
        self.bindings.iter().any(|(id, _)| *id == t)
    }
}

// ---------------------------------------------------------------------------
// The nominal tables
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Tables {
    pub tycons: Vec<TyCon>,
    pub fns: Vec<FnInfo>,
    pub traits: Vec<TraitInfo>,
    pub consts: Vec<ConstInfo>,
    pub ctx_types: Vec<CtxType>,
    /// Exactly one candidate per `(trait, type)`. Coherence, orphan rules, and
    /// instance search are not restricted here — they are unrepresentable.
    pub impls: HashMap<(TraitId, TyConId), ImplInfo>,
    /// The traits each type implements, ascending.
    ///
    /// The same argument `effect_traits` makes below, for the other question
    /// `impls` was scanned to answer: `resolve_method` asks "which traits does
    /// this type implement?" on every method call that is not already in the
    /// method table, and answering it by walking `impls` made resolving one
    /// method cost as much as the whole compilation declares, the standard
    /// library included. `add_impl` is the only way a conformance comes into
    /// existence, so this cannot fall out of step, and the list is kept sorted
    /// where the scan sorted afterwards, so the answer is the same one.
    traits_by_con: HashMap<TyConId, Vec<TraitId>>,
    /// `defining type -> method name -> function`. Methods supplied by an
    /// `impl` live in the same namespace and are found here too, so an `impl`
    /// introduces no second resolution path.
    ///
    /// Nested rather than keyed by `(TyConId, String)`: a tuple key cannot be
    /// borrowed, so every lookup — one per method call in the program — had to
    /// allocate a `String` to ask. The inner map's key is `Borrow<str>`, and
    /// the outer one is `Copy`.
    methods: HashMap<TyConId, HashMap<String, FnId>>,
    /// `[T]` has no type constructor of its own; its defining module is
    /// `core/list`.
    pub array_methods: HashMap<String, FnId>,
    pub ctx_decls: Vec<ContextDeclInfo>,
    prim_ids: HashMap<Prim, TyConId>,
    /// `member name -> position` for the types whose variant or field list is
    /// long enough that `TyCon::variant_index`'s scan is the cost of checking
    /// them. One `match` arm per variant means one scan per arm, so an enum
    /// with N variants was N²/2 string comparisons before any pattern was
    /// looked at.
    ///
    /// A side table rather than a member of `TyCon`, because a `TyCon` is
    /// built by several phases and finished by `elaborate_signatures`; here
    /// there is one place that fills it, `index_members`, and a type whose
    /// index was never built falls back to the scan rather than answering
    /// wrongly. Sparse: only the types past the threshold have an entry, and
    /// the vector may be shorter than `tycons`.
    member_index: Vec<Option<Box<HashMap<String, usize>>>>,
    /// Per type constructor, per type parameter: whether *holding* a value of
    /// that constructor can hand you the argument at that position. See
    /// `compute_variance`.
    ///
    /// Empty until `compute_variance` runs, and `provides` answers `true` for
    /// anything it has no row for — so a question asked before the fixpoint is
    /// settled gets the conservative answer rather than a wrong one, and a
    /// type constructor minted afterwards is treated as holding everything.
    variance: Vec<Vec<bool>>,
    /// Which traits are effects, kept as a list because the effect predicates
    /// below ask "does this type constructor implement *any* effect?" once per
    /// type-constructor node they walk.
    ///
    /// The question used to be answered by scanning `impls` — every
    /// conformance in the compilation, standard library included — so the cost
    /// of checking one function grew with the number of `impl` blocks
    /// anywhere in the repository. Asking it of the effects instead is a hash
    /// lookup each, and a program declares a handful of effects and thousands
    /// of impls. `add_trait` is the only way a trait comes into existence and
    /// `is_effect` is fixed at that point, so this cannot fall out of step.
    effect_traits: Vec<TraitId>,
}

impl Tables {
    pub fn tycon(&self, id: TyConId) -> &TyCon {
        self.tycons.get(id.index()).or_ice("every TyConId was minted by add_tycon on this table")
    }

    /// Whether this is `Option`, the one type the backend gives a
    /// representation of its own.
    ///
    /// Identified by shape as well as by name, so that a user type called
    /// `Option` — which the language permits, in another module — is not
    /// mistaken for it. `Infer::is_known_option` asks the *nominal* question
    /// against the id the prelude registered; the two are not interchangeable.
    pub fn is_option(&self, id: TyConId) -> bool {
        let t = self.tycon(id);
        t.name == "Option"
            && t.generics.len() == 1
            && matches!(&t.def, TyDef::Enum { variants }
                if matches!(variants.as_slice(), [some, none]
                    if some.name == "Some"
                        && some.fields.len() == 1
                        && none.name == "None"
                        && none.fields.is_empty()))
    }

    /// `T`, when `ty` is `Option<T>`.
    pub fn option_payload<'a>(&self, ty: &'a Ty) -> Option<&'a Ty> {
        match ty {
            Ty::Con(id, args) if self.is_option(*id) => args.first(),
            _ => None,
        }
    }

    /// Whether a value of this type can itself be `None`, and so cannot be
    /// told apart from one by being `undefined`. Only these need the boxed
    /// form (`$some`/`$val`).
    pub fn is_option_ty(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Con(id, _) if self.is_option(*id))
    }

    pub fn fn_info(&self, id: FnId) -> &FnInfo {
        self.fns.get(id.index()).or_ice("every FnId was minted by add_fn on this table")
    }

    pub fn trait_(&self, id: TraitId) -> &TraitInfo {
        self.traits.get(id.index()).or_ice("every TraitId was minted by add_trait on this table")
    }

    pub fn const_(&self, id: ConstId) -> &ConstInfo {
        self.consts.get(id.index()).or_ice("every ConstId was minted by add_const on this table")
    }

    pub fn ctx_type(&self, id: CtxTypeId) -> &CtxType {
        self.ctx_types
            .get(id.index())
            .or_ice("every CtxTypeId was minted by add_ctx_type on this table")
    }

    /// The same entries, for the phases that fill them in. Ids come from the
    /// `add_*` methods below, so the index is always in range.
    pub fn tycon_mut(&mut self, id: TyConId) -> &mut TyCon {
        self.tycons.get_mut(id.index()).or_ice("every TyConId was minted by add_tycon")
    }

    pub fn fn_info_mut(&mut self, id: FnId) -> &mut FnInfo {
        self.fns.get_mut(id.index()).or_ice("every FnId was minted by add_fn")
    }

    pub fn trait_mut(&mut self, id: TraitId) -> &mut TraitInfo {
        self.traits.get_mut(id.index()).or_ice("every TraitId was minted by add_trait")
    }

    pub fn const_mut(&mut self, id: ConstId) -> &mut ConstInfo {
        self.consts.get_mut(id.index()).or_ice("every ConstId was minted by add_const")
    }

    pub fn ctx_decl_mut(&mut self, id: ContextDeclId) -> &mut ContextDeclInfo {
        self.ctx_decls.get_mut(id.index()).or_ice("every ContextDeclId was minted by add_ctx_decl")
    }

    pub fn add_tycon(&mut self, c: TyCon) -> TyConId {
        let id = TyConId(self.tycons.len() as u32);
        self.tycons.push(c);
        id
    }

    /// Builds `id`'s member-name index, once its `def` is settled. Below the
    /// threshold the scan is faster than the hash, so nothing is stored and
    /// the lookups below fall back to it.
    pub fn index_members(&mut self, id: TyConId) {
        /// A list shorter than this is quicker to walk than to hash.
        const THRESHOLD: usize = 8;

        let members: Vec<&str> = match &self.tycon(id).def {
            TyDef::Enum { variants } => variants.iter().map(|v| v.name.as_str()).collect(),
            TyDef::Struct { fields, .. } => fields.iter().map(|f| f.name.as_str()).collect(),
            TyDef::Prim(_) => return,
        };
        if members.len() < THRESHOLD {
            return;
        }
        let mut map: HashMap<String, usize> = HashMap::default();
        // First wins, so that a duplicate name resolves to the same member the
        // scan found. `resolve` reports the duplicate separately.
        for (i, name) in members.iter().enumerate() {
            map.entry((*name).to_owned()).or_insert(i);
        }
        if self.member_index.len() <= id.index() {
            self.member_index.resize_with(id.index().saturating_add(1), || None);
        }
        if let Some(slot) = self.member_index.get_mut(id.index()) {
            *slot = Some(Box::new(map));
        }
    }

    /// The index for `id`, when there is one and it is the list the caller
    /// means. A type's members are its variants or its fields, never both, so
    /// asking for the other list must miss rather than find a member of this
    /// one.
    fn member_map(&self, id: TyConId, enums: bool) -> Option<&HashMap<String, usize>> {
        if matches!(self.tycon(id).def, TyDef::Enum { .. }) != enums {
            return None;
        }
        self.member_index.get(id.index())?.as_deref()
    }

    /// The position of a variant in `id`'s variant list.
    pub fn variant_index(&self, id: TyConId, name: &str) -> Option<usize> {
        match self.member_map(id, true) {
            Some(map) => map.get(name).copied(),
            None => self.tycon(id).variant_index(name),
        }
    }

    /// The position of a field in `id`'s field list.
    pub fn field_index(&self, id: TyConId, name: &str) -> Option<usize> {
        match self.member_map(id, false) {
            Some(map) => map.get(name).copied(),
            None => self.tycon(id).field_index(name),
        }
    }

    /// `con` applied to its own parameters — `Option<T>` for `Option`. This is
    /// the `impl` head of a `derive`, which names a type constructor rather
    /// than an instantiation of one, and of a builtin conformance, which is
    /// registered against a primitive that takes no arguments at all.
    pub fn generic_head(&self, con: TyConId) -> Ty {
        let arity = self.tycon(con).generics.len();
        Ty::Con(con, (0..arity).map(|i| Ty::Param(i as u32)).collect())
    }

    /// Records a conformance, unless there is one already. Returns whether it
    /// was recorded, so a caller that wants `entry().or_insert()`'s behaviour
    /// gets it and one that has already reported the duplicate can ignore it.
    pub fn add_impl(&mut self, info: ImplInfo) -> bool {
        let key = (info.trait_id, info.self_con);
        if self.impls.contains_key(&key) {
            return false;
        }
        let list = self.traits_by_con.entry(info.self_con).or_default();
        if let Err(at) = list.binary_search(&info.trait_id) {
            list.insert(at, info.trait_id);
        }
        self.impls.insert(key, info);
        true
    }

    /// The method `name` on `con`, whether declared there or supplied by an
    /// `impl`.
    pub fn method(&self, con: TyConId, name: &str) -> Option<FnId> {
        self.methods.get(&con)?.get(name).copied()
    }

    /// Records a method, unless `con` already has one by that name. Returns
    /// whether it was recorded.
    pub fn add_method(&mut self, con: TyConId, name: &str, f: FnId) -> bool {
        let by_name = self.methods.entry(con).or_default();
        if by_name.contains_key(name) {
            return false;
        }
        by_name.insert(name.to_owned(), f);
        true
    }

    /// Every method name `con` has, for the "did you mean" a failed lookup
    /// offers. Unordered, and `nearest` is order-independent.
    pub fn method_names(&self, con: TyConId) -> impl Iterator<Item = &str> {
        self.methods.get(&con).into_iter().flat_map(|m| m.keys().map(String::as_str))
    }

    /// The traits `con` implements, ascending.
    pub fn traits_of_con(&self, con: TyConId) -> &[TraitId] {
        self.traits_by_con.get(&con).map_or(&[][..], Vec::as_slice)
    }

    pub fn add_fn(&mut self, f: FnInfo) -> FnId {
        let id = FnId(self.fns.len() as u32);
        self.fns.push(f);
        id
    }

    pub fn add_trait(&mut self, t: TraitInfo) -> TraitId {
        let id = TraitId(self.traits.len() as u32);
        if t.is_effect {
            self.effect_traits.push(id);
        }
        self.traits.push(t);
        id
    }

    /// Whether this type constructor implements any effect.
    ///
    /// Answered from `impls`, which is filled in by conformance registration —
    /// so every caller must run after it. That is why rule 26 is checked in a
    /// pass of its own (`Checker::check_ctx_rules`) rather than while
    /// signatures are elaborated: asked earlier, this returned `false` for
    /// every concrete effect implementor and a function taking one as an
    /// ordinary parameter was admitted.
    /// Whether the constructor itself implements an effect, so a value of it
    /// *is* a capability.
    ///
    /// Public because `middle::monomorphize` records the answer on
    /// [`crate::compiler::middle::monomorphize::Shapes`] for the passes that
    /// hold a `Program` and no `Tables` — `middle::rc` asks it of a callee's
    /// type, and `middle::native` takes no `Tables`.
    pub fn con_carries_effect(&self, con: TyConId) -> bool {
        self.effect_traits.iter().any(|t| self.impls.contains_key(&(*t, con)))
    }

    /// The first concrete constructor in `ty` that implements an effect,
    /// together with the effect it implements.
    ///
    /// `is_effect_carrying` answers yes or no; this answers *why*, for the
    /// case where the type mentions no bound anywhere and a reader would
    /// otherwise have nothing to look at. Walks the same positions the
    /// predicate does, so it never names a constructor the predicate did not
    /// count.
    pub fn effect_implementor(&self, ty: &Ty) -> Option<(TyConId, TraitId)> {
        match ty {
            Ty::Con(id, args) => {
                let own = self.effect_traits.iter().find(|t| self.impls.contains_key(&(**t, *id)));
                if let Some(t) = own {
                    return Some((*id, *t));
                }
                args.iter().enumerate().find_map(|(k, a)| {
                    if self.provides(*id, k) { self.effect_implementor(a) } else { None }
                })
            }
            Ty::Array(e) => self.effect_implementor(e),
            Ty::Tuple(es) => es.iter().find_map(|e| self.effect_implementor(e)),
            Ty::Fn(_, r) => self.effect_implementor(r),
            _ => None,
        }
    }

    /// Whether holding a `con<...>` can hand you its `i`th type argument.
    ///
    /// `true` for anything the fixpoint has no answer for, which is the
    /// conservative direction: the predicates below use this to *stop*
    /// descending, so a missing row costs precision and never soundness.
    pub fn provides(&self, con: TyConId, i: usize) -> bool {
        match self.variance.get(con.index()) {
            Some(row) => row.get(i).copied().unwrap_or(true),
            None => true,
        }
    }

    /// Computes `provides` for every type constructor, once the type bodies
    /// are elaborated and before anything asks.
    ///
    /// The rule is one line — a constructor provides its `i`th argument when
    /// some field it stores does:
    ///
    /// ```text
    /// provides(con, i)      =  ∃ field f of con . pos(f.ty, i)
    /// pos(Param(j), i)      =  j == i
    /// pos(Array(e), i)      =  pos(e, i)
    /// pos(Tuple(es), i)     =  ∃ e . pos(e, i)
    /// pos(Fn(_, r), i)      =  pos(r, i)                       // params dropped
    /// pos(Con(c, as), i)    =  ∃ k . provides(c, k) ∧ pos(as[k], i)
    /// ```
    ///
    /// Dropping a function type's parameters is the same rule the `Ty::Fn` arm
    /// of `is_effect_carrying` already rests on, generalised from the one
    /// built-in constructor that has a contravariant position to every
    /// user-declared one: holding a `fn(C, Event) => ()` never hands you a
    /// `C`, because to get one out you would have to supply it first. So an
    /// `enum Node<C> { Btn(fn(C, Event) => ()), Group([Node<C>]) }` provides
    /// no `C`, and `fn mount<C: Ui>(ctx: C, root: Node<C>)` is a function with
    /// one context rather than two.
    ///
    /// Monotone from all-false, so this is the *least* fixpoint: a parameter
    /// is provided only if some finite chain of fields hands it over. That is
    /// what makes a recursive type answer honestly — `Group([Node<C>])` needs
    /// `provides(Node, 0)` to already be true to make it true, so it stays
    /// false — and it is why the loop terminates: each pass only ever sets a
    /// bit, and there are finitely many.
    ///
    /// Over-approximating is safe in both users below: an extra `true` only
    /// makes them say "effect-carrying" more often, which is the direction
    /// that rejects programs rather than admitting them.
    pub fn compute_variance(&mut self) {
        let mut table: Vec<Vec<bool>> =
            self.tycons.iter().map(|c| vec![false; c.generics.len()]).collect();
        // Only a generic constructor has a row that can change, and a program
        // declares far more `Int`s than `Option`s. Listing them once keeps the
        // fixpoint off the primitives entirely rather than walking every empty
        // field list once per pass.
        let generic: Vec<usize> = self
            .tycons
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.generics.is_empty())
            .map(|(i, _)| i)
            .collect();
        loop {
            let mut changed = false;
            for &index in &generic {
                let Some(con) = self.tycons.get(index) else { continue };
                let arity = con.generics.len();
                // A struct has fields and no variants; an enum the reverse.
                // Both accessors answer `&[]` for the other shape, so this is
                // every type a value of `con` can hold.
                let stored = con
                    .fields()
                    .iter()
                    .chain(con.variants().iter().flat_map(|v| v.fields.iter()));
                for field in stored {
                    for i in 0..arity {
                        let known = table
                            .get(index)
                            .and_then(|row| row.get(i))
                            .copied()
                            .unwrap_or(true);
                        if known || !Tables::occurs_provided(&table, &field.ty, i) {
                            continue;
                        }
                        if let Some(cell) = table.get_mut(index).and_then(|row| row.get_mut(i)) {
                            *cell = true;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        self.variance = table;
    }

    /// `pos` from `compute_variance`, against a partial table.
    fn occurs_provided(table: &[Vec<bool>], ty: &Ty, i: usize) -> bool {
        match ty {
            Ty::Param(j) => *j as usize == i,
            Ty::Array(e) => Tables::occurs_provided(table, e, i),
            Ty::Tuple(es) => es.iter().any(|e| Tables::occurs_provided(table, e, i)),
            Ty::Fn(_, r) => Tables::occurs_provided(table, r, i),
            Ty::Con(c, args) => args.iter().enumerate().any(|(k, a)| {
                table.get(c.index()).and_then(|row| row.get(k)).copied().unwrap_or(true)
                    && Tables::occurs_provided(table, a, i)
            }),
            _ => false,
        }
    }

    pub fn add_const(&mut self, c: ConstInfo) -> ConstId {
        let id = ConstId(self.consts.len() as u32);
        self.consts.push(c);
        id
    }

    pub fn ctx_decl(&self, id: ContextDeclId) -> &ContextDeclInfo {
        self.ctx_decls
            .get(id.index())
            .or_ice("every ContextDeclId was minted by add_ctx_decl on this table")
    }

    pub fn add_ctx_decl(&mut self, c: ContextDeclInfo) -> ContextDeclId {
        let id = ContextDeclId(self.ctx_decls.len() as u32);
        self.ctx_decls.push(c);
        id
    }

    pub fn add_ctx_type(&mut self, c: CtxType) -> CtxTypeId {
        let id = CtxTypeId(self.ctx_types.len() as u32);
        self.ctx_types.push(c);
        id
    }

    pub fn register_prim(&mut self, p: Prim, id: TyConId) {
        self.prim_ids.insert(p, id);
    }

    pub fn prim_id(&self, p: Prim) -> TyConId {
        *self
            .prim_ids
            .get(&p)
            .or_ice("register_primitives records every Prim::all() before anything asks for one")
    }

    pub fn prim(&self, p: Prim) -> Ty {
        Ty::Con(self.prim_id(p), Vec::new())
    }

    pub fn as_prim(&self, ty: &Ty) -> Option<Prim> {
        match ty {
            Ty::Con(id, _) => match self.tycon(*id).def {
                TyDef::Prim(p) => Some(p),
                _ => None,
            },
            _ => None,
        }
    }

    /// A type is effect-carrying if it is a type variable with an effect
    /// bound, or any type that can *hand one over* — so a struct that stores a
    /// context is effect-carrying too (SPEC 10.2).
    ///
    /// "Can hand one over" rather than "mentions one": a type argument counts
    /// only where the constructor `provides` it. `Node<C>`, whose `C` appears
    /// only as a handler's parameter, is data even when `C: Ui`, for the same
    /// reason `fn(C, A) => B` is.
    pub fn is_effect_carrying(&self, ty: &Ty, generics: &[GenericInfo]) -> bool {
        match ty {
            Ty::Param(i) => generics
                .get(*i as usize)
                .is_some_and(|g| g.bounds.iter().any(|b| self.trait_(*b).is_effect)),
            Ty::Ctx(_) => true,
            Ty::Con(id, args) => {
                self.con_carries_effect(*id)
                    || args.iter().enumerate().any(|(k, a)| {
                        self.provides(*id, k) && self.is_effect_carrying(a, generics)
                    })
            }
            Ty::Array(e) => self.is_effect_carrying(e, generics),
            Ty::Tuple(es) => es.iter().any(|e| self.is_effect_carrying(e, generics)),
            // Only the result counts. A function that *accepts* a context is
            // not one that *carries* one — `fn(C, A) => B` is exactly the
            // shape `list.mapCtx` takes, and SPEC 10.6 mandates it. A
            // function that *returns* a context does carry it.
            Ty::Fn(_, r) => self.is_effect_carrying(r, generics),
            _ => false,
        }
    }

    /// Whether a value of this type *could* carry an effect under some
    /// instantiation of the enclosing signature's generics.
    ///
    /// `is_effect_carrying` answers the question a *signature* asks: does this
    /// type, as written, mention an effect? That is the right question for the
    /// `ctx` rule, and the wrong one for the capture rule, because a generic
    /// body is checked once and polymorphically (SPEC 13.5) — so at the point
    /// the capture rule runs, `T` is opaque and nothing rules out `T := C` for
    /// a context type `C`. A predicate that answers "no" for `T` is not an
    /// inductive invariant:
    ///
    /// ```text
    /// fn wrap<T>(x: T, f: fn(T) => ()): fn() => () { fn() => f(x) }
    /// ```
    ///
    /// Every step there is sanctioned by `is_effect_carrying` and the
    /// composition smuggles a capability into a closure whose type mentions no
    /// effect at all. So the capture rule asks this question instead, and an
    /// unbounded type parameter answers "yes".
    ///
    /// One kind of parameter escapes: one carrying an **ordinary trait**
    /// bound. A type is either part of the world or part of your data
    /// (SPEC 10.1), and `Infer::satisfies_seen` enforces that at every
    /// instantiation — an effect-carrying type satisfies no ordinary bound —
    /// so `T: Eq` cannot be a context and `xs.any(fn(x) => x == needle)` stays
    /// legal. A parameter bounded only by effects has no such guarantee, and
    /// `is_effect_carrying` has already answered `true` for it anyway.
    ///
    /// A **function type** answers `false` outright, where `is_effect_carrying`
    /// looks at its result. The two are asking different questions. The `ctx`
    /// rule asks what a type *says*, and `fn() => C` says "hands out a
    /// context", so it must arrive as `ctx`. The capture rule asks what a
    /// value *holds*, and a closure holds exactly what it captured — which is
    /// what this rule checks. So the answer for closures is inductive rather
    /// than structural: no closure holds a capability, because no closure was
    /// allowed to capture one, and a context cannot be constructed inside a
    /// lambda (SPEC 11.3). That is what keeps `fn compose<A, B, C>(f: fn(A) =>
    /// B, g: fn(B) => C): fn(A) => C` legal.
    ///
    /// A **type argument** the constructor does not `provide` answers `false`
    /// here too, and for a stronger reason than above: this rule asks what a
    /// value holds, and a `struct Signal<T>(Int)` holds no `T` at all under
    /// any instantiation. A `Prop<T>` — which stores one — still answers
    /// `true`, so an ordinary bound is still the way to capture that.
    ///
    /// Everything else is the shape of `is_effect_carrying`.
    pub fn may_carry_effect(&self, ty: &Ty, generics: &[GenericInfo]) -> bool {
        match ty {
            Ty::Param(i) => match generics.get(*i as usize) {
                Some(g) => !g.bounds.iter().any(|b| !self.trait_(*b).is_effect),
                None => true,
            },
            Ty::Ctx(_) => true,
            Ty::Con(id, args) => {
                self.con_carries_effect(*id)
                    || args.iter().enumerate().any(|(k, a)| {
                        self.provides(*id, k) && self.may_carry_effect(a, generics)
                    })
            }
            Ty::Array(e) => self.may_carry_effect(e, generics),
            Ty::Tuple(es) => es.iter().any(|e| self.may_carry_effect(e, generics)),
            // Spelled out rather than folded into the catch-all, because this
            // is the one place the two predicates deliberately disagree.
            Ty::Fn(..) => false,
            _ => false,
        }
    }

    /// Nominal conformance: a lookup, never a search.
    pub fn implements(&self, ty: &Ty, tr: TraitId) -> bool {
        match ty {
            Ty::Con(id, _) => self.impls.contains_key(&(tr, *id)),
            Ty::Ctx(id) => self.ctx_type(*id).has(tr),
            Ty::Error => true,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Unification
// ---------------------------------------------------------------------------

/// The substitution for one function body. Local, because top-level signatures
/// are mandatory and no inference crosses a function boundary.
#[derive(Default)]
pub struct Subst {
    slots: Vec<Option<Ty>>,
    classes: Vec<Option<NumClass>>,
    /// The span the variable was created at, for defaulting diagnostics.
    spans: Vec<Span>,
}

impl Subst {
    pub fn fresh(&mut self, span: Span) -> Ty {
        let id = TyVarId(self.slots.len() as u32);
        self.slots.push(None);
        self.classes.push(None);
        self.spans.push(span);
        Ty::Var(id)
    }

    /// A numeric literal gets a fresh variable constrained to the integer
    /// types or the float types; ordinary unification then decides
    /// (SPEC 5.1.1).
    pub fn fresh_num(&mut self, class: NumClass, span: Span) -> Ty {
        let ty = self.fresh(span);
        if let Ty::Var(id) = ty {
            *self.class_slot(id) = Some(class);
        }
        ty
    }

    /// The class cell for a variable. Every [`TyVarId`] comes from `fresh`,
    /// which pushes one cell per table in step, so the id is always in range.
    fn class_slot(&mut self, id: TyVarId) -> &mut Option<NumClass> {
        self.classes.get_mut(id.index()).or_ice("every TyVarId was minted by Subst::fresh")
    }

    pub fn class_of(&self, id: TyVarId) -> Option<NumClass> {
        *self.classes.get(id.index()).or_ice("every TyVarId was minted by Subst::fresh")
    }

    pub fn span_of(&self, id: TyVarId) -> Span {
        *self.spans.get(id.index()).or_ice("every TyVarId was minted by Subst::fresh")
    }

    pub fn get(&self, id: TyVarId) -> Option<&Ty> {
        self.slots.get(id.index()).or_ice("every TyVarId was minted by Subst::fresh").as_ref()
    }

    fn set(&mut self, id: TyVarId, ty: Ty) {
        *self.slots.get_mut(id.index()).or_ice("every TyVarId was minted by Subst::fresh") =
            Some(ty);
    }

    /// Follows bound variables one level at a time.
    pub fn shallow(&self, ty: &Ty) -> Ty {
        self.shallow_ref(ty).clone()
    }

    /// The same, without copying.
    ///
    /// Following a variable to what it stands for reads a type; it does not
    /// need to own one. `shallow` copied the type before looking at it, so
    /// every step of `unify`, `resolve` and `occurs` began by deep-copying the
    /// type it was about to take apart — and a type is a tree, so
    /// `Result<[Str], Error>` was copied whole at every node, twice per
    /// unification step.
    ///
    /// Callers that must hand an owned type onwards still use `shallow`; the
    /// ones that only look use this.
    pub fn shallow_ref<'t>(&'t self, ty: &'t Ty) -> &'t Ty {
        /// Returned when the chain does not terminate. A `static` rather than
        /// a promoted `&Ty::Error`, which `Ty` is not eligible for.
        static NON_TERMINATING: Ty = Ty::Error;
        let mut cur = ty;
        let mut guard: u32 = 0;
        while let Ty::Var(id) = cur {
            match self.get(*id) {
                Some(next) => cur = next,
                None => break,
            }
            guard = guard.saturating_add(1);
            if guard > 1000 {
                return &NON_TERMINATING;
            }
        }
        cur
    }

    /// Applies the substitution everywhere.
    ///
    /// `shallow_ref` rather than `shallow`: this rebuilds the type from its
    /// parts, so the copy `shallow` made at every level of the recursion was
    /// dropped again immediately, and this runs over every type of every node
    /// of every checked body.
    pub fn resolve(&self, ty: &Ty) -> Ty {
        match self.shallow_ref(ty) {
            Ty::Con(id, args) => {
                Ty::Con(*id, args.iter().map(|a| self.resolve(a)).collect())
            }
            Ty::Array(e) => Ty::Array(Box::new(self.resolve(e))),
            Ty::Tuple(es) => Ty::Tuple(es.iter().map(|e| self.resolve(e)).collect()),
            Ty::Fn(ps, r) => {
                Ty::Fn(ps.iter().map(|p| self.resolve(p)).collect(), Box::new(self.resolve(r)))
            }
            other => other.clone(),
        }
    }

    fn occurs(&self, id: TyVarId, ty: &Ty) -> bool {
        match self.shallow_ref(ty) {
            Ty::Var(v) => *v == id,
            Ty::Con(_, args) => args.iter().any(|a| self.occurs(id, a)),
            Ty::Array(e) => self.occurs(id, e),
            Ty::Tuple(es) => es.iter().any(|e| self.occurs(id, e)),
            Ty::Fn(ps, r) => ps.iter().any(|p| self.occurs(id, p)) || self.occurs(id, r),
            _ => false,
        }
    }

    /// Structural unification. `Error` unifies with anything, so one mistake
    /// does not cascade into ten.
    pub fn unify(&mut self, tables: &Tables, a: &Ty, b: &Ty) -> Result<(), (Ty, Ty)> {
        // The cases that are decided by looking, and so need no copy. Most
        // unification in a real program is two primitives or a variable
        // against itself, and taking those here is what keeps the copy below
        // to the cases that genuinely take a type apart.
        match (self.shallow_ref(a), self.shallow_ref(b)) {
            (Ty::Error, _) | (_, Ty::Error) => return Ok(()),
            (Ty::Var(x), Ty::Var(y)) if x == y => return Ok(()),
            (Ty::Unit, Ty::Unit) | (Ty::SelfTy, Ty::SelfTy) => return Ok(()),
            (Ty::Param(x), Ty::Param(y)) if x == y => return Ok(()),
            (Ty::Ctx(x), Ty::Ctx(y)) if x == y => return Ok(()),
            (Ty::Con(x, xs), Ty::Con(y, ys)) if x == y && xs.is_empty() && ys.is_empty() => {
                return Ok(())
            }
            _ => {}
        }
        let a = self.shallow(a);
        let b = self.shallow(b);
        match (&a, &b) {
            (Ty::Error, _) | (_, Ty::Error) => Ok(()),
            (Ty::Var(x), Ty::Var(y)) if x == y => Ok(()),
            (Ty::Var(x), _) => self.bind(tables, *x, &b),
            (_, Ty::Var(y)) => self.bind(tables, *y, &a),
            (Ty::Unit, Ty::Unit) => Ok(()),
            (Ty::SelfTy, Ty::SelfTy) => Ok(()),
            (Ty::Param(x), Ty::Param(y)) if x == y => Ok(()),
            (Ty::Ctx(x), Ty::Ctx(y)) if x == y => Ok(()),
            (Ty::Con(x, xs), Ty::Con(y, ys)) if x == y && xs.len() == ys.len() => {
                for (p, q) in xs.iter().zip(ys) {
                    self.unify(tables, p, q)?;
                }
                Ok(())
            }
            (Ty::Array(x), Ty::Array(y)) => self.unify(tables, x, y),
            (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => {
                for (p, q) in xs.iter().zip(ys) {
                    self.unify(tables, p, q)?;
                }
                Ok(())
            }
            (Ty::Fn(xs, xr), Ty::Fn(ys, yr)) if xs.len() == ys.len() => {
                for (p, q) in xs.iter().zip(ys) {
                    self.unify(tables, p, q)?;
                }
                self.unify(tables, xr, yr)
            }
            _ => Err((a, b)),
        }
    }

    fn bind(&mut self, tables: &Tables, id: TyVarId, ty: &Ty) -> Result<(), (Ty, Ty)> {
        if self.occurs(id, ty) {
            return Err((Ty::Var(id), ty.clone()));
        }
        // A literal's class travels with it: binding an integer-class variable
        // to a float type is what makes `let x: F64 = 1` an error rather than
        // a silent promotion.
        if let Some(class) = self.class_of(id) {
            match self.shallow(ty) {
                Ty::Var(other) => {
                    let slot = self.class_slot(other);
                    match *slot {
                        None => *slot = Some(class),
                        Some(existing) if existing != class => {
                            return Err((Ty::Var(id), ty.clone()))
                        }
                        Some(_) => {}
                    }
                }
                resolved => {
                    let ok = match tables.as_prim(&resolved) {
                        Some(p) => match class {
                            NumClass::Int => p.is_integer(),
                            NumClass::Float => p.is_float(),
                        },
                        None => false,
                    };
                    if !ok {
                        return Err((Ty::Var(id), ty.clone()));
                    }
                }
            }
        }
        self.set(id, ty.clone());
        Ok(())
    }

    /// Only if nothing constrains a literal does the default apply — `Int` for
    /// integer literals, `Float` for float literals (SPEC 5.1.1).
    pub fn default_numerics(&mut self, tables: &Tables) {
        for (slot, class) in self.slots.iter_mut().zip(&self.classes) {
            if slot.is_some() {
                continue;
            }
            if let Some(class) = class {
                *slot = Some(match class {
                    NumClass::Int => tables.prim(Prim::I64),
                    NumClass::Float => tables.prim(Prim::F64),
                });
            }
        }
    }

    /// Every variable still unbound once a body has been checked stands for a
    /// type nothing in the body constrains, and becomes `()`.
    ///
    /// Inference is local to one body (SPEC 13.3), so a variable left unbound
    /// at the end of one is unbound for good: no later body can narrow it, and
    /// no signature carries it outwards. A compiler must still name a type for
    /// it, and `()` is the one with a single value and no structure — which is
    /// what a value the body never inspects has.
    ///
    /// This costs no representation. `middle::layout` already gives `Ty::Var`
    /// and `Ty::Unit` the same zero layout, so the artifact is the one that was
    /// emitted before; what changes is that `monomorphize::descriptor` can
    /// describe the type — `Desc::Unit` rather than `Desc::Opaque` — so
    /// `middle::derives` can generate the structural operations over it.
    /// `assert.some(Option.None)` is the shape that noticed: nothing constrains
    /// the payload, so a native failure report had no `Show` to call and
    /// printed neither side.
    ///
    /// Called after every check a body's diagnostics come from, so that a
    /// variable no constraint reached is *reported* as unresolved exactly where
    /// it was before and only *represented* differently.
    pub fn default_unconstrained(&mut self) {
        for slot in &mut self.slots {
            if slot.is_none() {
                *slot = Some(Ty::Unit);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The types inside a type
// ---------------------------------------------------------------------------

/// The fields of a struct, tuple or context, as types, in declaration order.
///
/// The same walk `middle::layout` does, which is why the offsets in a `Layout`
/// line up with this list index for index. It exists because a layout answers
/// *where* a field is and not *what* it is, and every reference-counting and
/// descriptor walk needs both.
///
/// It lives here rather than in a backend because it reads nothing but these
/// tables: three backends asking the same question of the same table is one
/// question.
pub fn field_types(tables: &Tables, ty: &Ty) -> Vec<Ty> {
    match ty {
        Ty::Tuple(elements) => elements.clone(),
        Ty::Ctx(id) => tables.ctx_type(*id).bindings.iter().map(|(_, t)| t.clone()).collect(),
        Ty::Con(id, args) => match &tables.tycon(*id).def {
            TyDef::Struct { fields, .. } => {
                fields.iter().map(|f| substitute(&f.ty, args, None)).collect()
            }
            TyDef::Prim(_) | TyDef::Enum { .. } => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// One variant's fields, as types, in declaration order.
pub fn variant_types(tables: &Tables, ty: &Ty, variant: usize) -> Vec<Ty> {
    let Ty::Con(id, args) = ty else { return Vec::new() };
    match &tables.tycon(*id).def {
        TyDef::Enum { .. } => match tables.tycon(*id).variants().get(variant) {
            Some(v) => v.fields.iter().map(|f| substitute(&f.ty, args, None)).collect(),
            None => Vec::new(),
        },
        TyDef::Prim(_) | TyDef::Struct { .. } => Vec::new(),
    }
}

/// Every trait a type constructor is known to satisfy, by name.
///
/// Reads the real impl table rather than a list, so it cannot drift from what
/// was registered — which is what both callers want it for: one reports the
/// set in a diagnostic, the other asserts `I64`'s is maximal among the
/// integers.
pub fn traits_of(tables: &Tables, con: TyConId) -> std::collections::BTreeSet<String> {
    tables
        .impls
        .keys()
        .filter(|(_, c)| *c == con)
        .map(|(t, _)| tables.trait_(*t).name.clone())
        .collect()
}

/// Where `Ok` and `Err` sit in a `Result`'s variant list, and what `Err`
/// carries.
///
/// By name rather than by index: `core/result` declares `Ok` first, but a
/// backend that hard-coded `0` and `1` would be reading a declaration order
/// out of a table that records it, and the two would have to be kept in step
/// by hand. `None` for anything that is not a two-armed `Result`.
pub fn result_shape(tables: &Tables, ty: &Ty) -> Option<(usize, usize, Ty)> {
    let Ty::Con(id, args) = ty else { return None };
    let variants = tables.tycon(*id).variants();
    let ok = variants.iter().position(|v| v.name == "Ok")?;
    let err = variants.iter().position(|v| v.name == "Err")?;
    Some((ok, err, args.get(err)?.clone()))
}

// ---------------------------------------------------------------------------
// Substituting generic parameters
// ---------------------------------------------------------------------------

/// Replaces `Param(i)` with `args[i]`, and `Self` with `self_ty`.
pub fn substitute(ty: &Ty, args: &[Ty], self_ty: Option<&Ty>) -> Ty {
    match ty {
        Ty::Param(i) => args.get(*i as usize).cloned().unwrap_or(Ty::Error),
        Ty::SelfTy => self_ty.cloned().unwrap_or(Ty::SelfTy),
        Ty::Con(id, xs) => {
            Ty::Con(*id, xs.iter().map(|x| substitute(x, args, self_ty)).collect())
        }
        Ty::Array(e) => Ty::Array(Box::new(substitute(e, args, self_ty))),
        Ty::Tuple(es) => Ty::Tuple(es.iter().map(|e| substitute(e, args, self_ty)).collect()),
        Ty::Fn(ps, r) => Ty::Fn(
            ps.iter().map(|p| substitute(p, args, self_ty)).collect(),
            Box::new(substitute(r, args, self_ty)),
        ),
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

/// Renders a type the way a program would write it. Diagnostics print whichever
/// spelling the program used where they can, but the canonical name otherwise.
pub fn show(tables: &Tables, subst: Option<&Subst>, generics: &[GenericInfo], ty: &Ty) -> String {
    let resolved = match subst {
        Some(s) => s.resolve(ty),
        None => ty.clone(),
    };
    let mut out = String::new();
    write_ty(&mut out, tables, subst, generics, &resolved);
    out
}

/// How a diagnostic names a type. A literal is named by the type it defaults
/// to (SPEC 5.1.1), so `Code` and `Literal` both render a spelling the program
/// could have written; the class rides along only so the advice can tell a
/// literal from a value that already has a type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spelling {
    Code(String),
    /// A numeric literal, named by the type it takes when nothing pins it.
    Literal(NumClass),
    /// A variable nothing in the body constrains; `finish` makes it `()`.
    Unconstrained,
}

impl Spelling {
    /// The name alone: backticked where it is code, bare where it is prose, so
    /// that a message never quotes a phrase as if it were syntax.
    pub fn quoted(&self) -> String {
        match self {
            Spelling::Code(name) => format!("`{name}`"),
            Spelling::Literal(class) => format!("`{}`", class.default_name()),
            Spelling::Unconstrained => "an unknown type".to_string(),
        }
    }

    /// The same as a noun phrase, for a sentence that supplies the article.
    pub fn noun_phrase(&self) -> String {
        match self {
            Spelling::Code(name) => format!("a `{name}`"),
            Spelling::Literal(class) => class.default_noun_phrase().to_string(),
            Spelling::Unconstrained => self.quoted(),
        }
    }

    /// The bare name, for a sentence that punctuates it itself. An unpinned
    /// literal answers with the type it would default to.
    pub fn name(&self) -> &str {
        match self {
            Spelling::Code(name) => name,
            Spelling::Literal(class) => class.default_name(),
            Spelling::Unconstrained => "_",
        }
    }
}

/// How a type is named in a diagnostic: the same spelling `show` gives, plus
/// whether it came from a literal. `show` stays the renderer everywhere else.
pub fn show_in_diagnostic(
    tables: &Tables,
    subst: &Subst,
    generics: &[GenericInfo],
    ty: &Ty,
) -> Spelling {
    if let Ty::Var(id) = subst.shallow(ty) {
        return match subst.class_of(id) {
            Some(class) => Spelling::Literal(class),
            None => Spelling::Unconstrained,
        };
    }
    Spelling::Code(show(tables, Some(subst), generics, ty))
}

fn write_ty(
    out: &mut String,
    tables: &Tables,
    subst: Option<&Subst>,
    generics: &[GenericInfo],
    ty: &Ty,
) {
    match ty {
        Ty::Var(id) => {
            // Nothing has pinned this literal yet, so the name to print is the
            // one it would default to — a spelling a program can write.
            match subst.and_then(|s| s.class_of(*id)) {
                Some(class) => out.push_str(class.default_name()),
                None => {
                    let _ = write!(out, "_{}", id.0);
                }
            }
        }
        Ty::Param(i) => match generics.get(*i as usize) {
            Some(g) => out.push_str(&g.name),
            None => {
                let _ = write!(out, "?{i}");
            }
        },
        Ty::Con(id, args) => {
            out.push_str(&tables.tycon(*id).name);
            if !args.is_empty() {
                out.push('<');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_ty(out, tables, subst, generics, a);
                }
                out.push('>');
            }
        }
        Ty::Array(e) => {
            out.push('[');
            write_ty(out, tables, subst, generics, e);
            out.push(']');
        }
        Ty::Tuple(es) => {
            out.push('(');
            for (i, e) in es.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_ty(out, tables, subst, generics, e);
            }
            out.push(')');
        }
        Ty::Fn(ps, r) => {
            out.push_str("fn(");
            for (i, p) in ps.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_ty(out, tables, subst, generics, p);
            }
            out.push_str(") => ");
            write_ty(out, tables, subst, generics, r);
        }
        Ty::Unit => out.push_str("()"),
        Ty::Ctx(_) => out.push_str("a context"),
        Ty::SelfTy => out.push_str("Self"),
        Ty::Error => out.push_str("<error>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables_with_prims() -> Tables {
        let mut t = Tables::default();
        for p in Prim::all() {
            let id = t.add_tycon(TyCon {
                name: p.name().to_string(),
                module: ModuleId(0),
                generics: Vec::new(),
                def: TyDef::Prim(*p),
                exported: true,
                span: Span::NONE,
            });
            t.register_prim(*p, id);
        }
        t
    }

    #[test]
    fn int_literals_unify_with_any_integer_width() {
        let t = tables_with_prims();
        let mut s = Subst::default();
        let lit = s.fresh_num(NumClass::Int, Span::NONE);
        assert!(s.unify(&t, &lit, &t.prim(Prim::U8)).is_ok());
        assert_eq!(s.resolve(&lit), t.prim(Prim::U8));
    }

    #[test]
    fn an_integer_literal_does_not_become_a_float() {
        // There is no implicit promotion of any kind, not even for literals.
        let t = tables_with_prims();
        let mut s = Subst::default();
        let lit = s.fresh_num(NumClass::Int, Span::NONE);
        assert!(s.unify(&t, &lit, &t.prim(Prim::F64)).is_err());
    }

    #[test]
    fn an_unpinned_literal_is_named_by_the_type_it_defaults_to() {
        let t = tables_with_prims();
        let mut s = Subst::default();
        let int = s.fresh_num(NumClass::Int, Span::NONE);
        let float = s.fresh_num(NumClass::Float, Span::NONE);
        assert_eq!(show_in_diagnostic(&t, &s, &[], &int).quoted(), "`Int`");
        assert_eq!(show_in_diagnostic(&t, &s, &[], &float).quoted(), "`Float`");
        assert_eq!(show_in_diagnostic(&t, &s, &[], &int).noun_phrase(), "an `Int`");
        assert_eq!(show_in_diagnostic(&t, &s, &[], &float).noun_phrase(), "a `Float`");
        // Nested inside a larger type it is the same name, so one literal is
        // never given two spellings.
        assert_eq!(show(&t, Some(&s), &[], &Ty::Array(Box::new(int))), "[Int]");
        assert_eq!(show(&t, Some(&s), &[], &Ty::Array(Box::new(float))), "[Float]");
    }

    #[test]
    fn unconstrained_literals_default() {
        let t = tables_with_prims();
        let mut s = Subst::default();
        let i = s.fresh_num(NumClass::Int, Span::NONE);
        let f = s.fresh_num(NumClass::Float, Span::NONE);
        s.default_numerics(&t);
        assert_eq!(s.resolve(&i), t.prim(Prim::I64));
        assert_eq!(s.resolve(&f), t.prim(Prim::F64));
    }

    #[test]
    fn a_variable_nothing_constrains_becomes_unit() {
        // Including inside a type that *is* determined: `Option<_>` is not an
        // opaque type, it is `Option<()>`, which is what lets
        // `monomorphize::descriptor` describe it and `middle::derives` generate
        // a `Show` for a failure report to call.
        let t = tables_with_prims();
        let mut s = Subst::default();
        let free = s.fresh(Span::NONE);
        let bound = s.fresh(Span::NONE);
        assert!(s.unify(&t, &bound, &t.prim(Prim::Str)).is_ok());
        let inside = Ty::Tuple(vec![free.clone(), bound.clone()]);
        s.default_unconstrained();
        assert_eq!(s.resolve(&free), Ty::Unit);
        assert_eq!(s.resolve(&bound), t.prim(Prim::Str));
        assert_eq!(s.resolve(&inside), Ty::Tuple(vec![Ty::Unit, t.prim(Prim::Str)]));
    }

    #[test]
    fn occurs_check() {
        let t = tables_with_prims();
        let mut s = Subst::default();
        let v = s.fresh(Span::NONE);
        let arr = Ty::Array(Box::new(v.clone()));
        assert!(s.unify(&t, &v, &arr).is_err());
    }

    #[test]
    fn the_error_type_unifies_with_anything() {
        let t = tables_with_prims();
        let mut s = Subst::default();
        assert!(s.unify(&t, &Ty::Error, &t.prim(Prim::Str)).is_ok());
        assert!(s.unify(&t, &t.prim(Prim::Bool), &Ty::Error).is_ok());
    }

    #[test]
    fn int_ranges_are_exact() {
        assert_eq!(Prim::U8.int_range(), Some((0, 255)));
        assert_eq!(Prim::I8.int_range(), Some((-128, 127)));
        assert_eq!(Prim::U64.int_range(), Some((0, u64::MAX as u128)));
    }

    #[test]
    fn the_wide_integers_are_bigints_and_nothing_else_is() {
        // The line is the width a double still holds every integer of.
        let wide = [Prim::I64, Prim::U64, Prim::I128, Prim::U128];
        assert!(wide.iter().all(|p| p.is_bigint()));
        let narrow = [Prim::I8, Prim::I16, Prim::I32, Prim::U8, Prim::U16, Prim::U32];
        assert!(narrow.iter().all(|p| !p.is_bigint()));
        assert!([Prim::F32, Prim::F64, Prim::Bool, Prim::Str].iter().all(|p| !p.is_bigint()));
    }
}
