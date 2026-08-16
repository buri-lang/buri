import Buri.Patterns.Correct

/-!
# What the algorithm is supposed to compute

`Useful` is the specification; `isUseful` is the implementation. The
exhaustiveness theorems are the two directions relating them.

## Which direction matters for what

This is worth being precise about, because the two directions protect
different things and only one of them is a safety property.

* **`isUseful = true → Useful`** ("no false positives"). The compiler reports
  a non-exhaustive match only when a value really is uncovered. This is the
  *expressiveness* direction: without it the checker could reject programs that
  are perfectly fine.

* **`Useful → isUseful = true`** ("no false negatives"). If a value is
  uncovered, the compiler finds it. This is the *safety* direction, and it is
  the one that makes `match` progress: a well-typed `match` that the checker
  accepted always has an arm that fires, so evaluation never gets stuck. It is
  also the direction `exhaustive_correct` below is a restatement of.

Note the asymmetry with the surface diagnostics: the compiler *reports* when
`isUseful` is true and *accepts* when it is false, so it is the false-negative
direction that can let a bad program through.
-/

namespace Buri

/-- `v` is useful against `P` when some well-typed value vector matches `v` and
no row of `P`. -/
def Useful (S : Signature) (ts : List Ty) (P : List Row) (v : Row) : Prop :=
  ∃ vs, Forall₂ (HasType S) vs ts ∧ Row.matches v vs = true ∧ ¬ Matrix.covers P vs

/-- A `match` is exhaustive when a bare wildcard is no longer useful against
its arms -- `exhaust.rs:478`. -/
def isExhaustive (S : Signature) (limit : Nat) (t : Ty) (arms : List Row) : Bool :=
  !isUseful S limit arms [Pattern.wildcard] [t]

/-- A `let` pattern must be irrefutable (SPEC 14 rule 2), which is the same
question asked of a one-row matrix. -/
def isIrrefutable (S : Signature) (limit : Nat) (t : Ty) (p : Pattern) : Bool :=
  isExhaustive S limit t [[p]]

/-!
## The matrix invariant

Every hypothesis the correctness lemmas in `Correct.lean` need, bundled. It is
preserved by `specialize` and `defaultMatrix`, which is what lets the induction in
`useful_complete` go through.

`arity` is the head column's arity, so the `subpatterns.length ≤ arity` clause is the
one discussed at length in `Correct.lean`: it records that `exhaust.rs:114`
never builds a pattern with more sub-patterns than its constructor has fields.
-/
structure MatrixWellFormed (P : List Row) (width : Nat) (arity : Nat) : Prop where
  /-- Every row has as many columns as the value vector has values. -/
  width_eq : ∀ r ∈ P, r.length = width
  /-- Or-patterns were removed by `expand` (`exhaust.rs:194`). -/
  orFree : ∀ r ∈ P, ∀ alternatives rest, r ≠ .or alternatives :: rest
  /-- Rest patterns were removed by `expand_lengths` (`exhaust.rs:163`). -/
  restFree : ∀ r ∈ P, ∀ n subpatterns rest, r ≠ .constructor (.arrayRest n) subpatterns :: rest
  /-- No pattern carries more sub-patterns than its constructor has fields. -/
  subArity : ∀ r ∈ P, ∀ c subpatterns rest, r = .constructor c subpatterns :: rest → subpatterns.length ≤ arity

theorem MatrixWellFormed.rows_ne_nil {P : List Row} {w a : Nat} (h : MatrixWellFormed P (w + 1) a)
    (r : Row) (hr : r ∈ P) : r ≠ [] := by
  intro hnil
  have := h.width_eq r hr
  rw [hnil] at this
  simp at this

/-!
## Status

Proved, with no `sorry` and no axioms beyond Lean's three
(`propext`, `Quot.sound`, `Classical.choice`) -- `Audit.lean` checks that:

* `isUseful` **terminates** -- `Measure.lean`, via the lexicographic measure.
* `covers_specialize`, `covers_of_specialize`, `covers_of_defaultMatrix` --
  the matrix operations compute the coverage they should.
* `Matrix.WellFormed.specialize`, `.defaultMatrix` -- the invariant survives
  both steps.
* `isUseful_false_covers` -- **completeness**: a `false` answer really does
  mean the matrix covers everything.
* `exhaustive_correct`, `unreachable_correct`, `irrefutable_correct` --
  `Exhaustive.lean`, the three claims the compiler makes, inside the bounded
  universe.
* `expandLengths_sound`, `topDisjuncts_sound` -- the two rewrites the compiler
  applies first only narrow what a pattern matches.
* `Pattern.matches_truncate` -- no arm can tell a long array from its
  truncation, since `limit` exceeds every length any arm mentions.
* `exhaustive_correct_unbounded` -- `EndToEnd.lean`: the array-length
  restriction removed, stated over the arms the programmer wrote.

Still open:

* **`useful_sound`**, the converse ("no false positives"): the compiler reports
  a non-exhaustive match only when a value really is uncovered. It needs
  *inhabitation* -- exhibiting a witness at a missing constructor means
  building values at its field types -- which is where SPEC 14 rule 14
  (recursive types must be productive) becomes load-bearing rather than
  stylistic. This is the expressiveness direction, not the safety one, and
  `findings/README.md` §6 records a case where it is in fact **false**: a
  nested alternation makes the checker reject an exhaustive match.
-/

end Buri
