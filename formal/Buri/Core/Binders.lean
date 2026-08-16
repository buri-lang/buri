import Buri.Patterns.Exhaustive

/-!
# What a pattern binds

The exhaustiveness development never asks what a pattern *binds*, only what it
*matches* -- `lower` erased `PatKind::Bind` to `Pat::Wild` before the algorithm
ever saw it. The core language does have to ask, so this file reads a wildcard
as a **binder**: `Pattern.wildcard` binds the value it matches, and a pattern
binds one variable per wildcard, left to right, outermost first.

That is a faithful over-approximation of Buri, where `x` binds and `_` does
not: a body is free to ignore a binding. What it buys is that the core language
reuses `Pattern` verbatim, so `exhaustive_correct_unbounded` applies to it
directly instead of through an erasure between two pattern types.

## Or-patterns

`.Some(x) | .None` is rejected by Buri (`or-pattern-bindings`), and the reason
is visible here: the two alternatives would bind different things, so the arm
body has no single context. `binderTypes` therefore reads an alternation's
binders off its **first** alternative, and `UniformBinders` is the side
condition -- a premise of the `match` and `let` typing rules -- that every
alternative agrees. The algorithmic checker discharges it conservatively, by
demanding that alternatives bind nothing at all; `Sound.lean` says why that is
sound and not complete.
-/

namespace Buri

/-! ## How many variables a pattern binds

`binderCount` is deliberately independent of the signature and the type: it is
what `Expr.subst` needs to know when it steps under an arm, and substitution
cannot consult a type. `binderTypes_length` is the statement that the two agree
-- for **well-formed** patterns, which is exactly where they can, since a
pattern carrying more sub-patterns than its constructor has fields would count
binders the type side never sees.
-/

mutual

def Pattern.binderCount : Pattern → Nat
  | .wildcard => 1
  | .constructor _ subpatterns => Pattern.binderCountList subpatterns
  | .or alternatives => Pattern.binderCountHead alternatives

def Pattern.binderCountList : List Pattern → Nat
  | [] => 0
  | p :: ps => Pattern.binderCount p + Pattern.binderCountList ps

def Pattern.binderCountHead : List Pattern → Nat
  | [] => 0
  | p :: _ => Pattern.binderCount p

end

/-! ## What types it binds them at -/

mutual

def Pattern.binderTypes (S : Signature) : Ty → Pattern → List Ty
  | t, .wildcard => [t]
  | t, .constructor c subpatterns =>
      Pattern.binderTypesList S (c.fieldTypes S t) subpatterns
  | t, .or alternatives => Pattern.binderTypesHead S t alternatives

def Pattern.binderTypesList (S : Signature) : List Ty → List Pattern → List Ty
  | _, [] => []
  | [], _ :: _ => []
  | fieldType :: fieldTypes, p :: ps =>
      Pattern.binderTypes S fieldType p ++ Pattern.binderTypesList S fieldTypes ps

def Pattern.binderTypesHead (S : Signature) : Ty → List Pattern → List Ty
  | _, [] => []
  | t, p :: _ => Pattern.binderTypes S t p

end

/-! ## Alternatives must agree -/

mutual

/-- Every alternation in the pattern binds the same types in every
alternative. This is SPEC's `or-pattern-bindings` rule, as a proposition. -/
def Pattern.UniformBinders (S : Signature) : Ty → Pattern → Prop
  | _, .wildcard => True
  | t, .constructor c subpatterns =>
      Pattern.UniformBindersList S (c.fieldTypes S t) subpatterns
  | t, .or alternatives =>
      Pattern.UniformBindersAlts S t (Pattern.binderTypesHead S t alternatives) alternatives

def Pattern.UniformBindersList (S : Signature) : List Ty → List Pattern → Prop
  | _, [] => True
  | [], _ :: _ => True
  | fieldType :: fieldTypes, p :: ps =>
      Pattern.UniformBinders S fieldType p ∧ Pattern.UniformBindersList S fieldTypes ps

def Pattern.UniformBindersAlts (S : Signature) (t : Ty) (types : List Ty) : List Pattern → Prop
  | [] => True
  | p :: ps =>
      (Pattern.UniformBinders S t p ∧ Pattern.binderTypes S t p = types) ∧
      Pattern.UniformBindersAlts S t types ps

end

theorem Pattern.UniformBindersAlts.mem {S : Signature} {t : Ty} {types : List Ty} :
    ∀ {alternatives : List Pattern} {p : Pattern},
      Pattern.UniformBindersAlts S t types alternatives → p ∈ alternatives →
      Pattern.UniformBinders S t p ∧ Pattern.binderTypes S t p = types := by
  intro alternatives
  induction alternatives with
  | nil => intro p _ hp; exact absurd hp List.not_mem_nil
  | cons q qs ih =>
    intro p h hp
    simp only [Pattern.UniformBindersAlts] at h
    rcases List.mem_cons.mp hp with rfl | hp'
    · exact h.1
    · exact ih h.2 hp'

/-! ## The counts agree

Under well-formedness -- which caps a pattern's sub-pattern list at its
constructor's arity -- the type-directed `binderTypes` and the type-free
`binderCount` produce the same number of binders. That is what lets
`Expr.subst` step under a `match` arm without consulting the signature.
-/

theorem Pattern.binderTypesList_length {S : Signature} :
    ∀ (fieldTypes : List Ty) (subpatterns : List Pattern),
      Pattern.WellFormedPrefix S fieldTypes subpatterns →
      (∀ p ∈ subpatterns, ∀ t, Pattern.WellFormed S t p →
        (Pattern.binderTypes S t p).length = p.binderCount) →
      (Pattern.binderTypesList S fieldTypes subpatterns).length
        = Pattern.binderCountList subpatterns := by
  intro fieldTypes
  induction fieldTypes with
  | nil =>
    intro subpatterns hwf _
    cases subpatterns with
    | nil => rfl
    | cons _ _ => exact absurd hwf (by simp [Pattern.WellFormedPrefix])
  | cons ft fts ih =>
    intro subpatterns hwf hsub
    cases subpatterns with
    | nil => rfl
    | cons p ps =>
      simp only [Pattern.WellFormedPrefix] at hwf
      simp only [Pattern.binderTypesList, Pattern.binderCountList, List.length_append,
        hsub p (by simp) ft hwf.1, ih ps hwf.2 fun q hq => hsub q (by simp [hq])]

theorem Pattern.binderTypes_length {S : Signature} :
    ∀ (p : Pattern) (t : Ty), Pattern.WellFormed S t p →
      (Pattern.binderTypes S t p).length = p.binderCount := by
  intro p
  induction p using Pattern.ind' with
  | wildcard => intro t _; rfl
  | constructor head subpatterns ih =>
    intro t hwf
    cases head with
    | arrayRest n => exact absurd hwf (by simp [Pattern.WellFormed])
    | _ => exact Pattern.binderTypesList_length _ subpatterns hwf ih
  | or alternatives ih =>
    intro t hwf
    cases alternatives with
    | nil => rfl
    | cons a as =>
      exact ih a (by simp) t (Pattern.WellFormedAlternatives.mem hwf (by simp))

end Buri
