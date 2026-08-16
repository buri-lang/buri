import Buri.Patterns.WellFormed
import Buri.Patterns.Correct
import Buri.Patterns.Spec

/-!
# Completeness: the checker never misses an uncovered value

The safety direction. If `isUseful` returns *false* -- which is what makes the
compiler accept a `match` -- then the matrix really does cover every well-typed
value vector the pattern vector matches.

Read at the top level with the pattern vector `[wildcard]`, that is exactly
`exhaustive_correct`: an accepted `match` has an arm that fires for every value
of the scrutinee's type, so evaluation cannot get stuck at a `match`.

## What the induction needs

Three invariants travel with the recursion, and each earns its place:

* **`Matrix.WellFormed`** -- rows are or-free, rest-free, have one pattern per
  type, and carry no more sub-patterns than their constructor has fields.
  `WellFormed.lean` proves both matrix steps preserve it.
* **`Row.WellFormed`** on the *pattern vector* too. This is what makes the
  or-pattern case vacuous: `expand` runs before `useful` is ever called, so an
  alternation cannot reach the algorithm. (`exhaustiveness.rs` still has a
  branch for it; that branch is dead code.)
* **`Value.BoundedList`** -- the array-length restriction. `allConstructors`
  enumerates array constructors only up to `limit`, so this theorem holds in
  the universe the algorithm reasons about. `Expand.lean` discharges it.

## One-directional coverage

Completeness needs only one direction of each matrix step, and the `default`
one is far cheaper than the biconditional in `Correct.lean`:
`covers_of_defaultMatrix` needs **no hypotheses at all**, because every default
row came from a wildcard-headed row and a wildcard matches whatever sits in
that column. The biconditionals are what `useful_sound` will need; completeness
gets away with less.
-/

namespace Buri

/-- Coverage of the tail by the default matrix lifts to coverage of the whole
vector, whatever the head value is. -/
theorem covers_of_defaultMatrix {matrix : List Row} {value : Value} {values : List Value}
    (h : Matrix.covers (defaultMatrix matrix) values) :
    Matrix.covers matrix (value :: values) := by
  obtain ⟨row', hmem, hmatch⟩ := h
  obtain ⟨row, hrow, hdefault⟩ := List.mem_filterMap.mp hmem
  refine ⟨row, hrow, ?_⟩
  match row with
  | .wildcard :: rest =>
    simp only [defaultRow, Option.some.injEq] at hdefault
    subst hdefault
    show (Pattern.matches .wildcard value && Pattern.matchesAll rest values) = true
    simp only [Pattern.matches_wildcard, Bool.true_and]
    exact hmatch
  | [] => simp [defaultRow] at hdefault
  | .constructor _ _ :: _ => simp [defaultRow] at hdefault
  | .or _ :: _ => simp [defaultRow] at hdefault

theorem Value.BoundedList.append {limit : Nat} {as bs : List Value}
    (ha : Value.BoundedList limit as) (hb : Value.BoundedList limit bs) :
    Value.BoundedList limit (as ++ bs) := by
  rw [Value.BoundedList_iff] at ha hb ⊢
  intro v hv
  rcases List.mem_append.mp hv with h | h
  · exact ha v h
  · exact hb v h

/-- Lifting coverage back through `specialize`. Both constructor branches of
the induction end this way, so the well-formedness plumbing lives here once. -/
private theorem covers_lift {signature : Signature} {t : Ty} {ts : List Ty}
    {matrix : List Row} {target : Constructor} {v₀ : Value} {values : List Value}
    (hmatrix : Matrix.WellFormed signature (t :: ts) matrix)
    (hasTarget : v₀.constructorOf = some target)
    (hfields : v₀.fields.length = target.arity signature t)
    (h : Matrix.covers (specialize matrix target (target.arity signature t))
      (v₀.fields ++ values)) :
    Matrix.covers matrix (v₀ :: values) :=
  covers_of_specialize (target := target) hasTarget hfields
    (fun r hr sp rest heq => Row.WellFormed.head_arity (hmatrix r hr) heq)
    (fun r hr => Row.WellFormed.head_not_rest (hmatrix r hr))
    h

/-- **Completeness of the usefulness algorithm.**

