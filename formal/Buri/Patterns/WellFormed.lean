import Buri.Patterns.Decompose

/-!
# Well-formed pattern matrices

The invariant the usefulness algorithm runs under. It bundles the four side
conditions that `Correct.lean`'s coverage lemmas need, and the point of this
file is that `specialize` and `defaultMatrix` both *preserve* it -- which is
what lets the completeness proof induct.

A pattern is well formed at a type when:

* it is not an or-pattern -- `expand` has already split those into rows;
* it is not a rest pattern -- `expand_lengths` has already rewritten those;
* it carries no more sub-patterns than its constructor has fields; and
* each sub-pattern is well formed at the corresponding field type.

The third clause is the one that matters. `specializeRow` pads *and truncates*
to the constructor's arity. Padding is harmless; truncation is not. A pattern
carrying more sub-patterns than the constructor has fields would be rejected by
`matchesAll` (which fails when patterns outlast values) yet accepted after
truncation -- the matrix would gain coverage it never had, and a non-exhaustive
match could be accepted. `lower` never builds such a pattern, because it sizes
the sub-pattern vector by the largest field *index*. This invariant is what
records that the fact is load-bearing rather than incidental.
-/

namespace Buri

mutual

/-- A lowered, expanded pattern, checked against the type it is matched at.

Note what is *not* required: or-freeness. `expand` (`exhaustiveness.rs`) splits
only the alternations it finds at the top of a column, so a nested one -- as in
`.Some(true | false)` -- survives into the matrix and is later exposed by
`specialize`. Requiring or-freeness here would make these theorems inapplicable
to the matrices the compiler actually builds. -/
def Pattern.WellFormed (signature : Signature) : Ty → Pattern → Prop
  | _, .wildcard => True
  | type, .or alternatives => Pattern.WellFormedAlternatives signature type alternatives
  | _, .constructor (.arrayRest _) _ => False
  | type, .constructor head subpatterns =>
      Pattern.WellFormedPrefix signature (head.fieldTypes signature type) subpatterns

/-- The sub-patterns of a constructor: no longer than the field-type list, and
well formed pointwise against it. A *prefix*, not a full list, because
`lower` may produce fewer sub-patterns than there are fields. -/
def Pattern.WellFormedPrefix (signature : Signature) : List Ty → List Pattern → Prop
  | _, [] => True
  | [], _ :: _ => False
  | fieldType :: fieldTypes, p :: ps =>
      Pattern.WellFormed signature fieldType p ∧
      Pattern.WellFormedPrefix signature fieldTypes ps

/-- Every alternative of an or-pattern is well formed at the same type. -/
def Pattern.WellFormedAlternatives (signature : Signature) : Ty → List Pattern → Prop
  | _, [] => True
  | type, p :: ps =>
      Pattern.WellFormed signature type p ∧
      Pattern.WellFormedAlternatives signature type ps

end

theorem Pattern.WellFormedAlternatives.mem {signature : Signature} {type : Ty} :
    ∀ {alternatives : List Pattern} {p : Pattern},
      Pattern.WellFormedAlternatives signature type alternatives → p ∈ alternatives →
      Pattern.WellFormed signature type p := by
  intro alternatives
  induction alternatives with
  | nil => intro p _ hp; exact absurd hp List.not_mem_nil
  | cons q qs ih =>
    intro p h hp
    simp only [Pattern.WellFormedAlternatives] at h
    rcases List.mem_cons.mp hp with rfl | hp'
    · exact h.1
    · exact ih h.2 hp'

/-- A row is well formed at a type vector when it has exactly one pattern per
type and each is well formed. -/
def Row.WellFormed (signature : Signature) : List Ty → Row → Prop
  | [], [] => True
  | type :: types, p :: ps =>
      Pattern.WellFormed signature type p ∧ Row.WellFormed signature types ps
  | _, _ => False

def Matrix.WellFormed (signature : Signature) (types : List Ty) (matrix : List Row) : Prop :=
  ∀ row ∈ matrix, Row.WellFormed signature types row

/-! ## Basic facts -/

theorem Pattern.WellFormed.wildcard {signature : Signature} {type : Ty} :
    Pattern.WellFormed signature type .wildcard := trivial

theorem Row.WellFormed.replicate_wildcard (signature : Signature) :
    ∀ types : List Ty,
      Row.WellFormed signature types (List.replicate types.length Pattern.wildcard) := by
  intro types
  induction types with
  | nil => trivial
  | cons type types ih =>
    simp only [List.length_cons, List.replicate]
    exact ⟨Pattern.WellFormed.wildcard, ih⟩

