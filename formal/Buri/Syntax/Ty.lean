/-!
# Types

A transcription of `cli/src/compiler/semantics/types.rs:188`. Ten Rust constructors become seven
here; each difference is deliberate:

* `Ty::Error` is excluded. `unify` returns `Ok` for `(Error, _)`
  (`types.rs:693`), `satisfies` returns `true` (`infer.rs:585`), and
  `implements` returns `true` (`types.rs:588`) -- a declarative system holding
  such a type derives everything at it. It is an error-*recovery* artifact, and
  its contract ("a diagnostic was already reported here") is not a typing
  property. What licenses the exclusion is a Rust-side invariant, not an
  argument: *if the checker reports no diagnostics, no `Ty::Error` survives in
  any body*.
* `Ty::SelfTy` is eliminated at elaboration, matching what
  `types::substitute(&ty, &args, self_ty)` already does with its third argument.
* `Ty::Var` is algorithmic and belongs to `Buri/Infer/`, not to the declarative
  system. That split is the whole content of the inference theorems.
* Primitives are not a separate constructor: in Rust a primitive *is* a `TypeConstructor`
  (`tables.prim_id`), reached as `Ty::Con(id, [])` with `TypeDefinition::Primitive(..)`.
  Keeping that shape is what lets `allConstructors` match the Rust `all_ctors` case
  for case.
-/

namespace Buri

abbrev TypeConstructorId := Nat
abbrev ContextTypeId := Nat

/-- `types.rs:44`. Only `bool` is load-bearing for exhaustiveness: it is the one
primitive whose constructor set is small enough to enumerate. The rest are
present so `TypeDefinition.prim` can name them, and so "needs a `_` arm" becomes a
statement about a specific type rather than a catch-all. -/
inductive Primitive where
  | bool
  | i8 | i16 | i32 | i64 | i128
  | u8 | u16 | u32 | u64 | u128
  | f32 | f64
  | char | str | template
  deriving DecidableEq, Repr

/-- `types.rs:188`. -/
inductive Ty where
  /-- A nominal type: a primitive, a struct, or an enum. -/
  | con : TypeConstructorId → List Ty → Ty
  | array : Ty → Ty
  | tuple : List Ty → Ty
  | fn : List Ty → Ty → Ty
  | unit : Ty
  /-- The generated type of a `context { ... }` value (SPEC 11.3). -/
  | ctx : ContextTypeId → Ty
  /-- A rigid generic parameter, by index into the item's generic list. -/
  | param : Nat → Ty
  deriving Repr

/-!
## Induction

`Ty` recurses through `List Ty`, so it is a *nested* inductive: the `induction`
tactic refuses it, and `deriving DecidableEq` has no handler for it. What Lean
does generate is a mutual recursor carrying a second motive over `List Ty`,
and instantiating that second motive at `fun ts => ∀ t ∈ ts, motive t` gives
the eliminator every later proof actually wants.

Writing this once, before anything needs it, is what keeps every subsequent
proof from re-deriving it by hand. The same shape is needed again for `Pattern`,
and in Stage 3 for `Term`.

The motive lands in `Prop`, not `Sort _`. That is forced rather than chosen:
recovering `motive a` from `a ∈ hd :: tl` means casing on the membership proof,
and `Or.casesOn` may only eliminate into `Prop`. It costs nothing, because
*definitions* by recursion on `Ty` are handled by Lean's equation compiler
directly -- the custom eliminator is only ever needed for proofs.
-/

@[elab_as_elim]
theorem Ty.ind' {motive : Ty → Prop}
    (con : ∀ c args, (∀ a ∈ args, motive a) → motive (.con c args))
    (array : ∀ e, motive e → motive (.array e))
    (tuple : ∀ ts, (∀ t ∈ ts, motive t) → motive (.tuple ts))
    (fn : ∀ ps r, (∀ p ∈ ps, motive p) → motive r → motive (.fn ps r))
    (unit : motive .unit)
    (ctx : ∀ k, motive (.ctx k))
    (param : ∀ n, motive (.param n))
    : ∀ t, motive t :=
  fun t =>
    Ty.rec (motive_1 := motive) (motive_2 := fun ts => ∀ t ∈ ts, motive t)
      con array tuple fn unit ctx param
      (fun _ h => absurd h List.not_mem_nil)
      (fun _ _ ihHd ihTl a ha => by
        rcases List.mem_cons.mp ha with rfl | h'
        · exact ihHd
        · exact ihTl a h')
      t

end Buri
