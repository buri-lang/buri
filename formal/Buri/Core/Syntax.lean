import Buri.Core.Binders

/-!
# The core expression language

The fragment of Buri a declarative type system can adjudicate. `formal/README.md`
gives the honest number: of the 83 reject cases in `cli/tests/reject`, roughly
50 test rules that live between source bytes and HIR -- name resolution,
visibility, opacity, method resolution, `derive` expansion, `?` desugaring
-- and only about 15 are core-typing rules. This file is aimed at those 15:
variables, application, constructors, `match`, `let`, lambdas, and generic
instantiation.

## Two design decisions worth defending

**Every data form is one node.** `variant`, `struct`, `tuple`, `unit`, `bool`,
a literal, and an array literal are all `Expr.node k args` for a `NodeKind k`.
That is not laziness: these are exactly the forms the pattern algorithm sees as
a `Constructor` with fields (`Value.constructorOf`, `Value.fields`), so keeping
them one node makes `Expr.erase` a one-liner, gives `match` a single canonical
forms lemma, and collapses what would otherwise be seven near-identical
congruence rules in the operational semantics into one. `NodeKind.ctor` and
`NodeKind.fieldTys` are the projections that recover the distinctions, and they
line up with `exhaustiveness.rs`'s `Ctor` and `Ctor::field_types` by
construction.

**Substitution needs no shifting.** Terms are de Bruijn, `var 0` is the
innermost binding, and a context is a `List Ty` with `Γ[0]` innermost. What is
ever substituted is a **closed** value, so `Expr.subst` carries a depth and
never lifts:

```
subst σ d (var i) = if i < d then var i else σ[i - d]
```

There is not one lifting lemma in this development. That is partly de Bruijn
hygiene and partly the language: Buri has no type-variable *binders* either --
generics are instantiated positionally against `Ty.param n` -- so `substitute`
is a plain fold with no capture-avoidance obligation.

## Binders in patterns

A `Pattern` here is the same `Pattern` the exhaustiveness development uses,
which is the *lowered* form: `lower` already erased `PatKind::Bind` to
`Pat::Wild`. So a wildcard is read as a **binder**: `Pattern.wildcard` binds the
value it matches, and a pattern binds one variable per wildcard, left to right.
That is a faithful over-approximation of Buri, where `x` binds and `_` does not
-- the body is free to ignore a binding. It is also what keeps
`exhaustive_correct_unbounded` directly applicable, rather than needing a second
pattern type and an erasure between them.
-/

namespace Buri

abbrev FnId := Nat

/-- The data constructors, as an expression head. One per `Constructor`, plus
the type information `Constructor` does not carry. -/
inductive NodeKind where
  /-- Variant `i` of enum `c`, at generic arguments `targs`. -/
  | variant : TypeConstructorId → Nat → List Ty → NodeKind
  | structE : TypeConstructorId → List Ty → NodeKind
  | tuple : List Ty → NodeKind
  | unitE : NodeKind
  | boolE : TypeConstructorId → Bool → NodeKind
  | litE : TypeConstructorId → String → NodeKind
  /-- An array literal, at its element type. Its *length* comes from the
  argument list, which is why `ctor` and `fieldTys` below take it. -/
  | arrayE : Ty → NodeKind
  deriving Repr

/-- The type a node has. -/
def NodeKind.resultTy : NodeKind → Ty
  | .variant c _ targs => .con c targs
  | .structE c targs => .con c targs
  | .tuple ts => .tuple ts
  | .unitE => .unit
  | .boolE c _ => .con c []
  | .litE c _ => .con c []
  | .arrayE e => .array e

/-- The types a node's arguments must have. This is `Ctor::field_types`
(`exhaustiveness.rs`) read as a *construction* rule rather than a destruction
one, and `NodeKind.fieldTys_ctor` below is the statement that the two agree. -/
def NodeKind.fieldTys (S : Signature) : NodeKind → Nat → List Ty
  | .variant c i targs, _ => S.variantFieldTypes c i targs
  | .structE c targs, _ => S.structFieldTypes c targs
  | .tuple ts, _ => ts
  | .unitE, _ => []
  | .boolE _ _, _ => []
  | .litE _ _, _ => []
  | .arrayE e, n => List.replicate n e

/-- The pattern constructor a node with `n` arguments is matched by. -/
def NodeKind.ctor : NodeKind → Nat → Constructor
  | .variant c i _, _ => .variant c i
  | .structE _ _, _ => .single
  | .tuple _, _ => .single
  | .unitE, _ => .single
  | .boolE _ b, _ => .bool b
  | .litE _ s, _ => .lit s
  | .arrayE _, n => .array n

