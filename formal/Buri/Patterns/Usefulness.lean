import Buri.Patterns.Measure

/-!
# The usefulness algorithm

`Ctx::useful` (`exhaustiveness.rs`), as a total function -- no fuel, no
partiality. `Measure.lean` is the argument that it is one.
The algorithm is Maranget's, from *Warnings for pattern matching* (JFP 2007),
linked from `reference/README.md`.
-/

namespace Buri

-- `hc` below is referenced only from `decreasing_by`, which the
-- unused-variable linter does not look at; it is what tells the termination
-- argument that the constructor being specialised on already heads a row.
set_option linter.unusedVariables false in
/-- Is the pattern vector `v` useful against the matrix `P` -- does it match
some value vector that no row of `P` matches?

`limit` is the array-length bound of `exhaustiveness.rs`: one more than the
longest array length any pattern in the match distinguishes.

The four branches are exactly Rust's, in the same order. Rust returns
`Option<Vec<Witness>>` -- the witness is what the diagnostic renders -- but the
witness is a *presentation* concern: the checker branches only on `.is_some()`.
So this returns `Bool`, and the existence of a witness is recovered as a
theorem rather than carried as data. -/
def isUseful (S : Signature) (limit : Nat) : List Row → Row → List Ty → Bool
  | P, [], _ => P.isEmpty
  | P, .or alternatives :: v, ts =>
      alternatives.attach.any fun p => isUseful S limit P (p.1 :: v) ts
  | P, .constructor c subpatterns :: v, ts =>
      let t := ts.headD .unit
      let a := c.arity S t
      isUseful S limit (specialize P c a)
        ((subpatterns ++ List.replicate a Pattern.wildcard).take a ++ v)
        (c.fieldTypes S t ++ ts.tail)
  | P, .wildcard :: v, ts =>
      let t := ts.headD .unit
      match allConstructors S limit t with
      | some all =>
          if hc : all.all (fun c => decide (c ∈ headConstructors P)) then
            all.attach.any fun c =>
              isUseful S limit (specialize P c.1 (c.1.arity S t))
                (List.replicate (c.1.arity S t) Pattern.wildcard ++ v)
                (c.1.fieldTypes S t ++ ts.tail)
          else
            isUseful S limit (defaultMatrix P) v ts.tail
      | none => isUseful S limit (defaultMatrix P) v ts.tail
termination_by P v _ => (Matrix.weight P + Row.weight v, v.length)
decreasing_by
  -- `or`: the alternative weighs less than the alternation, and the tail of
  -- the vector weighs at least one, so the product drops.
  · left
    have hp : Pattern.weight p.1 ≤ Pattern.weightSum alternatives :=
      Pattern.weight_le_weightSum p.2
    have hv := Row.one_le_weight v
    simp only [Row.weight_cons, Pattern.weight_or, Nat.add_mul, Nat.one_mul]
    have := Nat.mul_le_mul_right (Row.weight v) hp
    omega
  -- `constructor`: the head node is consumed, and the matrix cannot grow.
  · left
    have hm := Matrix.weight_specialize_le P c (c.arity S (ts.headD .unit))
    have hv := Row.one_le_weight v
    have hpad := Nat.mul_le_mul_right (Row.weight v)
      (Row.weight_pad_le subpatterns (c.arity S (ts.headD .unit)))
    simp only [Row.weight_cons, Row.weight_append, Pattern.weight_constructor,
      Nat.add_mul, Nat.one_mul]
    omega
  -- `wildcard`, complete: the vector's weight is unchanged -- `_` and `arity`
  -- copies of `_` both weigh one -- so the matrix must shrink. And it does,
  -- because completeness put this very constructor at the head of some row.
  · left
    have hmem : c.1 ∈ headConstructors P := by
      have := List.all_eq_true.mp hc c.1 c.2
      simpa using this
    have := Matrix.weight_specialize_lt P c.1 (c.1.arity S (ts.headD .unit)) hmem
    simp only [Row.weight_cons, Row.weight_append, Row.weight_replicate_wildcard,
      Pattern.weight_wildcard, Nat.one_mul]
    omega
  -- `wildcard`, incomplete: `defaultMatrix` cannot grow the matrix, so the
  -- first component falls or stays put; when it stays put the vector still
  -- loses a column.
  · have hle := Matrix.weight_defaultMatrix_le P
    rcases Nat.lt_or_ge (Matrix.weight (defaultMatrix P)) (Matrix.weight P) with hlt | hge
    · left
      simp only [Row.weight_cons, Pattern.weight_wildcard, Nat.one_mul]
      omega
    · have heq : Matrix.weight (defaultMatrix P) = Matrix.weight P := Nat.le_antisymm hle hge
      simp only [Row.weight_cons, Pattern.weight_wildcard, Nat.one_mul, heq]
      right
      simp
  · have hle := Matrix.weight_defaultMatrix_le P
    rcases Nat.lt_or_ge (Matrix.weight (defaultMatrix P)) (Matrix.weight P) with hlt | hge
    · left
      simp only [Row.weight_cons, Pattern.weight_wildcard, Nat.one_mul]
      omega
    · have heq : Matrix.weight (defaultMatrix P) = Matrix.weight P := Nat.le_antisymm hle hge
      simp only [Row.weight_cons, Pattern.weight_wildcard, Nat.one_mul, heq]
      right
      simp

