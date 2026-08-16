import Buri.Signature
import Buri.Syntax.Constructor
import Buri.Util.Forall2

/-!
# Values and value typing

`Value` is shaped to line up with `exhaust.rs`'s `Constructor` (`exhaust.rs:21`) one
constructor at a time, because every theorem about the pattern algorithm
relates the two. In particular structs, tuples and unit share a single
`Value.single`, exactly as they share `Constructor::Single` -- each has exactly one
constructor, and the algorithm never needs to tell them apart.

`Value.opaqueValue` stands for values of function, context, and type-parameter type. No
pattern can inspect one, so its internal structure is irrelevant here; what
matters is that such values exist and that `allConstructors` reports *no* enumerable
constructor set for their types. (`opaque` is a Lean keyword, hence the
abbreviation.)
-/

namespace Buri

/-- A runtime value, at the granularity the pattern algorithm sees. -/
inductive Value where
  /-- Variant `i` of enum `c`, with its payload. -/
  | variant : TypeConstructorId → Nat → List Value → Value
  /-- A struct, a tuple, or unit: the types with exactly one constructor. -/
  | single : List Value → Value
  | bool : Bool → Value
  | array : List Value → Value
  /-- An integer, float, string, or char, rendered the way `exhaust.rs:31`
  renders it. The set is too large to enumerate, which is the whole reason
  `Constructor::Lit` exists and why completeness is conditional (SPEC 7.3). -/
  | lit : String → Value
  /-- A function, a context, or a value at a type parameter. -/
  | opaqueValue : Value
  deriving Repr

