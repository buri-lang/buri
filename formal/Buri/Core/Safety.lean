import Buri.Core.Semantics

/-!
# Type safety

Progress and preservation for the core language, and hence -- by the usual
induction over a reduction sequence -- that a well-typed program never gets
stuck.

## Where exhaustiveness enters

The `match` case of progress is the reason the exhaustiveness development
exists. It needs: *the scrutinee value matches some arm*. That is exactly the
semantic premise of the `matchE` typing rule, and the chain from the checker to
that premise is

```
isExhaustive S limit t (armRows (compileArms limit arms)) = true
  --(exhaustive_correct_unbounded)-->  ∀ v : t, ∃ arm, Pattern.matches arm v
  --(matchE premise)                -->  the arm exists for scrutinee.erase
  --(Typing.erase)                  -->  scrutinee.erase really has type t
  --(Pattern.bind_isSome)           -->  the arm's pattern binds
  --(matchArms_isSome)              -->  `matchArms` selects one
```

`Sound.lean` closes the first link, `Core.Progress` below closes the rest.
Nothing in this file re-proves anything about the usefulness algorithm; the
work was done once, in `Complete.lean`, and is consumed here.
-/

namespace Buri

/-- **Progress.** A closed, well-typed expression is a value or steps.

The two premises beyond typing are: the program's declared bodies check at
every instantiation (`Program.WellFormed`), which is what makes a `call` to a
declared function reducible; and nothing else. -/
theorem progress {S : Signature} {P : Program} :
    ∀ (e : Expr) {t : Ty}, Typing S P [] e t →
      Expr.IsValue e ∨ ∃ e', Step S P e e' := by
  intro e
  induction e using Expr.ind' with
  | var i =>
    intro t h
    have := Typing.var_inversion h
    simp at this
  | lam ts body _ => intro t _; exact .inl .lam
  | app f args ihf iha =>
    intro t h
    obtain ⟨ts, hf, hargs⟩ := Typing.app_inversion h
    rcases ihf hf with hvf | ⟨f', hstep⟩
    · obtain ⟨body, rfl, _⟩ := Typing.canonical_fn hf hvf
      rcases first_non_value args with hall | ⟨pre, a, post, rfl, hpre, hna⟩
      · exact .inr ⟨_, .appBeta hall⟩
      · obtain ⟨b, hab, _⟩ := hargs.middle
        rcases iha a (by simp) hab with hv | ⟨a', hstep⟩
        · exact absurd hv hna
        · exact .inr ⟨_, .appArg hvf hpre hstep⟩
    · exact .inr ⟨_, .appFn hstep⟩
  | call f targs args iha =>
    intro t h
    obtain ⟨d, hd, _, rfl, hargs⟩ := Typing.call_inversion h
    rcases first_non_value args with hall | ⟨pre, a, post, rfl, hpre, hna⟩
    · exact .inr ⟨_, .callDelta hall hd⟩
    · obtain ⟨b, hab, _⟩ := hargs.middle
      rcases iha a (by simp) hab with hv | ⟨a', hstep⟩
      · exact absurd hv hna
      · exact .inr ⟨_, .callArg hpre hstep⟩
  | node k args iha =>
    intro t h
    obtain ⟨rfl, _, hargs⟩ := Typing.node_inversion h
    rcases first_non_value args with hall | ⟨pre, a, post, rfl, hpre, hna⟩
    · exact .inl (.node hall)
    · obtain ⟨b, hab, _⟩ := hargs.middle
      rcases iha a (by simp) hab with hv | ⟨a', hstep⟩
      · exact absurd hv hna
      · exact .inr ⟨_, .nodeArg hpre hstep⟩
  | letE p bound body ihb _ =>
    intro t h
    obtain ⟨bt, hb, hwf, _, hirr, _⟩ := Typing.letE_inversion h
    rcases ihb hb with hvb | ⟨bound', hstep⟩
    · -- Irrefutability, transported to the expression level.
      have hmatch : Pattern.matches p bound.erase = true :=
        hirr bound.erase (Typing.erase bound hb hvb)
      have := Pattern.bind_isSome p bt hwf bound
      rw [hmatch] at this
      obtain ⟨σ, hσ⟩ := Option.isSome_iff_exists.mp this
      exact .inr ⟨_, .letBind hvb hσ⟩
    · exact .inr ⟨_, .letBound hstep⟩
  | matchE st scrutinee arms ihs _ =>
    intro t h
    obtain ⟨hs, hwf, _, _, hex⟩ := Typing.matchE_inversion h
    rcases ihs hs with hvs | ⟨scrutinee', hstep⟩
    · -- Exhaustiveness, transported to the expression level.
      obtain ⟨a, ha, hm⟩ := hex scrutinee.erase (Typing.erase scrutinee hs hvs)
      have hbind : (Pattern.bind a.1 scrutinee).isSome = true := by
        rw [Pattern.bind_isSome a.1 st (hwf a ha) scrutinee, hm]
      obtain ⟨σ, body, hsel⟩ := matchArms_isSome ⟨a, ha, hbind⟩
      exact .inr ⟨_, .matchArm hvs hsel⟩
    · exact .inr ⟨_, .matchScrutinee hstep⟩

