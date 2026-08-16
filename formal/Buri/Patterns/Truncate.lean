import Buri.Patterns.Complete

/-!
# Long arrays have short representatives

The algorithm reasons inside a universe where arrays are no longer than
`limit` (`Decompose.lean`). This file is why that loses nothing.

`limit` is chosen as one more than the longest array length any pattern in the
match mentions (`exhaustiveness.rs`, the `+ 1` in `check`). So **no pattern can
tell an array of length `limit` from a longer one**: a fixed-length test
`array k` has `k < limit` and fails on both, and a rest test `arrayRest n` has
`n < limit` and succeeds on both, inspecting only the first `n` elements, which
truncation preserves.

`Value.truncate` cuts every array in a value down to `limit` elements,
hereditarily. `Pattern.matches_truncate` is the statement that patterns cannot
see the difference.

That `+ 1` is doing real work. Without it a match on `[]`, `[_]`, `[_, _]`
would have `limit = 2`, `allConstructors` would report `array 0, array 1,
array 2`, all three would be present, the algorithm would call the set complete
-- and length-3 arrays, which match nothing, would slip through.
-/

namespace Buri

/-! ## The longest array length a pattern mentions -/

mutual

/-- `length_limit` from `exhaustiveness.rs`. -/
def Pattern.lengthLimit : Pattern → Nat
  | .wildcard => 0
  | .or alternatives => Pattern.lengthLimitList alternatives
  | .constructor (.array n) subpatterns => max n (Pattern.lengthLimitList subpatterns)
  | .constructor (.arrayRest n) subpatterns => max n (Pattern.lengthLimitList subpatterns)
  | .constructor _ subpatterns => Pattern.lengthLimitList subpatterns

def Pattern.lengthLimitList : List Pattern → Nat
  | [] => 0
  | p :: ps => max (Pattern.lengthLimit p) (Pattern.lengthLimitList ps)

end

theorem Pattern.lengthLimit_le_of_mem {p : Pattern} {ps : List Pattern} (h : p ∈ ps) :
    Pattern.lengthLimit p ≤ Pattern.lengthLimitList ps := by
  induction ps with
  | nil => exact absurd h List.not_mem_nil
  | cons q qs ih =>
    rcases List.mem_cons.mp h with rfl | h'
    · simp only [Pattern.lengthLimitList]; omega
    · have := ih h'
      simp only [Pattern.lengthLimitList]
      omega

/-- Sub-patterns never mention a longer array than their parent. -/
theorem Pattern.lengthLimitList_le_constructor (head : Constructor) (subpatterns : List Pattern) :
    Pattern.lengthLimitList subpatterns
      ≤ Pattern.lengthLimit (.constructor head subpatterns) := by
  cases head <;> simp only [Pattern.lengthLimit] <;> omega

/-- The array length a constructor pattern tests is below its own limit's
successor -- i.e. strictly below any `bound` exceeding the limit. -/
theorem Pattern.array_lt_of_lengthLimit {n bound : Nat} {subpatterns : List Pattern}
    (h : Pattern.lengthLimit (.constructor (.array n) subpatterns) < bound) : n < bound := by
  simp only [Pattern.lengthLimit] at h; omega

theorem Pattern.arrayRest_lt_of_lengthLimit {n bound : Nat} {subpatterns : List Pattern}
    (h : Pattern.lengthLimit (.constructor (.arrayRest n) subpatterns) < bound) : n < bound := by
  simp only [Pattern.lengthLimit] at h; omega

/-! ## Truncation -/

mutual

/-- Cut every array in the value down to `bound` elements, hereditarily. -/
def Value.truncate (bound : Nat) : Value → Value
  | .variant c i vs => .variant c i (Value.truncateList bound vs)
  | .single vs => .single (Value.truncateList bound vs)
  | .bool b => .bool b
  | .array vs => .array ((Value.truncateList bound vs).take bound)
  | .lit s => .lit s
  | .opaqueValue => .opaqueValue

def Value.truncateList (bound : Nat) : List Value → List Value
  | [] => []
  | v :: vs => Value.truncate bound v :: Value.truncateList bound vs

end

@[simp] theorem Value.truncate_variant (bound c i vs) :
    Value.truncate bound (.variant c i vs) = .variant c i (Value.truncateList bound vs) := rfl
@[simp] theorem Value.truncate_single (bound vs) :
    Value.truncate bound (.single vs) = .single (Value.truncateList bound vs) := rfl
@[simp] theorem Value.truncate_bool (bound b) :
    Value.truncate bound (.bool b) = .bool b := rfl
