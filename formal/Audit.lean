import Buri

/-!
# Axiom audit

Run with `lake env lean formal/Audit.lean`. Every line must print exactly
`[propext, Classical.choice, Quot.sound]` or a subset of it. If `sorryAx`
appears anywhere, something in this development is admitted rather than proved
and every claim resting on it is void.

This is the formal analogue of `conformance.rs:57 conformance_suite_can_fail`:
a proof development that cannot be caught cheating is not evidence.
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
#print axioms Buri.Matrix.nodeCount_specialize_lt
#print axioms Buri.Matrix.nodeCount_specialize_le
#print axioms Buri.Matrix.nodeCount_defaultMatrix_le
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

-- The two matrix operations compute the coverage they should
#print axioms Buri.covers_specialize
#print axioms Buri.covers_defaultMatrix
#print axioms Buri.Pattern.matchesAll_pad
