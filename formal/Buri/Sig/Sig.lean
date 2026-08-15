import Buri.Syntax.Ty

/-!
# Signatures

The nominal declarations -- structs, enums, primitives -- as a *parameter*
rather than as syntax. This mirrors Rust's `Tables` (`types.rs:453`), and the
field names are kept identical on purpose: it is what makes the Stage 4 JSON
bridge a transcription rather than a translation, and what lets a reviewer hold
both files open side by side.

Every judgement in this development is parameterised by a `Sig`. Nothing is
ever inferred from a declaration's *shape* -- conformance is nominal
(SPEC 5.12.1), so a lookup here is a lookup, never a search.
-/

namespace Buri

/-- `types.rs:240`. Only the type matters for exhaustiveness; the name is a
diagnostics concern. -/
structure FieldInfo where
  ty : Ty
  deriving Repr

/-- `types.rs:248`. -/
structure VariantInfo where
  fields : List FieldInfo
  deriving Repr

/-- `types.rs:259`. -/
inductive TyDef where
  | «struct» : List FieldInfo → TyDef
  | «enum» : List VariantInfo → TyDef
  | prim : Prim → TyDef
  deriving Repr

/-- `types.rs:267`. -/
structure TyCon where
  «def» : TyDef
  /-- How many generic parameters the declaration takes. -/
  arity : Nat
  deriving Repr

/-- `types.rs:453`, restricted to what the pattern theorems need. Traits,
impls, context types and function signatures join it in Stage 3. -/
structure Sig where
  tycons : List TyCon
  deriving Repr

/-- A lookup, never a search (SPEC 5.12.1). -/
def Sig.tycon (S : Sig) (c : TyConId) : Option TyCon :=
  S.tycons[c]?

/-!
## Generic instantiation

`types::substitute(&ty, &args, self_ty)` with `self_ty = None`. Buri has no
type-variable *binders* -- generics are instantiated positionally against
`Ty.param n` -- so this is a plain fold with no capture-avoidance obligation
and not a single lifting lemma. That is a gift from the language design and it
removes what is usually the most expensive part of a mechanisation.

An out-of-range parameter index is left alone rather than erroring; `WFSig`
below rules it out, and leaving it total keeps every later proof free of a
partiality side condition.
-/
def substitute (args : List Ty) : Ty → Ty
  | .param i => args.getD i (.param i)
  | .con c ts => .con c (ts.attach.map fun ⟨t, _⟩ => substitute args t)
  | .array e => .array (substitute args e)
  | .tuple ts => .tuple (ts.attach.map fun ⟨t, _⟩ => substitute args t)
  | .fn ps r => .fn (ps.attach.map fun ⟨p, _⟩ => substitute args p) (substitute args r)
  | .unit => .unit
  | .ctx k => .ctx k

/-- The field types of variant `i` of enum `c`, instantiated at `args`.
Mirrors the `Ctor::Variant` arm of `exhaust.rs:53 field_types`. -/
def Sig.variantFieldTys (S : Sig) (c : TyConId) (i : Nat) (args : List Ty) : List Ty :=
  match S.tycon c with
  | some ⟨.enum vs, _⟩ =>
      match vs[i]? with
      | some v => v.fields.map fun f => substitute args f.ty
      | none => []
  | _ => []

/-- The field types of struct `c`, instantiated at `args`. Mirrors the
`Ctor::Single` arm of `exhaust.rs:53 field_types`. -/
def Sig.structFieldTys (S : Sig) (c : TyConId) (args : List Ty) : List Ty :=
  match S.tycon c with
  | some ⟨.struct fs, _⟩ => fs.map fun f => substitute args f.ty
  | _ => []

/-- How many variants enum `c` has; `0` for anything else. -/
def Sig.variantCount (S : Sig) (c : TyConId) : Nat :=
  match S.tycon c with
  | some ⟨.enum vs, _⟩ => vs.length
  | _ => 0