theorem Row.WellFormed.append {signature : Signature} :
    ∀ {types₁ types₂ : List Ty} {row₁ row₂ : Row},
      Row.WellFormed signature types₁ row₁ → Row.WellFormed signature types₂ row₂ →
      Row.WellFormed signature (types₁ ++ types₂) (row₁ ++ row₂) := by
  intro types₁
  induction types₁ with
  | nil =>
    intro types₂ row₁ row₂ h₁ h₂
    cases row₁ with
    | nil => exact h₂
    | cons _ _ => exact absurd h₁ (by simp [Row.WellFormed])
  | cons type types ih =>
    intro types₂ row₁ row₂ h₁ h₂
    cases row₁ with
    | nil => exact absurd h₁ (by simp [Row.WellFormed])
    | cons p ps =>
      simp only [Row.WellFormed] at h₁
      exact ⟨h₁.1, ih h₁.2 h₂⟩

/-- Padding a well-formed sub-pattern prefix out to the full field list yields a
well-formed row. This is exactly the shape `specializeRow` produces. -/
theorem Pattern.WellFormedPrefix.pad {signature : Signature} :
    ∀ {fieldTypes : List Ty} {subpatterns : List Pattern},
      Pattern.WellFormedPrefix signature fieldTypes subpatterns →
      Row.WellFormed signature fieldTypes
        (subpatterns ++ List.replicate (fieldTypes.length - subpatterns.length) Pattern.wildcard) := by
  intro fieldTypes
  induction fieldTypes with
  | nil =>
    intro subpatterns h
    cases subpatterns with
    | nil => trivial
    | cons _ _ => exact absurd h (by simp [Pattern.WellFormedPrefix])
  | cons fieldType fieldTypes ih =>
    intro subpatterns h
    cases subpatterns with
    | nil =>
      simp only [List.nil_append, List.length_nil, Nat.sub_zero, List.length_cons]
      exact Row.WellFormed.replicate_wildcard signature (fieldType :: fieldTypes)
    | cons p ps =>
      simp only [Pattern.WellFormedPrefix] at h
      simp only [List.cons_append, List.length_cons, Nat.succ_sub_succ]
      exact ⟨h.1, ih h.2⟩

theorem Pattern.WellFormedPrefix.length_le {signature : Signature} :
    ∀ {fieldTypes : List Ty} {subpatterns : List Pattern},
      Pattern.WellFormedPrefix signature fieldTypes subpatterns →
      subpatterns.length ≤ fieldTypes.length := by
  intro fieldTypes
  induction fieldTypes with
  | nil =>
    intro subpatterns h
    cases subpatterns with
    | nil => simp
    | cons _ _ => exact absurd h (by simp [Pattern.WellFormedPrefix])
  | cons fieldType fieldTypes ih =>
    intro subpatterns h
    cases subpatterns with
    | nil => simp
    | cons p ps =>
      simp only [Pattern.WellFormedPrefix] at h
      simpa using ih h.2

/-! ## What a well-formed row's head cannot be -/

theorem Row.WellFormed.nonEmpty {signature : Signature} {type : Ty} {types : List Ty} {row : Row}
    (h : Row.WellFormed signature (type :: types) row) : row ≠ [] := by
  cases row with
  | nil => exact absurd h (by simp [Row.WellFormed])
  | cons _ _ => simp

theorem Row.WellFormed.head_not_rest {signature : Signature} {type : Ty} {types : List Ty}
    {row : Row} (h : Row.WellFormed signature (type :: types) row) :
    ∀ n subpatterns rest, row ≠ .constructor (.arrayRest n) subpatterns :: rest := by
  intro n subpatterns rest heq
  subst heq
  simp only [Row.WellFormed] at h
  exact absurd h.1 (by simp [Pattern.WellFormed])

theorem Row.WellFormed.head_arity {signature : Signature} {type : Ty} {types : List Ty}
    {row : Row} {head : Constructor} {subpatterns rest : List Pattern}
    (h : Row.WellFormed signature (type :: types) row)
    (heq : row = .constructor head subpatterns :: rest) :
    subpatterns.length ≤ head.arity signature type := by
  subst heq
  simp only [Row.WellFormed] at h
  have hp := h.1
  cases head with
  | arrayRest n => exact absurd hp (by simp [Pattern.WellFormed])
  | _ =>
    have := Pattern.WellFormedPrefix.length_le (signature := signature) hp
    rwa [Constructor.fieldTypes_length] at this