/-- The side condition each kind carries, and exactly the premise of the
matching `HasType` rule: a variant index is in range, a struct id really names
a struct, a `bool` literal sits at `Bool`, a non-`bool` literal does not. -/
def NodeKind.WellFormed (S : Signature) : NodeKind → Prop
  | .variant c i _ => i < S.variantCount c
  | .structE c _ => S.isStruct c = true
  | .tuple _ => True
  | .unitE => True
  | .boolE c _ => S.primitiveOf c = some .bool
  | .litE c _ => ∃ p, S.primitiveOf c = some p ∧ p ≠ .bool
  | .arrayE _ => True

/-- Applying a generic instantiation to a node's type annotations. -/
def NodeKind.substTy (args : List Ty) : NodeKind → NodeKind
  | .variant c i targs => .variant c i (targs.map (substitute args))
  | .structE c targs => .structE c (targs.map (substitute args))
  | .tuple ts => .tuple (ts.map (substitute args))
  | .unitE => .unitE
  | .boolE c b => .boolE c b
  | .litE c s => .litE c s
  | .arrayE e => .arrayE (substitute args e)

/-- The core expression language. -/
inductive Expr where
  /-- A de Bruijn index into the context; `var 0` is the innermost binding. -/
  | var : Nat → Expr
  /-- `fn(x₁ : t₁, .., xₙ : tₙ) => body`. The parameter types are annotations,
  as they are in the surface language: Buri infers no parameter type. -/
  | lam : List Ty → Expr → Expr
  | app : Expr → List Expr → Expr
  /-- A call to a top-level function at a generic instantiation. -/
  | call : FnId → List Ty → List Expr → Expr
  | node : NodeKind → List Expr → Expr
  /-- `let p = bound; body`. The pattern must be irrefutable (design/static-rules.md rule 2). -/
  | letE : Pattern → Expr → Expr → Expr
  /-- `match (scrutinee) { arms }`, annotated with the scrutinee's type. -/
  | matchE : Ty → Expr → List (Pattern × Expr) → Expr
  deriving Repr

