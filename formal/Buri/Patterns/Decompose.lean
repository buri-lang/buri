import Buri.Patterns.Matrix

/-!
# Decomposing a well-typed value

The bridge between `HasType` and the pattern algorithm. Three facts, and the
completeness proof is essentially these three plus an induction:

1. a well-typed value's fields are well-typed at its constructor's field types;
2. hence it has exactly `arity` of them; and
3. its constructor is one of the ones `allConstructors` enumerates.

The third is where arrays become interesting, and it is the reason
`Value.Bounded` exists.

## Why arrays need a bound

`allConstructors` reports an array type's constructors as `array 0 .. array limit`
(`exhaustiveness.rs`, the `Ty::Array` arm). A value `array vs` has constructor
`array vs.length`, so fact 3 is simply *false* for an array longer than
`limit` -- such a value's constructor is not in the enumerated set at all.

That is not a bug in the algorithm; it is the algorithm reasoning inside a
*bounded universe*. It is sound there because `expand_lengths` has already
rewritten every rest pattern into fixed lengths up to `limit`, and because
`limit` is one more than the longest length any pattern mentions, so no
pattern can tell an array of length `limit` from a longer one. `Bounded` names
the universe; `Buri/Patterns/Expand.lean` is where the restriction is
discharged and the theorem lifted to all values.
-/

namespace Buri

/-! Every array reachable from a value is no longer than `limit`. -/
mutual

/-- Every array reachable from this value is no longer than `limit`. -/
def Value.Bounded (limit : Nat) : Value → Prop
  | .variant _ _ vs => Value.BoundedList limit vs
  | .single vs => Value.BoundedList limit vs
  | .bool _ => True
  | .array vs => vs.length ≤ limit ∧ Value.BoundedList limit vs
  | .lit _ => True
  | .opaqueValue => True

def Value.BoundedList (limit : Nat) : List Value → Prop
  | [] => True
  | v :: vs => Value.Bounded limit v ∧ Value.BoundedList limit vs

end

theorem Value.BoundedList_iff {limit : Nat} {vs : List Value} :
    Value.BoundedList limit vs ↔ ∀ v ∈ vs, Value.Bounded limit v := by
  induction vs with
  | nil => simp [Value.BoundedList]
  | cons v vs ih =>
    simp only [Value.BoundedList, ih]
    constructor
    · rintro ⟨hv, hvs⟩ w hw
      rcases List.mem_cons.mp hw with rfl | h'
      · exact hv
      · exact hvs w h'
    · intro h
      exact ⟨h v (by simp), fun w hw => h w (by simp [hw])⟩

/-- Boundedness passes to a value's fields, which is what keeps the induction
in `useful_complete` going. -/
theorem Value.Bounded.fields {limit : Nat} {v : Value}
    (h : Value.Bounded limit v) : ∀ w ∈ v.fields, Value.Bounded limit w := by
  cases v with
  | variant c i vs => exact Value.BoundedList_iff.mp h
  | single vs => exact Value.BoundedList_iff.mp h
  | array vs => exact Value.BoundedList_iff.mp h.2
  | bool _ => intro w hw; simp [Value.fields] at hw
  | lit _ => intro w hw; simp [Value.fields] at hw
  | opaqueValue => intro w hw; simp [Value.fields] at hw

/-! ## Fields are well-typed -/

/-- A well-typed value's fields are well-typed at its constructor's field
types. This is what lets `specialize` replace a column by `arity` columns
without losing the typing invariant. -/
theorem HasType.fields_typed {S : Signature} {t : Ty} {v : Value} {target : Constructor}
    (hv : S ⊢ᵥ v : t) (hasTarget : v.constructorOf = some target) :
    Forall₂ (HasType S) v.fields (target.fieldTypes S t) := by
  cases hv <;>
    simp only [Value.constructorOf, Option.some.injEq, reduceCtorEq] at hasTarget <;> subst hasTarget <;>
    first
      | exact .nil
      | simpa [Value.fields, Constructor.fieldTypes, Ty.typeArguments, Ty.elementType] using
          ‹Forall₂ _ _ _›

theorem HasType.fields_length {S : Signature} {t : Ty} {v : Value} {target : Constructor}
    (hv : S ⊢ᵥ v : t) (hasTarget : v.constructorOf = some target) :
    v.fields.length = target.arity S t := by
  have := (HasType.fields_typed hv hasTarget).length_eq
  rwa [Constructor.fieldTypes_length] at this

/-! ## The constructor set really is complete -/

/-- When `allConstructors` reports a constructor set, every well-typed value of that
type -- inside the bounded universe -- is built with one of them.

This is the lemma that turns "the matrix mentions every constructor" into
"the matrix matches every value", and it is the heart of exhaustiveness. -/
theorem HasType.constructorOf_mem {S : Signature} {limit : Nat} {t : Ty} {v : Value} {all : List Constructor}
    (hv : S ⊢ᵥ v : t) (hall : allConstructors S limit t = some all)
    (hb : Value.Bounded limit v) :
    ∃ target, v.constructorOf = some target ∧ target ∈ all := by
  cases t with
  | con c args =>
    simp only [allConstructors] at hall
    split at hall
    · -- enum
      next vs ar heq =>
        have hen : S.isEnum c = true := by simp [Signature.isEnum, heq]
        obtain ⟨i, ws, rfl, hlt, _⟩ := HasType.enum_inversion hen hv
        refine ⟨.variant c i, rfl, ?_⟩
        cases hall
        have : i < vs.length := by
          simpa [Signature.variantCount, heq] using hlt
        exact List.mem_map.mpr ⟨i, List.mem_range.mpr this, rfl⟩
    · -- struct
      next fs ar heq =>
        have hst : S.isStruct c = true := by simp [Signature.isStruct, heq]
        obtain ⟨ws, rfl, _⟩ := HasType.struct_inversion hst hv
        cases hall
        exact ⟨.single, rfl, by simp⟩
    · -- prim bool
      next ar heq =>
        have hp : S.primitiveOf c = some .bool := by simp [Signature.primitiveOf, heq]
        obtain ⟨b, rfl⟩ := HasType.bool_inversion hp hv
        cases hall
        cases b
        · exact ⟨.bool false, rfl, by simp⟩
        · exact ⟨.bool true, rfl, by simp⟩
    · simp at hall
    · simp at hall
  | tuple ts =>
    simp only [allConstructors, Option.some.injEq] at hall
    obtain ⟨ws, rfl, _⟩ := HasType.tuple_inversion hv
    exact ⟨.single, rfl, by simp [← hall]⟩
  | unit =>
    simp only [allConstructors, Option.some.injEq] at hall
    have := HasType.unit_inversion hv
    subst this
    exact ⟨.single, rfl, by simp [← hall]⟩
  | array e =>
    simp only [allConstructors, Option.some.injEq] at hall
    subst hall
    obtain ⟨ws, rfl, _⟩ := HasType.array_inversion hv
    refine ⟨.array ws.length, rfl, ?_⟩
    have hle : ws.length ≤ limit := by
      simpa [Value.Bounded] using hb.1
    exact List.mem_map.mpr ⟨ws.length, List.mem_range.mpr (by omega), rfl⟩
  | fn _ _ => simp [allConstructors] at hall
  | ctx _ => simp [allConstructors] at hall
  | param _ => simp [allConstructors] at hall

end Buri