@[simp] theorem Value.truncate_array (bound vs) :
    Value.truncate bound (.array vs) = .array ((Value.truncateList bound vs).take bound) := rfl
@[simp] theorem Value.truncate_lit (bound s) :
    Value.truncate bound (.lit s) = .lit s := rfl
@[simp] theorem Value.truncate_opaque (bound) :
    Value.truncate bound .opaqueValue = .opaqueValue := rfl

@[simp] theorem Value.Bounded_variant (bound c i vs) :
    Value.Bounded bound (.variant c i vs) = Value.BoundedList bound vs := rfl
@[simp] theorem Value.Bounded_single (bound vs) :
    Value.Bounded bound (.single vs) = Value.BoundedList bound vs := rfl
@[simp] theorem Value.Bounded_array (bound vs) :
    Value.Bounded bound (.array vs) = (vs.length ≤ bound ∧ Value.BoundedList bound vs) := rfl

@[simp] theorem Value.truncateList_length (bound : Nat) (vs : List Value) :
    (Value.truncateList bound vs).length = vs.length := by
  induction vs with
  | nil => rfl
  | cons v vs ih => simp [Value.truncateList, ih]

theorem Value.truncateList_eq_map (bound : Nat) (vs : List Value) :
    Value.truncateList bound vs = vs.map (Value.truncate bound) := by
  induction vs with
  | nil => rfl
  | cons v vs ih => simp [Value.truncateList, ih]

theorem Value.truncateList_take (bound n : Nat) (vs : List Value) :
    (Value.truncateList bound vs).take n = Value.truncateList bound (vs.take n) := by
  rw [Value.truncateList_eq_map, Value.truncateList_eq_map, List.map_take]

theorem Value.truncateList_mem {bound : Nat} {vs : List Value} {w : Value}
    (h : w ∈ Value.truncateList bound vs) : ∃ v ∈ vs, w = Value.truncate bound v := by
  rw [Value.truncateList_eq_map] at h
  obtain ⟨v, hv, rfl⟩ := List.mem_map.mp h
  exact ⟨v, hv, rfl⟩

/-! ## Truncated values are bounded -/

theorem Value.truncate_bounded (bound : Nat) :
    ∀ v : Value, Value.Bounded bound (Value.truncate bound v) := by
  intro v
  induction v using Value.ind' with
  | variant c i vs ih =>
    simp only [Value.truncate_variant, Value.Bounded_variant]
    refine Value.BoundedList_iff.mpr fun w hw => ?_
    obtain ⟨u, hu, rfl⟩ := Value.truncateList_mem hw
    exact ih u hu
  | single vs ih =>
    simp only [Value.truncate_single, Value.Bounded_single]
    refine Value.BoundedList_iff.mpr fun w hw => ?_
    obtain ⟨u, hu, rfl⟩ := Value.truncateList_mem hw
    exact ih u hu
  | array vs ih =>
    simp only [Value.truncate_array, Value.Bounded_array]
    refine ⟨by simp only [List.length_take, Value.truncateList_length]; omega, ?_⟩
    refine Value.BoundedList_iff.mpr fun w hw => ?_
    obtain ⟨u, hu, rfl⟩ := Value.truncateList_mem (List.mem_of_mem_take hw)
    exact ih u hu
  | bool b => trivial
  | lit s => trivial
  | opaqueValue => trivial

/-! ## Truncation preserves typing -/

/-- The list version, stated separately so the value cases stay one line each. -/
theorem Forall₂.truncate {signature : Signature} {bound : Nat} {vs : List Value} {ts : List Ty}
    (h : Forall₂ (HasType signature) vs ts)
    (ih : ∀ v ∈ vs, ∀ t, (signature ⊢ᵥ v : t) → (signature ⊢ᵥ Value.truncate bound v : t)) :
    Forall₂ (HasType signature) (Value.truncateList bound vs) ts := by
  induction h with
  | nil => exact .nil
  | @cons a b as bs hvt _ ihf =>
    exact .cons (ih a (by simp) b hvt) (ihf fun w hw => ih w (by simp [hw]))

