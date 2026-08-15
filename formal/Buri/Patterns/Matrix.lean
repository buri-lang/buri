import Buri.Patterns.Pat

/-!
# The pattern matrix operations

`specialize`, `default`, `headCtors` and `allCtors`, transcribed from
`exhaust.rs:236-262`, together with the arity and field-type functions of
`exhaust.rs:36-83`.

## What reaches the algorithm

`exhaust.rs:445 check` calls `useful` only on rows produced by
`expand(vec![expand_lengths(low, limit)])`. Both rewrites run *first*, so by
the time a pattern reaches `useful`:

* it contains no `Pat::Or` -- `expand` (`exhaust.rs:194`) has split every
  alternation into separate rows; and
* it contains no `Ctor::ArrayRest` -- `expand_lengths` (`exhaust.rs:163`) has
  rewritten each into a disjunction of fixed lengths.

That is worth stating precisely, because it is load-bearing. `allCtors` reports
the constructor set of an array type as `array 0 .. array limit` and *nothing
else*, so a surviving `arrayRest` row would be silently dropped by every
`specialize` -- the matrix would lose coverage it actually had, and the checker
would report a non-exhaustive match that was exhaustive. The reason that never
happens is the rewrite, not the algorithm. `RestFree` below names the property
the theorems then assume, and `Expand.lean` is where it is discharged.
-/

namespace Buri

/-- No `arrayRest` anywhere. Established by `expandLengths`, assumed by the
usefulness theorems. -/
def Pat.RestFree : Pat → Prop
  | .wild => True
  | .ctor (.arrayRest _) _ => False
  | .ctor _ subs => ∀ p ∈ subs, Pat.RestFree p
  | .or alts => ∀ p ∈ alts, Pat.RestFree p

/-- No `or` anywhere. Established by `expand`. -/
def Pat.OrFree : Pat → Prop
  | .wild => True
  | .ctor _ subs => ∀ p ∈ subs, Pat.OrFree p
  | .or _ => False

/-!
## Arity and field types

`Ctor::arity` (`exhaust.rs:38`) and `Ctor::field_types` (`exhaust.rs:53`).
Arity is a property of the declaration; field types additionally depend on the
instantiation, via `substitute`.
-/

def Ctor.arity (S : Sig) : Ctor → Ty → Nat
  | .variant c i, _ => S.variantArity c i
  | .single, .tuple ts => ts.length
  | .single, .con c _ => S.structArity c
  | .single, _ => 0
  | .bool _, _ => 0
  | .lit _, _ => 0
  | .array n, _ => n
  | .arrayRest n, _ => n

/-- The generic arguments a type applies, or none. `exhaust.rs:56` spells this
inline as `match ty { Ty::Con(_, a) => a.clone(), _ => Vec::new() }`. -/
def Ty.argsOf : Ty → List Ty
  | .con _ args => args
  | _ => []

/-- An array's element type. The `_` case is unreachable at a well-typed
input, where an `array`/`arrayRest` constructor only ever meets an array type;
Rust fills it with `Ty::Error` (`exhaust.rs:76`), which this development does
not have, so it fills it with `unit`. Nothing depends on the choice: every
theorem below carries a typing hypothesis that rules the case out. -/
def Ty.elemOf : Ty → Ty
  | .array e => e
  | _ => .unit

def Ctor.fieldTys (S : Sig) : Ctor → Ty → List Ty
  | .variant c i, t => S.variantFieldTys c i t.argsOf
  | .single, .tuple ts => ts
  | .single, .con c args => S.structFieldTys c args
  | .single, _ => []
  | .bool _, _ => []
  | .lit _, _ => []
  | .array n, t => List.replicate n t.elemOf
  | .arrayRest n, t => List.replicate n t.elemOf

/-- Arity and field types agree in length, unconditionally. Every
`specialize` step relies on this: it is what lets a row's head be replaced by
exactly `arity` sub-patterns at exactly `arity` types. -/
theorem Ctor.fieldTys_length (S : Sig) (c : Ctor) (t : Ty) :
    (c.fieldTys S t).length = c.arity S t := by
  cases c <;> cases t <;>
    simp [Ctor.fieldTys, Ctor.arity, Sig.variantFieldTys_length, Sig.structFieldTys_length]

/-!
## The complete constructor set

`Ctx::all_ctors` (`exhaust.rs:218`). `none` means "too many to enumerate",
which is the case for every primitive except `Bool`, and for function, context
and type-parameter types. SPEC 7.3 states this as a deliberate limit:
exhaustiveness is not attempted over integer or string ranges.

Arrays are finite *here* only because rest patterns were already expanded into
fixed lengths and nothing in the match distinguishes anything longer than
`limit`. See `Expand.lean` for why that is sound.
-/
def allCtors (S : Sig) (limit : Nat) : Ty → Option (List Ctor)
  | .con c _ =>
      match S.tycon c with
      | some ⟨.enum vs, _⟩ => some ((List.range vs.length).map (Ctor.variant c))
      | some ⟨.struct _, _⟩ => some [.single]
      | some ⟨.prim .bool, _⟩ => some [.bool false, .bool true]
      | some ⟨.prim _, _⟩ => none
      | none => none
  | .tuple _ => some [.single]
  | .unit => some [.single]
  | .array _ => some ((List.range (limit + 1)).map Ctor.array)
  | .fn _ _ => none
  | .ctx _ => none
  | .param _ => none

/-!
## Matrix operations
-/

/-- `Ctx::specialize` (`exhaust.rs:236`): the rows whose first pattern is
`ctor`, with that pattern's sub-patterns spliced in. A wildcard head matches
every constructor and expands to `arity` wildcards; sub-patterns are padded and
truncated to `arity`, which is what makes the short `subs` vectors that
`exhaust.rs:114` produces for record patterns work. -/
def specializeRow (ct : Ctor) (arity : Nat) : Row → Option Row
  | [] => none
  | .wild :: rest => some (List.replicate arity Pat.wild ++ rest)
  | .ctor c subs :: rest =>
      if c = ct then
        some ((subs ++ List.replicate arity Pat.wild).take arity ++ rest)
      else none
  | .or _ :: _ => none

def specialize (P : List Row) (ct : Ctor) (arity : Nat) : List Row :=
  P.filterMap (specializeRow ct arity)

/-- `Ctx::default_matrix` (`exhaust.rs:252`): rows whose first pattern is a
wildcard, with that column dropped. -/
def defaultRow : Row → Option Row
  | .wild :: rest => some rest
  | _ => none

def defaultMat (P : List Row) : List Row :=
  P.filterMap defaultRow

/-- `Ctx::head_ctors` (`exhaust.rs:260`). -/
def headCtors (P : List Row) : List Ctor :=
  P.filterMap fun r =>
    match r with
    | .ctor c _ :: _ => some c
    | _ => none

/-- The guard `exhaust.rs:328` tests before splitting on constructors. -/
def isComplete (S : Sig) (limit : Nat) (t : Ty) (P : List Row) : Bool :=
  match allCtors S limit t with
  | some all => all.all fun c => headCtors P |>.contains c
  | none => false

end Buri
