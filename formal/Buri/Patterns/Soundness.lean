import Buri.Patterns.Exhaustive

/-!
# Why `useful_sound` is not here

`Complete.lean` proves the safety direction: `isUseful = false` means the matrix
really covers everything, so an accepted `match` never gets stuck. The converse
-- `isUseful = true → Useful`, "the compiler reports a non-exhaustive match only
when a value really is uncovered" -- is the *expressiveness* direction, and it
is the one still open.

`findings/README.md` 6 used to be the reason: a nested alternation made the
checker reject an exhaustive `match`, so the theorem was flatly false. That is
fixed, and the model here is of the fixed algorithm. What replaced it is a
sharper obstacle, and this file records it as a theorem rather than as a
paragraph.

## The obstacle

`Pattern.WellFormed` is the invariant every theorem in `Patterns/` runs under.
It says a pattern carries no more sub-patterns than its constructor has fields,
and that each is well formed at the corresponding field type. What it does
**not** say is that the head constructor *belongs to* the type at all -- and it
cannot, because for a nullary constructor the check is vacuous:
`Constructor.fieldTypes S (.bool b) t` is `[]` at every `t`, so
`.constructor (.bool true) []` is well formed at an enum type.

`useful_not_sound_at_wellFormed` below exhibits exactly that: a one-column
matrix that is empty, a pattern vector holding `true` matched against an enum
type, and the algorithm reporting it useful -- correctly, since it has no row to
contradict it -- where no value of that type matches it at all.

So `useful_sound` needs a pattern **typing** judgment, not just
well-formedness: `.bool b` types only at `Bool`, `.variant c i` only at
`Ty.con c args`, `.array n` only at an array type. That judgment is what
`semantics/patterns.rs` computes and what this development, aimed at the
`match`-progress theorem, never needed -- `isUseful_false_covers` is true for
ill-typed patterns, because a pattern that matches nothing only ever *loses*
coverage.

## And then inhabitation

Even with pattern typing, the `wildcard`-with-incomplete-constructor-set branch
needs a *witness*: a value built at a constructor the matrix does not mention.
That is inhabitation, and it is where SPEC 14 rule 14 (recursive types must be
productive) stops being stylistic. At a non-enumerable type it additionally
needs a literal outside a finite set -- true, but a separate argument.

Both are honest work, and neither is done. `formal/README.md` lists them under
what is not proved.
-/

namespace Buri

/-- A signature with one enum, `E`, having a single payload-free variant. -/
private def oneEnum : Signature :=
  { typeConstructors := [⟨.enum [⟨[]⟩], 0⟩] }

private def enumTy : Ty := .con 0 []

/-- **`useful_sound` is false if `Pattern.WellFormed` is all that is assumed of
the pattern vector.**

The pattern is `true`, the scrutinee type is an enum, and the matrix is empty.
The algorithm answers "useful", which is the right answer to the question it was
asked -- an empty matrix covers nothing -- but there is no value of the
scrutinee's type that the vector matches, so `Useful` fails. A pattern typing
judgment is the missing premise; see the module docstring. -/
theorem useful_not_sound_at_wellFormed :
    ∃ (S : Signature) (t : Ty) (p : Pattern),
      Pattern.WellFormed S t p ∧
      isUseful S 1 [] [p] [t] = true ∧
      ¬ Useful S [t] [] [p] := by
  refine ⟨oneEnum, enumTy, .constructor (.bool true) [], trivial, ?_, ?_⟩
  · -- The constructor branch consumes the head and leaves an empty vector
    -- against an empty matrix.
    rw [isUseful_constructor]
    simp only [Constructor.arity, Constructor.fieldTypes, List.replicate,
      List.append_nil, List.take_nil, _root_.Buri.specialize, List.flatMap_nil, List.tail_cons]
    rw [isUseful_nil]
    rfl
  · -- No value of an enum type is a `bool`.
    rintro ⟨values, htyped, hmatches, -⟩
    cases htyped with
    | cons hv _ =>
      have hen : oneEnum.isEnum 0 = true := rfl
      obtain ⟨i, ws, rfl, -, -⟩ := HasType.enum_inversion hen hv
      rename_i rest _
      have hm : (Pattern.matches (.constructor (.bool true) []) (.variant 0 i ws) &&
          Pattern.matchesAll [] rest) = true := hmatches
      simp [Pattern.matches, Value.constructorOf] at hm

end Buri
