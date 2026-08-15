# Mechanising Buri's type system

A Lean 4 formalisation of Buri's type system, aimed at four theorems: type
safety, the purity/capability theorem, exhaustiveness correctness, and
inference soundness. This directory is **Stage 1 and part of Stage 2** of that
plan.

**Nothing here is on the path to building a `buri` binary.** `formal/` is not a
Cargo workspace member and the toolchain never invokes Lake.

## Running it

```sh
nix develop                       # provides cargo, bun, and elan
cd formal
lake build                        # must succeed
lake env lean Audit.lean          # must print no `sorryAx`
grep -rn 'sorry\|admit' Buri/     # must find nothing outside doc comments
```

The toolchain is pinned in `lean-toolchain` (Lean 4.33.0). `elan` reads that
file, which is how Lean projects pin; it does fetch a toolchain tarball on
first use, which is weaker than the hermeticity standard `cli/` holds itself
to. That is a deliberate trade, and it is acceptable only because this
directory is a developer artifact rather than part of the build.

**No Mathlib, and no dependencies at all** -- the same policy, for the same
reason, as `cli/Cargo.toml`'s empty `[dependencies]`. Everything used is core
Lean: `List`, `Nat`, `omega`. Mathlib would buy `Finset`, but modelling a
context's bindings as a *set* would silently paper over the duplicate-binding
rule (SPEC §14 rule 33), and `List` + `Nodup` is what the Rust
`Vec<(TraitId, Ty)>` actually is.

## What is proved

Everything below is machine-checked and depends on nothing but Lean's three
standard axioms (`propext`, `Quot.sound`, `Classical.choice`). `Audit.lean`
checks that, and is the formal analogue of `conformance.rs:57`'s canary: a
proof development that cannot be caught cheating is not evidence.

| Result | Where | What it says |
|---|---|---|
| `Ty.ind'`, `Value.ind'`, `Pat.ind'` | `Syntax/`, `Dynamics/`, `Patterns/` | Usable induction for the nested inductives |
| `Sig.isEnum_isStruct` &co. | `Sig/Sig.lean` | A type constructor is a struct, an enum, or a primitive -- never two |
| `Ctor.fieldTys_length` | `Patterns/Matrix.lean` | Arity and field types always agree in length |
| `HasType.*_inv` | `Dynamics/Value.lean` | Canonical forms, one per type former |
| **`usefulDec` terminates** | `Patterns/{Measure,Usefulness}.lean` | The usefulness algorithm is a total function -- no fuel, no partiality |
| `Mat.nodes_specialize_lt` | `Patterns/Measure.lean` | The load-bearing termination step (see below) |
| **`covers_specialize`** | `Patterns/Correct.lean` | `specialize` computes the coverage it should |
| **`covers_defaultMat`** | `Patterns/Correct.lean` | `defaultMat` computes the coverage it should |

### The termination argument

`Ctx::useful` (`exhaust.rs:293`) is not structurally recursive: the
wildcard-with-complete-constructor-set branch replaces one `_` by `arity` fresh
`_`s, so the pattern vector *grows*. The measure is lexicographic --
`(matrix nodes + vector nodes, vector length)`, counting constructor nodes only
-- and the interesting case is that one: the vector's node count is unchanged,
so the matrix must shrink instead. It does, because completeness forces some
row to be headed by the very constructor being specialised on, and `specialize`
peels that head off.

That is `Mat.nodes_specialize_lt`, and the shape of it is worth noticing: **the
algorithm terminates precisely because it only expands a wildcard when the
matrix has already paid for the expansion.**

### Two side conditions that are not bookkeeping

`covers_specialize` carries hypotheses that record real facts about the Rust
code rather than artefacts of the encoding:

* **`subs.length ≤ arity`.** `specializeRow` pads *and truncates* to the
  constructor's arity (`exhaust.rs:246-250`). Padding is harmless; truncation
  is not. A pattern carrying *more* sub-patterns than the constructor has
  fields would be rejected by `matchPad` but accepted after truncation -- the
  matrix would gain coverage it never had, and a non-exhaustive match could be
  accepted. `exhaust.rs:114` never builds one, because it sizes `subs` by the
  largest field *index*. The hypothesis is what records that this is
  load-bearing.

* **rest-freeness.** `specializeRow` drops any row headed by `arrayRest`, since
  `arrayRest n` never equals `array k`. A surviving rest pattern would lose the
  coverage it provided. `expand_lengths` removes them before the algorithm
  runs, which is why this is a hypothesis rather than a case.

A related structural observation, from reading `exhaust.rs:445 check`: because
`expand` and `expand_lengths` both run *before* `useful`, **the `Pat::Or` arm
of `Ctx::useful` (`exhaust.rs:294-306`) is dead code**, and `Ctor::ArrayRest`
never reaches the algorithm at all.

