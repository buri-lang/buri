//! Types, the nominal tables, and unification.
//!
//! The type system is nominal throughout: every type has a declaration, and
//! trait conformance is declared rather than inferred from shape. That is what
//! makes checking `T: Ord` one lookup in one table keyed by `(trait, type)`
//! rather than a search (SPEC 5.12.1), and it is why nothing in this module
//! needs a fixpoint (SPEC 13.6).

use crate::diagnostics::Span;
use std::collections::HashMap;
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

    /// Every numeric type is a JavaScript `number`. `Int` is `I64`, and a
    /// double holds every integer up to 2^53 exactly; past that, precision is
    /// lost. Overflow and underflow are undefined, so that loss is within what
    /// the language permits, and it is what keeps ordinary integer code as
    /// fast as ordinary JavaScript (SPEC 15, open question 8).
    pub fn is_bigint(self) -> bool {
        false
    }

    /// The largest integer a `number` represents unambiguously: 2^53 itself is
    /// the image of two different integers, so the last safe one is below it.
    /// Beyond this an integer type's arithmetic is undefined rather than
    /// wrong-but-defined.
    pub const EXACT_INTEGER_LIMIT: u128 = (1 << 53) - 1;

    /// The range `Checked` answers about: the type's own range, narrowed to
    /// what a double still represents exactly. A `Checked` operation that said
    /// `.Some` outside this would be reporting a value it cannot actually
    /// hold, which is the one thing `Checked` exists to rule out.
    pub fn exact_int_range(self) -> Option<(i128, u128)> {
        let (lo, hi) = self.int_range()?;
        let limit = Prim::EXACT_INTEGER_LIMIT;
        Some((lo.max(-(limit as i128)), hi.min(limit)))
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

/// Where a declaration's syntax lives: which module, and which item in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AstRef {
    pub module: ModuleId,
    pub item: u32,
    /// For a method inside an `impl`, the index within that impl's methods.
    pub sub: u32,
}

impl AstRef {
    pub const NONE: AstRef = AstRef { module: ModuleId(u32::MAX), item: 0, sub: u32::MAX };
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

#[derive(Clone, Debug)]
pub struct ImplInfo {
    pub trait_id: TraitId,
    pub self_con: TyConId,
    /// One `FnId` per trait method, in the trait's declaration order.
    pub methods: Vec<FnId>,
    pub span: Span,
    /// Generated by `derive` rather than written by hand.
    pub derived: bool,
}

/// A `context` declaration. It takes no parameters and is constructed by
/// calling it, so each use gets a fresh one.
#[derive(Clone, Debug)]
pub struct ContextDeclInfo {
    pub name: String,
    pub module: ModuleId,
    pub exported: bool,
    /// The generated type, filled in once the declaration is checked.
    pub ty: Option<CtxTypeId>,
    /// The nullary function that builds a fresh one. The parentheses in
    /// `Hermetic()` are not decoration: a test's `Fs` and its captured
    /// `Stdout` accumulate what the test does to them, so two tests sharing
    /// one value would share its state.
    pub ctor: Option<FnId>,
    pub span: Span,
    pub ast: AstRef,
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
    /// `(defining type, method name) -> function`. Methods supplied by an
    /// `impl` live in the same namespace and are found here too, so an `impl`
    /// introduces no second resolution path.
    pub methods: HashMap<(TyConId, String), FnId>,
    /// `[T]` has no type constructor of its own; its defining module is
    /// `core/list`.
    pub array_methods: HashMap<String, FnId>,
    pub ctx_decls: Vec<ContextDeclInfo>,
    prim_ids: HashMap<Prim, TyConId>,
}

impl Tables {
    pub fn tycon(&self, id: TyConId) -> &TyCon {
        &self.tycons[id.index()]
    }