/-- How many fields variant `i` of enum `c` has. This is `Ctor::arity`'s
`Ctor::Variant` arm (`exhaust.rs:38`), and it is deliberately independent of
the instantiation: arity is a property of the declaration. -/
def Sig.variantArity (S : Sig) (c : TyConId) (i : Nat) : Nat :=
  match S.tycon c with
  | some ⟨.enum vs, _⟩ => (vs[i]?).elim 0 (·.fields.length)
  | _ => 0

/-- How many fields struct `c` has; `Ctor::arity`'s `Ctor::Single` arm. -/
def Sig.structArity (S : Sig) (c : TyConId) : Nat :=
  match S.tycon c with
  | some ⟨.struct fs, _⟩ => fs.length
  | _ => 0

def Sig.isStruct (S : Sig) (c : TyConId) : Bool :=
  match S.tycon c with
  | some ⟨.struct _, _⟩ => true
  | _ => false

def Sig.isEnum (S : Sig) (c : TyConId) : Bool :=
  match S.tycon c with
  | some ⟨.enum _, _⟩ => true
  | _ => false

/-- The primitive a type constructor names, if it names one. -/
def Sig.primOf (S : Sig) (c : TyConId) : Option Prim :=
  match S.tycon c with
  | some ⟨.prim p, _⟩ => some p
  | _ => none

/-! A type constructor is a struct, an enum, or a primitive -- never two of
them. These three lemmas are what make the canonical-forms inversions in
`Dynamics/Value.lean` one line each instead of a case explosion. -/

theorem Sig.isEnum_isStruct {S : Sig} {c} (h : S.isEnum c = true) : S.isStruct c = false := by
  unfold Sig.isEnum at h; unfold Sig.isStruct
  split at h
  · next heq => simp [heq]
  · simp at h

theorem Sig.isEnum_primOf {S : Sig} {c} (h : S.isEnum c = true) : S.primOf c = none := by
  unfold Sig.isEnum at h; unfold Sig.primOf
  split at h
  · next heq => simp [heq]
  · simp at h

theorem Sig.isStruct_primOf {S : Sig} {c} (h : S.isStruct c = true) : S.primOf c = none := by
  unfold Sig.isStruct at h; unfold Sig.primOf
  split at h
  · next heq => simp [heq]
  · simp at h

theorem Sig.isStruct_isEnum {S : Sig} {c} (h : S.isStruct c = true) : S.isEnum c = false := by
  unfold Sig.isStruct at h; unfold Sig.isEnum
  split at h
  · next heq => simp [heq]
  · simp at h

/-- A primitive has no variants, so `variantCount` is `0` -- which is what
makes the `variant` typing rule's `i < variantCount c` premise unsatisfiable
at a primitive type. -/
theorem Sig.primOf_variantCount {S : Sig} {c p} (h : S.primOf c = some p) :
    S.variantCount c = 0 := by
  unfold Sig.primOf at h; unfold Sig.variantCount
  split at h
  · next heq => simp [heq]
  · simp at h

theorem Sig.isStruct_variantCount {S : Sig} {c} (h : S.isStruct c = true) :
    S.variantCount c = 0 := by
  unfold Sig.isStruct at h; unfold Sig.variantCount
  split at h
  · next heq => simp [heq]
  · simp at h

theorem Sig.variantFieldTys_length (S : Sig) (c : TyConId) (i : Nat) (args : List Ty) :
    (S.variantFieldTys c i args).length = S.variantArity c i := by
  unfold Sig.variantFieldTys Sig.variantArity
  cases h : S.tycon c with
  | none => simp
  | some tc =>
    cases tc with
    | mk d ar =>
      cases d with
      | «enum» vs => cases hv : vs[i]? <;> simp [hv]
      | «struct» _ => simp
      | prim _ => simp

theorem Sig.structFieldTys_length (S : Sig) (c : TyConId) (args : List Ty) :
    (S.structFieldTys c args).length = S.structArity c := by
  unfold Sig.structFieldTys Sig.structArity
  cases h : S.tycon c with
  | none => simp
  | some tc =>
    cases tc with
    | mk d ar => cases d <;> simp

end Buri
