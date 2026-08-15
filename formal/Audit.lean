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
#print axioms Buri.Ctor.fieldTys_length
#print axioms Buri.Sig.variantFieldTys_length
#print axioms Buri.Sig.structFieldTys_length

-- Canonical forms
#print axioms Buri.HasType.con_enum_inv
#print axioms Buri.HasType.con_struct_inv
#print axioms Buri.HasType.con_bool_inv
#print axioms Buri.HasType.con_lit_inv
#print axioms Buri.HasType.tuple_inv
#print axioms Buri.HasType.unit_inv
#print axioms Buri.HasType.array_inv

-- Termination of the usefulness algorithm
#print axioms Buri.Mat.nodes_specialize_lt
#print axioms Buri.Mat.nodes_specialize_le
#print axioms Buri.Mat.nodes_defaultMat_le
#print axioms Buri.usefulDec

-- The two matrix operations compute the coverage they should
#print axioms Buri.covers_specialize
#print axioms Buri.covers_defaultMat
#print axioms Buri.Pat.matchPad_pad
