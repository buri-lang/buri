import Buri.Core.Check
import Buri.Core.Safety

/-!
# Checker soundness

If `Expr.infer` returns a type, the declarative judgment holds at that type:

```lean
theorem infer_sound (e) (Γ) (t) : Expr.infer S P Γ e = some t → Typing S P Γ e t
```

Only this direction. The converse -- completeness, "if the term is well typed
the checker finds it" -- is **false** for the checker as modelled, and
deliberately so in one place: `Pattern.uniformBindersB` demands that
or-pattern alternatives bind *nothing*, where the declarative rule only demands
that they bind the *same* things. That is a real gap and it is recorded here
rather than papered over.

## What the two `match`-shaped rules cost

`letE` and `matchE` are the only rules whose declarative premises are not
syntactic. Discharging them is the entire reason `Patterns/` exists:

* `armsOk_exhaustive` turns `armsExhaustive S t ps = true` -- a run of the
  usefulness algorithm -- into `∀ v : t, ∃ p ∈ ps, Pattern.matches p v`, via
  `exhaustive_correct_unbounded`;
* `armsOk_irrefutable` is the one-armed case, via
  `irrefutable_correct_unbounded`.

Both are stated over *all* values, with no array-length restriction, which is
what the typing rules need and what `Truncate.lean` bought.
-/

namespace Buri

/-! ## What `armsOk` yields -/

theorem armOk_mem {S : Signature} {t : Ty} {ps : List Pattern}
    (h : ps.all (armOk S t) = true) {p : Pattern} (hp : p ∈ ps) :
    Pattern.WellFormed S t p ∧ Pattern.LoweredArrays p ∧ Pattern.UniformBinders S t p := by
  have := List.all_eq_true.mp h p hp
  simp only [armOk, Bool.and_eq_true] at this
  obtain ⟨⟨hwf, hlow⟩, huni⟩ := this
  have hwf' := Pattern.wellFormedB_sound p t hwf
  exact ⟨hwf', Pattern.loweredArraysB_sound p hlow, Pattern.uniformBindersB_sound p t hwf' huni⟩

theorem armsOk_wellFormed {S : Signature} {t : Ty} {ps : List Pattern}
    (h : armsOk S t ps = true) : ∀ p ∈ ps, Pattern.WellFormed S t p := by
  simp only [armsOk, Bool.and_eq_true] at h
  exact fun p hp => (armOk_mem h.1.1 hp).1

theorem armsOk_uniform {S : Signature} {t : Ty} {ps : List Pattern}
    (h : armsOk S t ps = true) : ∀ p ∈ ps, Pattern.UniformBinders S t p := by
  simp only [armsOk, Bool.and_eq_true] at h
  exact fun p hp => (armOk_mem h.1.1 hp).2.2

/-- **The bridge.** A run of the usefulness algorithm that reports the arms
exhaustive really does mean every well-typed value of the scrutinee's type is
matched by one of them. This is `exhaustive_correct_unbounded` with the
checker's own side conditions discharged. -/
theorem armsOk_exhaustive {S : Signature} {t : Ty} {ps : List Pattern}
    (h : armsOk S t ps = true) :
    ∀ v, (S ⊢ᵥ v : t) → ∃ p ∈ ps, Pattern.matches p v = true := by
  simp only [armsOk, Bool.and_eq_true] at h
  obtain ⟨⟨hall, hcompiled⟩, hexhaustive⟩ := h
  exact exhaustive_correct_unbounded S (armLimit ps) t ps
    (fun p hp => (armOk_mem hall hp).2.1)
    (fun p hp => lengthLimit_lt_armLimit hp)
    (fun q hq => Pattern.wellFormedB_sound q t (List.all_eq_true.mp hcompiled q hq))
    hexhaustive

/-- The one-armed case: irrefutability of a `let` pattern (design/static-rules.md rule 2). -/
theorem armsOk_irrefutable {S : Signature} {t : Ty} {p : Pattern}
    (h : armsOk S t [p] = true) : ∀ v, (S ⊢ᵥ v : t) → Pattern.matches p v = true := by
  intro v hv
  obtain ⟨q, hq, hm⟩ := armsOk_exhaustive h v hv
  rcases List.mem_singleton.mp hq with rfl
  exact hm

/-! ## Soundness -/

