import Buri.Core.Bind

/-!
# The declarative typing judgment

`Typing S P Γ e t` -- "under signature `S` and program `P`, in context `Γ`, the
expression `e` has type `t`". Contexts are `List Ty` with `Γ[0]` innermost,
matching `Expr.var`'s de Bruijn indices.

This is the *declarative* system: what it means for a program to be well typed,
not how a checker decides it. `Check.lean` gives the algorithm and `Sound.lean`
relates the two.

## The two semantic premises

`letE` and `matchE` each carry a premise that is a statement about *values*
rather than about syntax:

* `let p = e` requires `∀ v : t, Pattern.matches p v` -- irrefutability, design/static-rules.md
  rule 2;
* `match` requires `∀ v : t, ∃ arm, Pattern.matches arm v` -- exhaustiveness,
  SPEC 7.3.

Stating them semantically is deliberate. It is what makes `match` progress
immediate -- the premise *is* what progress needs -- and it is what gives
`exhaustive_correct_unbounded` a job: the checker establishes the syntactic
condition `isExhaustive ... = true`, and that theorem is exactly the bridge to
the semantic one. `Sound.lean` is where the two meet.

## Generic instantiation

`call f targs args` types the arguments at `d.params` *instantiated at*
`targs`, and has type `d.ret` instantiated at `targs`. That is
`types::substitute(&ty, &args, None)`, positionally, with no binders anywhere.

`Program.WellFormed` states the corresponding obligation on bodies **per
instantiation**: for every `targs`, the instantiated body checks at the
instantiated signature. The natural statement would quantify over the generic
signature and derive the instantiated one by a type-substitution lemma on
derivations; that lemma is *not* proved here, and the reason is the semantic
premises above -- exhaustiveness at `t` does not obviously give exhaustiveness
at `substitute targs t`, since the two range over different sets of values.
`formal/README.md` lists this under what is not proved.
-/

namespace Buri

mutual

inductive Typing (S : Signature) (P : Program) : List Ty → Expr → Ty → Prop where
  | var {Γ i t} :
      Γ[i]? = some t →
      Typing S P Γ (.var i) t
  | lam {Γ ts body r} :
      Typing S P (ts ++ Γ) body r →
      Typing S P Γ (.lam ts body) (.fn ts r)
  | app {Γ f args ts r} :
      Typing S P Γ f (.fn ts r) →
      TypingList S P Γ args ts →
      Typing S P Γ (.app f args) r
  | call {Γ f targs args d} :
      P.fn f = some d →
      targs.length = d.generics →
      TypingList S P Γ args (d.params.map (substitute targs)) →
      Typing S P Γ (.call f targs args) (substitute targs d.ret)
  | node {Γ k args} :
      NodeKind.WellFormed S k →
      TypingList S P Γ args (k.fieldTys S args.length) →
      Typing S P Γ (.node k args) k.resultTy
  | letE {Γ p bound body t r} :
      Typing S P Γ bound t →
      Pattern.WellFormed S t p →
      Pattern.UniformBinders S t p →
      (∀ v, (S ⊢ᵥ v : t) → Pattern.matches p v = true) →
      Typing S P (Pattern.binderTypes S t p ++ Γ) body r →
      Typing S P Γ (.letE p bound body) r
  | matchE {Γ t scrutinee arms r} :
      Typing S P Γ scrutinee t →
      (∀ a ∈ arms, Pattern.WellFormed S t a.1) →
      (∀ a ∈ arms, Pattern.UniformBinders S t a.1) →
      (∀ a ∈ arms, Typing S P (Pattern.binderTypes S t a.1 ++ Γ) a.2 r) →
      (∀ v, (S ⊢ᵥ v : t) → ∃ a ∈ arms, Pattern.matches a.1 v = true) →
      Typing S P Γ (.matchE t scrutinee arms) r

/-- Pointwise typing of an argument list. This is `Forall₂ (Typing S P Γ)`
spelled out: Lean will not accept `Forall₂` *inside* the definition of `Typing`
itself, because a nested inductive's parameters may not mention variables bound
by the constructor, and `Γ` is one. `TypingList_iff` immediately converts, and
nothing after that line mentions `TypingList` again. -/
inductive TypingList (S : Signature) (P : Program) : List Ty → List Expr → List Ty → Prop where
  | nil {Γ} : TypingList S P Γ [] []
  | cons {Γ e t es ts} :
      Typing S P Γ e t → TypingList S P Γ es ts → TypingList S P Γ (e :: es) (t :: ts)

end

