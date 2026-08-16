import Buri.Patterns.Pattern

/-!
# The pattern matrix operations

`specialize`, `default`, `headConstructors` and `allConstructors`, transcribed from
`exhaust.rs:236-262`, together with the arity and field-type functions of
`exhaust.rs:36-83`.

## What reaches the algorithm

`exhaust.rs:445 check` calls `useful` only on rows produced by
`expand(vec![expand_lengths(low, limit)])`. Both rewrites run *first*, so by
the time a pattern reaches `useful`:

* it contains no `Pattern::Or` -- `expand` (`exhaust.rs:194`) has split every
  alternation into separate rows; and
* it contains no `Constructor::ArrayRest` -- `expand_lengths` (`exhaust.rs:163`) has
  rewritten each into a disjunction of fixed lengths.

That is worth stating precisely, because it is load-bearing. `allConstructors` reports
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
def Pattern.RestFree : Pattern → Prop
  | .wildcard => True
  | .constructor (.arrayRest _) _ => False
  | .constructor _ subpatterns => ∀ p ∈ subpatterns, Pattern.RestFree p
  | .or alternatives => ∀ p ∈ alternatives, Pattern.RestFree p

/-- No `or` anywhere. Established by `expand`. -/
def Pattern.OrFree : Pattern → Prop
  | .wildcard => True
  | .constructor _ subpatterns => ∀ p ∈ subpatterns, Pattern.OrFree p
  | .or _ => False

/-!
## Arity and field types

`Constructor::arity` (`exhaust.rs:38`) and `Constructor::field_types` (`exhaust.rs:53`).
Arity is a property of the declaration; field types additionally depend on the
instantiation, via `substitute`.
-/

def Constructor.arity (S : Signature) : Constructor → Ty → Nat
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
def Ty.typeArguments : Ty → List Ty
  | .con _ args => args
  | _ => []

/-- An array's element type. The `_` case is unreachable at a well-typed
input, where an `array`/`arrayRest` constructor only ever meets an array type;
Rust fills it with `Ty::Error` (`exhaust.rs:76`), which this development does
not have, so it fills it with `unit`. Nothing depends on the choice: every
theorem below carries a typing hypothesis that rules the case out. -/
def Ty.elementType : Ty → Ty
  | .array e => e
  | _ => .unit

def Constructor.fieldTypes (S : Signature) : Constructor → Ty → List Ty
  | .variant c i, t => S.variantFieldTypes c i t.typeArguments
  | .single, .tuple ts => ts
  | .single, .con c args => S.structFieldTypes c args
  | .single, _ => []
  | .bool _, _ => []
  | .lit _, _ => []
  | .array n, t => List.replicate n t.elementType
  | .arrayRest n, t => List.replicate n t.elementType

/-- Arity and field types agree in length, unconditionally. Every
`specialize` step relies on this: it is what lets a row's head be replaced by
exactly `arity` sub-patterns at exactly `arity` types. -/
theorem Constructor.fieldTypes_length (S : Signature) (c : Constructor) (t : Ty) :
    (c.fieldTypes S t).length = c.arity S t := by
  cases c <;> cases t <;>
    simp [Constructor.fieldTypes, Constructor.arity, Signature.variantFieldTys_length, Signature.structFieldTys_length]

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
def allConstructors (S : Signature) (limit : Nat) : Ty → Option (List Constructor)
  | .con c _ =>
      match S.typeConstructor c with
      | some ⟨.enum vs, _⟩ => some ((List.range vs.length).map (Constructor.variant c))
      | some ⟨.struct _, _⟩ => some [.single]
      | some ⟨.prim .bool, _⟩ => some [.bool false, .bool true]
      | some ⟨.prim _, _⟩ => none
      | none => none
  | .tuple _ => some [.single]
  | .unit => some [.single]
  | .array _ => some ((List.range (limit + 1)).map Constructor.array)
  | .fn _ _ => none
  | .ctx _ => none
  | .param _ => none

/-!
## Matrix operations
-/

/-- `Ctx::specialize` (`exhaust.rs:236`): the rows whose first pattern is
`constructor`, with that pattern's sub-patterns spliced in. A wildcard head matches
every constructor and expands to `arity` wildcards; sub-patterns are padded and
truncated to `arity`, which is what makes the short `subpatterns` vectors that
`exhaust.rs:114` produces for record patterns work. -/
def specializeRow (target : Constructor) (arity : Nat) : Row → Option Row
  | [] => none
  | .wildcard :: rest => some (List.replicate arity Pattern.wildcard ++ rest)
  | .constructor c subpatterns :: rest =>
      if c = target then
        some ((subpatterns ++ List.replicate arity Pattern.wildcard).take arity ++ rest)
      else none
  | .or _ :: _ => none

def specialize (P : List Row) (target : Constructor) (arity : Nat) : List Row :=
  P.filterMap (specializeRow target arity)

/-- `Ctx::default_matrix` (`exhaust.rs:252`): rows whose first pattern is a
wildcard, with that column dropped. -/
def defaultRow : Row → Option Row
  | .wildcard :: rest => some rest
  | _ => none

def defaultMatrix (P : List Row) : List Row :=
  P.filterMap defaultRow

/-- `Ctx::head_ctors` (`exhaust.rs:260`). -/
def headConstructors (P : List Row) : List Constructor :=
  P.filterMap fun r =>
    match r with
    | .constructor c _ :: _ => some c
    | _ => none

/-- The guard `exhaust.rs:328` tests before splitting on constructors. -/
def isComplete (S : Signature) (limit : Nat) (t : Ty) (P : List Row) : Bool :=
  match allConstructors S limit t with
  | some all => all.all fun c => headConstructors P |>.contains c
  | none => false

end Buri
