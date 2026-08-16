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
    | cons hxb restFree =>
      intro a ha
      rcases List.mem_cons.mp ha with rfl | h'
      · exact hxb
      · exact ih restFree a h'

/-- Transport along a function on the left. Every place a list of expressions
is erased to a list of values pointwise goes through this. -/
theorem imp_map {γ : Type} {Q : γ → β → Prop} {f : α → γ} {as : List α} {bs : List β}
    (h : Forall₂ R as bs) (himp : ∀ a ∈ as, ∀ b, R a b → Q (f a) b) :
    Forall₂ Q (as.map f) bs := by
  induction h with
  | nil => exact .nil
  | @cons a b as bs hab _ ih =>
    exact .cons (himp a (by simp) b hab) (ih fun x hx => himp x (by simp [hx]))

/-- Reading off the element related to a known one on the right. This is how
the `var` case of the substitution lemma finds the term it substitutes. -/
theorem getElem? {as : List α} {bs : List β} (h : Forall₂ R as bs) :
    ∀ (i : Nat) (b : β), bs[i]? = some b → ∃ a, as[i]? = some a ∧ R a b := by
  induction h with
  | nil => intro i b hb; simp at hb
  | @cons a b as bs hab _ ih =>
    intro i b' hb'
    cases i with
    | zero => simp at hb'; exact ⟨a, by simp, hb' ▸ hab⟩
    | succ k => simpa using ih k b' (by simpa using hb')

/-- Isolating one element of the left list, with a licence to replace it by
anything else related to the same right-hand element. Every congruence rule in
`Semantics.lean` steps one argument of a list and leaves the rest alone; this
is how the typing derivation follows. -/
theorem middle {pre post : List α} {a : α} {bs : List β}
    (h : Forall₂ R (pre ++ a :: post) bs) :
    ∃ b, R a b ∧ ∀ a', R a' b → Forall₂ R (pre ++ a' :: post) bs := by
  induction pre generalizing bs with
  | nil =>
    cases h with
    | cons hab hrest => exact ⟨_, hab, fun a' ha' => .cons ha' hrest⟩
  | cons x xs ih =>
    cases h with
    | cons hxb hrest =>
      obtain ⟨b, hab, hrepl⟩ := ih hrest
      exact ⟨b, hab, fun a' ha' => .cons hxb (hrepl a' ha')⟩

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
