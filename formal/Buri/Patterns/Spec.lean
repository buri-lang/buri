import Buri.Patterns.Correct

/-!
# What the algorithm is supposed to compute

`Useful` is the specification; `usefulDec` is the implementation. The
exhaustiveness theorems are the two directions relating them.

## Which direction matters for what

This is worth being precise about, because the two directions protect
different things and only one of them is a safety property.

* **`usefulDec = true → Useful`** ("no false positives"). The compiler reports
  a non-exhaustive match only when a value really is uncovered. This is the
  *expressiveness* direction: without it the checker could reject programs that
  are perfectly fine.

* **`Useful → usefulDec = true`** ("no false negatives"). If a value is
  uncovered, the compiler finds it. This is the *safety* direction, and it is
  the one that makes `match` progress: a well-typed `match` that the checker
  accepted always has an arm that fires, so evaluation never gets stuck. It is
  also the direction `exhaustive_correct` below is a restatement of.

Note the asymmetry with the surface diagnostics: the compiler *reports* when
`usefulDec` is true and *accepts* when it is false, so it is the false-negative
direction that can let a bad program through.
-/

namespace Buri

/-- `v` is useful against `P` when some well-typed value vector matches `v` and
no row of `P`. -/
def Useful (S : Sig) (ts : List Ty) (P : List Row) (v : Row) : Prop :=
  ∃ vs, Forall₂ (HasType S) vs ts ∧ Row.matches v vs = true ∧ ¬ Matrix.covers P vs

/-- A `match` is exhaustive when a bare wildcard is no longer useful against
its arms -- `exhaust.rs:478`. -/
def exhaustiveDec (S : Sig) (limit : Nat) (t : Ty) (arms : List Row) : Bool :=
  !usefulDec S limit arms [Pat.wild] [t]

/-- A `let` pattern must be irrefutable (SPEC 14 rule 2), which is the same
question asked of a one-row matrix. -/
def irrefutableDec (S : Sig) (limit : Nat) (t : Ty) (p : Pat) : Bool :=
  exhaustiveDec S limit t [[p]]

/-!
## The matrix invariant

Every hypothesis the correctness lemmas in `Correct.lean` need, bundled. It is
preserved by `specialize` and `defaultMat`, which is what lets the induction in
`useful_complete` go through.

`arity` is the head column's arity, so the `subs.length ≤ arity` clause is the
one discussed at length in `Correct.lean`: it records that `exhaust.rs:114`
never builds a pattern with more sub-patterns than its constructor has fields.
-/
structure MatrixWF (P : List Row) (width : Nat) (arity : Nat) : Prop where
  /-- Every row has as many columns as the value vector has values. -/
  width_eq : ∀ r ∈ P, r.length = width
  /-- Or-patterns were removed by `expand` (`exhaust.rs:194`). -/
  orFree : ∀ r ∈ P, ∀ alts rest, r ≠ .or alts :: rest
  /-- Rest patterns were removed by `expand_lengths` (`exhaust.rs:163`). -/
  restFree : ∀ r ∈ P, ∀ n subs rest, r ≠ .ctor (.arrayRest n) subs :: rest
  /-- No pattern carries more sub-patterns than its constructor has fields. -/
  subArity : ∀ r ∈ P, ∀ c subs rest, r = .ctor c subs :: rest → subs.length ≤ arity

theorem MatrixWF.rows_ne_nil {P : List Row} {w a : Nat} (h : MatrixWF P (w + 1) a)
    (r : Row) (hr : r ∈ P) : r ≠ [] := by
  intro hnil
  have := h.width_eq r hr
  rw [hnil] at this
  simp at this

/-!
## Status

What is proved, with no `sorry` anywhere in this development:

* `Ctor.fieldTys_length` -- arity and field types always agree.
* `HasType.*_inv` -- canonical forms for every type former.
* `usefulDec` **terminates**, by the lexicographic measure in `Measure.lean`.
  The load-bearing step is `Mat.nodes_specialize_lt`: the algorithm may only
  expand a wildcard when the matrix already accounts for every constructor,
  and that is exactly what makes the expansion pay for itself.
* `covers_specialize` and `covers_defaultMat` -- the two matrix operations
  compute the coverage they are supposed to (`specialize_correct` and
  `default_correct`).

What is **not** proved yet, stated precisely so the gap is not mistaken for a
detail:

1. `useful_complete : MatrixWF .. → Useful S ts P v → usefulDec S limit P v ts = true`,
   and its corollary `exhaustive_correct`. The two `covers_*` lemmas above are
   its inductive steps; what remains is the induction itself (via
   `usefulDec.induct`) plus showing `MatrixWF` is preserved by each step.
2. `useful_sound`, the converse. It additionally needs *inhabitation* -- to
   exhibit a witness at a missing constructor you must build values at its
   field types -- which is where SPEC 14 rule 14 (recursive types must be
   productive) becomes load-bearing rather than stylistic.
3. **The array case, which is the one to watch.** `allCtors` reports an array
   type's constructors as `array 0 .. array limit` and nothing longer, so the
   algorithm reasons in a universe where arrays are bounded. That is sound only
   because of an invariant the algorithm never states:

   > `limit` is one more than the longest array length any pattern mentions
   > (`exhaust.rs:450`), and fixed-length patterns therefore only ever produce
   > heads `array k` with `k < limit`. So `array limit ∈ headCtors P` can only
   > have come from an expanded *rest* pattern -- which covers every length at
   > or above its minimum. Hence whenever the algorithm believes the array
   > constructors are complete, some rest pattern really does cover the tail.

   Both halves of `expandLengths`'s correctness turn on this, and the plan
   flagged array-rest as the likeliest place for a real bug. Reading the
   algorithm did not turn one up -- the `+ 1` at `exhaust.rs:450` is doing
   exactly the work it needs to -- but "did not turn one up" is not a proof,
   and this is the obligation that would settle it.
-/

end Buri
