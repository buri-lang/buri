import Buri.Patterns.Usefulness

/-!
# What the matrix operations mean

`specialize` and `defaultMat` are the two steps the usefulness algorithm takes,
and everything about its correctness reduces to a single question each:

* **`specialize`**: when the scrutinee's head value is built with constructor
  `ct`, is the original matrix's coverage of `v :: vs` the same as the
  specialised matrix's coverage of `fields(v) ++ vs`?
* **`defaultMat`**: when the head value's constructor appears nowhere in the
  matrix, is the original matrix's coverage of `v :: vs` the same as the
  default matrix's coverage of `vs`?

Both are proved below. They are stated on *rows* first, because that is where
the side conditions live, and then lifted to matrices.

## The side conditions, and why they are real

Two hypotheses recur, and neither is bookkeeping:

* **`subs.length ≤ arity`.** `specializeRow` pads *and truncates* a row's
  sub-patterns to the constructor's arity, mirroring `exhaust.rs:246-250`. The
  padding is harmless. The truncation is not: if a pattern carried *more*
  sub-patterns than the constructor has fields, `matchPad` would reject the
  value (it fails when patterns outlast values) while the truncated row would
  accept it -- the matrix would gain coverage it did not have, and a
  non-exhaustive match could be accepted. `exhaust.rs:114` never produces such
  a pattern, because it sizes `subs` by the largest field *index*; the
  hypothesis is what records that this is load-bearing rather than incidental.

* **rest-freeness.** `specializeRow` drops any row whose head is an
  `arrayRest`, because `arrayRest n ≠ array k` for every `k`. A surviving rest
  pattern would therefore lose the coverage it provided. `expand_lengths`
  (`exhaust.rs:163`) removes them before the algorithm ever runs, which is why
  this is a hypothesis here rather than a case.
-/

namespace Buri

/-! ## Matching, decomposed -/

@[simp] theorem Pat.matchPad_cons (p : Pat) (ps : List Pat) (v : Value) (vs : List Value) :
    Pat.matchPad (p :: ps) (v :: vs) = (Pat.matches p v && Pat.matchPad ps vs) := rfl

@[simp] theorem Pat.matchPad_cons_nil (p : Pat) (ps : List Pat) :
    Pat.matchPad (p :: ps) [] = false := rfl

/-- Every constructor but `arrayRest` matches by projection. -/
theorem Pat.matches_ctor (ct : Ctor) (subs : List Pat) (v : Value)
    (hrest : ∀ n, ct ≠ .arrayRest n) :
    Pat.matches (.ctor ct subs) v
      = (decide (v.ctorOf = some ct) && Pat.matchPad subs v.fieldsOf) := by
  cases ct with
  | arrayRest n => exact absurd rfl (hrest n)
  | _ => cases v <;> rfl

/-- Splitting a pointwise match at a boundary where the pattern and value
prefixes agree in length. -/
theorem Pat.matchPad_append {ps₁ ps₂ : List Pat} {vs₁ vs₂ : List Value}
    (h : ps₁.length = vs₁.length) :
    Pat.matchPad (ps₁ ++ ps₂) (vs₁ ++ vs₂)
      = (Pat.matchPad ps₁ vs₁ && Pat.matchPad ps₂ vs₂) := by
  induction ps₁ generalizing vs₁ with
  | nil => cases vs₁ with
    | nil => simp
    | cons _ _ => simp at h
  | cons p ps ih =>
    cases vs₁ with
    | nil => simp at h
    | cons v vs =>
      simp only [List.cons_append, Pat.matchPad_cons, Bool.and_assoc]
      rw [ih (by simpa using h)]