/-- The eliminator every proof below wants: `Expr` recurses through
`List Expr` and through `List (Pattern × Expr)`, so Lean's generated recursor
carries four motives. Instantiating the list ones at `∀ x ∈ xs, motive x` is
what makes ordinary structural induction available. Same shape as `Ty.ind'`,
`Value.ind'` and `Pattern.ind'`, and for the same reason. -/
@[elab_as_elim]
theorem Expr.ind' {motive : Expr → Prop}
    (var : ∀ i, motive (.var i))
    (lam : ∀ ts body, motive body → motive (.lam ts body))
    (app : ∀ f args, motive f → (∀ a ∈ args, motive a) → motive (.app f args))
    (call : ∀ f targs args, (∀ a ∈ args, motive a) → motive (.call f targs args))
    (node : ∀ k args, (∀ a ∈ args, motive a) → motive (.node k args))
    (letE : ∀ p bound body, motive bound → motive body → motive (.letE p bound body))
    (matchE : ∀ t scrutinee arms, motive scrutinee →
      (∀ a ∈ arms, motive a.2) → motive (.matchE t scrutinee arms))
    : ∀ e, motive e :=
  fun e =>
    Expr.rec (motive_1 := motive) (motive_2 := fun es => ∀ x ∈ es, motive x)
      (motive_3 := fun as => ∀ x ∈ as, motive x.2) (motive_4 := fun a => motive a.2)
      var lam app call node letE matchE
      (fun _ h => absurd h List.not_mem_nil)
      (fun _ _ ihHd ihTl a ha => by
        rcases List.mem_cons.mp ha with rfl | h'
        · exact ihHd
        · exact ihTl a h')
      (fun _ h => absurd h List.not_mem_nil)
      (fun _ _ ihHd ihTl a ha => by
        rcases List.mem_cons.mp ha with rfl | h'
        · exact ihHd
        · exact ihTl a h')
      (fun _ _ ih => ih)
      e

/-! ## Values

A value is a lambda, or a node all of whose arguments are values. Nothing else:
a `var` is not closed, and `app`, `call`, `letE` and `matchE` all reduce.
-/

inductive Expr.IsValue : Expr → Prop where
  | lam {ts body} : Expr.IsValue (.lam ts body)
  | node {k args} : (∀ a ∈ args, Expr.IsValue a) → Expr.IsValue (.node k args)

/-! ## Erasure to the pattern algorithm's `Value`

`Value` is what `Pattern.matches` and `HasType` are stated over. Erasure throws
away exactly what no pattern can inspect: a lambda becomes `opaqueValue`.
-/

/-! Erasure to the pattern algorithm's `Value`. A lambda -- and anything not
yet a value -- becomes `opaqueValue`, which is exactly the point: no pattern
can inspect one. -/

mutual

/-- The `Value` an expression erases to. -/
def Expr.erase : Expr → Value
  | .node (.variant c i _) args => .variant c i (Expr.eraseList args)
  | .node (.structE _ _) args => .single (Expr.eraseList args)
  | .node (.tuple _) args => .single (Expr.eraseList args)
  | .node .unitE args => .single (Expr.eraseList args)
  | .node (.boolE _ b) _ => .bool b
  | .node (.litE _ s) _ => .lit s
  | .node (.arrayE _) args => .array (Expr.eraseList args)
  | _ => .opaqueValue

def Expr.eraseList : List Expr → List Value
  | [] => []
  | e :: es => Expr.erase e :: Expr.eraseList es

end

@[simp] theorem Expr.eraseList_length (es : List Expr) :
    (Expr.eraseList es).length = es.length := by
  induction es with
  | nil => rfl
  | cons e es ih => simp [Expr.eraseList, ih]

theorem Expr.eraseList_eq_map (es : List Expr) :
    Expr.eraseList es = es.map Expr.erase := by
  induction es with
  | nil => rfl
  | cons e es ih => simp [Expr.eraseList, ih]

/-- What a pattern tests an expression against -- the same projection
`Value.constructorOf` is, one level up. -/
def Expr.constructorOf : Expr → Option Constructor
  | .node k args => some (k.ctor args.length)
  | _ => none

def Expr.fields : Expr → List Expr
  | .node _ args => args
  | _ => []

/-- An array literal's elements, for the one pattern -- `arrayRest` -- that
inspects a range of lengths rather than a fixed one. -/
def Expr.asArray : Expr → Option (List Expr)
  | .node (.arrayE _) args => some args
  | _ => none

/-! ## Substitution

Simultaneous, capture-free by construction: `σ` holds closed terms, so
descending under a binder only raises the depth. The list companions are what
make the recursion structural through `List Expr` and `List (Pattern × Expr)`.
-/

mutual

def Expr.subst (σ : List Expr) : Nat → Expr → Expr
  | d, .var i => if i < d then .var i else σ.getD (i - d) (.var i)
  | d, .lam ts body => .lam ts (Expr.subst σ (d + ts.length) body)
  | d, .app f args => .app (Expr.subst σ d f) (Expr.substList σ d args)
  | d, .call f targs args => .call f targs (Expr.substList σ d args)
  | d, .node k args => .node k (Expr.substList σ d args)
  | d, .letE p bound body =>
      .letE p (Expr.subst σ d bound) (Expr.subst σ (d + p.binderCount) body)
  | d, .matchE t scrutinee arms =>
      .matchE t (Expr.subst σ d scrutinee) (Expr.substArms σ d arms)

def Expr.substList (σ : List Expr) : Nat → List Expr → List Expr
  | _, [] => []
  | d, e :: es => Expr.subst σ d e :: Expr.substList σ d es

def Expr.substArms (σ : List Expr) : Nat → List (Pattern × Expr) → List (Pattern × Expr)
  | _, [] => []
  | d, (p, body) :: rest =>
      (p, Expr.subst σ (d + p.binderCount) body) :: Expr.substArms σ d rest

end

/-! Applying a generic instantiation to every type annotation in a term. This
is the only reason `Expr` carries types at all, and it is a plain fold --
`Ty.param` is an index, not a bound name, so there is nothing to avoid
capturing. -/

mutual

/-- Instantiate every type annotation in a term at `args`. -/
def Expr.substTy (args : List Ty) : Expr → Expr
  | .var i => .var i
  | .lam ts body => .lam (ts.map (substitute args)) (Expr.substTy args body)
  | .app f es => .app (Expr.substTy args f) (Expr.substTyList args es)
  | .call f targs es => .call f (targs.map (substitute args)) (Expr.substTyList args es)
  | .node k es => .node (k.substTy args) (Expr.substTyList args es)
  | .letE p bound body => .letE p (Expr.substTy args bound) (Expr.substTy args body)
  | .matchE t scrutinee arms =>
      .matchE (substitute args t) (Expr.substTy args scrutinee) (Expr.substTyArms args arms)

def Expr.substTyList (args : List Ty) : List Expr → List Expr
  | [] => []
  | e :: es => Expr.substTy args e :: Expr.substTyList args es

def Expr.substTyArms (args : List Ty) : List (Pattern × Expr) → List (Pattern × Expr)
  | [] => []
  | (p, body) :: rest => (p, Expr.substTy args body) :: Expr.substTyArms args rest

end

/-! ## Programs

A top-level function, with its generic arity, its parameter and result types
(which may mention `Ty.param`), and its body. `Program.WellFormed` is stated
per *instantiation* -- see `Typing.lean` for why that is an assumption rather
than a theorem.
-/

structure FnDecl where
  generics : Nat
  params : List Ty
  ret : Ty
  body : Expr
  deriving Repr

structure Program where
  fns : List FnDecl
  deriving Repr

def Program.fn (P : Program) (f : FnId) : Option FnDecl := P.fns[f]?

/-- The body of `d`, instantiated at `targs`. -/
def FnDecl.instantiate (d : FnDecl) (targs : List Ty) : Expr := d.body.substTy targs

end Buri