theorem HasType.truncate {signature : Signature} (bound : Nat) :
    ∀ (v : Value) (t : Ty), (signature ⊢ᵥ v : t) →
      (signature ⊢ᵥ Value.truncate bound v : t) := by
  intro v
  induction v using Value.ind' with
  | variant c i vs ih =>
    intro t hv
    cases hv with
    | variant hlt hf => exact .variant hlt (Forall₂.truncate hf ih)
  | single vs ih =>
    intro t hv
    cases hv with
    | «struct» hst hf => exact .struct hst (Forall₂.truncate hf ih)
    | tuple hf => exact .tuple (Forall₂.truncate hf ih)
    | unit => exact .unit
  | array vs ih =>
    intro t hv
    cases hv with
    | array hf =>
      refine .array (Forall₂.to_replicate fun w hw => ?_)
      obtain ⟨u, hu, rfl⟩ := Value.truncateList_mem (List.mem_of_mem_take hw)
      exact ih u hu _ (Forall₂.of_replicate hf u hu)
  | bool b => intro t hv; cases hv with | bool hp => exact .bool hp
  | lit s => intro t hv; cases hv with | lit hp hne => exact .lit hp hne
  | opaqueValue =>
    intro t hv
    cases hv with
    | fnv => exact .fnv
    | ctxv => exact .ctxv
    | paramv => exact .paramv

/-! ## Patterns cannot see the truncation

The representative lemma. Everything above is scaffolding for it. -/

theorem Value.constructorOf_truncate_of_not_array {bound : Nat} {v : Value}
    (h : ∀ vs, v ≠ .array vs) :
    (Value.truncate bound v).constructorOf = v.constructorOf := by
  cases v with
  | array vs => exact absurd rfl (h vs)
  | _ => rfl

theorem Value.fields_truncate_of_not_array {bound : Nat} {v : Value}
    (h : ∀ vs, v ≠ .array vs) :
    (Value.truncate bound v).fields = Value.truncateList bound v.fields := by
  cases v with
  | array vs => exact absurd rfl (h vs)
  | variant c i vs => rfl
  | single vs => rfl
  | bool b => rfl
  | lit s => rfl
  | opaqueValue => rfl

theorem Pattern.matchesAll_truncateList (bound : Nat) (subpatterns : List Pattern) :
    ∀ (ws : List Value),
      (∀ p ∈ subpatterns, ∀ w, Pattern.matches p (Value.truncate bound w) = Pattern.matches p w) →
      Pattern.matchesAll subpatterns (Value.truncateList bound ws)
        = Pattern.matchesAll subpatterns ws := by
  induction subpatterns with
  | nil => intro ws _; simp
  | cons p ps ih =>
    intro ws hih
    cases ws with
    | nil => rfl
    | cons w ws =>
      show (Pattern.matches p (Value.truncate bound w) &&
            Pattern.matchesAll ps (Value.truncateList bound ws)) = _
      rw [hih p (by simp) w, ih ws (fun q hq => hih q (by simp [hq]))]
      rfl

theorem Pattern.matchesAny_truncate (bound : Nat) (alternatives : List Pattern) (v : Value)
    (hih : ∀ p ∈ alternatives, Pattern.matches p (Value.truncate bound v)
              = Pattern.matches p v) :
    Pattern.matchesAny alternatives (Value.truncate bound v)
      = Pattern.matchesAny alternatives v := by
  induction alternatives with
  | nil => rfl
  | cons q qs ih =>
    show (Pattern.matches q (Value.truncate bound v) ||
          Pattern.matchesAny qs (Value.truncate bound v)) = _
    rw [hih q (by simp), ih (fun r hr => hih r (by simp [hr]))]
    rfl

/-- Truncation is invisible when the scrutinee is not an array: the
constructor is unchanged and the fields are truncated pointwise. -/
private theorem matches_truncate_of_value_not_array {bound : Nat} {head : Constructor}
    {subpatterns : List Pattern} {v : Value}
    (hrest : ∀ n, head ≠ .arrayRest n)
    (hv : ∀ vs, v ≠ .array vs)
    (hsubs : ∀ q ∈ subpatterns, ∀ w,
      Pattern.matches q (Value.truncate bound w) = Pattern.matches q w) :
    Pattern.matches (.constructor head subpatterns) (Value.truncate bound v)
      = Pattern.matches (.constructor head subpatterns) v := by
  rw [Pattern.matches_constructor _ _ _ hrest, Pattern.matches_constructor _ _ _ hrest,
      Value.constructorOf_truncate_of_not_array hv,
      Value.fields_truncate_of_not_array hv,
      Pattern.matchesAll_truncateList bound subpatterns _ hsubs]

