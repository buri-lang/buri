import Buri.Core.Typing

/-!
# Weakening and substitution

The two structural lemmas every preservation proof rests on.

**Weakening** appends to the *right* of the context. That is the only direction
that is free with de Bruijn indices: `Γ[i]` and `(Γ ++ Γ')[i]` agree for every
`i < Γ.length`, so no index moves and no term is rewritten. Weakening on the
left would need a lift, and this development never needs one.

**Substitution** replaces the *rightmost* segment of the context by closed
terms:

```
Typing (Γ₁ ++ Δ) e t  →  Forall₂ (Typing []) σ Δ  →  Typing Γ₁ (subst σ Γ₁.length e) t
```

`Δ` on the right and `σ` closed are what make `Expr.subst`'s shift-free
definition correct. Every reduction in `Semantics.lean` instantiates this with
`Γ₁ = []`: a beta-redex substitutes closed argument values for the lambda's
parameters, a `match` substitutes closed bound values for an arm's binders.

The one place the pattern machinery reappears is the `letE` and `matchE` cases,
where the depth `Expr.subst` steps by -- `Pattern.binderCount`, computed with no
reference to a type -- has to equal the number of types `Pattern.binderTypes`
produced. That is `Pattern.binderTypes_length`, and it needs the pattern to be
well formed, which the typing rule supplies.
-/

namespace Buri

/-! ## Unfolding the list companions -/

theorem Expr.substList_eq_map (σ : List Expr) (d : Nat) (es : List Expr) :
    Expr.substList σ d es = es.map (Expr.subst σ d) := by
  induction es with
  | nil => rfl
  | cons e es ih => simp [Expr.substList, ih]

theorem Expr.substArms_eq_map (σ : List Expr) (d : Nat) (arms : List (Pattern × Expr)) :
    Expr.substArms σ d arms
      = arms.map (fun a => (a.1, Expr.subst σ (d + a.1.binderCount) a.2)) := by
  induction arms with
  | nil => rfl
  | cons a as ih => cases a; simp [Expr.substArms, ih]

/-! ## Weakening -/