    /// Whether this is `Option`, the one type the backend gives a
    /// representation of its own.
    ///
    /// Identified by shape as well as by name, so that a user type called
    /// `Option` — which the language permits, in another module — is not
    /// mistaken for it.
    pub fn is_option(&self, id: TyConId) -> bool {
        let t = self.tycon(id);
        t.name == "Option"
            && t.generics.len() == 1
            && matches!(&t.def, TyDef::Enum { variants }
                if variants.len() == 2
                    && variants[0].name == "Some"
                    && variants[0].fields.len() == 1
                    && variants[1].name == "None"
                    && variants[1].fields.is_empty())
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

    pub fn fun(&self, id: FnId) -> &FnInfo {
        &self.fns[id.index()]
    }

    pub fn trait_(&self, id: TraitId) -> &TraitInfo {
        &self.traits[id.index()]
    }

    pub fn const_(&self, id: ConstId) -> &ConstInfo {
        &self.consts[id.index()]
    }

    pub fn ctx_type(&self, id: CtxTypeId) -> &CtxType {
        &self.ctx_types[id.index()]
    }

    pub fn add_tycon(&mut self, c: TyCon) -> TyConId {
        let id = TyConId(self.tycons.len() as u32);
        self.tycons.push(c);
        id
    }

    pub fn add_fn(&mut self, f: FnInfo) -> FnId {
        let id = FnId(self.fns.len() as u32);
        self.fns.push(f);
        id
    }

    pub fn add_trait(&mut self, t: TraitInfo) -> TraitId {
        let id = TraitId(self.traits.len() as u32);
        self.traits.push(t);
        id
    }

    pub fn add_const(&mut self, c: ConstInfo) -> ConstId {
        let id = ConstId(self.consts.len() as u32);
        self.consts.push(c);
        id
    }

    pub fn ctx_decl(&self, id: ContextDeclId) -> &ContextDeclInfo {
        &self.ctx_decls[id.index()]
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
        self.prim_ids[&p]
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
    /// bound, or any type mentioning one — so a struct that stores a context
    /// is effect-carrying too (SPEC 10.2).
    pub fn is_effect_carrying(&self, ty: &Ty, generics: &[GenericInfo]) -> bool {
        match ty {
            Ty::Param(i) => generics
                .get(*i as usize)
                .is_some_and(|g| g.bounds.iter().any(|b| self.trait_(*b).is_effect)),
            Ty::Ctx(_) => true,
            Ty::Con(id, args) => {
                args.iter().any(|a| self.is_effect_carrying(a, generics))
                    || self.impls.keys().any(|(t, c)| *c == *id && self.trait_(*t).is_effect)
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
    /// Everything else is the shape of `is_effect_carrying`.
    pub fn may_carry_effect(&self, ty: &Ty, generics: &[GenericInfo]) -> bool {
        match ty {
            Ty::Param(i) => match generics.get(*i as usize) {
                Some(g) => !g.bounds.iter().any(|b| !self.trait_(*b).is_effect),
                None => true,
            },
            Ty::Ctx(_) => true,
            Ty::Con(id, args) => {
                args.iter().any(|a| self.may_carry_effect(a, generics))
                    || self.impls.keys().any(|(t, c)| *c == *id && self.trait_(*t).is_effect)
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
            self.classes[id.index()] = Some(class);
        }
        ty
    }

    pub fn class_of(&self, id: TyVarId) -> Option<NumClass> {
        self.classes[id.index()]
    }

    pub fn span_of(&self, id: TyVarId) -> Span {
        self.spans[id.index()]
    }

    pub fn var_count(&self) -> usize {
        self.slots.len()
    }

    pub fn get(&self, id: TyVarId) -> Option<&Ty> {
        self.slots[id.index()].as_ref()
    }

    fn set(&mut self, id: TyVarId, ty: Ty) {
        self.slots[id.index()] = Some(ty);
    }

    /// Follows bound variables one level at a time.
    pub fn shallow(&self, ty: &Ty) -> Ty {
        let mut cur = ty.clone();
        let mut guard = 0;
        while let Ty::Var(id) = cur {
            match self.get(id) {
                Some(next) => cur = next.clone(),
                None => break,
            }
            guard += 1;
            if guard > 1000 {
                return Ty::Error;
            }
        }
        cur
    }

    /// Applies the substitution everywhere.
    pub fn resolve(&self, ty: &Ty) -> Ty {
        match self.shallow(ty) {
            Ty::Con(id, args) => Ty::Con(id, args.iter().map(|a| self.resolve(a)).collect()),
            Ty::Array(e) => Ty::Array(Box::new(self.resolve(&e))),
            Ty::Tuple(es) => Ty::Tuple(es.iter().map(|e| self.resolve(e)).collect()),
            Ty::Fn(ps, r) => {
                Ty::Fn(ps.iter().map(|p| self.resolve(p)).collect(), Box::new(self.resolve(&r)))
            }
            other => other,
        }
    }

    fn occurs(&self, id: TyVarId, ty: &Ty) -> bool {
        match self.shallow(ty) {
            Ty::Var(v) => v == id,
            Ty::Con(_, args) => args.iter().any(|a| self.occurs(id, a)),
            Ty::Array(e) => self.occurs(id, &e),
            Ty::Tuple(es) => es.iter().any(|e| self.occurs(id, e)),
            Ty::Fn(ps, r) => ps.iter().any(|p| self.occurs(id, p)) || self.occurs(id, &r),
            _ => false,
        }
    }

    /// Structural unification. `Error` unifies with anything, so one mistake
    /// does not cascade into ten.
    pub fn unify(&mut self, tables: &Tables, a: &Ty, b: &Ty) -> Result<(), (Ty, Ty)> {
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
        if let Some(class) = self.classes[id.index()] {
            match self.shallow(ty) {
                Ty::Var(other) => {
                    if self.classes[other.index()].is_none() {
                        self.classes[other.index()] = Some(class);
                    } else if self.classes[other.index()] != Some(class) {
                        return Err((Ty::Var(id), ty.clone()));
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
        for i in 0..self.slots.len() {
            if self.slots[i].is_some() {
                continue;
            }
            if let Some(class) = self.classes[i] {
                let ty = match class {
                    NumClass::Int => tables.prim(Prim::I64),
                    NumClass::Float => tables.prim(Prim::F64),
                };
                self.slots[i] = Some(ty);
            }
        }
    }
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

fn write_ty(
    out: &mut String,
    tables: &Tables,
    subst: Option<&Subst>,
    generics: &[GenericInfo],
    ty: &Ty,
) {
    match ty {
        Ty::Var(id) => {
            match subst.and_then(|s| s.class_of(*id)) {
                Some(NumClass::Int) => out.push_str("{integer}"),
                Some(NumClass::Float) => out.push_str("{float}"),
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
    fn no_numeric_type_is_a_bigint() {
        // Every one of them compiles to a JavaScript `number`.
        assert!(Prim::all().iter().all(|p| !p.is_bigint()));
    }
}