## What is not proved

Stated precisely, so the gap is not mistaken for a detail. See
`Patterns/Spec.lean` for the Lean-level statements.

1. **`useful_complete`** (`Useful → usefulDec = true`) and its corollary
   `exhaustive_correct`. This is the *safety* direction -- it is what makes a
   `match` the checker accepted always have an arm that fires. The two
   `covers_*` lemmas are its inductive steps; what remains is the induction
   (via the generated `usefulDec.induct`) plus showing the `MatrixWF` invariant
   is preserved at each step.
2. **`useful_sound`** (the converse, "no false positives"). Additionally needs
   *inhabitation*: exhibiting a witness at a missing constructor means building
   values at its field types, which is where SPEC §14 rule 14 (recursive types
   must be productive) becomes load-bearing rather than stylistic.
3. **The array case.** `allCtors` reports an array type's constructors as
   `array 0 .. array limit` and nothing longer, so the algorithm reasons in a
   universe where arrays are bounded. That is sound only because of an
   invariant the algorithm never states:

   > `limit` is one more than the longest array length any pattern mentions
   > (`exhaust.rs:450`), so fixed-length patterns only ever produce heads
   > `array k` with `k < limit`. Therefore `array limit ∈ headCtors P` can only
   > have come from an expanded *rest* pattern -- which covers every length at
   > or above its minimum. Whenever the algorithm believes the array
   > constructors are complete, some rest pattern really does cover the tail.

   The plan flagged array-rest as the likeliest place for a real bug. Reading
   the algorithm did not turn one up: the `+ 1` at `exhaust.rs:450` is doing
   exactly the work it needs to. But "did not turn one up" is not a proof, and
   this is the obligation that would settle it.

## Modelling decisions

Three constructors of Rust's `Ty` (`types.rs:188`) are deliberately absent:

* **`Ty::Error` is excluded.** `unify` returns `Ok` for `(Error, _)`
  (`types.rs:693`), `satisfies` returns `true` (`infer.rs:585`), `implements`
  returns `true` (`types.rs:588`) -- a declarative system holding such a type
  derives *everything* at it. It is an error-recovery artefact whose contract
  ("a diagnostic was already reported here") is not a typing property. What
  licenses the exclusion is not an argument but a Rust-side invariant that
  needs writing: *if the checker reports no diagnostics, no `Ty::Error`
  survives in any body.* Until that test exists, this is an assumption.
* **`Ty::SelfTy` is eliminated at elaboration**, matching what
  `types::substitute(&ty, &args, self_ty)` already does. Same caveat, same
  missing test.
* **`Ty::Var` is algorithmic** and belongs to the inference stage, not the
  declarative system.

Primitives are *not* a separate constructor, because in Rust a primitive is a
`TyCon` reached as `Ty::Con(id, [])`. Keeping that shape is what lets
`allCtors` match Rust's `all_ctors` case for case.

Buri has **no type-variable binders** -- generics are instantiated positionally
against `Ty.param n` -- so type substitution is a plain fold with no
capture-avoidance obligation and not one lifting lemma. That is a gift from the
language design, and it removes what is usually the most expensive part of a
mechanisation.

## Blind spots

What this exercise will not catch, whatever else gets proved:

* **Everything from source bytes to HIR.** Lexing, parsing, name resolution,
  visibility, opacity, method resolution, `derive` expansion, `Self`
  substitution, `?`/`??` desugaring, and the role-based context-construction
  rules. Going through the reject corpus, **roughly 50 of the 83 cases test
  rules that live entirely in that gap**; only about 15 are core-typing rules a
  model like this one can adjudicate. That ratio is the most important honest
  number here.
* **`mono.rs`.** A type error reintroduced during monomorphisation is invisible
  to a pre-monomorphisation proof.
* **`codegen.rs` / `js.rs`.** The largest unproved surface, with a known gap:
  `Prim::is_bigint()` is `false` for every type and `EXACT_INTEGER_LIMIT` is
  `2^53 - 1`, so `I64`/`I128` arithmetic is inexact above `2^53`. A proof about
  `I128` arithmetic describes a language the JS backend does not implement.
* **Intrinsics.** `builtins.rs` is axioms in any model of this kind.
* **Diagnostics.** These theorems say an error occurs, never *which* one. The
  reject corpus's exact-output goldens are the right tool, and already exist.

## Findings

`findings/` holds the Stage 0 results -- five hand-written Buri programs,
written to check predictions made by reading the checker. Three confirmed real
defects, including a **falsification of the purity theorem** of SPEC §10.4.
Those did not need Lean, and they are the reason Stage 0 is worth doing before
Stage 5.
