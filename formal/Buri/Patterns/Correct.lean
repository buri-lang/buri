import Buri.Patterns.WellFormed

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

Both are biconditionals, proved below on *rows* first and then lifted to
matrices. Both run by the recursion the operations themselves use, so the
or-headed row -- which now distributes rather than being dropped -- is a case
rather than a hypothesis.

## The side condition, and why it is real

`Row.WellFormed` is the single hypothesis, and two of its clauses are doing
work rather than bookkeeping:

* **`subpatterns.length ≤ arity`.** `specializeRow` pads *and truncates* a
  row's sub-patterns to the constructor's arity, mirroring the Rust. The
  padding is harmless. The truncation is not: if a pattern carried *more*
  sub-patterns than the constructor has fields, `matchesAll` would reject the
  value (it fails when patterns outlast values) while the truncated row would
  accept it -- the matrix would gain coverage it did not have, and a
  non-exhaustive match could be accepted. `lower` never produces such a
  pattern, because it sizes `subpatterns` by the largest field *index*.

* **rest-freeness.** `specializeRow` drops any row whose head is an
  `arrayRest`, because `arrayRest n ≠ array k` for every `k`. A surviving rest
  pattern would therefore lose the coverage it provided. `expand_lengths`
  removes them before the algorithm ever runs, which is why this is a
  hypothesis here rather than a case.
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
  | zero => simp
  | succ k ih =>
    cases vs with
    | nil => simp at h
    | cons v vs =>
      have h' : k ≤ vs.length := by simpa using h
      simp [List.replicate, ih vs h']

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
  rw [pad_take hs, hleft, hright]

/-! ## Two facts about `any` over a distributed alternation -/

theorem Pattern.matchesAny_eq_any (alternatives : List Pattern) (v : Value) :
    Pattern.matchesAny alternatives v = alternatives.any fun p => Pattern.matches p v := by
  induction alternatives with
  | nil => rfl
  | cons a as ih => simp only [Pattern.matchesAny, List.any_cons, ih]

private theorem any_and_right {α : Type} (l : List α) (f : α → Bool) (b : Bool) :
    (l.any fun a => f a && b) = (l.any f && b) := by
  induction l with
  | nil => simp
  | cons a as ih => simp only [List.any_cons, ih]; cases f a <;> cases b <;> simp

private theorem any_congr_mem {α : Type} {l : List α} {f g : α → Bool}
    (h : ∀ a ∈ l, f a = g a) : l.any f = l.any g := by
  induction l with
  | nil => rfl
  | cons a as ih =>
    simp only [List.any_cons, h a (by simp), ih fun b hb => h b (by simp [hb])]

/-! ## `specialize`

The row-level statement is an *equality of booleans*, not a one-way
implication: a row matches `v :: vs` exactly when one of its specialisations
matches `v.fields ++ vs`. Getting both directions at once is what the
distributing `specialize` buys -- the old, dropping version needed or-freeness
for the forward direction, and `findings/README.md` 6 is the counterexample
that showed the difference is real. -/

/-- A row's coverage of `v :: vs` is decided by its specialisations' coverage
of `v.fields ++ vs`, when `v` is built with `target`. -/
theorem specializeRow_matches {signature : Signature} {type : Ty} {types : List Ty}
    {target : Constructor} {v : Value} {vs : List Value}
    (hasTarget : v.constructorOf = some target)
    (fieldCount : v.fields.length = target.arity signature type) :
    ∀ (row : Row), Row.WellFormed signature (type :: types) row →
      Row.matches row (v :: vs)
        = (specializeRow target (target.arity signature type) row).any
            (fun row' => Row.matches row' (v.fields ++ vs)) := by
  intro row
  induction row using specializeRow.induct target with
  | case1 => intro h; exact absurd h (by simp [Row.WellFormed])
  | case2 rest =>
    intro _
    rw [specializeRow_wildcard]
    simp only [List.any_cons, List.any_nil, Bool.or_false]
    show Pattern.matchesAll (Pattern.wildcard :: rest) (v :: vs)
        = Pattern.matchesAll
            (List.replicate (target.arity signature type) Pattern.wildcard ++ rest)
            (v.fields ++ vs)
    rw [Pattern.matchesAll_cons, Pattern.matches_wildcard, Bool.true_and,
        Pattern.matchesAll_append (by simp [fieldCount]),
        Pattern.matchesAll_replicate_wildcard _ v.fields (by omega), Bool.true_and]
  | case3 subpatterns rest =>
    intro h
    have hnotrest : ∀ n, target ≠ .arrayRest n := fun n hn =>
      Row.WellFormed.head_not_rest h n subpatterns rest (by rw [hn])
    have harity := Row.WellFormed.head_arity h (head := target) (subpatterns := subpatterns)
      (rest := rest) rfl
    rw [specializeRow_constructor, if_pos rfl]
    simp only [List.any_cons, List.any_nil, Bool.or_false]
    show Pattern.matchesAll (Pattern.constructor target subpatterns :: rest) (v :: vs)
        = Pattern.matchesAll
            ((subpatterns ++ List.replicate (target.arity signature type)
                Pattern.wildcard).take (target.arity signature type) ++ rest)
            (v.fields ++ vs)
    rw [Pattern.matchesAll_append (by
          simp only [List.length_take, List.length_append, List.length_replicate, fieldCount]
          omega),
        Pattern.matchesAll_pad harity fieldCount, Pattern.matchesAll_cons,
        Pattern.matches_constructor _ _ _ hnotrest, hasTarget]
    simp
  | case4 c subpatterns rest hne =>
    intro h
    have hnotrest : ∀ n, c ≠ .arrayRest n := fun n hn =>
      Row.WellFormed.head_not_rest h n subpatterns rest (by rw [hn])
    rw [specializeRow_constructor, if_neg hne]
    show Pattern.matchesAll (Pattern.constructor c subpatterns :: rest) (v :: vs) = _
    rw [Pattern.matchesAll_cons, Pattern.matches_constructor _ _ _ hnotrest, hasTarget]
    simp [Ne.symm hne]
  | case5 alternatives rest ih =>
    intro h
    show Pattern.matchesAll (Pattern.or alternatives :: rest) (v :: vs) = _
    rw [Pattern.matchesAll_cons]
    show (Pattern.matchesAny alternatives v && Pattern.matchesAll rest vs) = _
    rw [Pattern.matchesAny_eq_any, ← any_and_right, specializeRow_or, List.any_flatMap]
    exact any_congr_mem fun a ha =>
      ih ⟨a, ha⟩ ⟨Pattern.WellFormedAlternatives.mem h.1 ha, h.2⟩

/-! ## `defaultMatrix` -/

/-- When the head value's constructor appears nowhere in the row's head column,
the row's coverage of `v :: vs` is decided by its default rows' coverage of
`vs`. -/
theorem defaultRow_matches {signature : Signature} {type : Ty} {types : List Ty}
    {v : Value} {vs : List Value} :
    ∀ (row : Row), Row.WellFormed signature (type :: types) row →
      (∀ c ∈ Pattern.headConstructors (row.headD .wildcard), v.constructorOf ≠ some c) →
      Row.matches row (v :: vs) = (defaultRow row).any (fun row' => Row.matches row' vs) := by
  intro row
  induction row using defaultRow.induct with
  | case1 => intro h; exact absurd h (by simp [Row.WellFormed])
  | case2 rest =>
    intro _ _
    rw [defaultRow_wildcard]
    simp only [List.any_cons, List.any_nil, Bool.or_false]
    show Pattern.matchesAll (Pattern.wildcard :: rest) (v :: vs) = Pattern.matchesAll rest vs
    rw [Pattern.matchesAll_cons, Pattern.matches_wildcard, Bool.true_and]
  | case3 c subpatterns rest =>
    intro h hmissing
    have hnotrest : ∀ n, c ≠ .arrayRest n := fun n hn =>
      Row.WellFormed.head_not_rest h n subpatterns rest (by rw [hn])
    have hne := hmissing c (by simp)
    rw [defaultRow_constructor]
    show Pattern.matchesAll (Pattern.constructor c subpatterns :: rest) (v :: vs) = _
    rw [Pattern.matchesAll_cons, Pattern.matches_constructor _ _ _ hnotrest]
    simp [hne]
  | case4 alternatives rest ih =>
    intro h hmissing
    show Pattern.matchesAll (Pattern.or alternatives :: rest) (v :: vs) = _
    rw [Pattern.matchesAll_cons]
    show (Pattern.matchesAny alternatives v && Pattern.matchesAll rest vs) = _
    rw [Pattern.matchesAny_eq_any, ← any_and_right, defaultRow_or, List.any_flatMap]
    refine any_congr_mem fun a ha => ?_
    refine ih ⟨a, ha⟩ ⟨Pattern.WellFormedAlternatives.mem h.1 ha, h.2⟩ (fun c hc => ?_)
    refine hmissing c ?_
    simp only [List.headD_cons, Pattern.headConstructors_or, List.mem_flatMap]
    exact ⟨a, ha, by simpa using hc⟩

/-- A default row that matches came from a row that matched -- with no
hypotheses at all, because every default row is reached through a wildcard in
the head column, and a wildcard matches whatever sits there. -/
theorem defaultRow_matches_of_mem : ∀ (row row' : Row) (v : Value) (vs : List Value),
    row' ∈ defaultRow row → Row.matches row' vs = true → Row.matches row (v :: vs) = true := by
  intro row
  induction row using defaultRow.induct with
  | case1 => intro row' v vs hd; simp at hd
  | case2 rest =>
    intro row' v vs hd hm
    simp only [defaultRow_wildcard, List.mem_singleton] at hd
    show (Pattern.matches .wildcard v && Pattern.matchesAll rest vs) = true
    simp only [Pattern.matches_wildcard, Bool.true_and, ← hd]
    exact hm
  | case3 c subpatterns rest => intro row' v vs hd; simp at hd
  | case4 alternatives rest ih =>
    intro row' v vs hd hm
    simp only [defaultRow_or, List.mem_flatMap] at hd
    obtain ⟨a, ha, hd'⟩ := hd
    have hsplit : (Pattern.matches a v && Pattern.matchesAll rest vs) = true :=
      ih ⟨a, ha⟩ row' v vs hd' hm
    obtain ⟨h₁, h₂⟩ := Bool.and_eq_true .. |>.mp hsplit
    show (Pattern.matchesAny alternatives v && Pattern.matchesAll rest vs) = true
    simp [Pattern.matchesAny_iff.mpr ⟨a, ha, h₁⟩, h₂]

/-! ## Lifting to matrices -/

/-- **`specialize_correct`**: coverage is decided by the specialised matrix.
The workhorse of the completeness proof. -/
theorem covers_specialize {signature : Signature} {type : Ty} {types : List Ty}
    {matrix : List Row} {target : Constructor} {v : Value} {vs : List Value}
    (hwf : Matrix.WellFormed signature (type :: types) matrix)
    (hasTarget : v.constructorOf = some target)
    (fieldCount : v.fields.length = target.arity signature type) :
    Matrix.covers matrix (v :: vs)
      ↔ Matrix.covers (specialize matrix target (target.arity signature type)) (v.fields ++ vs) := by
  constructor
  · rintro ⟨row, hrow, hm⟩
    rw [specializeRow_matches hasTarget fieldCount row (hwf row hrow)] at hm
    obtain ⟨row', hmem, hm'⟩ := List.any_eq_true.mp hm
    exact ⟨row', List.mem_flatMap.mpr ⟨row, hrow, hmem⟩, hm'⟩
  · rintro ⟨row', hrow', hm⟩
    obtain ⟨row, hrow, hmem⟩ := List.mem_flatMap.mp hrow'
    refine ⟨row, hrow, ?_⟩
    rw [specializeRow_matches hasTarget fieldCount row (hwf row hrow)]
    exact List.any_eq_true.mpr ⟨row', hmem, hm⟩

/-- **`default_correct`**: when the head value's constructor is absent from
every row's head, coverage is decided by the default matrix. -/
theorem covers_defaultMatrix {signature : Signature} {type : Ty} {types : List Ty}
    {matrix : List Row} {v : Value} {vs : List Value}
    (hwf : Matrix.WellFormed signature (type :: types) matrix)
    (hmissing : ∀ c ∈ headConstructors matrix, v.constructorOf ≠ some c) :
    Matrix.covers matrix (v :: vs) ↔ Matrix.covers (defaultMatrix matrix) vs := by
  have hrow : ∀ row ∈ matrix, Row.matches row (v :: vs)
      = (defaultRow row).any (fun row' => Row.matches row' vs) := by
    intro row hrowmem
    refine defaultRow_matches row (hwf row hrowmem) (fun c hc => hmissing c ?_)
    refine List.mem_flatMap.mpr ⟨row, hrowmem, ?_⟩
    match row with
    | [] => simp at hc
    | p :: _ => simpa using hc
  constructor
  · rintro ⟨row, hrowmem, hm⟩
    rw [hrow row hrowmem] at hm
    obtain ⟨row', hmem, hm'⟩ := List.any_eq_true.mp hm
    exact ⟨row', List.mem_flatMap.mpr ⟨row, hrowmem, hmem⟩, hm'⟩
  · rintro ⟨row', hrow'mem, hm⟩
    obtain ⟨row, hrowmem, hmem⟩ := List.mem_flatMap.mp hrow'mem
    exact ⟨row, hrowmem, by rw [hrow row hrowmem]; exact List.any_eq_true.mpr ⟨row', hmem, hm⟩⟩

/-- Coverage of the tail by the default matrix lifts to coverage of the whole
vector, whatever the head value is. This direction needs **no hypotheses at
all**, which is what makes the completeness proof cheap. -/
theorem covers_of_defaultMatrix {matrix : List Row} {v : Value} {vs : List Value}
    (h : Matrix.covers (defaultMatrix matrix) vs) :
    Matrix.covers matrix (v :: vs) := by
  obtain ⟨row', hmem, hmatch⟩ := h
  obtain ⟨row, hrow, hd⟩ := List.mem_flatMap.mp hmem
  exact ⟨row, hrow, defaultRow_matches_of_mem row row' v vs hd hmatch⟩

/-- The right-to-left half of `covers_specialize`, which needs no
well-formedness at all: a specialised row that matches came from a row that
matched. -/
theorem covers_of_specialize {signature : Signature} {type : Ty} {types : List Ty}
    {matrix : List Row} {target : Constructor} {v : Value} {vs : List Value}
    (hwf : Matrix.WellFormed signature (type :: types) matrix)
    (hasTarget : v.constructorOf = some target)
    (fieldCount : v.fields.length = target.arity signature type)
    (h : Matrix.covers (specialize matrix target (target.arity signature type))
      (v.fields ++ vs)) :
    Matrix.covers matrix (v :: vs) :=
  (covers_specialize hwf hasTarget fieldCount).mpr h

end Buri