/-- **Preservation.** Stepping does not change the type. -/
theorem preservation {S : Signature} {P : Program} (hP : Program.WellFormed S P) :
    ∀ {e e' : Expr}, Step S P e e' → ∀ {t : Ty}, Typing S P [] e t → Typing S P [] e' t := by
  intro e e' hstep
  induction hstep with
  | appFn _ ih =>
    intro t h
    obtain ⟨ts, hf, hargs⟩ := Typing.app_inversion h
    exact .app (ih hf) (TypingList_iff.mpr hargs)
  | @appArg f pre a a' post _ _ _ ih =>
    intro t h
    obtain ⟨ts, hf, hargs⟩ := Typing.app_inversion h
    obtain ⟨b, hab, hrepl⟩ := hargs.middle
    exact .app hf (TypingList_iff.mpr (hrepl a' (ih hab)))
  | @appBeta ts body args hvals =>
    intro t h
    obtain ⟨ts', hf, hargs⟩ := Typing.app_inversion h
    obtain ⟨r, heq, hbody⟩ := Typing.lam_inversion hf
    cases heq
    exact Typing.subst_nil (by simpa using hbody) hargs
  | @callArg f targs pre a a' post _ _ ih =>
    intro t h
    obtain ⟨d, hd, hg, rfl, hargs⟩ := Typing.call_inversion h
    obtain ⟨b, hab, hrepl⟩ := hargs.middle
    exact .call hd hg (TypingList_iff.mpr (hrepl a' (ih hab)))
  | @callDelta f targs args d _ hd =>
    intro t h
    obtain ⟨d', hd', hg, rfl, hargs⟩ := Typing.call_inversion h
    rw [hd] at hd'
    cases hd'
    exact Typing.subst_nil (hP _ _ hd _ hg) hargs
  | @nodeArg k pre a a' post _ _ ih =>
    intro t h
    obtain ⟨rfl, hk, hargs⟩ := Typing.node_inversion h
    obtain ⟨b, hab, hrepl⟩ := hargs.middle
    refine .node hk (TypingList_iff.mpr ?_)
    have hlen : (pre ++ a' :: post).length = (pre ++ a :: post).length := by simp
    rw [hlen]
    exact hrepl a' (ih hab)
  | letBound _ ih =>
    intro t h
    obtain ⟨bt, hb, hwf, hu, hirr, hbody⟩ := Typing.letE_inversion h
    exact .letE (ih hb) hwf hu hirr hbody
  | @letBind p bound body σ _ hbind =>
    intro t h
    obtain ⟨bt, hb, hwf, hu, _, hbody⟩ := Typing.letE_inversion h
    exact Typing.subst_nil (by simpa using hbody)
      (Pattern.bind_typed p bt bound σ hb hwf hu hbind)
  | matchScrutinee _ ih =>
    intro t h
    obtain ⟨hs, hwf, hu, hbody, hex⟩ := Typing.matchE_inversion h
    exact .matchE (ih hs) hwf hu hbody hex
  | @matchArm st scrutinee arms σ body _ hsel =>
    intro t h
    obtain ⟨hs, hwf, hu, hbody, _⟩ := Typing.matchE_inversion h
    obtain ⟨a, ha, hab, hbind⟩ := matchArms_mem hsel
    subst hab
    exact Typing.subst_nil (by simpa using hbody a ha)
      (Pattern.bind_typed a.1 st scrutinee σ hs (hwf a ha) (hu a ha) hbind)

/-! ## Safety

The usual corollary. `Steps` is the reflexive-transitive closure; a *stuck*
term is one that is neither a value nor able to step. Type safety says a
well-typed term never reaches one.
-/

inductive Steps (S : Signature) (P : Program) : Expr → Expr → Prop where
  | refl {e} : Steps S P e e
  | tail {e e' e''} : Steps S P e e' → Step S P e' e'' → Steps S P e e''

/-- **Type safety.** No reduct of a closed, well-typed term is stuck. -/
theorem type_safety {S : Signature} {P : Program} (hP : Program.WellFormed S P)
    {e e' : Expr} {t : Ty} (h : Typing S P [] e t) (hsteps : Steps S P e e') :
    Expr.IsValue e' ∨ ∃ e'', Step S P e' e'' := by
  have hty : Typing S P [] e' t := by
    induction hsteps with
    | refl => exact h
    | tail _ hstep ih => exact preservation hP hstep ih
  exact progress e' hty

end Buri
