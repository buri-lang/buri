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

theorem Pat.nodes_le_nodesList {p : Pat} {ps : List Pat} (h : p ∈ ps) :
    Pat.nodes p ≤ Pat.nodesList ps := by
  induction ps with
  | nil => exact absurd h List.not_mem_nil
  | cons x xs ih =>
    rcases List.mem_cons.mp h with rfl | h'
    · simp [Pat.nodesList]
    · have := ih h'
      simp only [Pat.nodesList]
      omega

/-- Is the pattern vector `v` useful against the matrix `P` -- does it match
some value vector that no row of `P` matches?

`limit` is the array-length bound of `exhaust.rs:450`: one more than the
longest array length any pattern in the match distinguishes. -/
def usefulDec (S : Sig) (limit : Nat) : List Row → Row → List Ty → Bool
  | P, [], _ => P.isEmpty
  | P, .or alts :: v, ts =>
      alts.attach.any fun p => usefulDec S limit P (p.1 :: v) ts
  | P, .ctor c subs :: v, ts =>
      let t := ts.headD .unit
      let a := c.arity S t
      usefulDec S limit (specialize P c a)
        ((subs ++ List.replicate a Pat.wild).take a ++ v)
        (c.fieldTys S t ++ ts.tail)
  | P, .wild :: v, ts =>
      let t := ts.headD .unit
      match hall : allCtors S limit t with
      | some all =>
          if hc : all.all (fun c => decide (c ∈ headCtors P)) then
            all.attach.any fun c =>
              usefulDec S limit (specialize P c.1 (c.1.arity S t))
                (List.replicate (c.1.arity S t) Pat.wild ++ v)
                (c.1.fieldTys S t ++ ts.tail)
          else
            usefulDec S limit (defaultMat P) v ts.tail
      | none => usefulDec S limit (defaultMat P) v ts.tail
termination_by P v _ => (Mat.nodes P + Row.nodes v, v.length)
decreasing_by
  -- `or`: the alternative is a strict sub-pattern of the alternation.
  · left
    have hp : Pat.nodes p.1 ≤ Pat.nodesList alts := Pat.nodes_le_nodesList p.2
    simp only [Row.nodes_cons, Pat.nodes]
    omega
  -- `ctor`: the head node is consumed.
  · left
    have hm := Mat.nodes_specialize_le P c (c.arity S (ts.headD .unit))
    have hr := Row.nodes_take_le (c.arity S (ts.headD .unit))
      (subs ++ List.replicate (c.arity S (ts.headD .unit)) Pat.wild)
    simp only [Row.nodes_append, Row.nodes_replicate_wild, Nat.add_zero] at hr
    simp only [Row.nodes_cons, Row.nodes_append, Pat.nodes_ctor]
    omega
  -- `wild`, complete: the vector's node count is unchanged, so the matrix
  -- must shrink -- and it does, because completeness put this very
  -- constructor at the head of some row.
  · left
    have hmem : c.1 ∈ headCtors P := by
      have := List.all_eq_true.mp hc c.1 c.2
      simpa using this
    have := Mat.nodes_specialize_lt P c.1 (c.1.arity S (ts.headD .unit)) hmem
    simp only [Row.nodes_cons, Row.nodes_append, Row.nodes_replicate_wild,
      Pat.nodes_wild, Nat.zero_add]
    omega
  -- `wild`, incomplete: `defaultMat` only drops rows, so the first component
  -- falls or stays put; when it stays put the vector still loses a column.
  · have hle := Mat.nodes_defaultMat_le P
    rcases Nat.lt_or_ge (Mat.nodes (defaultMat P)) (Mat.nodes P) with hlt | hge
    · left
      simp only [Row.nodes_cons, Pat.nodes_wild, Nat.zero_add]
      omega
    · have heq : Mat.nodes (defaultMat P) = Mat.nodes P := Nat.le_antisymm hle hge
      simp only [Row.nodes_cons, Pat.nodes_wild, Nat.zero_add, heq]
      right
      simp
  · have hle := Mat.nodes_defaultMat_le P
    rcases Nat.lt_or_ge (Mat.nodes (defaultMat P)) (Mat.nodes P) with hlt | hge
    · left
      simp only [Row.nodes_cons, Pat.nodes_wild, Nat.zero_add]
      omega
    · have heq : Mat.nodes (defaultMat P) = Mat.nodes P := Nat.le_antisymm hle hge
      simp only [Row.nodes_cons, Pat.nodes_wild, Nat.zero_add, heq]
      right
      simp

end Buri
