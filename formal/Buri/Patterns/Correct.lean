import Buri.Patterns.Usefulness

/-!
# What the matrix operations mean

`specialize` and `defaultMatrix` are the two steps the usefulness algorithm takes,
and everything about its correctness reduces to a single question each:

* **`specialize`**: when the scrutinee's head value is built with constructor
  `target`, is the original matrix's coverage of `v :: vs` the same as the
  specialised matrix's coverage of `fields(v) ++ vs`?
* **`defaultMatrix`**: when the head value's constructor appears nowhere in the
  matrix, is the original matrix's coverage of `v :: vs` the same as the
  default matrix's coverage of `vs`?

Both are proved below. They are stated on *rows* first, because that is where
the side conditions live, and then lifted to matrices.

## The side conditions, and why they are real

Two hypotheses recur, and neither is bookkeeping:

* **`subpatterns.length ≤ arity`.** `specializeRow` pads *and truncates* a row's
  sub-patterns to the constructor's arity, mirroring `exhaust.rs:246-250`. The
  padding is harmless. The truncation is not: if a pattern carried *more*
  sub-patterns than the constructor has fields, `matchesAll` would reject the
  value (it fails when patterns outlast values) while the truncated row would
  accept it -- the matrix would gain coverage it did not have, and a
  non-exhaustive match could be accepted. `exhaust.rs:114` never produces such
  a pattern, because it sizes `subpatterns` by the largest field *index*; the
  hypothesis is what records that this is load-bearing rather than incidental.

* **rest-freeness.** `specializeRow` drops any row whose head is an
  `arrayRest`, because `arrayRest n ≠ array k` for every `k`. A surviving rest
  pattern would therefore lose the coverage it provided. `expand_lengths`
  (`exhaust.rs:163`) removes them before the algorithm ever runs, which is why
  this is a hypothesis here rather than a case.
-/

namespace Buri

/-! ## Matching, decomposed -/

@[simp] theorem Pattern.matchesAll_cons (p : Pattern) (ps : List Pattern) (v : Value) (vs : List Value) :
    Pattern.matchesAll (p :: ps) (v :: vs) = (Pattern.matches p v && Pattern.matchesAll ps vs) := rfl

@[simp] theorem Pattern.matchesAll_cons_nil (p : Pattern) (ps : List Pattern) :
    Pattern.matchesAll (p :: ps) [] = false := rfl

/-- Every constructor but `arrayRest` matches by projection. -/
theorem Pattern.matches_constructor (target : Constructor) (subpatterns : List Pattern) (v : Value)
    (restFree : ∀ n, target ≠ .arrayRest n) :
    Pattern.matches (.constructor target subpatterns) v
      = (decide (v.constructorOf = some target) && Pattern.matchesAll subpatterns v.fields) := by
  cases target with
  | arrayRest n => exact absurd rfl (restFree n)
  | _ => cases v <;> rfl

/-- Splitting a pointwise match at a boundary where the pattern and value
prefixes agree in length. -/
theorem Pattern.matchesAll_append {ps₁ ps₂ : List Pattern} {vs₁ vs₂ : List Value}
    (h : ps₁.length = vs₁.length) :
    Pattern.matchesAll (ps₁ ++ ps₂) (vs₁ ++ vs₂)
      = (Pattern.matchesAll ps₁ vs₁ && Pattern.matchesAll ps₂ vs₂) := by
  induction ps₁ generalizing vs₁ with
  | nil => cases vs₁ with
    | nil => simp
    | cons _ _ => simp at h
  | cons p ps ih =>
    cases vs₁ with
    | nil => simp at h
    | cons v vs =>
      simp only [List.cons_append, Pattern.matchesAll_cons, Bool.and_assoc]
      rw [ih (by simpa using h)]