private theorem inferList_sound {S : Signature} {P : Program} {Γ : List Ty} :
    ∀ (es : List Expr) (ts : List Ty),
      (∀ e ∈ es, ∀ t, Expr.infer S P Γ e = some t → Typing S P Γ e t) →
      Expr.inferList S P Γ es = some ts →
      Forall₂ (Typing S P Γ) es ts := by
  intro es
  induction es with
  | nil => intro ts _ h; simp only [Expr.inferList, Option.some.injEq] at h; subst h; exact .nil
  | cons e es ih =>
    intro ts hsound h
    simp only [Expr.inferList] at h
    cases he : Expr.infer S P Γ e with
    | none => rw [he] at h; simp at h
    | some t =>
      cases hes : Expr.inferList S P Γ es with
      | none => rw [he, hes] at h; simp at h
      | some ts' =>
        rw [he, hes] at h
        simp only [Option.some.injEq] at h
        subst h
        exact .cons (hsound e (by simp) t he)
          (ih ts' (fun q hq => hsound q (by simp [hq])) hes)

private theorem inferArms_sound {S : Signature} {P : Program} {Γ : List Ty} {t : Ty} :
    ∀ (arms : List (Pattern × Expr)) (r : Ty),
      (∀ a ∈ arms, ∀ (Γ' : List Ty) (t' : Ty),
        Expr.infer S P Γ' a.2 = some t' → Typing S P Γ' a.2 t') →
      Expr.inferArms S P Γ t arms = some r →
      ∀ a ∈ arms, Typing S P (Pattern.binderTypes S t a.1 ++ Γ) a.2 r := by
  intro arms
  induction arms with
  | nil => intro r _ h; simp [Expr.inferArms] at h
  | cons a as ih =>
    intro r hsound h
    match a with
    | (p, body) =>
      simp only [Expr.inferArms] at h
      cases hb : Expr.infer S P (Pattern.binderTypes S t p ++ Γ) body with
      | none => simp [hb] at h
      | some r₀ =>
        cases hrest : Expr.inferArms S P Γ t as with
        | none =>
          simp only [hb, hrest] at h
          split at h
          · next hnil =>
            simp only [Option.some.injEq] at h
            subst h
            intro c hc
            have hasnil : as = [] := List.isEmpty_iff.mp hnil
            subst hasnil
            rcases List.mem_singleton.mp hc with rfl
            exact hsound (p, body) (by simp) _ _ hb
          · simp at h
        | some r' =>
          simp only [hb, hrest] at h
          split at h
          · next heq =>
            simp only [Option.some.injEq] at h
            subst h
            intro c hc
            rcases List.mem_cons.mp hc with rfl | hc'
            · exact hsound (p, body) (by simp) _ _ hb
            · rw [heq]
              exact ih r' (fun q hq => hsound q (by simp [hq])) hrest c hc'
          · simp at h

/-- **Checker soundness.** If the algorithm returns a type, the declarative
judgment holds at it. -/
theorem infer_sound {S : Signature} {P : Program} :
    ∀ (e : Expr) (Γ : List Ty) (t : Ty), Expr.infer S P Γ e = some t → Typing S P Γ e t := by
  intro e
  induction e using Expr.ind' with
  | var i => intro Γ t h; exact .var h
  | lam ts body ih =>
    intro Γ t h
    simp only [Expr.infer, Option.map_eq_some_iff] at h
    obtain ⟨r, hr, rfl⟩ := h
    exact .lam (ih (ts ++ Γ) r hr)
  | app f args ihf iha =>
    intro Γ t h
    simp only [Expr.infer] at h
    split at h
    · next ts r hf =>
      split at h
      · next hargs =>
        simp only [Option.some.injEq] at h
        subst h
        exact .app (ihf Γ _ hf) (TypingList_iff.mpr
          (inferList_sound args ts (fun q hq => iha q hq Γ) hargs))
      · simp at h
    · simp at h
  | call f targs args iha =>
    intro Γ t h
    simp only [Expr.infer] at h
    split at h
    · next d hd =>
      split at h
      · next hcond =>
        simp only [Option.some.injEq] at h
        subst h
        exact .call hd hcond.1 (TypingList_iff.mpr
          (inferList_sound args _ (fun q hq => iha q hq Γ) hcond.2))
      · simp at h
    · simp at h
  | node k args iha =>
    intro Γ t h
    simp only [Expr.infer] at h
    split at h
    · next hcond =>
      simp only [Option.some.injEq] at h
      subst h
      exact .node (NodeKind.wellFormedB_sound hcond.1) (TypingList_iff.mpr
        (inferList_sound args _ (fun q hq => iha q hq Γ) hcond.2))
    · simp at h
  | letE p bound body ihb ihbody =>
    intro Γ t h
    simp only [Expr.infer] at h
    split at h
    · next bt hbt =>
      split at h
      · next harms =>
        refine .letE (ihb Γ bt hbt) ?_ ?_ (armsOk_irrefutable harms) (ihbody _ t h)
        · exact armsOk_wellFormed harms p (by simp)
        · exact armsOk_uniform harms p (by simp)
      · simp at h
    · simp at h
  | matchE st scrutinee arms ihs iha =>
    intro Γ t h
    simp only [Expr.infer] at h
    split at h
    · next hcond =>
      obtain ⟨hscrut, harms⟩ := hcond
      refine .matchE (ihs Γ st hscrut) (fun a ha => ?_) (fun a ha => ?_)
        (inferArms_sound arms t (fun a ha Γ' t' => iha a ha Γ' t') h) (fun v hv => ?_)
      · exact armsOk_wellFormed harms a.1 (List.mem_map.mpr ⟨a, ha, rfl⟩)
      · exact armsOk_uniform harms a.1 (List.mem_map.mpr ⟨a, ha, rfl⟩)
      · obtain ⟨q, hq, hm⟩ := armsOk_exhaustive harms v hv
        obtain ⟨a, ha, rfl⟩ := List.mem_map.mp hq
        exact ⟨a, ha, hm⟩
    · simp at h

/-- The checking judgment, sound. -/
theorem check_sound {S : Signature} {P : Program} {Γ : List Ty} {e : Expr} {t : Ty}
    (h : Expr.check S P Γ e t = true) : Typing S P Γ e t :=
  infer_sound e Γ t (by simpa [Expr.check] using h)

/-- **What the checker buys.** A closed term the algorithm accepts, in a
program whose declared bodies check, never gets stuck. This is the composition
of `check_sound` with `type_safety`, and it is the sentence the whole
development exists to license. -/
theorem checked_never_stuck {S : Signature} {P : Program} (hP : Program.WellFormed S P)
    {e e' : Expr} {t : Ty} (h : Expr.check S P [] e t = true) (hsteps : Steps S P e e') :
    Expr.IsValue e' ∨ ∃ e'', Step S P e' e'' :=
  type_safety hP (check_sound h) hsteps

end Buri
