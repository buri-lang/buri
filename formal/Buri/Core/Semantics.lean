import Buri.Core.Subst

/-!
# Small-step operational semantics

Call by value, arguments left to right, closed terms only. Eleven rules: four
congruences that step a sub-term, three that step one element of an argument
list, and four that actually reduce -- beta, delta (a call to a declared
function), `let`, and `match`.

Argument lists are stepped by naming the split point, `pre ++ a :: post` with
every element of `pre` already a value. That is the standard evaluation-context
encoding written out, and `first_non_value` below is the only lemma it needs:
a list is all values, or it has a first one that is not.

## What is deliberately absent

* **Errors.** SPEC 6.10 makes an abort observable, and `findings/README.md` 1 is
  the observation that a pure call is therefore not eliminable. Modelling
  aborts belongs with the purity theorem, not with type safety: progress here
  says a well-typed term is a value or steps, which is the statement that
  *typing* never gets stuck.
* **Intrinsics.** `semantics/builtins.rs` is axioms in any model of this kind
  (`formal/README.md`, "Blind spots"). A call to a declared function reduces to
  its body; a call to a builtin has no body to reduce to and is out of scope.
* **Guards.** A guarded arm covers nothing for exhaustiveness purposes
  (`exhaustiveness.rs`'s `check` skips it when extending `covering`), so a
  `match` whose only matching arm is guarded can get stuck. Arms here are
  unguarded, which is what makes the progress theorem true.
-/

namespace Buri

inductive Step (S : Signature) (P : Program) : Expr → Expr → Prop where
  | appFn {f f' args} :
      Step S P f f' →
      Step S P (.app f args) (.app f' args)
  | appArg {f pre a a' post} :
      Expr.IsValue f → (∀ x ∈ pre, Expr.IsValue x) → Step S P a a' →
      Step S P (.app f (pre ++ a :: post)) (.app f (pre ++ a' :: post))
  | appBeta {ts body args} :
      (∀ x ∈ args, Expr.IsValue x) →
      Step S P (.app (.lam ts body) args) (Expr.subst args 0 body)
  | callArg {f targs pre a a' post} :
      (∀ x ∈ pre, Expr.IsValue x) → Step S P a a' →
      Step S P (.call f targs (pre ++ a :: post)) (.call f targs (pre ++ a' :: post))
  | callDelta {f targs args d} :
      (∀ x ∈ args, Expr.IsValue x) → P.fn f = some d →
      Step S P (.call f targs args) (Expr.subst args 0 (d.instantiate targs))
  | nodeArg {k pre a a' post} :
      (∀ x ∈ pre, Expr.IsValue x) → Step S P a a' →
      Step S P (.node k (pre ++ a :: post)) (.node k (pre ++ a' :: post))
  | letBound {p bound bound' body} :
      Step S P bound bound' →
      Step S P (.letE p bound body) (.letE p bound' body)
  | letBind {p bound body σ} :
      Expr.IsValue bound → Pattern.bind p bound = some σ →
      Step S P (.letE p bound body) (Expr.subst σ 0 body)
  | matchScrutinee {t scrutinee scrutinee' arms} :
      Step S P scrutinee scrutinee' →
      Step S P (.matchE t scrutinee arms) (.matchE t scrutinee' arms)
  | matchArm {t scrutinee arms σ body} :
      Expr.IsValue scrutinee → matchArms arms scrutinee = some (σ, body) →
      Step S P (.matchE t scrutinee arms) (Expr.subst σ 0 body)

/-! ## The list-splitting lemma

Classical, and unavoidably so: `Expr.IsValue` is a `Prop`, and progress has to
decide it. `Classical.choice` is one of the three axioms `Audit.lean` permits.
-/

theorem first_non_value (es : List Expr) :
    (∀ x ∈ es, Expr.IsValue x) ∨
      ∃ pre a post, es = pre ++ a :: post ∧ (∀ x ∈ pre, Expr.IsValue x) ∧ ¬ Expr.IsValue a := by
  induction es with
  | nil => exact .inl (by simp)
  | cons e es ih =>
    by_cases he : Expr.IsValue e
    · rcases ih with hall | ⟨pre, a, post, rfl, hpre, ha⟩
      · refine .inl fun x hx => ?_
        rcases List.mem_cons.mp hx with rfl | hx'
        · exact he
        · exact hall x hx'
      · refine .inr ⟨e :: pre, a, post, rfl, fun x hx => ?_, ha⟩
        rcases List.mem_cons.mp hx with rfl | hx'
        · exact he
        · exact hpre x hx'
    · exact .inr ⟨[], e, es, rfl, by simp, he⟩

/-! ## `matchArms` and the arm that fires -/

/-- If some arm's pattern binds, the first one that does is selected. -/
theorem matchArms_isSome {arms : List (Pattern × Expr)} {scrutinee : Expr}
    (h : ∃ a ∈ arms, (Pattern.bind a.1 scrutinee).isSome = true) :
    ∃ σ body, matchArms arms scrutinee = some (σ, body) := by
  induction arms with
  | nil => obtain ⟨a, ha, _⟩ := h; exact absurd ha List.not_mem_nil
  | cons a as ih =>
    cases hb : Pattern.bind a.1 scrutinee with
    | some σ =>
      refine ⟨σ, a.2, ?_⟩
      match a with
      | (p, body) => simp only [matchArms, hb]
    | none =>
      have hrest : ∃ b ∈ as, (Pattern.bind b.1 scrutinee).isSome = true := by
        obtain ⟨b, hbmem, hbb⟩ := h
        rcases List.mem_cons.mp hbmem with rfl | hb'
        · rw [hb] at hbb; simp at hbb
        · exact ⟨b, hb', hbb⟩
      obtain ⟨σ, body, hstep⟩ := ih hrest
      refine ⟨σ, body, ?_⟩
      match a with
      | (p, b) => simp only [matchArms, hb]; exact hstep

/-- The arm `matchArms` selected really is one of the arms, and its pattern
really did bind what was substituted. -/
theorem matchArms_mem {arms : List (Pattern × Expr)} {scrutinee : Expr}
    {σ : List Expr} {body : Expr} (h : matchArms arms scrutinee = some (σ, body)) :
    ∃ a ∈ arms, a.2 = body ∧ Pattern.bind a.1 scrutinee = some σ := by
  induction arms with
  | nil => simp [matchArms] at h
  | cons a as ih =>
    match a, h with
    | (p, b), h =>
      simp only [matchArms] at h
      cases hb : Pattern.bind p scrutinee with
      | some τ =>
        rw [hb] at h
        simp only [Option.some.injEq, Prod.mk.injEq] at h
        exact ⟨(p, b), by simp, h.2.symm ▸ rfl, by rw [hb, h.1]⟩
      | none =>
        rw [hb] at h
        obtain ⟨c, hc, hcb, hcbind⟩ := ih h
        exact ⟨c, by simp [hc], hcb, hcbind⟩

end Buri