/-!
## Per-branch equations

`isUseful` is defined by well-founded recursion, so it does not reduce
definitionally. These four lemmas expose one unfolding step per branch, which
is all any proof about it ever needs. Stating them here keeps the `match hall :
...` dependent-match plumbing in one place instead of in every proof.
-/

/-- The empty pattern vector: useful exactly when the matrix has no rows. -/
theorem isUseful_nil (signature : Signature) (limit : Nat) (matrix : List Row) (types : List Ty) :
    isUseful signature limit matrix [] types = matrix.isEmpty :=
  isUseful.eq_1 signature limit matrix types

/-- The `or` branch. Reachable, despite `expand`: that pass splits only the
alternations at the top of a column, so a nested one survives into the matrix
and `specialize` later exposes it. -/
theorem isUseful_or (signature : Signature) (limit : Nat) (matrix : List Row)
    (alternatives v : List Pattern) (types : List Ty) :
    isUseful signature limit matrix (.or alternatives :: v) types
      = alternatives.attach.any (fun p => isUseful signature limit matrix (p.1 :: v) types) :=
  isUseful.eq_2 signature limit matrix types alternatives v

/-- The `constructor` branch. This is Lean's generated `isUseful.eq_3`, named
for readability. -/
theorem isUseful_constructor (signature : Signature) (limit : Nat) (matrix : List Row)
    (head : Constructor) (subpatterns v : List Pattern) (types : List Ty) :
    isUseful signature limit matrix (.constructor head subpatterns :: v) types
      = isUseful signature limit
          (specialize matrix head (head.arity signature (types.headD .unit)))
          ((subpatterns ++ List.replicate
              (head.arity signature (types.headD .unit)) Pattern.wildcard).take
            (head.arity signature (types.headD .unit)) ++ v)
          (head.fieldTypes signature (types.headD .unit) ++ types.tail) :=
  isUseful.eq_3 signature limit matrix types head subpatterns v

/-- The `wildcard` branch, when the constructor set is enumerable and the
matrix mentions every constructor.