/-! ## The operations preserve it

Both are now recursive -- an or-headed row distributes into one row per
alternative -- so both proofs run by the recursion the operation itself uses,
and the alternation case is exactly `WellFormedAlternatives.mem`. -/

theorem Row.WellFormed.specializeRow {signature : Signature} {type : Ty} {types : List Ty}
    {target : Constructor} :
    ∀ (row : Row), Row.WellFormed signature (type :: types) row →
      ∀ row' ∈ _root_.Buri.specializeRow target (target.arity signature type) row,
        Row.WellFormed signature (target.fieldTypes signature type ++ types) row' := by
  have harity : (target.fieldTypes signature type).length = target.arity signature type :=
    Constructor.fieldTypes_length _ _ _
  intro row
  induction row using _root_.Buri.specializeRow.induct target with
  | case1 => intro h; exact absurd h (by simp [Row.WellFormed])
  | case2 rest =>
    intro h row' hmem
    simp only [specializeRow_wildcard, List.mem_singleton] at hmem
    subst hmem
    refine Row.WellFormed.append ?_ h.2
    rw [← harity]
    exact Row.WellFormed.replicate_wildcard signature _
  | case3 subpatterns rest =>
    intro h row' hmem
    simp at hmem
    subst hmem
    refine Row.WellFormed.append ?_ h.2
    -- The padded-and-truncated sub-pattern list is exactly the padded one,
    -- because well-formedness caps its length at the arity.
    have hp : Pattern.WellFormedPrefix signature
        (target.fieldTypes signature type) subpatterns := by
      have := h.1
      cases target with
      | arrayRest n => exact absurd this (by simp [Pattern.WellFormed])
      | _ => exact this
    have hle : subpatterns.length ≤ target.arity signature type := by
      have := Pattern.WellFormedPrefix.length_le (signature := signature) hp
      rwa [Constructor.fieldTypes_length] at this
    rw [pad_take hle, ← harity]
    exact Pattern.WellFormedPrefix.pad hp
  | case4 c subpatterns rest hne =>
    intro _ row' hmem
    simp [hne] at hmem
  | case5 alternatives rest ih =>
    intro h row' hmem
    simp only [specializeRow_or, List.mem_flatMap] at hmem
    obtain ⟨a, ha, hmem'⟩ := hmem
    exact ih ⟨a, ha⟩ ⟨Pattern.WellFormedAlternatives.mem h.1 ha, h.2⟩ row' hmem'

theorem Row.WellFormed.defaultRow {signature : Signature} {type : Ty} {types : List Ty} :
    ∀ (row : Row), Row.WellFormed signature (type :: types) row →
      ∀ row' ∈ _root_.Buri.defaultRow row, Row.WellFormed signature types row' := by
  intro row
  induction row using _root_.Buri.defaultRow.induct with
  | case1 => intro h; exact absurd h (by simp [Row.WellFormed])
  | case2 rest =>
    intro h row' hmem
    simp only [defaultRow_wildcard, List.mem_singleton] at hmem
    subst hmem
    exact h.2
  | case3 c subpatterns rest => intro _ row' hmem; simp at hmem
  | case4 alternatives rest ih =>
    intro h row' hmem
    simp only [defaultRow_or, List.mem_flatMap] at hmem
    obtain ⟨a, ha, hmem'⟩ := hmem
    exact ih ⟨a, ha⟩ ⟨Pattern.WellFormedAlternatives.mem h.1 ha, h.2⟩ row' hmem'

theorem Matrix.WellFormed.specialize {signature : Signature} {type : Ty} {types : List Ty}
    {matrix : List Row} {target : Constructor}
    (h : Matrix.WellFormed signature (type :: types) matrix) :
    Matrix.WellFormed signature (target.fieldTypes signature type ++ types)
      (_root_.Buri.specialize matrix target (target.arity signature type)) := by
  intro row' hmem
  obtain ⟨row, hrow, hspec⟩ := List.mem_flatMap.mp hmem
  exact Row.WellFormed.specializeRow row (h row hrow) row' hspec

theorem Matrix.WellFormed.defaultMatrix {signature : Signature} {type : Ty} {types : List Ty}
    {matrix : List Row}
    (h : Matrix.WellFormed signature (type :: types) matrix) :
    Matrix.WellFormed signature types (_root_.Buri.defaultMatrix matrix) := by
  intro row' hmem
  obtain ⟨row, hrow, hd⟩ := List.mem_flatMap.mp hmem
  exact Row.WellFormed.defaultRow row (h row hrow) row' hd

end Buri
