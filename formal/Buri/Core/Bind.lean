import Buri.Core.Syntax

/-!
# Matching an expression against a pattern

`Pattern.matches` answers *whether* a pattern fires; the operational semantics
also needs *what it binds*. `Pattern.bind` is the same traversal returning the
bound sub-expressions, in the wildcard order `Pattern.binderTypes` uses.

The two are the same function up to erasure, and that is the content of
`Pattern.bind_isSome`:

```
(Pattern.bind p e).isSome = Pattern.matches p e.erase
```

which is what carries `exhaustive_correct_unbounded` -- a statement about
`Value` and `Pattern.matches` -- into `match` progress, a statement about
`Expr` and `Pattern.bind`. Without it the exhaustiveness development and the
core language would be two disconnected artefacts.

## The one side condition

`Pattern.WellFormed`. A `bool` or a literal has no fields, so `Value.bool` and
`Value.lit` carry none -- but an `Expr.node` for a `bool` still has an argument
*list*, which is empty for every well-formed term and which `Expr.fields`
returns verbatim. Well-formedness caps a pattern's sub-patterns at its
constructor's arity, which for `bool` and a literal is zero, so the two sides
agree. Without it, `.constructor (.bool true) [_]` would bind against a node
that `matches` rejects.
-/

namespace Buri

mutual

/-- Match `e` against `p`, returning what the wildcards bound, or `none`. The
branch structure is `Pattern.matches`'s, constructor for constructor. -/
def Pattern.bind : Pattern → Expr → Option (List Expr)
  | .wildcard, e => some [e]
  | .or alternatives, e => Pattern.bindAny alternatives e
  | .constructor (.arrayRest n) subpatterns, e =>
      match e.asArray with
      | some es => if n ≤ es.length then Pattern.bindAll subpatterns (es.take n) else none
      | none => none
  | .constructor target subpatterns, e =>
      if e.constructorOf = some target then Pattern.bindAll subpatterns e.fields else none

/-- Pointwise, where a missing pattern is a wildcard that binds nothing -- the
same padding convention `Pattern.matchesAll` uses. -/
def Pattern.bindAll : List Pattern → List Expr → Option (List Expr)
  | [], _ => some []
  | _ :: _, [] => none
  | p :: ps, e :: es =>
      match Pattern.bind p e, Pattern.bindAll ps es with
      | some σ₁, some σ₂ => some (σ₁ ++ σ₂)
      | _, _ => none

/-- The first alternative that fires wins, which is why `UniformBinders` has to
be a side condition: without it the choice would be observable in the arm
body's context. -/
def Pattern.bindAny : List Pattern → Expr → Option (List Expr)
  | [], _ => none
  | p :: ps, e =>
      match Pattern.bind p e with
      | some σ => some σ
      | none => Pattern.bindAny ps e

end

/-- The first arm that fires, with its bindings. `match` is first-match, so
this is deterministic. -/
def matchArms : List (Pattern × Expr) → Expr → Option (List Expr × Expr)
  | [], _ => none
  | (p, body) :: rest, e =>
      match Pattern.bind p e with
      | some σ => some (σ, body)
      | none => matchArms rest e

/-! ## Erasure commutes with the projections a pattern uses

Both are unconditional. `Expr.fields` and `Value.fields` differ only at a
`bool` or literal node, where the value form carries no fields at all -- and
that is the gap `Pattern.WellFormed` closes below.
-/

@[simp] theorem Expr.erase_constructorOf (e : Expr) :
    (Expr.erase e).constructorOf = e.constructorOf := by
  match e with
  | .node k args =>
    cases k <;>
      simp [Expr.erase, Expr.constructorOf, Value.constructorOf, NodeKind.ctor]
  | .var _ | .lam _ _ | .app _ _ | .call _ _ _ | .letE _ _ _ | .matchE _ _ _ =>
    rfl

theorem Expr.eraseList_take (n : Nat) (es : List Expr) :
    Expr.eraseList (es.take n) = (Expr.eraseList es).take n := by
  simp [Expr.eraseList_eq_map, List.map_take]

/-- The fields agree except at a `bool` or literal node, where the value form
has none. -/
theorem Expr.erase_fields (e : Expr) :
    (Expr.erase e).fields = Expr.eraseList e.fields ∨ (Expr.erase e).fields = [] := by
  match e with
  | .node k args =>
    cases k <;>
      first
        | exact Or.inl rfl
        | exact Or.inr rfl
  | .var _ | .lam _ _ | .app _ _ | .call _ _ _ | .letE _ _ _ | .matchE _ _ _ =>
    exact Or.inr rfl

/-- At every constructor that *has* fields, the two agree outright. The
excluded heads are exactly the ones whose `fieldTypes` is empty. -/
theorem Expr.erase_fields_of_ctor {e : Expr} {c : Constructor}
    (hc : e.constructorOf = some c) (hb : ∀ b, c ≠ .bool b) (hl : ∀ s, c ≠ .lit s) :
    (Expr.erase e).fields = Expr.eraseList e.fields := by
  match e with
  | .node k args =>
    cases k <;> simp only [Expr.constructorOf, NodeKind.ctor, Option.some.injEq] at hc <;>
      first
        | rfl
        | (subst hc; exact absurd rfl (by first | exact hb _ | exact hl _))
  | .var _ | .lam _ _ | .app _ _ | .call _ _ _ | .letE _ _ _ | .matchE _ _ _ =>
    simp [Expr.constructorOf] at hc

/-! ## `bind` and `matches` are the same question -/

