/-!
# Pointwise lifting of a relation to lists

Core Lean has no `List.Forall₂` -- it lives in Mathlib, which this development
deliberately does not depend on (see `lakefile.toml`). Ten lines here is a
better trade than a four-gigabyte dependency and a second toolchain pin.

Buri's function types are n-ary (`Ty.fn : List Ty → Ty → Ty`), matching
`Ty::Fn(Vec<Ty>, Box<Ty>)`. Currying would be prettier but would misrepresent
the one thing that matters most: `is_effect_carrying(Fn(params, ret))` looks
*only* at `ret` (`types.rs:575`), and under currying `fn(C, A) => B` becomes
`C -> (A -> B)`, whose "result" is `A -> B`. The analysis would change shape.
Keeping it n-ary means this lifting is needed everywhere, so it is worth having
the lemmas in one place.
-/

namespace Buri

inductive Forall₂ {α β : Type} (R : α → β → Prop) : List α → List β → Prop where
  | nil : Forall₂ R [] []
  | cons {a b as bs} : R a b → Forall₂ R as bs → Forall₂ R (a :: as) (b :: bs)

namespace Forall₂

variable {α β : Type} {R : α → β → Prop}

theorem length_eq {as : List α} {bs : List β} : Forall₂ R as bs → as.length = bs.length := by
  intro h; induction h with
  | nil => rfl
  | cons _ _ ih => simp [ih]

theorem append {as₁ bs₁ as₂ bs₂} :
    Forall₂ R as₁ bs₁ → Forall₂ R as₂ bs₂ → Forall₂ R (as₁ ++ as₂) (bs₁ ++ bs₂) := by
  intro h₁ h₂; induction h₁ with
  | nil => exact h₂
  | cons hab _ ih => exact .cons hab ih

/-- Splitting at the boundary of a known-length prefix. This is the workhorse
for every `specialize` step, where a row's head is replaced by its
sub-patterns and the tail carries on unchanged. -/
theorem split_right {as : List α} {bs₁ bs₂ : List β} :
    Forall₂ R as (bs₁ ++ bs₂) →
    ∃ cs₁ cs₂, as = cs₁ ++ cs₂ ∧ Forall₂ R cs₁ bs₁ ∧ Forall₂ R cs₂ bs₂ := by
  intro h
  induction bs₁ generalizing as with
  | nil => exact ⟨[], as, rfl, .nil, h⟩
  | cons b bs ih =>
    cases h with
    | cons hab habs =>
      rename_i a as'
      obtain ⟨cs₁, cs₂, rfl, h₁, h₂⟩ := ih habs
      exact ⟨a :: cs₁, cs₂, rfl, .cons hab h₁, h₂⟩

theorem split_left {as₁ as₂ : List α} {bs : List β} :
    Forall₂ R (as₁ ++ as₂) bs →
    ∃ ds₁ ds₂, bs = ds₁ ++ ds₂ ∧ Forall₂ R as₁ ds₁ ∧ Forall₂ R as₂ ds₂ := by
  intro h
  induction as₁ generalizing bs with
  | nil => exact ⟨[], bs, rfl, .nil, h⟩
  | cons a as ih =>
    cases h with
    | cons hab habs =>
      rename_i b bs'
      obtain ⟨ds₁, ds₂, rfl, h₁, h₂⟩ := ih habs
      exact ⟨b :: ds₁, ds₂, rfl, .cons hab h₁, h₂⟩

/-- Every element of a list related to a `replicate` is related to the repeated
element. Arrays are homogeneous, so this is how `HasType` on an array is
consumed. -/
theorem of_replicate {as : List α} {b : β} :
    Forall₂ R as (List.replicate as.length b) → ∀ a ∈ as, R a b := by
  intro h
  induction as with
  | nil => intro a ha; exact absurd ha List.not_mem_nil
  | cons x xs ih =>
    simp only [List.length_cons, List.replicate] at h
    cases h with
    | cons hxb hrest =>
      intro a ha
      rcases List.mem_cons.mp ha with rfl | h'
      · exact hxb
      · exact ih hrest a h'

theorem to_replicate {as : List α} {b : β} :
    (∀ a ∈ as, R a b) → Forall₂ R as (List.replicate as.length b) := by
  intro h
  induction as with
  | nil => exact .nil
  | cons x xs ih =>
    simp only [List.length_cons, List.replicate]
    exact .cons (h x (by simp)) (ih fun a ha => h a (by simp [ha]))

end Forall₂

end Buri