/-- Truncation is invisible when the scrutinee *is* an array but the pattern
tests some other constructor: both sides simply fail. -/
private theorem matches_truncate_of_head_not_array {bound : Nat} {head : Constructor}
    {subpatterns : List Pattern} {vs : List Value}
    (hrest : ∀ n, head ≠ .arrayRest n)
    (hhead : ∀ n, head ≠ .array n) :
    Pattern.matches (.constructor head subpatterns) (Value.truncate bound (.array vs))
      = Pattern.matches (.constructor head subpatterns) (.array vs) := by
  rw [Pattern.matches_constructor _ _ _ hrest, Pattern.matches_constructor _ _ _ hrest]
  simp only [Value.truncate_array, Value.constructorOf]
  cases head with
  | array n => exact absurd rfl (hhead n)
  | arrayRest n => exact absurd rfl (hrest n)
  | _ => simp

/-- **The representative lemma.** A pattern that mentions no array length at or
above `bound` matches a value exactly when it matches that value's truncation.

This is what licenses the algorithm's bounded universe: the `+ 1` in
`exhaustiveness.rs` makes `bound` exceed every length any pattern tests, so
truncation is invisible. -/
theorem Pattern.matches_truncate (bound : Nat) :
    ∀ (p : Pattern) (v : Value), p.lengthLimit < bound →
      Pattern.matches p (Value.truncate bound v) = Pattern.matches p v := by
  intro p
  induction p using Pattern.ind' with
  | wildcard => intro v _; simp
  | or alternatives ihs =>
    intro v hlen
    refine Pattern.matchesAny_truncate bound alternatives v (fun q hq => ?_)
    exact ihs q hq v (by
      have := Pattern.lengthLimit_le_of_mem hq
      simp only [Pattern.lengthLimit] at hlen
      omega)
  | constructor head subpatterns ihs =>
    intro v hlen
    have hsubs : ∀ q ∈ subpatterns, ∀ w,
        Pattern.matches q (Value.truncate bound w) = Pattern.matches q w := by
      intro q hq w
      refine ihs q hq w ?_
      have h₁ := Pattern.lengthLimit_le_of_mem hq
      have h₂ := Pattern.lengthLimitList_le_constructor head subpatterns
      omega
    cases head with
    | arrayRest n =>
      have hn : n < bound := Pattern.arrayRest_lt_of_lengthLimit hlen
      cases v with
      | array vs =>
        show (decide (n ≤ _) && Pattern.matchesAll subpatterns (List.take n _))
          = (decide (n ≤ vs.length) && Pattern.matchesAll subpatterns (vs.take n))
        have htake : (((Value.truncateList bound vs).take bound).take n)
            = Value.truncateList bound (vs.take n) := by
          rw [List.take_take, Nat.min_eq_left (by omega : n ≤ bound),
              Value.truncateList_take]
        have hiff : (n ≤ ((Value.truncateList bound vs).take bound).length)
            ↔ (n ≤ vs.length) := by
          simp only [List.length_take, Value.truncateList_length]
          omega
        rw [htake, Pattern.matchesAll_truncateList bound subpatterns (vs.take n) hsubs]
        congr 1
        first
          | omega
          | (simp only [decide_eq_decide, List.length_take, Value.truncateList_length]; omega)
      | _ => rfl
    | array k =>
      have hk : k < bound := Pattern.array_lt_of_lengthLimit hlen
      cases v with
      | array vs =>
        rw [Pattern.matches_constructor _ _ _ (by intro n; simp),
            Pattern.matches_constructor _ _ _ (by intro n; simp)]
        simp only [Value.truncate_array, Value.constructorOf, Value.fields,
          List.length_take, Value.truncateList_length]
        by_cases hvk : vs.length = k
        · -- Lengths agree, and `k < bound`, so the truncation is the identity.
          have hshort : (Value.truncateList bound vs).take bound
              = Value.truncateList bound vs := by
            refine List.take_of_length_le ?_
            simp only [Value.truncateList_length]
            omega
          rw [hshort, Pattern.matchesAll_truncateList bound subpatterns vs hsubs]
          simp only [hvk, Nat.min_eq_right (Nat.le_of_lt hk)]
        · -- Lengths differ on both sides, so both conjunctions are false.
          have hne : min bound vs.length ≠ k := by omega
          simp [hne, hvk]
      | _ =>
        exact matches_truncate_of_value_not_array (by intro n; simp) (by intro ws; simp) hsubs
    | _ =>
      cases v with
      | array vs =>
        exact matches_truncate_of_head_not_array (by intro n; simp) (by intro n; simp)
      | _ =>
        exact matches_truncate_of_value_not_array (by intro n; simp) (by intro ws; simp) hsubs

end Buri
