import Buri.Patterns.Measure

/-!
# The usefulness algorithm

`Ctx::useful` (`exhaust.rs:293`), as a total function. Rust returns
`Option<Vec<Witness>>` -- the witness is what the diagnostic renders -- but the
witness is a *presentation* concern: the checker branches only on
`.is_some()`. So this returns `Bool`, and the existence of a witness is
recovered as a theorem (`useful_sound`) rather than carried as data.

The four branches are exactly Rust's, in the same order.
-/

namespace Buri

theorem Pattern.nodeCount_le_nodeCountList {p : Pattern} {ps : List Pattern} (h : p ∈ ps) :
    Pattern.nodeCount p ≤ Pattern.nodeCountList ps := by
  induction ps with
  | nil => exact absurd h List.not_mem_nil
  | cons x xs ih =>
    rcases List.mem_cons.mp h with rfl | h'
    · simp [Pattern.nodeCountList]
    · have := ih h'
      simp only [Pattern.nodeCountList]
      omega

/-- Is the pattern vector `v` useful against the matrix `P` -- does it match
some value vector that no row of `P` matches?

`limit` is the array-length bound of `exhaust.rs:450`: one more than the
longest array length any pattern in the match distinguishes. -/
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
      match hall : allConstructors S limit t with
      | some all =>
          if hc : all.all (fun c => decide (c ∈ headConstructors P)) then
            all.attach.any fun c =>
              isUseful S limit (specialize P c.1 (c.1.arity S t))
                (List.replicate (c.1.arity S t) Pattern.wildcard ++ v)
                (c.1.fieldTypes S t ++ ts.tail)
          else
            isUseful S limit (defaultMatrix P) v ts.tail
      | none => isUseful S limit (defaultMatrix P) v ts.tail
termination_by P v _ => (Matrix.nodeCount P + Row.nodeCount v, v.length)
decreasing_by
  -- `or`: the alternative is a strict sub-pattern of the alternation.
  · left
    have hp : Pattern.nodeCount p.1 ≤ Pattern.nodeCountList alternatives := Pattern.nodeCount_le_nodeCountList p.2
    simp only [Row.nodeCount_cons, Pattern.nodeCount]
    omega
  -- `constructor`: the head node is consumed.
  · left
    have hm := Matrix.nodeCount_specialize_le P c (c.arity S (ts.headD .unit))
    have hr := Row.nodeCount_take_le (c.arity S (ts.headD .unit))
      (subpatterns ++ List.replicate (c.arity S (ts.headD .unit)) Pattern.wildcard)
    simp only [Row.nodeCount_append, Row.nodeCount_replicate_wildcard, Nat.add_zero] at hr
    simp only [Row.nodeCount_cons, Row.nodeCount_append, Pattern.nodeCount_constructor]
    omega
  -- `wildcard`, complete: the vector's node count is unchanged, so the matrix
  -- must shrink -- and it does, because completeness put this very
  -- constructor at the head of some row.
  · left
    have hmem : c.1 ∈ headConstructors P := by
      have := List.all_eq_true.mp hc c.1 c.2
      simpa using this
    have := Matrix.nodeCount_specialize_lt P c.1 (c.1.arity S (ts.headD .unit)) hmem
    simp only [Row.nodeCount_cons, Row.nodeCount_append, Row.nodeCount_replicate_wildcard,
      Pattern.nodeCount_wildcard, Nat.zero_add]
    omega
  -- `wildcard`, incomplete: `defaultMatrix` only drops rows, so the first component
  -- falls or stays put; when it stays put the vector still loses a column.
  · have hle := Matrix.nodeCount_defaultMatrix_le P
    rcases Nat.lt_or_ge (Matrix.nodeCount (defaultMatrix P)) (Matrix.nodeCount P) with hlt | hge
    · left
      simp only [Row.nodeCount_cons, Pattern.nodeCount_wildcard, Nat.zero_add]
      omega
    · have heq : Matrix.nodeCount (defaultMatrix P) = Matrix.nodeCount P := Nat.le_antisymm hle hge
      simp only [Row.nodeCount_cons, Pattern.nodeCount_wildcard, Nat.zero_add, heq]
      right
      simp
  · have hle := Matrix.nodeCount_defaultMatrix_le P
    rcases Nat.lt_or_ge (Matrix.nodeCount (defaultMatrix P)) (Matrix.nodeCount P) with hlt | hge
    · left
      simp only [Row.nodeCount_cons, Pattern.nodeCount_wildcard, Nat.zero_add]
      omega
    · have heq : Matrix.nodeCount (defaultMatrix P) = Matrix.nodeCount P := Nat.le_antisymm hle hge
      simp only [Row.nodeCount_cons, Pattern.nodeCount_wildcard, Nat.zero_add, heq]
      right
      simp