theorem TypingList_iff {S : Signature} {P : Program} {Γ : List Ty} :
    ∀ {es : List Expr} {ts : List Ty},
      TypingList S P Γ es ts ↔ Forall₂ (Typing S P Γ) es ts := by
  intro es
  induction es with
  | nil =>
    intro ts
    exact ⟨fun h => by cases h; exact .nil, fun h => by cases h; exact .nil⟩
  | cons e es ih =>
    intro ts
    constructor
    · intro h; cases h with | cons hd tl => exact .cons hd (ih.mp tl)
    · intro h; cases h with | cons hd tl => exact .cons hd (ih.mpr tl)

/-! ## Inversion

The judgment is syntax-directed, so each of these is one `cases`. They exist
because `cases` on the judgment itself only unifies when the *type* index is a
variable, and several proofs below meet it at a specific type.
-/

theorem Typing.var_inversion {S P Γ i t} (h : Typing S P Γ (.var i) t) : Γ[i]? = some t := by
  cases h with | var hi => exact hi

theorem Typing.lam_inversion {S P Γ ts body t} (h : Typing S P Γ (.lam ts body) t) :
    ∃ r, t = .fn ts r ∧ Typing S P (ts ++ Γ) body r := by
  cases h with | lam hb => exact ⟨_, rfl, hb⟩

theorem Typing.app_inversion {S P Γ f args t} (h : Typing S P Γ (.app f args) t) :
    ∃ ts, Typing S P Γ f (.fn ts t) ∧ Forall₂ (Typing S P Γ) args ts := by
  cases h with | app hf hargs => exact ⟨_, hf, TypingList_iff.mp hargs⟩

theorem Typing.call_inversion {S P Γ f targs args t} (h : Typing S P Γ (.call f targs args) t) :
    ∃ d, P.fn f = some d ∧ targs.length = d.generics ∧ t = substitute targs d.ret ∧
      Forall₂ (Typing S P Γ) args (d.params.map (substitute targs)) := by
  cases h with | call hd hg hargs => exact ⟨_, hd, hg, rfl, TypingList_iff.mp hargs⟩

theorem Typing.node_inversion {S P Γ k args t} (h : Typing S P Γ (.node k args) t) :
    t = k.resultTy ∧ NodeKind.WellFormed S k ∧
      Forall₂ (Typing S P Γ) args (k.fieldTys S args.length) := by
  cases h with | node hk hargs => exact ⟨rfl, hk, TypingList_iff.mp hargs⟩

theorem Typing.letE_inversion {S P Γ p bound body t} (h : Typing S P Γ (.letE p bound body) t) :
    ∃ bt, Typing S P Γ bound bt ∧ Pattern.WellFormed S bt p ∧ Pattern.UniformBinders S bt p ∧
      (∀ v, (S ⊢ᵥ v : bt) → Pattern.matches p v = true) ∧
      Typing S P (Pattern.binderTypes S bt p ++ Γ) body t := by
  cases h with | letE hb hwf hu hirr hbody => exact ⟨_, hb, hwf, hu, hirr, hbody⟩

theorem Typing.matchE_inversion {S P Γ st scrutinee arms t}
    (h : Typing S P Γ (.matchE st scrutinee arms) t) :
    Typing S P Γ scrutinee st ∧
      (∀ a ∈ arms, Pattern.WellFormed S st a.1) ∧
      (∀ a ∈ arms, Pattern.UniformBinders S st a.1) ∧
      (∀ a ∈ arms, Typing S P (Pattern.binderTypes S st a.1 ++ Γ) a.2 t) ∧
      (∀ v, (S ⊢ᵥ v : st) → ∃ a ∈ arms, Pattern.matches a.1 v = true) := by
  cases h with | matchE hs hwf hu hbody hex => exact ⟨hs, hwf, hu, hbody, hex⟩

/-- Every declared function's body checks, at every instantiation of its
generics. See the module docstring for why this is stated per-instantiation. -/
def Program.WellFormed (S : Signature) (P : Program) : Prop :=
  ∀ f d, P.fn f = some d → ∀ targs : List Ty, targs.length = d.generics →
    Typing S P (d.params.map (substitute targs)) (d.instantiate targs) (substitute targs d.ret)

/-! ## Construction and destruction agree

