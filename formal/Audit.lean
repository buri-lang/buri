import Buri

/-!
# Axiom audit

Run with `lake env lean formal/Audit.lean`. Every line must print exactly
`[propext, Classical.choice, Quot.sound]` or a subset of it. If `sorryAx`
appears anywhere, something in this development is admitted rather than proved
and every claim resting on it is void.

This is the formal analogue of `language/conformance.rs`'s `conformance_suite_can_fail`:
a proof development that cannot be caught cheating is not evidence. The Rust
side of the same idea is `cli/tests/vectors/lean.rs`, which replays this
development's exhaustiveness verdicts against the real checker.

57 results, in the order the development builds them.
-/

open Buri

-- Signatures and types
#print axioms Buri.Constructor.fieldTypes_length
#print axioms Buri.Signature.variantFieldTys_length
#print axioms Buri.Signature.structFieldTys_length

-- Canonical forms
#print axioms Buri.HasType.enum_inversion
#print axioms Buri.HasType.struct_inversion
#print axioms Buri.HasType.bool_inversion
#print axioms Buri.HasType.literal_inversion
#print axioms Buri.HasType.tuple_inversion
#print axioms Buri.HasType.unit_inversion
#print axioms Buri.HasType.array_inversion

-- Termination of the usefulness algorithm
#print axioms Buri.Matrix.weight_specialize_lt
#print axioms Buri.Matrix.weight_specialize_le
#print axioms Buri.Matrix.weight_defaultMatrix_le
#print axioms Buri.Row.weight_specializeRow_lt
#print axioms Buri.isUseful

-- Well-formedness is preserved by both matrix operations
#print axioms Buri.Matrix.WellFormed.specialize
#print axioms Buri.Matrix.WellFormed.defaultMatrix

-- Decomposing a well-typed value
#print axioms Buri.HasType.fields_typed
#print axioms Buri.HasType.fields_length
#print axioms Buri.HasType.constructorOf_mem

-- Completeness, and the three theorems it yields
#print axioms Buri.isUseful_false_covers
#print axioms Buri.exhaustive_correct
#print axioms Buri.unreachable_correct
#print axioms Buri.irrefutable_correct

-- The two rewrites the compiler applies before the algorithm
#print axioms Buri.Pattern.expandLengths_sound
#print axioms Buri.Pattern.topDisjuncts_sound
#print axioms Buri.compileArms_sound

-- Long arrays have short representatives
#print axioms Buri.Value.truncate_bounded
#print axioms Buri.HasType.truncate
#print axioms Buri.Pattern.matches_truncate

-- The headline theorem
#print axioms Buri.exhaustive_correct_unbounded

-- The two matrix operations compute the coverage they should, in both
-- directions -- which is what distributing over an or-headed row bought.
#print axioms Buri.covers_specialize
#print axioms Buri.covers_defaultMatrix
#print axioms Buri.specializeRow_matches
#print axioms Buri.defaultRow_matches
#print axioms Buri.Pattern.matchesAll_pad
#print axioms Buri.irrefutable_correct_unbounded

-- Why the converse direction is still open
#print axioms Buri.useful_not_sound_at_wellFormed

-- The core language: erasure into the pattern algorithm's world
#print axioms Buri.NodeKind.fieldTys_ctor
#print axioms Buri.Typing.erase
#print axioms Buri.Typing.canonical_fn
#print axioms Buri.Pattern.bind_isSome
#print axioms Buri.Pattern.bind_typed
#print axioms Buri.Pattern.binderTypes_length

-- Structural lemmas
#print axioms Buri.Typing.weaken
#print axioms Buri.Typing.subst

-- Type safety
#print axioms Buri.progress
#print axioms Buri.preservation
#print axioms Buri.type_safety

-- The checker
#print axioms Buri.Ty.beq_iff
#print axioms Buri.Pattern.wellFormedB_sound
#print axioms Buri.Pattern.uniformBindersB_sound
#print axioms Buri.armsOk_exhaustive
#print axioms Buri.armsOk_irrefutable
#print axioms Buri.infer_sound
#print axioms Buri.check_sound
#print axioms Buri.checked_never_stuck
