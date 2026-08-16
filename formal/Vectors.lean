import Buri

/-!
# Test-vector generator

Runs the Lean exhaustiveness algorithm over an enumerated corpus of `match`
statements and writes the verdicts to `formal/vectors/exhaustiveness.txt`, where
`cli/tests/lean_vectors.rs` replays them against the Rust checker.

Regenerate with:

```sh
cd formal && lake env lean --run Vectors.lean
```

The file is checked in, so the Rust suite never needs Lean.

## What one vector is

A scrutinee type, drawn from a fixed prelude of declarations, and an ordered
list of `match` arms drawn from that type's pattern pool. For each, this
computes two things the Rust checker also computes:

* **exhaustive** -- `isExhaustive` on the compiled arms, with `limit` the same
  `max(length_limit) + 1` `exhaustiveness.rs` uses; and
* **unreachable** -- the indices of arms not useful against the arms before
  them, which is `check`'s per-arm loop.

Both sides are driven from the same pattern pool: each entry carries the Lean
`Pattern` *and* the Buri surface syntax that lowers to it. That pairing is the
only place the two models can drift, and it is 60 lines of table rather than a
translation.
-/

open Buri

namespace BuriVectors

/-! ## The prelude, and the signature that mirrors it

Type-constructor ids are positional, and the enum variant order below is the
declaration order in `preludeLines`. Getting either wrong makes the vectors
disagree with Rust, which is exactly what the bridge is for.
-/

def preludeLines : List String :=
  [ "enum Color { Red, Green, Blue }",
    "enum Opt { None, Some(Bool) }",
    "enum Pair { P(Bool, Bool) }",
    "struct Wrap { a: Bool }" ]

/-- id 0 -/ def cBool : TypeConstructorId := 0
/-- id 1 -/ def cInt : TypeConstructorId := 1
/-- id 2 -/ def cColor : TypeConstructorId := 2
/-- id 3 -/ def cOpt : TypeConstructorId := 3
/-- id 4 -/ def cPair : TypeConstructorId := 4
/-- id 5 -/ def cWrap : TypeConstructorId := 5

def tBool : Ty := .con cBool []
def tInt : Ty := .con cInt []

def signature : Signature :=
  { typeConstructors :=
      [ ⟨.prim .bool, 0⟩,
        ⟨.prim .i64, 0⟩,
        ⟨.enum [⟨[]⟩, ⟨[]⟩, ⟨[]⟩], 0⟩,
        ⟨.enum [⟨[]⟩, ⟨[⟨tBool⟩]⟩], 0⟩,
        ⟨.enum [⟨[⟨tBool⟩, ⟨tBool⟩]⟩], 0⟩,
        ⟨.struct [⟨tBool⟩], 0⟩ ] }

/-! ## The pattern pools -/

structure Case where
  pat : Pattern
  src : String

structure Scenario where
  ty : Ty
  tySrc : String
  pool : List Case

private def w : Pattern := .wildcard
private def pbool (b : Bool) : Pattern := .constructor (.bool b) []
private def pvariant (c : TypeConstructorId) (i : Nat) (subs : List Pattern) : Pattern :=
  .constructor (.variant c i) subs
private def psingle (subs : List Pattern) : Pattern := .constructor .single subs
private def parray (subs : List Pattern) : Pattern :=
  .constructor (.array subs.length) subs
private def parrayRest (subs : List Pattern) : Pattern :=
  .constructor (.arrayRest subs.length) subs
private def plit (s : String) : Pattern := .constructor (.lit s) []

def scenarios : List Scenario :=
  [ { ty := tBool, tySrc := "Bool",
      pool :=
        [ ⟨w, "_"⟩, ⟨pbool true, "true"⟩, ⟨pbool false, "false"⟩,
          ⟨.or [pbool true, pbool false], "true | false"⟩ ] },
    { ty := tInt, tySrc := "Int",
      pool := [ ⟨w, "_"⟩, ⟨plit "0", "0"⟩, ⟨plit "1", "1"⟩ ] },
    { ty := .con cColor [], tySrc := "Color",
      pool :=
        [ ⟨w, "_"⟩,
          ⟨pvariant cColor 0 [], ".Red"⟩,
          ⟨pvariant cColor 1 [], ".Green"⟩,
          ⟨pvariant cColor 2 [], ".Blue"⟩,
          ⟨.or [pvariant cColor 0 [], pvariant cColor 1 []], ".Red | .Green"⟩ ] },
    { ty := .con cOpt [], tySrc := "Opt",
      pool :=
        [ ⟨w, "_"⟩,
          ⟨pvariant cOpt 0 [], ".None"⟩,
          ⟨pvariant cOpt 1 [w], ".Some(_)"⟩,
          ⟨pvariant cOpt 1 [pbool true], ".Some(true)"⟩,
          ⟨pvariant cOpt 1 [pbool false], ".Some(false)"⟩,
          -- `findings/README.md` 6: the nested alternation that used to be
          -- rejected. Every vector containing this arm exercises the fix.
          ⟨pvariant cOpt 1 [.or [pbool true, pbool false]], ".Some(true | false)"⟩ ] },
    { ty := .con cPair [], tySrc := "Pair",
      pool :=
        [ ⟨w, "_"⟩,
          ⟨pvariant cPair 0 [w, w], ".P(_, _)"⟩,
          ⟨pvariant cPair 0 [pbool true, w], ".P(true, _)"⟩,
          ⟨pvariant cPair 0 [pbool false, w], ".P(false, _)"⟩,
          ⟨pvariant cPair 0 [w, pbool true], ".P(_, true)"⟩,
          ⟨pvariant cPair 0 [pbool true, pbool true], ".P(true, true)"⟩ ] },
    { ty := .con cWrap [], tySrc := "Wrap",
      pool :=
        [ ⟨w, "_"⟩,
          ⟨psingle [w], "Wrap { a: _ }"⟩,
          ⟨psingle [pbool true], "Wrap { a: true }"⟩,
          ⟨psingle [pbool false], "Wrap { a: false }"⟩ ] },
    { ty := .tuple [tBool, tBool], tySrc := "(Bool, Bool)",
      pool :=
        [ ⟨w, "_"⟩,
          ⟨psingle [w, w], "(_, _)"⟩,
          ⟨psingle [pbool true, w], "(true, _)"⟩,
          ⟨psingle [pbool false, w], "(false, _)"⟩,
          ⟨psingle [w, pbool true], "(_, true)"⟩,
          ⟨psingle [pbool true, pbool true], "(true, true)"⟩ ] },
    { ty := .array tBool, tySrc := "[Bool]",
      pool :=
        [ ⟨w, "_"⟩,
          ⟨parray [], "[]"⟩,
          ⟨parray [w], "[_]"⟩,
          ⟨parray [pbool true], "[true]"⟩,
          ⟨parray [w, w], "[_, _]"⟩,
          ⟨parrayRest [w], "[_, ..rest]"⟩,
          ⟨parrayRest [w, w], "[_, _, ..rest]"⟩ ] } ]

