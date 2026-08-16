import Buri.Syntax.Ty

/-!
# Pattern constructors

`Constructor` from `cli/src/compiler/semantics/exhaustiveness.rs` -- the head
constructor of a lowered pattern.

It lives in its own module because both `Value` and `Pattern` need it: a value's
`constructorOf` and a pattern's head are compared directly, and that comparison is the
whole of `specialize`.
-/

namespace Buri

/-- The head constructor of a pattern. -/
inductive Constructor where
  | variant : TypeConstructorId → Nat → Constructor
  /-- Structs, tuples and unit: exactly one constructor. -/
  | single : Constructor
  | bool : Bool → Constructor
  /-- A fixed-length array pattern. -/
  | array : Nat → Constructor
  /-- `[a, b, ..rest]` -- matches any length at or above `n`. Removed by
  `expand_lengths` before the usefulness algorithm runs. -/
  | arrayRest : Nat → Constructor
  /-- A literal drawn from a set too large to enumerate -- an integer, a
  string, a char, a float. Two different literals are two different
  constructors, and no finite set of them ever completes a match. -/
  | lit : String → Constructor
  deriving DecidableEq, Repr

end Buri