If `isUseful` returns `false`, every well-typed, length-bounded value vector
that the pattern vector matches is covered by the matrix. -/
theorem isUseful_false_covers (signature : Signature) (limit : Nat) :
    ∀ (matrix : List Row) (row : Row) (types : List Ty),
      isUseful signature limit matrix row types = false →
      Matrix.WellFormed signature types matrix →
      Row.WellFormed signature types row →
      ∀ values, Forall₂ (HasType signature) values types →
                Value.BoundedList limit values →
                Row.matches row values = true →
                Matrix.covers matrix values := by
  intro matrix row types
  induction matrix, row, types using isUseful.induct signature limit with
  | case1 matrix types =>
    -- Empty pattern vector: well-formedness forces the type vector empty too,
    -- so every row is empty and matches the empty value list.
    intro hfalse hmatrix hrow values htyped _ _
    cases types with
    | cons t ts => exact absurd hrow (by simp [Row.WellFormed])
    | nil =>
      cases htyped
      rw [isUseful] at hfalse
      cases matrix with
      | nil => simp at hfalse
      | cons r rest =>
        refine ⟨r, by simp, ?_⟩
        have hr : Row.WellFormed signature [] r := hmatrix r (by simp)
        match r, hr with
        | [], _ => rfl
  | case2 matrix alternatives v ts ih =>
    -- An alternation *is* reachable here: `expand` splits only the ones at the
    -- top of a column, so `.Some(true | false)` reaches the algorithm intact.
    intro hfalse hmatrix hrow values htyped hbounded hmatches
    cases ts with
    | nil => exact absurd hrow (by simp [Row.WellFormed])
    | cons t ts' =>
    cases values with
    | nil => cases htyped
    | cons v₀ vs' =>
    have hsplit : (Pattern.matchesAny alternatives v₀ && Pattern.matchesAll v vs') = true := hmatches
    obtain ⟨hany, htail⟩ := Bool.and_eq_true .. |>.mp hsplit
    obtain ⟨q, hq, hqm⟩ := Pattern.matchesAny_iff.mp hany
    rw [isUseful_or] at hfalse
    have hstep := List.any_eq_false.mp hfalse ⟨q, hq⟩ (List.mem_attach _ _)
    simp only [Bool.not_eq_true] at hstep
    refine ih ⟨q, hq⟩ hstep hmatrix ⟨?_, hrow.2⟩ _ htyped hbounded ?_
    · exact Pattern.WellFormedAlternatives.mem hrow.1 hq
    · show (Pattern.matches q v₀ && Pattern.matchesAll v vs') = true
      simp [hqm, htail]
  | case3 matrix head subpatterns v ts headType headArity ih =>
    intro hfalse hmatrix hrow values htyped hbounded hmatches
    cases ts with
    | nil => exact absurd hrow (by simp [Row.WellFormed])
    | cons t ts' =>
    cases values with
    | nil => cases htyped
    | cons v₀ vs' =>
    cases htyped with
    | cons hv₀ hvs' =>
    have hnotrest : ∀ n, head ≠ .arrayRest n := by
      intro n hn
      exact Row.WellFormed.head_not_rest hrow n subpatterns v (by rw [hn])
    have harity : subpatterns.length ≤ head.arity signature t :=
      Row.WellFormed.head_arity hrow rfl
    simp only [Row.matches, Pattern.matchesAll_cons, Bool.and_eq_true,
      Pattern.matches_constructor _ _ _ hnotrest, decide_eq_true_eq] at hmatches
    obtain ⟨⟨hasTarget, hsubs⟩, htail⟩ := hmatches
    have hfields : v₀.fields.length = head.arity signature t :=
      HasType.fields_length hv₀ hasTarget
    have hspec : specializeRow head (head.arity signature t)
        (Pattern.constructor head subpatterns :: v)
        = some ((subpatterns ++ List.replicate (head.arity signature t) Pattern.wildcard).take
            (head.arity signature t) ++ v) := by
      simp [specializeRow]
    have hmatches' : Row.matches
        ((subpatterns ++ List.replicate (head.arity signature t) Pattern.wildcard).take
          (head.arity signature t) ++ v) (v₀.fields ++ vs') = true := by
      show Pattern.matchesAll _ _ = true
      rw [Pattern.matchesAll_append (by
            simp only [List.length_take, List.length_append, List.length_replicate, hfields]
            omega),
          Pattern.matchesAll_pad harity hfields]
      simp [hsubs, htail]
    rw [isUseful_constructor] at hfalse
    refine covers_lift hmatrix hasTarget hfields ?_
    exact ih hfalse
      (Matrix.WellFormed.specialize (target := head) hmatrix)
      (Row.WellFormed.specializeRow (target := head) hrow hspec)
      _ (Forall₂.append (HasType.fields_typed hv₀ hasTarget) hvs')
      (Value.BoundedList.append
        (Value.BoundedList_iff.mpr (Value.Bounded.fields hbounded.1)) hbounded.2)
      hmatches'
  | case4 matrix v ts headType all hall hcomplete ih =>
    intro hfalse hmatrix hrow values htyped hbounded hmatches
    cases ts with
    | nil => exact absurd hrow (by simp [Row.WellFormed])
    | cons t ts' =>
    cases values with
    | nil => cases htyped
    | cons v₀ vs' =>
    cases htyped with
    | cons hv₀ hvs' =>
    -- The value's constructor is one the algorithm split on.
    obtain ⟨target, hasTarget, hmem⟩ :=
      HasType.constructorOf_mem hv₀ hall hbounded.1
    have hfields : v₀.fields.length = target.arity signature t :=
      HasType.fields_length hv₀ hasTarget
    have htail : Pattern.matchesAll v vs' = true := by
      have hm : Pattern.matchesAll (Pattern.wildcard :: v) (v₀ :: vs') = true := hmatches
      simpa using hm
    have hrow' : Row.WellFormed signature
        (target.fieldTypes signature t ++ ts')
        (List.replicate (target.arity signature t) Pattern.wildcard ++ v) := by
      refine Row.WellFormed.append ?_ hrow.2
      rw [← Constructor.fieldTypes_length signature target t]
      exact Row.WellFormed.replicate_wildcard signature _
    have hmatches' : Row.matches
        (List.replicate (target.arity signature t) Pattern.wildcard ++ v)
        (v₀.fields ++ vs') = true := by
      show Pattern.matchesAll _ _ = true
      rw [Pattern.matchesAll_append (by simp [hfields]),
          Pattern.matchesAll_replicate_wildcard _ _ (by omega)]
      simp [htail]
    rw [isUseful_wildcard_complete signature limit matrix v (t :: ts') all hall hcomplete] at hfalse
    have hstep := List.any_eq_false.mp hfalse ⟨target, hmem⟩ (List.mem_attach _ _)
    simp only [Bool.not_eq_true] at hstep
    refine covers_lift hmatrix hasTarget hfields ?_
    exact ih ⟨target, hmem⟩ hstep
      (Matrix.WellFormed.specialize (target := target) hmatrix) hrow'
      _ (Forall₂.append (HasType.fields_typed hv₀ hasTarget) hvs')
      (Value.BoundedList.append
        (Value.BoundedList_iff.mpr (Value.Bounded.fields hbounded.1)) hbounded.2)
      hmatches'
  | case5 matrix v ts headType all hall hincomplete ih =>
    intro hfalse hmatrix hrow values htyped hbounded hmatches
    cases ts with
    | nil => exact absurd hrow (by simp [Row.WellFormed])
    | cons t ts' =>
    cases values with
    | nil => cases htyped
    | cons v₀ vs' =>
    cases htyped with
    | cons hv₀ hvs' =>
    rw [isUseful_wildcard_incomplete signature limit matrix v (t :: ts') all hall
      hincomplete] at hfalse
    refine covers_of_defaultMatrix (ih hfalse (Matrix.WellFormed.defaultMatrix hmatrix)
      hrow.2 _ hvs' hbounded.2 ?_)
    show Pattern.matchesAll v vs' = true
    have hm : Pattern.matchesAll (Pattern.wildcard :: v) (v₀ :: vs') = true := hmatches
    simpa using hm
  | case6 matrix v ts headType hall ih =>
    intro hfalse hmatrix hrow values htyped hbounded hmatches
    cases ts with
    | nil => exact absurd hrow (by simp [Row.WellFormed])
    | cons t ts' =>
    cases values with
    | nil => cases htyped
    | cons v₀ vs' =>
    cases htyped with
    | cons hv₀ hvs' =>
    rw [isUseful_wildcard_unbounded signature limit matrix v (t :: ts') hall] at hfalse
    refine covers_of_defaultMatrix (ih hfalse (Matrix.WellFormed.defaultMatrix hmatrix)
      hrow.2 _ hvs' hbounded.2 ?_)
    show Pattern.matchesAll v vs' = true
    have hm : Pattern.matchesAll (Pattern.wildcard :: v) (v₀ :: vs') = true := hmatches
    simpa using hm

end Buri