theorem Typing.weaken {S : Signature} {P : Program} :
    ∀ (e : Expr) {Γ₁ : List Ty} {t : Ty} (Γ₂ : List Ty),
      Typing S P Γ₁ e t → Typing S P (Γ₁ ++ Γ₂) e t := by
  intro e
  induction e using Expr.ind' with
  | var i =>
    intro Γ₁ t Γ₂ h
    have hi := Typing.var_inversion h
    refine .var ?_
    have hlt : i < Γ₁.length := by
      rcases Nat.lt_or_ge i Γ₁.length with h' | h'
      · exact h'
      · rw [List.getElem?_eq_none h'] at hi; simp at hi
    rw [List.getElem?_append_left hlt]
    exact hi
  | lam ts body ih =>
    intro Γ₁ t Γ₂ h
    obtain ⟨r, rfl, hb⟩ := Typing.lam_inversion h
    refine .lam ?_
    rw [← List.append_assoc]
    exact ih Γ₂ hb
  | app f args ihf iha =>
    intro Γ₁ t Γ₂ h
    obtain ⟨ts, hf, hargs⟩ := Typing.app_inversion h
    exact .app (ihf Γ₂ hf) (TypingList_iff.mpr (by
      have := hargs.imp_map (f := id) (Q := Typing S P (Γ₁ ++ Γ₂))
        (fun a ha b hab => iha a ha Γ₂ hab)
      simpa using this))
  | call f targs args iha =>
    intro Γ₁ t Γ₂ h
    obtain ⟨d, hd, hg, rfl, hargs⟩ := Typing.call_inversion h
    exact .call hd hg (TypingList_iff.mpr (by
      have := hargs.imp_map (f := id) (Q := Typing S P (Γ₁ ++ Γ₂))
        (fun a ha b hab => iha a ha Γ₂ hab)
      simpa using this))
  | node k args iha =>
    intro Γ₁ t Γ₂ h
    obtain ⟨rfl, hk, hargs⟩ := Typing.node_inversion h
    exact .node hk (TypingList_iff.mpr (by
      have := hargs.imp_map (f := id) (Q := Typing S P (Γ₁ ++ Γ₂))
        (fun a ha b hab => iha a ha Γ₂ hab)
      simpa using this))
  | letE p bound body ihb ihbody =>
    intro Γ₁ t Γ₂ h
    obtain ⟨bt, hb, hwf, hu, hirr, hbody⟩ := Typing.letE_inversion h
    refine .letE (ihb Γ₂ hb) hwf hu hirr ?_
    rw [← List.append_assoc]
    exact ihbody Γ₂ hbody
  | matchE st scrutinee arms ihs iha =>
    intro Γ₁ t Γ₂ h
    obtain ⟨hs, hwf, hu, hbody, hex⟩ := Typing.matchE_inversion h
    refine .matchE (ihs Γ₂ hs) hwf hu (fun a ha => ?_) hex
    have := iha a ha Γ₂ (hbody a ha)
    rwa [List.append_assoc] at this

/-- The form the substitution lemma actually applies: a closed term is well
typed in any context. -/
theorem Typing.weaken_nil {S : Signature} {P : Program} {e : Expr} {t : Ty} (Γ : List Ty)
    (h : Typing S P [] e t) : Typing S P Γ e t := by
  simpa using Typing.weaken e (Γ₁ := []) Γ h

/-! ## Substitution -/

theorem Typing.subst {S : Signature} {P : Program} :
    ∀ (e : Expr) {Γ₁ Δ : List Ty} {t : Ty} {σ : List Expr},
      Typing S P (Γ₁ ++ Δ) e t → Forall₂ (Typing S P []) σ Δ →
      Typing S P Γ₁ (Expr.subst σ Γ₁.length e) t := by
  intro e
  induction e using Expr.ind' with
  | var i =>
    intro Γ₁ Δ t σ h hσ
    have hi := Typing.var_inversion h
    by_cases hlt : i < Γ₁.length
    · rw [Expr.subst, if_pos hlt]
      exact .var (by rwa [List.getElem?_append_left hlt] at hi)
    · rw [Expr.subst, if_neg hlt]
      rw [List.getElem?_append_right (by omega)] at hi
      obtain ⟨a, ha, hta⟩ := hσ.getElem? _ _ hi
      rw [List.getD_eq_getElem?_getD, ha]
      exact Typing.weaken_nil Γ₁ hta
  | lam ts body ih =>
    intro Γ₁ Δ t σ h hσ
    obtain ⟨r, rfl, hb⟩ := Typing.lam_inversion h
    rw [Expr.subst]
    refine .lam ?_
    have := ih (Γ₁ := ts ++ Γ₁) (Δ := Δ) (by rwa [List.append_assoc]) hσ
    simpa [Nat.add_comm] using this
  | app f args ihf iha =>
    intro Γ₁ Δ t σ h hσ
    obtain ⟨ts, hf, hargs⟩ := Typing.app_inversion h
    rw [Expr.subst]
    refine .app (ihf hf hσ) (TypingList_iff.mpr ?_)
    rw [Expr.substList_eq_map]
    exact hargs.imp_map fun a ha b hab => iha a ha hab hσ
  | call f targs args iha =>
    intro Γ₁ Δ t σ h hσ
    obtain ⟨d, hd, hg, rfl, hargs⟩ := Typing.call_inversion h
    rw [Expr.subst]
    refine .call hd hg (TypingList_iff.mpr ?_)
    rw [Expr.substList_eq_map]
    exact hargs.imp_map fun a ha b hab => iha a ha hab hσ
  | node k args iha =>
    intro Γ₁ Δ t σ h hσ
    obtain ⟨rfl, hk, hargs⟩ := Typing.node_inversion h
    rw [Expr.subst]
    refine .node hk (TypingList_iff.mpr ?_)
    rw [Expr.substList_eq_map]
    have hlen : (args.map (Expr.subst σ Γ₁.length)).length = args.length := by simp
    rw [hlen]
    exact hargs.imp_map fun a ha b hab => iha a ha hab hσ
  | letE p bound body ihb ihbody =>
    intro Γ₁ Δ t σ h hσ
    obtain ⟨bt, hb, hwf, hu, hirr, hbody⟩ := Typing.letE_inversion h
    rw [Expr.subst]
    refine .letE (ihb hb hσ) hwf hu hirr ?_
    have hcount : (Pattern.binderTypes S bt p).length = p.binderCount :=
      Pattern.binderTypes_length p bt hwf
    have := ihbody (Γ₁ := Pattern.binderTypes S bt p ++ Γ₁) (Δ := Δ)
      (by rwa [List.append_assoc]) hσ
    simpa [hcount, Nat.add_comm] using this
  | matchE st scrutinee arms ihs iha =>
    intro Γ₁ Δ t σ h hσ
    obtain ⟨hs, hwf, hu, hbody, hex⟩ := Typing.matchE_inversion h
    rw [Expr.subst, Expr.substArms_eq_map]
    refine .matchE (ihs hs hσ) (fun a ha => ?_) (fun a ha => ?_) (fun a ha => ?_) (fun v hv => ?_)
    · obtain ⟨b, hb, rfl⟩ := List.mem_map.mp ha
      exact hwf b hb
    · obtain ⟨b, hb, rfl⟩ := List.mem_map.mp ha
      exact hu b hb
    · obtain ⟨b, hb, rfl⟩ := List.mem_map.mp ha
      have hcount : (Pattern.binderTypes S st b.1).length = b.1.binderCount :=
        Pattern.binderTypes_length b.1 st (hwf b hb)
      have := iha b hb (Γ₁ := Pattern.binderTypes S st b.1 ++ Γ₁) (Δ := Δ)
        (by rw [List.append_assoc]; exact hbody b hb) hσ
      simpa [hcount, Nat.add_comm] using this
    · obtain ⟨a, ha, hm⟩ := hex v hv
      exact ⟨_, List.mem_map.mpr ⟨a, ha, rfl⟩, hm⟩

/-- The form every reduction rule uses: substituting closed values for the
whole context. -/
theorem Typing.subst_nil {S : Signature} {P : Program} {e : Expr} {Δ : List Ty} {t : Ty}
    {σ : List Expr} (h : Typing S P Δ e t) (hσ : Forall₂ (Typing S P []) σ Δ) :
    Typing S P [] (Expr.subst σ 0 e) t := by
  simpa using Typing.subst e (Γ₁ := []) (Δ := Δ) (by simpa using h) hσ

end Buri