/-!
## Per-branch equations

`isUseful` is defined by well-founded recursion, so it does not reduce
definitionally. These four lemmas expose one unfolding step per branch, which
is all any proof about it ever needs. Stating them here keeps the `match hall :
...` dependent-match plumbing in one place instead of in every proof.
-/

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

`split` normalises `List.headD` to `List.head?.getD` in some branches but not
others, so each branch tries the hypothesis in both forms. -/
theorem isUseful_wildcard_complete (signature : Signature) (limit : Nat) (matrix : List Row)
    (v : List Pattern) (types : List Ty) (all : List Constructor)
    (hall : allConstructors signature limit (types.headD .unit) = some all)
    (hcomplete : (all.all fun c => decide (c ∈ headConstructors matrix)) = true) :
    isUseful signature limit matrix (.wildcard :: v) types
      = all.attach.any (fun c => isUseful signature limit
          (specialize matrix c.1 (c.1.arity signature (types.headD .unit)))
          (List.replicate (c.1.arity signature (types.headD .unit)) Pattern.wildcard ++ v)
          (c.1.fieldTypes signature (types.headD .unit) ++ types.tail)) := by
  have hall' : allConstructors signature limit (types.head?.getD Ty.unit) = some all := by
    rwa [List.headD_eq_head?_getD] at hall
  rw [isUseful.eq_4]
  split
  · next all' heq =>
    first
      | (rw [hall] at heq)
      | (rw [hall'] at heq)
    cases heq
    rw [dif_pos hcomplete]
  · next heq =>
    first
      | (rw [hall] at heq)
      | (rw [hall'] at heq)
      | skip
    try simp at heq

/-- The `wildcard` branch, when some constructor is missing from the matrix. -/
theorem isUseful_wildcard_incomplete (signature : Signature) (limit : Nat) (matrix : List Row)
    (v : List Pattern) (types : List Ty) (all : List Constructor)
    (hall : allConstructors signature limit (types.headD .unit) = some all)
    (hincomplete : ¬ (all.all fun c => decide (c ∈ headConstructors matrix)) = true) :
    isUseful signature limit matrix (.wildcard :: v) types
      = isUseful signature limit (defaultMatrix matrix) v types.tail := by
  have hall' : allConstructors signature limit (types.head?.getD Ty.unit) = some all := by
    rwa [List.headD_eq_head?_getD] at hall
  rw [isUseful.eq_4]
  split
  · next all' heq =>
    first
      | (rw [hall] at heq)
      | (rw [hall'] at heq)
    cases heq
    rw [dif_neg hincomplete]
  · next heq =>
    first
      | (rw [hall] at heq)
      | (rw [hall'] at heq)
      | skip
    try simp at heq

/-- The `wildcard` branch at a type whose constructor set is too large to
enumerate -- an integer, string, char or float, or a function, context, or type
parameter. SPEC 7.3: such a match needs a `_` arm. -/
theorem isUseful_wildcard_unbounded (signature : Signature) (limit : Nat) (matrix : List Row)
    (v : List Pattern) (types : List Ty)
    (hall : allConstructors signature limit (types.headD .unit) = none) :
    isUseful signature limit matrix (.wildcard :: v) types
      = isUseful signature limit (defaultMatrix matrix) v types.tail := by
  have hall' : allConstructors signature limit (types.head?.getD Ty.unit) = none := by
    rwa [List.headD_eq_head?_getD] at hall
  rw [isUseful.eq_4]
  split
  · next all' heq =>
    first
      | (rw [hall] at heq)
      | (rw [hall'] at heq)
      | skip
    try simp at heq
  · next heq => rfl

end Buri