theorem Pat.matchPad_replicate_wild (n : Nat) (vs : List Value) (h : n ≤ vs.length) :
    Pat.matchPad (List.replicate n Pat.wild) vs = true := by
  induction n generalizing vs with
  | zero => simp [Pat.matchPad]
  | succ k ih =>
    cases vs with
    | nil => simp at h
    | cons v vs =>
      have h' : k ≤ vs.length := by simpa using h
      simp [List.replicate, Pat.matchPad, ih vs h']

/-- `matchPad` consumes only as many values as it has patterns, so extra
values on the right are invisible to it. -/
theorem Pat.matchPad_append_right {ps : List Pat} {vs₁ vs₂ : List Value}
    (h : ps.length ≤ vs₁.length) :
    Pat.matchPad ps (vs₁ ++ vs₂) = Pat.matchPad ps vs₁ := by
  induction ps generalizing vs₁ with
  | nil => simp [Pat.matchPad]
  | cons p ps ih =>
    cases vs₁ with
    | nil => simp at h
    | cons v vs =>
      have h' : ps.length ≤ vs.length := by simpa using h
      simp only [List.cons_append, Pat.matchPad_cons, ih h']

/-- Padding a short pattern list with wildcards does not change what it
matches, as long as the values are there to absorb the padding. This is what
makes `exhaust.rs:246-250`'s pad-and-truncate harmless -- given that `subs` was
never longer than the arity in the first place. -/
theorem Pat.matchPad_pad {subs : List Pat} {vs : List Value} {a : Nat}
    (hs : subs.length ≤ a) (hv : vs.length = a) :
    Pat.matchPad ((subs ++ List.replicate a Pat.wild).take a) vs
      = Pat.matchPad subs vs := by
  have hsplit : (subs ++ List.replicate a Pat.wild).take a
      = subs ++ List.replicate (a - subs.length) Pat.wild := by
    have hmin : min (a - subs.length) a = a - subs.length := by omega
    rw [List.take_append, List.take_of_length_le hs, List.take_replicate, hmin]
  have hk : subs.length ≤ vs.length := by omega
  -- Both sides are computed at `vs = vs.take |subs| ++ vs.drop |subs|`, where
  -- the padded side splits cleanly and the wildcard tail is discharged.
  have hleft : Pat.matchPad (subs ++ List.replicate (a - subs.length) Pat.wild) vs
      = Pat.matchPad subs (vs.take subs.length) := by
    have h := Pat.matchPad_append (ps₁ := subs)
      (ps₂ := List.replicate (a - subs.length) Pat.wild)
      (vs₁ := vs.take subs.length) (vs₂ := vs.drop subs.length) (by simp [hk])
    rw [List.take_append_drop] at h
    rw [h, Pat.matchPad_replicate_wild _ _ (by simp; omega)]
    simp
  have hright : Pat.matchPad subs vs = Pat.matchPad subs (vs.take subs.length) := by
    have h := Pat.matchPad_append_right (ps := subs) (vs₁ := vs.take subs.length)
      (vs₂ := vs.drop subs.length) (by simp [hk])
    rwa [List.take_append_drop] at h
  rw [hsplit, hleft, hright]

/-! ## `specialize` -/

/-- A row that survives specialisation matches the original value vector
exactly when the specialised row matches the decomposed one. -/
theorem specializeRow_some_matches {ct : Ctor} {a : Nat} {r r' : Row}
    {v : Value} {vs : List Value}
    (hspec : specializeRow ct a r = some r')
    (hct : v.ctorOf = some ct)
    (hlen : v.fieldsOf.length = a)
    (hsub : ∀ c subs rest, r = .ctor c subs :: rest → subs.length ≤ a)
    (hrest : ∀ n subs rest, r ≠ .ctor (.arrayRest n) subs :: rest) :
    Row.matches r (v :: vs) = Row.matches r' (v.fieldsOf ++ vs) := by
  match r with
  | [] => simp [specializeRow] at hspec
  | .or _ :: _ => simp [specializeRow] at hspec
  | .wild :: rest =>
    simp only [specializeRow, Option.some.injEq] at hspec
    subst hspec
    show Pat.matchPad (Pat.wild :: rest) (v :: vs) = Pat.matchPad _ _
    rw [Pat.matchPad_cons, Pat.matches_wild,
        Pat.matchPad_append (by simp [hlen]),
        Pat.matchPad_replicate_wild a v.fieldsOf (by omega)]
  | .ctor c subs :: rest =>
    have hcr : ∀ n, c ≠ .arrayRest n := by
      intro n hn; exact hrest n subs rest (by rw [hn])
    simp only [specializeRow] at hspec
    split at hspec
    · next hcc =>
      subst hcc
      simp only [Option.some.injEq] at hspec
      subst hspec
      show Pat.matchPad _ _ = Pat.matchPad _ _
      rw [Pat.matchPad_append (by simp [hlen]), Pat.matchPad_cons,
          Pat.matches_ctor _ _ _ hcr, hct]
      rw [Pat.matchPad_pad (hsub c subs rest rfl) hlen]
      simp
    · simp at hspec

/-- A row that does *not* survive specialisation matched nothing to begin
with -- provided it had a column to lose. -/
theorem specializeRow_none_matches {ct : Ctor} {a : Nat} {r : Row}
    {v : Value} {vs : List Value}
    (hspec : specializeRow ct a r = none)
    (hct : v.ctorOf = some ct)
    (hne : r ≠ [])
    (hrest : ∀ n subs rest, r ≠ .ctor (.arrayRest n) subs :: rest)
    (hor : ∀ alts rest, r ≠ .or alts :: rest) :
    Row.matches r (v :: vs) = false := by
  match r with
  | [] => exact absurd rfl hne
  | .or alts :: rest => exact absurd rfl (hor alts rest)
  | .wild :: rest => simp [specializeRow] at hspec
  | .ctor c subs :: rest =>
    have hcr : ∀ n, c ≠ .arrayRest n := by
      intro n hn; exact hrest n subs rest (by rw [hn])
    simp only [specializeRow] at hspec
    split at hspec
    · simp at hspec
    · next hcc =>
      show Pat.matchPad _ _ = false
      rw [Pat.matchPad_cons, Pat.matches_ctor _ _ _ hcr, hct]
      simp [Ne.symm hcc, hcc]

/-! ## `defaultMat` -/

/-- When the head value's constructor appears nowhere in the row, the row
matches `v :: vs` exactly when its `defaultMat` image matches `vs`. A
constructor-headed row matches neither. -/
theorem defaultRow_matches {r : Row} {v : Value} {vs : List Value}
    (hne : r ≠ [])
    (hor : ∀ alts rest, r ≠ .or alts :: rest)
    (hrest : ∀ n subs rest, r ≠ .ctor (.arrayRest n) subs :: rest)
    (hmiss : ∀ c subs rest, r = .ctor c subs :: rest → v.ctorOf ≠ some c) :
    Row.matches r (v :: vs)
      = (match defaultRow r with | some r' => Row.matches r' vs | none => false) := by
  match r with
  | [] => exact absurd rfl hne
  | .or alts :: rest => exact absurd rfl (hor alts rest)
  | .wild :: rest => simp [defaultRow, Row.matches, Pat.matchPad]
  | .ctor c subs :: rest =>
    have hcr : ∀ n, c ≠ .arrayRest n := by
      intro n hn; exact hrest n subs rest (by rw [hn])
    have := hmiss c subs rest rfl
    show Pat.matchPad _ _ = _
    rw [Pat.matchPad_cons, Pat.matches_ctor _ _ _ hcr]
    simp [defaultRow, this]

/-! ## Lifting to matrices -/

/-- `Matrix.covers` is decided by the specialised matrix. This is
`specialize_correct` in the plan's vocabulary, and the workhorse of the
completeness proof. -/
theorem covers_specialize {P : List Row} {ct : Ctor} {a : Nat}
    {v : Value} {vs : List Value}
    (hct : v.ctorOf = some ct)
    (hlen : v.fieldsOf.length = a)
    (hne : ∀ r ∈ P, r ≠ [])
    (hor : ∀ r ∈ P, ∀ alts rest, r ≠ .or alts :: rest)
    (hrest : ∀ r ∈ P, ∀ n subs rest, r ≠ .ctor (.arrayRest n) subs :: rest)
    (hsub : ∀ r ∈ P, ∀ c subs rest, r = .ctor c subs :: rest → subs.length ≤ a) :
    Matrix.covers P (v :: vs) ↔ Matrix.covers (specialize P ct a) (v.fieldsOf ++ vs) := by
  constructor
  · rintro ⟨r, hr, hm⟩
    cases hspec : specializeRow ct a r with
    | none =>
      rw [specializeRow_none_matches hspec hct (hne r hr) (hrest r hr) (hor r hr)] at hm
      exact absurd hm (by simp)
    | some r' =>
      refine ⟨r', ?_, ?_⟩
      · exact List.mem_filterMap.mpr ⟨r, hr, hspec⟩
      · rwa [specializeRow_some_matches hspec hct hlen (hsub r hr) (hrest r hr)] at hm
  · rintro ⟨r', hr', hm⟩
    obtain ⟨r, hr, hspec⟩ := List.mem_filterMap.mp hr'
    refine ⟨r, hr, ?_⟩
    rwa [specializeRow_some_matches hspec hct hlen (hsub r hr) (hrest r hr)]

/-- `default_correct`: when the head value's constructor is absent from every
row, coverage is decided by the default matrix. -/
theorem covers_defaultMat {P : List Row} {v : Value} {vs : List Value}
    (hne : ∀ r ∈ P, r ≠ [])
    (hor : ∀ r ∈ P, ∀ alts rest, r ≠ .or alts :: rest)
    (hrest : ∀ r ∈ P, ∀ n subs rest, r ≠ .ctor (.arrayRest n) subs :: rest)
    (hmiss : ∀ r ∈ P, ∀ c subs rest, r = .ctor c subs :: rest → v.ctorOf ≠ some c) :
    Matrix.covers P (v :: vs) ↔ Matrix.covers (defaultMat P) vs := by
  constructor
  · rintro ⟨r, hr, hm⟩
    rw [defaultRow_matches (hne r hr) (hor r hr) (hrest r hr) (hmiss r hr)] at hm
    cases hd : defaultRow r with
    | none => rw [hd] at hm; exact absurd hm (by simp)
    | some r' =>
      rw [hd] at hm
      exact ⟨r', List.mem_filterMap.mpr ⟨r, hr, hd⟩, hm⟩
  · rintro ⟨r', hr', hm⟩
    obtain ⟨r, hr, hd⟩ := List.mem_filterMap.mp hr'
    refine ⟨r, hr, ?_⟩
    rw [defaultRow_matches (hne r hr) (hor r hr) (hrest r hr) (hmiss r hr), hd]
    exact hm

end Buri
