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

Only the second is true. `expand` splits the alternations it finds at the
*top* of a column and leaves nested ones alone, so `.Some(true | false)`
reaches the algorithm with the alternation buried; `specialize` then peels the
`.Some` off and exposes it. That is `findings/README.md` 6, and the fix -- now
in `exhaustiveness.rs` -- is that `specialize` and `default_matrix`
*distribute* over an or-headed row rather than dropping it, and `head_ctors`
descends into alternatives. The model below is of the fixed algorithm; the
consequence for the proofs is that or-freeness is nowhere a hypothesis.

Rest-freeness is load-bearing, though. `allConstructors` reports the
constructor set of an array type as `array 0 .. array limit` and *nothing
else*, so a surviving `arrayRest` row would be silently dropped by every
`specialize` -- the matrix would lose coverage it actually had, and the checker
would report a non-exhaustive match that was exhaustive. The reason that never
happens is the rewrite, not the algorithm. `Pattern.WellFormed`
(`WellFormed.lean`) is where rest-freeness is recorded as a hypothesis, and
`Expand.lean` is where it is discharged.
-/

namespace Buri

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

Each of the three takes an or-headed row apart rather than dropping it, which
is the `findings/README.md` 6 fix. Distribution is why a row now maps to a
*list* of rows instead of an `Option`: `(a | b) :: rest` becomes two rows.
-/

/-- `Ctx::specialize` (`exhaustiveness.rs`): the rows whose first pattern is
`target`, with that pattern's sub-patterns spliced in.

* a wildcard head matches every constructor and expands to `arity` wildcards;
* a constructor head survives only if it *is* `target`, and its sub-patterns
  are padded and truncated to `arity` -- which is what makes the short
  `subpatterns` vectors `lower` produces for record patterns work;
* an **alternation** head distributes: one row per alternative, each carrying
  the rest of the original row, specialised in turn. Dropping the row instead
  would lose the coverage it provides. -/
def specializeRow (target : Constructor) (arity : Nat) : Row → List Row
  | [] => []
  | .wildcard :: rest => [List.replicate arity Pattern.wildcard ++ rest]
  | .constructor c subpatterns :: rest =>
      if c = target then
        [(subpatterns ++ List.replicate arity Pattern.wildcard).take arity ++ rest]
      else []
  | .or alternatives :: rest =>
      alternatives.attach.flatMap fun a => specializeRow target arity (a.1 :: rest)
termination_by row => row
decreasing_by
  have := List.sizeOf_lt_of_mem a.2
  simp_wf
  omega

/-- `specializeRow`'s pad-and-truncate, when the sub-patterns already fit.
Padding to the arity and then truncating there is just padding: the truncation
is a no-op. Every proof that touches `specializeRow` needs this, and the
hypothesis `subpatterns.length <= arity` is exactly the well-formedness clause
that makes the truncation harmless -- see `WellFormed.lean`. -/
theorem pad_take {alpha : Type} {l : List alpha} {x : alpha} {a : Nat} (h : l.length <= a) :
    (l ++ List.replicate a x).take a = l ++ List.replicate (a - l.length) x := by
  rw [List.take_append, List.take_of_length_le h, List.take_replicate,
      Nat.min_eq_left (by omega)]

/-! `attach` is a termination artefact -- the recursion is on an element of
`alternatives`, and Lean needs the membership proof to see it shrink. These
three equations put the plain `flatMap` back, and every proof below uses them
rather than unfolding the definitions. -/

theorem List.attach_flatMap {α β : Type} (l : List α) (f : α → List β) :
    (l.attach.flatMap fun a => f a.1) = l.flatMap f := by
  simp [List.flatMap_def]

@[simp] theorem specializeRow_nil (target : Constructor) (arity : Nat) :
    specializeRow target arity [] = [] := by rw [specializeRow]

@[simp] theorem specializeRow_wildcard (target : Constructor) (arity : Nat) (rest : Row) :
    specializeRow target arity (Pattern.wildcard :: rest)
      = [List.replicate arity Pattern.wildcard ++ rest] := by rw [specializeRow]

