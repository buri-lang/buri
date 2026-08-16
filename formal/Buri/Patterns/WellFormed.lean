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

theorem Row.WellFormed.length_eq {signature : Signature} :
    ∀ {types : List Ty} {row : Row},
      Row.WellFormed signature types row → row.length = types.length := by
  intro types
  induction types with
  | nil => intro row h; cases row with
    | nil => rfl
    | cons _ _ => exact absurd h (by simp [Row.WellFormed])
  | cons type types ih =>
    intro row h
    cases row with
    | nil => exact absurd h (by simp [Row.WellFormed])
    | cons p ps => simp [Row.WellFormed] at h; simp [ih h.2]

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

theorem Row.WellFormed.tail {signature : Signature} {type : Ty} {types : List Ty}
    {p : Pattern} {rest : Row} (h : Row.WellFormed signature (type :: types) (p :: rest)) :
    Row.WellFormed signature types rest := h.2

/-! ## The operations preserve it -/

theorem Row.WellFormed.specializeRow {signature : Signature} {type : Ty} {types : List Ty}
    {row row' : Row} {target : Constructor}
    (h : Row.WellFormed signature (type :: types) row)
    (specializes : specializeRow target (target.arity signature type) row = some row') :
    Row.WellFormed signature (target.fieldTypes signature type ++ types) row' := by
  have harity : (target.fieldTypes signature type).length = target.arity signature type :=
    Constructor.fieldTypes_length _ _ _
  match row with
  | [] => simp [_root_.Buri.specializeRow] at specializes
  | .or _ :: _ => simp [_root_.Buri.specializeRow] at specializes
  | .wildcard :: rest =>
    simp only [_root_.Buri.specializeRow, Option.some.injEq] at specializes
    subst specializes
    refine Row.WellFormed.append ?_ h.2
    rw [← harity]
    exact Row.WellFormed.replicate_wildcard signature _
  | .constructor head subpatterns :: rest =>
    simp only [_root_.Buri.specializeRow] at specializes
    split at specializes
    · next heq =>
      subst heq
      simp only [Option.some.injEq] at specializes
      subst specializes
      refine Row.WellFormed.append ?_ h.2
      -- The padded-and-truncated sub-pattern list is exactly the padded one,
      -- because well-formedness caps its length at the arity.
      have hp : Pattern.WellFormedPrefix signature
          (head.fieldTypes signature type) subpatterns := by
        have := h.1
        cases head with
        | arrayRest n => exact absurd this (by simp [Pattern.WellFormed])
        | _ => exact this
      have hle : subpatterns.length ≤ head.arity signature type := by
        have := Pattern.WellFormedPrefix.length_le (signature := signature) hp
        rwa [Constructor.fieldTypes_length] at this
      have hsplit : (subpatterns ++ List.replicate
            (head.arity signature type) Pattern.wildcard).take
              (head.arity signature type)
          = subpatterns ++ List.replicate
              (head.arity signature type - subpatterns.length) Pattern.wildcard := by
        have hmin : min (head.arity signature type - subpatterns.length)
            (head.arity signature type) = head.arity signature type - subpatterns.length := by
          omega
        rw [List.take_append, List.take_of_length_le hle, List.take_replicate, hmin]
      rw [hsplit, ← harity]
      exact Pattern.WellFormedPrefix.pad hp
    · simp at specializes

theorem Row.WellFormed.defaultRow {signature : Signature} {type : Ty} {types : List Ty}
    {row row' : Row}
    (h : Row.WellFormed signature (type :: types) row)
    (hd : defaultRow row = some row') :
    Row.WellFormed signature types row' := by
  match row with
  | .wildcard :: rest =>
    simp only [_root_.Buri.defaultRow, Option.some.injEq] at hd
    subst hd
    exact h.2
  | [] => simp [_root_.Buri.defaultRow] at hd
  | .constructor _ _ :: _ => simp [_root_.Buri.defaultRow] at hd
  | .or _ :: _ => simp [_root_.Buri.defaultRow] at hd

theorem Matrix.WellFormed.specialize {signature : Signature} {type : Ty} {types : List Ty}
    {matrix : List Row} {target : Constructor}
    (h : Matrix.WellFormed signature (type :: types) matrix) :
    Matrix.WellFormed signature (target.fieldTypes signature type ++ types)
      (_root_.Buri.specialize matrix target (target.arity signature type)) := by
  intro row' hmem
  obtain ⟨row, hrow, hspec⟩ := List.mem_filterMap.mp hmem
  exact Row.WellFormed.specializeRow (h row hrow) hspec

theorem Matrix.WellFormed.defaultMatrix {signature : Signature} {type : Ty} {types : List Ty}
    {matrix : List Row}
    (h : Matrix.WellFormed signature (type :: types) matrix) :
    Matrix.WellFormed signature types (_root_.Buri.defaultMatrix matrix) := by
  intro row' hmem
  obtain ⟨row, hrow, hd⟩ := List.mem_filterMap.mp hmem
  exact Row.WellFormed.defaultRow (h row hrow) hd

end Buri
