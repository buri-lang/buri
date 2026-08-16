import Buri.Core.Decide
import Buri.Core.Subst

/-!
# The algorithmic checker

`Expr.infer` is the executable counterpart of `Infer::check_expr`
(`cli/src/compiler/semantics/expressions.rs`): it walks the expression once,
syntax-directed, and returns the type or `none`.

## What is modelled, and what is not

Rust's `check_expr(e, expected: Option<&Ty>)` is *bidirectional*, and the
downward flow exists to feed unification: a numeric literal elaborates to a
fresh `Ty::Var` constrained to the integer classes, and the expected type is
unified into it. `Ty::Var`, `unify`, `default_numerics` and trait obligations
are the **inference stage**, and `formal/README.md` excludes them from the
declarative system by design -- a `Ty::Var` is not a type a declarative
judgment can hold.

So `Expr` here is the *post-inference* form: every binder, every generic
instantiation and every literal already carries its type. On that form the
Rust checker's two directions collapse into one, because `check_expr(e, Some(t))`
succeeds exactly when the inferred type equals `t`. `Expr.infer` is that
function, and

```
Expr.check S P Γ e t = (Expr.infer S P Γ e == some t)
```

is the checking judgment. **Unification and numeric defaulting are out of
scope**, and so is everything from source bytes to HIR.

## Where the checker does real work

Three places, and all three are where the exhaustiveness development is
consumed:

* `letE` runs the usefulness algorithm on a one-row matrix -- irrefutability,
  SPEC 14 rule 2;
* `matchE` runs it on the compiled arms -- exhaustiveness, SPEC 7.3;
* both check `Pattern.wellFormedB` on the *compiled* arms as well as the source
  ones, because `exhaustive_correct_unbounded` needs well-formedness of what
  the algorithm actually saw.

`limit` is `max(length_limit) + 1`, exactly as `exhaustiveness.rs`'s `check`
computes it.
-/

namespace Buri

/-- The array-length bound: one more than the longest length any arm mentions.
`exhaustiveness.rs`'s `check` computes `lowered.iter().map(length_limit).max()
.unwrap_or(0) + 1`. -/
def armLimit (ps : List Pattern) : Nat := Pattern.lengthLimitList ps + 1

theorem lengthLimit_lt_armLimit {ps : List Pattern} {p : Pattern} (h : p ∈ ps) :
    p.lengthLimit < armLimit ps := by
  have := Pattern.lengthLimit_le_of_mem h
  simp only [armLimit]
  omega

/-- The compiled arms are exhaustive, as a Boolean. This is the whole of
`exhaustiveness.rs`'s final `ctx.useful(&covering, &[Pat::Wild], &types)` test,
with the pre-passes in front of it. -/
def armsExhaustive (S : Signature) (t : Ty) (ps : List Pattern) : Bool :=
  isExhaustive S (armLimit ps) t (armRows (compileArms (armLimit ps) ps))

/-- Every side condition on a single source arm the checker can decide. -/
def armOk (S : Signature) (t : Ty) (p : Pattern) : Bool :=
  Pattern.wellFormedB S t p && Pattern.loweredArraysB p && Pattern.uniformBindersB p

/-- Everything `exhaustive_correct_unbounded` asks of a `match`'s arms. -/
def armsOk (S : Signature) (t : Ty) (ps : List Pattern) : Bool :=
  ps.all (armOk S t) && (compileArms (armLimit ps) ps).all (Pattern.wellFormedB S t)
    && armsExhaustive S t ps

/-- `NodeKind.WellFormed`, decided. -/
def NodeKind.wellFormedB (S : Signature) : NodeKind → Bool
  | .variant c i _ => i < S.variantCount c
  | .structE c _ => S.isStruct c
  | .tuple _ => true
  | .unitE => true
  | .boolE c _ => S.primitiveOf c == some .bool
  | .litE c _ => match S.primitiveOf c with
      | some p => p != .bool
      | none => false
  | .arrayE _ => true

theorem NodeKind.wellFormedB_sound {S : Signature} {k : NodeKind}
    (h : k.wellFormedB S = true) : NodeKind.WellFormed S k := by
  cases k with
  | variant c i targs => simpa [NodeKind.wellFormedB, NodeKind.WellFormed] using h
  | structE c targs => simpa [NodeKind.wellFormedB, NodeKind.WellFormed] using h
  | tuple ts => trivial
  | unitE => trivial
  | boolE c b => simpa [NodeKind.wellFormedB, NodeKind.WellFormed] using h
  | litE c s =>
    simp only [NodeKind.wellFormedB] at h
    split at h
    · next p hp => exact ⟨p, hp, by simpa using h⟩
    · simp at h
  | arrayE e => trivial

/-! ## The checker -/

mutual

/-- Infer the type of `e` in context `Γ`, or fail. -/
def Expr.infer (S : Signature) (P : Program) : List Ty → Expr → Option Ty
  | Γ, .var i => Γ[i]?
  | Γ, .lam ts body => (Expr.infer S P (ts ++ Γ) body).map (Ty.fn ts)
  | Γ, .app f args =>
      match Expr.infer S P Γ f with
      | some (.fn ts r) =>
          if Expr.inferList S P Γ args = some ts then some r else none
      | _ => none
  | Γ, .call f targs args =>
      match P.fn f with
      | some d =>
          if targs.length = d.generics ∧
             Expr.inferList S P Γ args = some (d.params.map (substitute targs)) then
            some (substitute targs d.ret)
          else none
      | none => none
  | Γ, .node k args =>
      if k.wellFormedB S = true ∧ Expr.inferList S P Γ args = some (k.fieldTys S args.length) then
        some k.resultTy
      else none
  | Γ, .letE p bound body =>
      match Expr.infer S P Γ bound with
      | some bt => if armsOk S bt [p] then Expr.infer S P (Pattern.binderTypes S bt p ++ Γ) body
                   else none
      | none => none
  | Γ, .matchE t scrutinee arms =>
      if Expr.infer S P Γ scrutinee = some t ∧ armsOk S t (arms.map Prod.fst) then
        Expr.inferArms S P Γ t arms
      else none

def Expr.inferList (S : Signature) (P : Program) : List Ty → List Expr → Option (List Ty)
  | _, [] => some []
  | Γ, e :: es =>
      match Expr.infer S P Γ e, Expr.inferList S P Γ es with
      | some t, some ts => some (t :: ts)
      | _, _ => none

/-- Every arm body must infer, and they must all agree. A `match` with no arms
is rejected: its type would be unconstrained. Rust reaches the same place by a
different route -- an armless `match` leaves the result variable unresolved. -/
def Expr.inferArms (S : Signature) (P : Program) : List Ty → Ty → List (Pattern × Expr) → Option Ty
  | _, _, [] => none
  | Γ, t, (p, body) :: rest =>
      match Expr.infer S P (Pattern.binderTypes S t p ++ Γ) body with
      | none => none
      | some r =>
          match Expr.inferArms S P Γ t rest with
          | none => if rest.isEmpty then some r else none
          | some r' => if r = r' then some r else none

end

/-- The checking judgment: does `e` check at the expected type `t`? -/
def Expr.check (S : Signature) (P : Program) (Γ : List Ty) (e : Expr) (t : Ty) : Bool :=
  Expr.infer S P Γ e = some t

end Buri