/-- The `List Value` companion motive, for the same reason `Ty.ind'` exists. -/
@[elab_as_elim]
theorem Value.ind' {motive : Value → Prop}
    (variant : ∀ c i vs, (∀ v ∈ vs, motive v) → motive (.variant c i vs))
    (single : ∀ vs, (∀ v ∈ vs, motive v) → motive (.single vs))
    (bool : ∀ b, motive (.bool b))
    (array : ∀ vs, (∀ v ∈ vs, motive v) → motive (.array vs))
    (lit : ∀ s, motive (.lit s))
    (opaqueValue : motive .opaqueValue)
    : ∀ v, motive v :=
  fun v =>
    Value.rec (motive_1 := motive) (motive_2 := fun vs => ∀ v ∈ vs, motive v)
      variant single bool array lit opaqueValue
      (fun _ h => absurd h List.not_mem_nil)
      (fun _ _ ihHd ihTl a ha => by
        rcases List.mem_cons.mp ha with rfl | h'
        · exact ihHd
        · exact ihTl a h')
      v

/-- The constructor a value is built with, or `none` for the values no pattern
can inspect. This is the projection that makes `specialize` correct: a row
survives specialisation on `target` exactly when the scrutinee's `constructorOf` is `target`.

An array's constructor records its *length*, which is what makes
`Constructor.array n` a fixed-length test -- and, ultimately, what makes the
array-length bound of `exhaustiveness.rs` necessary. -/
def Value.constructorOf : Value → Option Constructor
  | .variant c i _ => some (.variant c i)
  | .single _ => some .single
  | .bool b => some (.bool b)
  | .array vs => some (.array vs.length)
  | .lit s => some (.lit s)
  | .opaqueValue => none

/-- The sub-values a constructor holds. -/
def Value.fields : Value → List Value
  | .variant _ _ vs => vs
  | .single vs => vs
  | .bool _ => []
  | .array vs => vs
  | .lit _ => []
  | .opaqueValue => []

/-!
## Typing

Nested through `Forall₂` rather than spelled as a mutual inductive: mutual
inductives are what the `induction` tactic refuses, and every list lemma is
then unavailable exactly where it is needed most.
-/

inductive HasType (S : Signature) : Value → Ty → Prop where
  | variant {c i args vs} :
      i < S.variantCount c →
      Forall₂ (HasType S) vs (S.variantFieldTypes c i args) →
      HasType S (.variant c i vs) (.con c args)
  | «struct» {c args vs} :
      S.isStruct c = true →
      Forall₂ (HasType S) vs (S.structFieldTypes c args) →
      HasType S (.single vs) (.con c args)
  | tuple {vs ts} :
      Forall₂ (HasType S) vs ts →
      HasType S (.single vs) (.tuple ts)
  | unit :
      HasType S (.single []) .unit
  | bool {c b} :
      S.primitiveOf c = some .bool →
      HasType S (.bool b) (.con c [])
  | lit {c p s} :
      S.primitiveOf c = some p → p ≠ .bool →
      HasType S (.lit s) (.con c [])
  | array {vs e} :
      Forall₂ (HasType S) vs (List.replicate vs.length e) →
      HasType S (.array vs) (.array e)
  | fnv {ps r} : HasType S .opaqueValue (.fn ps r)
  | ctxv {k} : HasType S .opaqueValue (.ctx k)
  | paramv {n} : HasType S .opaqueValue (.param n)

notation:50 S " ⊢ᵥ " v " : " t => HasType S v t

/-! ## Canonical forms

Which values inhabit which types. These are the inversion lemmas the
exhaustiveness proof runs on: knowing a value has type `.con c args` where `c`
is an enum tells you it *is* a variant, which is what lets "every constructor
is covered" become "every value is matched". -/

theorem HasType.enum_inversion {S : Signature} {c args v} (hen : S.isEnum c = true) :
    (S ⊢ᵥ v : .con c args) →
    ∃ i vs, v = .variant c i vs ∧ i < S.variantCount c ∧
            Forall₂ (HasType S) vs (S.variantFieldTypes c i args) := by
  intro h
  cases h with
  | variant hlt hf => exact ⟨_, _, rfl, hlt, hf⟩
  | «struct» hst _ => simp [Signature.isEnum_isStruct hen] at hst
  | bool hp => simp [Signature.isEnum_primOf hen] at hp
  | lit hp _ => simp [Signature.isEnum_primOf hen] at hp

theorem HasType.struct_inversion {S : Signature} {c args v} (hst : S.isStruct c = true) :
    (S ⊢ᵥ v : .con c args) →
    ∃ vs, v = .single vs ∧ Forall₂ (HasType S) vs (S.structFieldTypes c args) := by
  intro h
  cases h with
  | «struct» _ hf => exact ⟨_, rfl, hf⟩
  | variant hlt _ => simp [Signature.isStruct_variantCount hst] at hlt
  | bool hp => simp [Signature.isStruct_primOf hst] at hp
  | lit hp _ => simp [Signature.isStruct_primOf hst] at hp

theorem HasType.bool_inversion {S : Signature} {c args v} (hp : S.primitiveOf c = some .bool) :
    (S ⊢ᵥ v : .con c args) → ∃ b, v = .bool b := by
  intro h
  cases h with
  | bool _ => exact ⟨_, rfl⟩
  | lit hp' nonEmpty => rw [hp] at hp'; exact absurd (Option.some.inj hp').symm nonEmpty
  | variant hlt _ => simp [Signature.primOf_variantCount hp] at hlt
  | «struct» hst _ => rw [Signature.isStruct_primOf hst] at hp; simp at hp

/-- A primitive that is not `Bool` -- an integer, float, string, or char. These
are the types `allConstructors` reports `none` for: their constructor sets are too
large to enumerate, so a match over them needs a `_` arm (SPEC 7.3). This is
the sole source of the conditional in `useful_complete`. -/
theorem HasType.literal_inversion {S : Signature} {c args v p}
    (hp : S.primitiveOf c = some p) (nonEmpty : p ≠ .bool) :
    (S ⊢ᵥ v : .con c args) → ∃ s, v = .lit s := by
  intro h
  cases h with
  | lit _ _ => exact ⟨_, rfl⟩
  | bool hp' => rw [hp] at hp'; exact absurd (Option.some.inj hp') nonEmpty
  | variant hlt _ => simp [Signature.primOf_variantCount hp] at hlt
  | «struct» hst _ => rw [Signature.isStruct_primOf hst] at hp; simp at hp

theorem HasType.tuple_inversion {S : Signature} {ts v} :
    (S ⊢ᵥ v : .tuple ts) → ∃ vs, v = .single vs ∧ Forall₂ (HasType S) vs ts := by
  intro h; cases h with | tuple hf => exact ⟨_, rfl, hf⟩

theorem HasType.unit_inversion {S : Signature} {v} : (S ⊢ᵥ v : .unit) → v = .single [] := by
  intro h; cases h with | unit => rfl

theorem HasType.array_inversion {S : Signature} {e v} :
    (S ⊢ᵥ v : .array e) → ∃ vs, v = .array vs ∧ ∀ w ∈ vs, S ⊢ᵥ w : e := by
  intro h; cases h with | array hf => exact ⟨_, rfl, Forall₂.of_replicate hf⟩

end Buri