/-! ## Running the algorithm

Exactly what `exhaustiveness.rs`'s `check` runs: one `limit` for the whole
match, arms compiled by `expand(expand_lengths(..))`, an incremental
`covering` matrix for the reachability pass, and a final wildcard-usefulness
test for exhaustiveness.
-/

def verdictExhaustive (sc : Scenario) (arms : List Pattern) : Bool :=
  armsExhaustive signature sc.ty arms

/-- The indices of arms the checker calls unreachable. -/
def verdictUnreachable (sc : Scenario) (arms : List Pattern) : List Nat :=
  let limit := armLimit arms
  let step : Nat × List Nat → Pattern → Nat × List Nat := fun (i, acc) p =>
    let covering := armRows (compileArms limit (arms.take i))
    let rows := compileArms limit [p]
    let useful := rows.any fun q => isUseful signature limit covering [q] [sc.ty]
    (i + 1, if useful then acc else acc ++ [i])
  (arms.foldl step (0, [])).2

/-! ## Enumeration

Every ordered selection of distinct pool entries, of length one, two or three.
Deterministic and total -- there is no seed because there is no randomness.
-/

def injectiveTuples : Nat → Nat → List (List Nat)
  | 0, _ => [[]]
  | k + 1, n =>
      (List.range n).flatMap fun i =>
        (injectiveTuples k n).filterMap fun rest =>
          if rest.contains i then none else some (i :: rest)

def selections (n : Nat) : List (List Nat) :=
  injectiveTuples 1 n ++ injectiveTuples 2 n ++ injectiveTuples 3 n

/-! ## Emitting -/

def natsToString : List Nat → String
  | [] => "-"
  | ns => String.intercalate "," (ns.map toString)

def renderVector (id : Nat) (sc : Scenario) (cases : List Case) : String :=
  let arms := cases.map (·.pat)
  let exhaustive := if verdictExhaustive sc arms then "E" else "N"
  let unreachable := natsToString (verdictUnreachable sc arms)
  String.intercalate "\t"
    ([toString id, exhaustive, unreachable, sc.tySrc] ++ cases.map (·.src))

def allVectors : List String :=
  let step : Nat × List String → Scenario → Nat × List String := fun (id, acc) sc =>
    let picks := selections sc.pool.length
    let rendered := picks.map fun idxs => idxs.filterMap fun i => sc.pool[i]?
    let step' : Nat × List String → List Case → Nat × List String := fun (j, acc') cs =>
      (j + 1, acc' ++ [renderVector j sc cs])
    let (id', rows) := rendered.foldl step' (id, [])
    (id', acc ++ rows)
  (scenarios.foldl step (0, [])).2

def header : List String :=
  [ "# Lean-generated exhaustiveness vectors for cli/tests/lean_vectors.rs.",
    "# Regenerate: cd formal && lake env lean --run Vectors.lean",
    "# P<TAB>line                 a prelude declaration, in order",
    "# id<TAB>E|N<TAB>unreachable<TAB>scrutinee-type<TAB>arm...",
    "#   E = the Lean model says the arms are exhaustive, N = they are not",
    "#   unreachable = comma-separated arm indices, or - for none" ]

def main : IO Unit := do
  let lines := header ++ preludeLines.map (fun l => "P\t" ++ l) ++ allVectors
  IO.FS.createDirAll "vectors"
  IO.FS.writeFile "vectors/exhaustiveness.txt" (String.intercalate "\n" lines ++ "\n")
  IO.println s!"wrote {allVectors.length} vectors"

end BuriVectors

def main : IO Unit := BuriVectors.main