The three `wildcard` equations share one wrinkle: `split` normalises
`List.headD` to `List.head?.getD` in some branches but not others, so the
hypothesis and the branch equation can end up in different forms. `simp_all`
reconciles them, which is why these are one line each rather than a
case-by-case rewrite. -/
theorem isUseful_wildcard_complete (signature : Signature) (limit : Nat) (matrix : List Row)
    (v : List Pattern) (types : List Ty) (all : List Constructor)
    (hall : allConstructors signature limit (types.headD .unit) = some all)
    (hcomplete : (all.all fun c => decide (c ∈ headConstructors matrix)) = true) :
    isUseful signature limit matrix (.wildcard :: v) types
      = all.attach.any (fun c => isUseful signature limit
          (specialize matrix c.1 (c.1.arity signature (types.headD .unit)))
          (List.replicate (c.1.arity signature (types.headD .unit)) Pattern.wildcard ++ v)
          (c.1.fieldTypes signature (types.headD .unit) ++ types.tail)) := by
  rw [isUseful.eq_4]; split <;> simp_all

/-- The `wildcard` branch, when some constructor is missing from the matrix. -/
theorem isUseful_wildcard_incomplete (signature : Signature) (limit : Nat) (matrix : List Row)
    (v : List Pattern) (types : List Ty) (all : List Constructor)
    (hall : allConstructors signature limit (types.headD .unit) = some all)
    (hincomplete : ¬ (all.all fun c => decide (c ∈ headConstructors matrix)) = true) :
    isUseful signature limit matrix (.wildcard :: v) types
      = isUseful signature limit (defaultMatrix matrix) v types.tail := by
  rw [isUseful.eq_4]; split <;> simp_all
  obtain ⟨c, hmem, hmissing⟩ := hincomplete
  exact fun hall' => absurd (hall' c hmem) hmissing

/-- The `wildcard` branch at a type whose constructor set is too large to
enumerate -- an integer, string, char or float, or a function, context, or type
parameter. SPEC 7.3: such a match needs a `_` arm. -/
theorem isUseful_wildcard_unbounded (signature : Signature) (limit : Nat) (matrix : List Row)
    (v : List Pattern) (types : List Ty)
    (hall : allConstructors signature limit (types.headD .unit) = none) :
    isUseful signature limit matrix (.wildcard :: v) types
      = isUseful signature limit (defaultMatrix matrix) v types.tail := by
  rw [isUseful.eq_4]; split <;> simp_all

/-!
## What the algorithm is supposed to compute

`Useful` is the specification; `isUseful` is the implementation, and the
exhaustiveness theorems are the two directions relating them. The two protect
different things, and only one is a safety property:

* **`isUseful = true → Useful`** ("no false positives"). The compiler reports a
  non-exhaustive match only when a value really is uncovered. This is the
  *expressiveness* direction: without it the checker could reject fine
  programs. It is `useful_sound`, in `Sound.lean`.

* **`Useful → isUseful = true`** ("no false negatives"). If a value is
  uncovered, the compiler finds it. This is the *safety* direction, and the one
  that makes `match` progress: a well-typed `match` the checker accepted always
  has an arm that fires. It is `isUseful_false_covers`, in `Complete.lean`.

Note the asymmetry with the surface diagnostics: the compiler *reports* when
`isUseful` is true and *accepts* when it is false, so it is the false-negative
direction that could let a bad program through.
-/

/-- `v` is useful against `P` when some well-typed value vector matches `v` and
no row of `P`. -/
def Useful (S : Signature) (ts : List Ty) (P : List Row) (v : Row) : Prop :=
  ∃ vs, Forall₂ (HasType S) vs ts ∧ Row.matches v vs = true ∧ ¬ Matrix.covers P vs

/-- A `match` is exhaustive when a bare wildcard is no longer useful against
its arms -- `exhaust.rs:478`. -/
def isExhaustive (S : Signature) (limit : Nat) (t : Ty) (arms : List Row) : Bool :=
  !isUseful S limit arms [Pattern.wildcard] [t]

/-- A `let` pattern must be irrefutable (design/static-rules.md rule 2), which is the same
question asked of a one-row matrix. -/
def isIrrefutable (S : Signature) (limit : Nat) (t : Ty) (p : Pattern) : Bool :=
  isExhaustive S limit t [[p]]

end Buri