`NodeKind.fieldTys` says what an expression node's arguments must be;
`Constructor.fieldTypes` (`exhaustiveness.rs`'s `Ctor::field_types`) says what a
pattern's sub-patterns are matched against. They are the same list. That is not
a coincidence to be checked case by case at every use -- it is this lemma, and
it is what makes `match` type-preserving.
-/

theorem NodeKind.fieldTys_ctor (S : Signature) (k : NodeKind) (n : Nat) :
    k.fieldTys S n = (k.ctor n).fieldTypes S k.resultTy := by
  cases k <;>
    simp [NodeKind.fieldTys, NodeKind.ctor, NodeKind.resultTy, Constructor.fieldTypes,
      Ty.typeArguments, Ty.elementType]

/-! ## Erasure preserves typing

A closed value's erasure is a well-typed `Value` at the same type. This is the
hinge between the core language and the exhaustiveness development: the latter
is stated entirely over `Value` and `HasType`, and this is what puts a core
expression into that world.
-/

theorem Typing.erase {S : Signature} {P : Program} :
    ∀ (e : Expr) {Γ : List Ty} {t : Ty}, Typing S P Γ e t → Expr.IsValue e →
      (S ⊢ᵥ e.erase : t) := by
  intro e
  induction e using Expr.ind' with
  | var i => intro Γ t _ hv; cases hv
  | app f args _ _ => intro Γ t _ hv; cases hv
  | call f targs args _ => intro Γ t _ hv; cases hv
  | letE p bound body _ _ => intro Γ t _ hv; cases hv
  | matchE t scrutinee arms _ _ => intro Γ t' _ hv; cases hv
  | lam ts body _ =>
    intro Γ t h _
    cases h with
    | lam _ => exact .fnv
  | node k args ih =>
    intro Γ t h hv
    cases hv with
    | node hargs =>
    obtain ⟨hres, hk, hty⟩ := Typing.node_inversion h
    subst hres
    have herase : Forall₂ (HasType S) (Expr.eraseList args) (k.fieldTys S args.length) := by
      rw [Expr.eraseList_eq_map]
      exact hty.imp_map fun a ha b hab => ih a ha hab (hargs a ha)
    cases k with
    | variant c i targs =>
      exact .variant hk (by simpa [NodeKind.fieldTys] using herase)
    | structE c targs =>
      exact .struct hk (by simpa [NodeKind.fieldTys] using herase)
    | tuple ts =>
      exact .tuple (by simpa [NodeKind.fieldTys] using herase)
    | unitE =>
      have hnil : args = [] := by
        have := hty.length_eq
        simpa [NodeKind.fieldTys] using List.eq_nil_of_length_eq_zero this
      subst hnil
      exact .unit
    | boolE c b => exact .bool hk
    | litE c s =>
      obtain ⟨p', hp, hne⟩ := hk
      exact .lit hp hne
    | arrayE elem =>
      refine .array ?_
      simpa [NodeKind.fieldTys, Expr.eraseList_length] using herase

/-! ## Canonical forms

At a function type, a value is a lambda. This is the only canonical-forms
lemma the core language needs of its own: every other type is inhabited by a
`node`, and `Decompose.lean` already says which.
-/

theorem Typing.canonical_fn {S : Signature} {P : Program} {Γ : List Ty}
    {e : Expr} {ts : List Ty} {r : Ty}
    (h : Typing S P Γ e (.fn ts r)) (hv : Expr.IsValue e) :
    ∃ body, e = .lam ts body ∧ Typing S P (ts ++ Γ) body r := by
  cases hv with
  | @lam ts' body =>
    obtain ⟨r', heq, hb⟩ := Typing.lam_inversion h
    cases heq
    exact ⟨body, rfl, hb⟩
  | @node k args _ =>
    obtain ⟨heq, _, _⟩ := Typing.node_inversion h
    exact absurd heq.symm (by cases k <;> simp [NodeKind.resultTy])

/-! ## What a pattern binds is well typed

The preservation half of pattern matching: if a well-typed value matches a
well-formed pattern, the sub-expressions it binds are well typed at exactly
`Pattern.binderTypes`. `UniformBinders` is what makes this true at an
alternation -- without it, `bindAny` picking a later alternative would produce
bindings at the *first* alternative's types.
-/

theorem Pattern.bindAll_typed {S : Signature} {P : Program} {Γ : List Ty} :
    ∀ (fieldTypes : List Ty) (subpatterns : List Pattern) (es : List Expr) (σ : List Expr),
      Forall₂ (Typing S P Γ) es fieldTypes →
      Pattern.WellFormedPrefix S fieldTypes subpatterns →
      Pattern.UniformBindersList S fieldTypes subpatterns →
      (∀ p ∈ subpatterns, ∀ t e τ, Typing S P Γ e t → Pattern.WellFormed S t p →
        Pattern.UniformBinders S t p → Pattern.bind p e = some τ →
        Forall₂ (Typing S P Γ) τ (Pattern.binderTypes S t p)) →
      Pattern.bindAll subpatterns es = some σ →
      Forall₂ (Typing S P Γ) σ (Pattern.binderTypesList S fieldTypes subpatterns) := by
  intro fieldTypes
  induction fieldTypes with
  | nil =>
    intro subpatterns es σ _ hwf _ _ hbind
    cases subpatterns with
    | nil => cases hbind; exact .nil
    | cons _ _ => exact absurd hwf (by simp [Pattern.WellFormedPrefix])
  | cons ft fts ih =>
    intro subpatterns es σ hty hwf huni hsub hbind
    cases subpatterns with
    | nil => cases hbind; exact .nil
    | cons p ps =>
      cases es with
      | nil => simp [Pattern.bindAll] at hbind
      | cons e es =>
        cases hty with
        | cons hte htes =>
          simp only [Pattern.WellFormedPrefix] at hwf
          simp only [Pattern.UniformBindersList] at huni
          simp only [Pattern.bindAll] at hbind
          cases hb : Pattern.bind p e with
          | none => rw [hb] at hbind; simp at hbind
          | some τ₁ =>
            cases hbs : Pattern.bindAll ps es with
            | none => rw [hb, hbs] at hbind; simp at hbind
            | some τ₂ =>
              rw [hb, hbs] at hbind
              simp only [Option.some.injEq] at hbind
              subst hbind
              exact Forall₂.append
                (hsub p (by simp) ft e τ₁ hte hwf.1 huni.1 hb)
                (ih ps es τ₂ htes hwf.2 huni.2
                  (fun q hq => hsub q (by simp [hq])) hbs)

theorem Pattern.bind_typed {S : Signature} {P : Program} {Γ : List Ty} :
    ∀ (p : Pattern) (t : Ty) (e : Expr) (σ : List Expr),
      Typing S P Γ e t → Pattern.WellFormed S t p → Pattern.UniformBinders S t p →
      Pattern.bind p e = some σ →
      Forall₂ (Typing S P Γ) σ (Pattern.binderTypes S t p) := by
  intro p
  induction p using Pattern.ind' with
  | wildcard =>
    intro t e σ hty _ _ hbind
    simp only [Pattern.bind, Option.some.injEq] at hbind
    subst hbind
    exact .cons hty .nil
  | or alternatives ih =>
    intro t e σ hty hwf huni hbind
    have hbindAny : Pattern.bindAny alternatives e = some σ := hbind
    -- Find the alternative that fired.
    have key : ∀ (alts : List Pattern), (∀ q ∈ alts, q ∈ alternatives) →
        Pattern.bindAny alts e = some σ →
        ∃ q ∈ alternatives, Pattern.bind q e = some σ := by
      intro alts
      induction alts with
      | nil => intro _ h; simp [Pattern.bindAny] at h
      | cons a as ihs =>
        intro hmem h
        simp only [Pattern.bindAny] at h
        cases hb : Pattern.bind a e with
        | none => rw [hb] at h; exact ihs (fun q hq => hmem q (by simp [hq])) h
        | some τ =>
          rw [hb] at h
          simp only [Option.some.injEq] at h
          subst h
          exact ⟨a, hmem a (by simp), hb⟩
    obtain ⟨q, hq, hqb⟩ := key alternatives (fun q hq => hq) hbindAny
    obtain ⟨huq, htq⟩ := Pattern.UniformBindersAlts.mem huni hq
    have := ih q hq t e σ hty (Pattern.WellFormedAlternatives.mem hwf hq) huq hqb
    rw [htq] at this
    exact this
  | constructor head subpatterns ih =>
    intro t e σ hty hwf huni hbind
    cases head with
    | arrayRest n => exact absurd hwf (by simp [Pattern.WellFormed])
    | _ =>
      rw [Pattern.bind_constructor _ _ _ (by intro n; simp)] at hbind
      split at hbind
      · next hc =>
        -- The scrutinee is a node built with exactly this constructor.
        match e, hc with
        | .node k args, hc =>
          simp only [Expr.constructorOf, Option.some.injEq] at hc
          cases hty with
          | node hk hargs =>
            refine Pattern.bindAll_typed (P := P) _ subpatterns args σ ?_ hwf huni ih hbind
            have hargs' := TypingList_iff.mp hargs
            rw [NodeKind.fieldTys_ctor, hc] at hargs'
            exact hargs'
      · simp at hbind

end Buri