theorem Pattern.matchesAll_replicate_wildcard (n : Nat) (vs : List Value) (h : n ≤ vs.length) :
    Pattern.matchesAll (List.replicate n Pattern.wildcard) vs = true := by
  induction n generalizing vs with
  | zero => simp [Pattern.matchesAll]
  | succ k ih =>
    cases vs with
    | nil => simp at h
    | cons v vs =>
      have h' : k ≤ vs.length := by simpa using h
      simp [List.replicate, Pattern.matchesAll, ih vs h']

/-- `matchesAll` consumes only as many values as it has patterns, so extra
values on the right are invisible to it. -/
theorem Pattern.matchesAll_append_right {ps : List Pattern} {vs₁ vs₂ : List Value}
    (h : ps.length ≤ vs₁.length) :
    Pattern.matchesAll ps (vs₁ ++ vs₂) = Pattern.matchesAll ps vs₁ := by
  induction ps generalizing vs₁ with
  | nil => simp [Pattern.matchesAll]
  | cons p ps ih =>
    cases vs₁ with
    | nil => simp at h
    | cons v vs =>
      have h' : ps.length ≤ vs.length := by simpa using h
      simp only [List.cons_append, Pattern.matchesAll_cons, ih h']

/-- Padding a short pattern list with wildcards does not change what it
matches, as long as the values are there to absorb the padding. This is what
makes `exhaust.rs:246-250`'s pad-and-truncate harmless -- given that `subpatterns` was
never longer than the arity in the first place. -/
theorem Pattern.matchesAll_pad {subpatterns : List Pattern} {vs : List Value} {a : Nat}
    (hs : subpatterns.length ≤ a) (hv : vs.length = a) :
    Pattern.matchesAll ((subpatterns ++ List.replicate a Pattern.wildcard).take a) vs
      = Pattern.matchesAll subpatterns vs := by
  have hsplit : (subpatterns ++ List.replicate a Pattern.wildcard).take a
      = subpatterns ++ List.replicate (a - subpatterns.length) Pattern.wildcard := by
    have hmin : min (a - subpatterns.length) a = a - subpatterns.length := by omega
    rw [List.take_append, List.take_of_length_le hs, List.take_replicate, hmin]
  have hk : subpatterns.length ≤ vs.length := by omega
  -- Both sides are computed at `vs = vs.take |subpatterns| ++ vs.drop |subpatterns|`, where
  -- the padded side splits cleanly and the wildcard tail is discharged.
  have hleft : Pattern.matchesAll (subpatterns ++ List.replicate (a - subpatterns.length) Pattern.wildcard) vs
      = Pattern.matchesAll subpatterns (vs.take subpatterns.length) := by
    have h := Pattern.matchesAll_append (ps₁ := subpatterns)
      (ps₂ := List.replicate (a - subpatterns.length) Pattern.wildcard)
      (vs₁ := vs.take subpatterns.length) (vs₂ := vs.drop subpatterns.length) (by simp [hk])
    rw [List.take_append_drop] at h
    rw [h, Pattern.matchesAll_replicate_wildcard _ _ (by simp; omega)]
    simp
  have hright : Pattern.matchesAll subpatterns vs = Pattern.matchesAll subpatterns (vs.take subpatterns.length) := by
    have h := Pattern.matchesAll_append_right (ps := subpatterns) (vs₁ := vs.take subpatterns.length)
      (vs₂ := vs.drop subpatterns.length) (by simp [hk])
    rwa [List.take_append_drop] at h
  rw [hsplit, hleft, hright]

/-! ## `specialize` -/

/-- A row that survives specialisation matches the original value vector
exactly when the specialised row matches the decomposed one. -/
theorem specializeRow_some_matches {target : Constructor} {a : Nat} {r r' : Row}
    {v : Value} {vs : List Value}
    (specializes : specializeRow target a r = some r')
    (hasTarget : v.constructorOf = some target)
    (fieldCount : v.fields.length = a)
    (subArity : ∀ subpatterns rest, r = .constructor target subpatterns :: rest → subpatterns.length ≤ a)
    (restFree : ∀ n subpatterns rest, r ≠ .constructor (.arrayRest n) subpatterns :: rest) :
    Row.matches r (v :: vs) = Row.matches r' (v.fields ++ vs) := by
  match r with
  | [] => simp [specializeRow] at specializes
  | .or _ :: _ => simp [specializeRow] at specializes
  | .wildcard :: rest =>
    simp only [specializeRow, Option.some.injEq] at specializes
    subst specializes
    show Pattern.matchesAll (Pattern.wildcard :: rest) (v :: vs) = Pattern.matchesAll _ _
    rw [Pattern.matchesAll_cons, Pattern.matches_wildcard,
        Pattern.matchesAll_append (by simp [fieldCount]),
        Pattern.matchesAll_replicate_wildcard a v.fields (by omega)]
  | .constructor c subpatterns :: rest =>
    have hcr : ∀ n, c ≠ .arrayRest n := by
      intro n hn; exact restFree n subpatterns rest (by rw [hn])
    simp only [specializeRow] at specializes
    split at specializes
    · next hcc =>
      subst hcc
      simp only [Option.some.injEq] at specializes
      subst specializes
      show Pattern.matchesAll _ _ = Pattern.matchesAll _ _
      rw [Pattern.matchesAll_append (by simp [fieldCount]), Pattern.matchesAll_cons,
          Pattern.matches_constructor _ _ _ hcr, hasTarget]
      rw [Pattern.matchesAll_pad (subArity subpatterns rest rfl) fieldCount]
      simp
    · simp at specializes

/-- A row that does *not* survive specialisation matched nothing to begin
with -- provided it had a column to lose. -/
theorem specializeRow_none_matches {target : Constructor} {a : Nat} {r : Row}
    {v : Value} {vs : List Value}
    (specializes : specializeRow target a r = none)
    (hasTarget : v.constructorOf = some target)
    (nonEmpty : r ≠ [])
    (restFree : ∀ n subpatterns rest, r ≠ .constructor (.arrayRest n) subpatterns :: rest)
    (orFree : ∀ alternatives rest, r ≠ .or alternatives :: rest) :
    Row.matches r (v :: vs) = false := by
  match r with
  | [] => exact absurd rfl nonEmpty
  | .or alternatives :: rest => exact absurd rfl (orFree alternatives rest)
  | .wildcard :: rest => simp [specializeRow] at specializes
  | .constructor c subpatterns :: rest =>
    have hcr : ∀ n, c ≠ .arrayRest n := by
      intro n hn; exact restFree n subpatterns rest (by rw [hn])
    simp only [specializeRow] at specializes
    split at specializes
    · simp at specializes
    · next hcc =>
      show Pattern.matchesAll _ _ = false
      rw [Pattern.matchesAll_cons, Pattern.matches_constructor _ _ _ hcr, hasTarget]
      simp [Ne.symm hcc, hcc]

/-! ## `defaultMatrix` -/

/-- When the head value's constructor appears nowhere in the row, the row
matches `v :: vs` exactly when its `defaultMatrix` image matches `vs`. A
constructor-headed row matches neither. -/
theorem defaultRow_matches {r : Row} {v : Value} {vs : List Value}
    (nonEmpty : r ≠ [])
    (orFree : ∀ alternatives rest, r ≠ .or alternatives :: rest)
    (restFree : ∀ n subpatterns rest, r ≠ .constructor (.arrayRest n) subpatterns :: rest)
    (notInMatrix : ∀ c subpatterns rest, r = .constructor c subpatterns :: rest → v.constructorOf ≠ some c) :
    Row.matches r (v :: vs)
      = (match defaultRow r with | some r' => Row.matches r' vs | none => false) := by
  match r with
  | [] => exact absurd rfl nonEmpty
  | .or alternatives :: rest => exact absurd rfl (orFree alternatives rest)
  | .wildcard :: rest => simp [defaultRow, Row.matches, Pattern.matchesAll]
  | .constructor c subpatterns :: rest =>
    have hcr : ∀ n, c ≠ .arrayRest n := by
      intro n hn; exact restFree n subpatterns rest (by rw [hn])
    have := notInMatrix c subpatterns rest rfl
    show Pattern.matchesAll _ _ = _
    rw [Pattern.matchesAll_cons, Pattern.matches_constructor _ _ _ hcr]
    simp [defaultRow, this]

/-! ## Lifting to matrices -/

/-- `Matrix.covers` is decided by the specialised matrix. This is
`specialize_correct` in the plan's vocabulary, and the workhorse of the
completeness proof. -/
theorem covers_specialize {P : List Row} {target : Constructor} {a : Nat}
    {v : Value} {vs : List Value}
    (hasTarget : v.constructorOf = some target)
    (fieldCount : v.fields.length = a)
    (nonEmpty : ∀ r ∈ P, r ≠ [])
    (orFree : ∀ r ∈ P, ∀ alternatives rest, r ≠ .or alternatives :: rest)
    (restFree : ∀ r ∈ P, ∀ n subpatterns rest, r ≠ .constructor (.arrayRest n) subpatterns :: rest)
    (subArity : ∀ r ∈ P, ∀ subpatterns rest, r = .constructor target subpatterns :: rest → subpatterns.length ≤ a) :
    Matrix.covers P (v :: vs) ↔ Matrix.covers (specialize P target a) (v.fields ++ vs) := by
  constructor
  · rintro ⟨r, hr, hm⟩
    cases specializes : specializeRow target a r with
    | none =>
      rw [specializeRow_none_matches specializes hasTarget (nonEmpty r hr) (restFree r hr) (orFree r hr)] at hm
      exact absurd hm (by simp)
    | some r' =>
      refine ⟨r', ?_, ?_⟩
      · exact List.mem_filterMap.mpr ⟨r, hr, specializes⟩
      · rwa [specializeRow_some_matches specializes hasTarget fieldCount (subArity r hr) (restFree r hr)] at hm
  · rintro ⟨r', hr', hm⟩
    obtain ⟨r, hr, specializes⟩ := List.mem_filterMap.mp hr'
    refine ⟨r, hr, ?_⟩
    rwa [specializeRow_some_matches specializes hasTarget fieldCount (subArity r hr) (restFree r hr)]

/-- `default_correct`: when the head value's constructor is absent from every
row, coverage is decided by the default matrix. -/
theorem covers_defaultMatrix {P : List Row} {v : Value} {vs : List Value}
    (nonEmpty : ∀ r ∈ P, r ≠ [])
    (orFree : ∀ r ∈ P, ∀ alternatives rest, r ≠ .or alternatives :: rest)
    (restFree : ∀ r ∈ P, ∀ n subpatterns rest, r ≠ .constructor (.arrayRest n) subpatterns :: rest)
    (notInMatrix : ∀ r ∈ P, ∀ c subpatterns rest, r = .constructor c subpatterns :: rest → v.constructorOf ≠ some c) :
    Matrix.covers P (v :: vs) ↔ Matrix.covers (defaultMatrix P) vs := by
  constructor
  · rintro ⟨r, hr, hm⟩
    rw [defaultRow_matches (nonEmpty r hr) (orFree r hr) (restFree r hr) (notInMatrix r hr)] at hm
    cases hd : defaultRow r with
    | none => rw [hd] at hm; exact absurd hm (by simp)
    | some r' =>
      rw [hd] at hm
      exact ⟨r', List.mem_filterMap.mpr ⟨r, hr, hd⟩, hm⟩
  · rintro ⟨r', hr', hm⟩
    obtain ⟨r, hr, hd⟩ := List.mem_filterMap.mp hr'
    refine ⟨r, hr, ?_⟩
    rw [defaultRow_matches (nonEmpty r hr) (orFree r hr) (restFree r hr) (notInMatrix r hr), hd]
    exact hm

/-- The right-to-left half of `covers_specialize`, which needs strictly fewer
hypotheses: no or-freeness and no non-emptiness.

That matters. `specialize` *drops* a row headed by an alternation, because
`specializeRow` has no case for one. Dropping rows can only lose coverage, so
the left-to-right direction genuinely needs or-freeness -- but this direction,
the one completeness uses, does not. Since `expand` leaves nested alternations
in the matrix (`.Some(true | false)` is the small example), that distinction is
what keeps these theorems applicable to the matrices the compiler really
builds. -/
theorem covers_of_specialize {matrix : List Row} {target : Constructor} {arity : Nat}
    {v₀ : Value} {values : List Value}
    (hasTarget : v₀.constructorOf = some target)
    (fieldCount : v₀.fields.length = arity)
    (subArity : ∀ r ∈ matrix, ∀ subpatterns rest,
      r = .constructor target subpatterns :: rest → subpatterns.length ≤ arity)
    (restFree : ∀ r ∈ matrix, ∀ n subpatterns rest,
      r ≠ .constructor (.arrayRest n) subpatterns :: rest)
    (h : Matrix.covers (specialize matrix target arity) (v₀.fields ++ values)) :
    Matrix.covers matrix (v₀ :: values) := by
  obtain ⟨r', hr', hm⟩ := h
  obtain ⟨r, hr, hspec⟩ := List.mem_filterMap.mp hr'
  refine ⟨r, hr, ?_⟩
  rwa [specializeRow_some_matches hspec hasTarget fieldCount (subArity r hr) (restFree r hr)]

end Buri