theorem Pattern.bindAll_isSome {S : Signature} :
    ∀ (fieldTypes : List Ty) (subpatterns : List Pattern),
      Pattern.WellFormedPrefix S fieldTypes subpatterns →
      (∀ p ∈ subpatterns, ∀ t, Pattern.WellFormed S t p →
        ∀ e, (Pattern.bind p e).isSome = Pattern.matches p e.erase) →
      ∀ es : List Expr,
        (Pattern.bindAll subpatterns es).isSome = Pattern.matchesAll subpatterns (Expr.eraseList es) := by
  intro fieldTypes
  induction fieldTypes with
  | nil =>
    intro subpatterns hwf _ es
    cases subpatterns with
    | nil => simp [Pattern.bindAll, Pattern.matchesAll]
    | cons _ _ => exact absurd hwf (by simp [Pattern.WellFormedPrefix])
  | cons ft fts ih =>
    intro subpatterns hwf hsub es
    cases subpatterns with
    | nil => simp [Pattern.bindAll, Pattern.matchesAll]
    | cons p ps =>
      simp only [Pattern.WellFormedPrefix] at hwf
      cases es with
      | nil => simp [Pattern.bindAll, Expr.eraseList]
      | cons e es =>
        have h₁ := hsub p (by simp) ft hwf.1 e
        have h₂ := ih ps hwf.2 (fun q hq => hsub q (by simp [hq])) es
        simp only [Pattern.bindAll, Expr.eraseList, Pattern.matchesAll_cons]
        cases hb : Pattern.bind p e <;> cases hbs : Pattern.bindAll ps es <;>
          rw [hb] at h₁ <;> rw [hbs] at h₂ <;> simp_all

/-- A head with no field types -- `bool` and a literal -- can carry no
sub-patterns at all. -/
private theorem subpatterns_nil_of_no_fields {S : Signature} {t : Ty}
    {head : Constructor} {subpatterns : List Pattern}
    (hempty : head.fieldTypes S t = [])
    (hwf : Pattern.WellFormedPrefix S (head.fieldTypes S t) subpatterns) : subpatterns = [] := by
  cases subpatterns with
  | nil => rfl
  | cons q qs => rw [hempty] at hwf; exact absurd hwf (by simp [Pattern.WellFormedPrefix])

/-- The unfolding equation for every head but `arrayRest`, which
`Pattern.WellFormed` excludes anyway. Mirrors `Pattern.matches_constructor`. -/
theorem Pattern.bind_constructor (head : Constructor) (subpatterns : List Pattern) (e : Expr)
    (hnotrest : ∀ n, head ≠ .arrayRest n) :
    Pattern.bind (.constructor head subpatterns) e
      = if e.constructorOf = some head then Pattern.bindAll subpatterns e.fields else none := by
  cases head with
  | arrayRest n => exact absurd rfl (hnotrest n)
  | _ => rfl

/-- The constructor case, with the head abstracted so the five well-formed
heads are one branch rather than five. `hfields` is where the `bool`/literal
asymmetry is paid for: either erasure keeps the fields, or the pattern has no
sub-patterns to look at. -/
private theorem bind_isSome_constructor {head : Constructor} {subpatterns : List Pattern}
    (e : Expr) (hnotrest : ∀ n, head ≠ .arrayRest n)
    (hall : ∀ es, (Pattern.bindAll subpatterns es).isSome
        = Pattern.matchesAll subpatterns (Expr.eraseList es))
    (hfields : ∀ e' : Expr, e'.constructorOf = some head →
      (Expr.erase e').fields = Expr.eraseList e'.fields ∨ subpatterns = []) :
    (Pattern.bind (.constructor head subpatterns) e).isSome
      = Pattern.matches (.constructor head subpatterns) e.erase := by
  rw [Pattern.bind_constructor head subpatterns e hnotrest,
      Pattern.matches_constructor _ _ _ hnotrest, Expr.erase_constructorOf]
  by_cases hc : e.constructorOf = some head
  · rw [if_pos hc, hc]
    simp only [decide_true, Bool.true_and]
    rcases hfields e hc with h | h
    · rw [h, hall e.fields]
    · subst h; simp [Pattern.bindAll]
  · rw [if_neg hc]
    simp [hc]

theorem Pattern.bind_isSome {S : Signature} :
    ∀ (p : Pattern) (t : Ty), Pattern.WellFormed S t p →
      ∀ e : Expr, (Pattern.bind p e).isSome = Pattern.matches p e.erase := by
  intro p
  induction p using Pattern.ind' with
  | wildcard => intro t _ e; simp [Pattern.bind]
  | or alternatives ih =>
    intro t hwf e
    induction alternatives with
    | nil => simp [Pattern.bind, Pattern.bindAny, Pattern.matches, Pattern.matchesAny]
    | cons a as ihs =>
      have hA := ih a (by simp) t (Pattern.WellFormedAlternatives.mem hwf (by simp)) e
      have hAs : (Pattern.bindAny as e).isSome = Pattern.matchesAny as e.erase :=
        ihs (fun q hq => ih q (by simp [hq])) hwf.2
      show (Pattern.bindAny (a :: as) e).isSome = Pattern.matchesAny (a :: as) e.erase
      simp only [Pattern.bindAny, Pattern.matchesAny]
      cases hb : Pattern.bind a e <;> rw [hb] at hA <;> simp_all
  | constructor head subpatterns ih =>
    intro t hwf e
    cases head with
    | arrayRest n => exact absurd hwf (by simp [Pattern.WellFormed])
    | _ =>
      refine bind_isSome_constructor e (by intro n; simp)
        (Pattern.bindAll_isSome _ subpatterns hwf ih) ?_
      intro e' hc
      first
        | exact Or.inl (Expr.erase_fields_of_ctor hc
            (fun b h => Constructor.noConfusion h) (fun s h => Constructor.noConfusion h))
        | exact Or.inr (subpatterns_nil_of_no_fields (t := t) rfl hwf)

end Buri