@[simp] theorem specializeRow_constructor (target : Constructor) (arity : Nat)
    (c : Constructor) (subpatterns : List Pattern) (rest : Row) :
    specializeRow target arity (Pattern.constructor c subpatterns :: rest)
      = if c = target then
          [(subpatterns ++ List.replicate arity Pattern.wildcard).take arity ++ rest]
        else [] := by rw [specializeRow]

@[simp] theorem specializeRow_or (target : Constructor) (arity : Nat)
    (alternatives : List Pattern) (rest : Row) :
    specializeRow target arity (Pattern.or alternatives :: rest)
      = alternatives.flatMap fun a => specializeRow target arity (a :: rest) := by
  rw [specializeRow]
  exact List.attach_flatMap alternatives (fun a => specializeRow target arity (a :: rest))

def specialize (P : List Row) (target : Constructor) (arity : Nat) : List Row :=
  P.flatMap (specializeRow target arity)

/-- `Ctx::default_matrix` (`exhaustiveness.rs`): rows whose first pattern is a
wildcard, with that column dropped -- and, since the fix, rows whose first
pattern is an alternation *one of whose alternatives* is a wildcard. -/
def defaultRow : Row → List Row
  | [] => []
  | .wildcard :: rest => [rest]
  | .constructor _ _ :: _ => []
  | .or alternatives :: rest =>
      alternatives.attach.flatMap fun a => defaultRow (a.1 :: rest)
termination_by row => row
decreasing_by
  have := List.sizeOf_lt_of_mem a.2
  simp_wf
  omega

@[simp] theorem defaultRow_nil : defaultRow [] = [] := by rw [defaultRow]

@[simp] theorem defaultRow_wildcard (rest : Row) : defaultRow (Pattern.wildcard :: rest) = [rest] := by
  rw [defaultRow]

@[simp] theorem defaultRow_constructor (c : Constructor) (subpatterns : List Pattern) (rest : Row) :
    defaultRow (Pattern.constructor c subpatterns :: rest) = [] := by rw [defaultRow]

@[simp] theorem defaultRow_or (alternatives : List Pattern) (rest : Row) :
    defaultRow (Pattern.or alternatives :: rest)
      = alternatives.flatMap fun a => defaultRow (a :: rest) := by
  rw [defaultRow]
  exact List.attach_flatMap alternatives (fun a => defaultRow (a :: rest))

def defaultMatrix (P : List Row) : List Row :=
  P.flatMap defaultRow

/-- `collect_head_ctors` (`exhaustiveness.rs`): the constructors a pattern's
head can start with. An alternation contributes every constructor any of its
alternatives does, so that a column covered by `true | false` counts as
complete. -/
def Pattern.headConstructors : Pattern → List Constructor
  | .wildcard => []
  | .constructor c _ => [c]
  | .or alternatives => alternatives.attach.flatMap fun a => Pattern.headConstructors a.1
decreasing_by
  have := List.sizeOf_lt_of_mem a.2
  simp_wf
  omega

@[simp] theorem Pattern.headConstructors_wildcard :
    Pattern.headConstructors .wildcard = [] := by rw [Pattern.headConstructors]

@[simp] theorem Pattern.headConstructors_constructor (c : Constructor) (subpatterns : List Pattern) :
    Pattern.headConstructors (.constructor c subpatterns) = [c] := by rw [Pattern.headConstructors]

@[simp] theorem Pattern.headConstructors_or (alternatives : List Pattern) :
    Pattern.headConstructors (.or alternatives)
      = alternatives.flatMap Pattern.headConstructors := by
  rw [Pattern.headConstructors]
  exact List.attach_flatMap alternatives Pattern.headConstructors

/-- `Ctx::head_ctors`. Rust deduplicates as it goes; nothing here ever asks for
more than membership, so this does not. -/
def headConstructors (P : List Row) : List Constructor :=
  P.flatMap fun row => match row with
    | [] => []
    | p :: _ => p.headConstructors

end Buri
