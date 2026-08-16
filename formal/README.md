# Mechanising Buri's type system

A Lean 4 formalisation of Buri's type system, aimed at four theorems: type
safety, the purity/capability theorem, exhaustiveness correctness, and
inference soundness.

**Exhaustiveness is done.** The headline result is
`exhaustive_correct_unbounded`: if the checker accepts a `match`, then for
every well-typed value of the scrutinee's type -- no restriction -- some arm
the programmer wrote fires. That is what makes `match` progress, and it holds
for the algorithm as written, including the array-length reasoning the plan
flagged as the likeliest place for a bug.

Type safety, purity, and inference remain unstarted.

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
checks that for 33 results, and is the formal analogue of `conformance.rs`'s
canary: a proof development that cannot be caught cheating is not evidence.

### The headline

```lean
theorem exhaustive_correct_unbounded (signature limit t) (arms : List Pattern)
    (hlowered : ∀ p ∈ arms, Pattern.LoweredArrays p)
    (hlen : ∀ p ∈ arms, p.lengthLimit < limit)
    (hwf : ∀ q ∈ compileArms limit arms, Pattern.WellFormed signature t q)
    (hexhaustive : isExhaustive signature limit t (armRows (compileArms limit arms)) = true) :
    ∀ v, (signature ⊢ᵥ v : t) → ∃ p ∈ arms, Pattern.matches p v = true
```

The hypotheses are exactly what the compiler establishes before calling the
algorithm: `lower` sizes an array constructor by its sub-pattern count, `limit`
is `max(length_limit) + 1`, and the compiled arms are rest-free and respect
constructor arities.

### The pieces

| Result | Where | What it says |
|---|---|---|
| **`isUseful` terminates** | `Measure.lean`, `Usefulness.lean` | The algorithm is a total function -- no fuel, no partiality |
| `Matrix.nodeCount_specialize_lt` | `Measure.lean` | The load-bearing termination step |
| `covers_specialize`, `covers_of_defaultMatrix` | `Correct.lean` | The matrix operations compute the coverage they should |
| `Matrix.WellFormed.specialize`, `.defaultMatrix` | `WellFormed.lean` | The invariant survives both steps |
| **`isUseful_false_covers`** | `Complete.lean` | Completeness: a `false` answer means the matrix really covers everything |
| `exhaustive_correct`, `unreachable_correct`, `irrefutable_correct` | `Exhaustive.lean` | The three claims the compiler makes |
| `expandLengths_sound`, `topDisjuncts_sound` | `Expand.lean` | The two pre-passes only narrow what a pattern matches |
| **`Pattern.matches_truncate`** | `Truncate.lean` | No arm can tell a long array from its truncation |
| `Constructor.fieldTypes_length`, `HasType.*_inversion` | `Matrix.lean`, `Value.lean` | Arities agree; canonical forms |

### Three arguments worth reading

**Termination.** `Ctx::useful` is not structurally recursive: the
wildcard-with-complete-constructor-set branch replaces one `_` by `arity` fresh
`_`s, so the pattern vector *grows*. The measure is lexicographic --
`(matrix nodes + vector nodes, vector length)`, counting constructor nodes only
-- and the interesting case is that one: the vector's node count is unchanged,
so the matrix must shrink instead. It does, because completeness forces some
row to be headed by the very constructor being specialised on. **The algorithm
terminates precisely because it only expands a wildcard when the matrix has
already paid for the expansion.**

**Arrays.** `allConstructors` reports an array type's constructors as
`array 0 .. array limit` and nothing longer, so the algorithm reasons in a
universe where arrays are bounded. That is sound because `limit` is one more
than the longest length any arm mentions, so no arm can distinguish an array of
length `limit` from a longer one -- `Pattern.matches_truncate`. The `+ 1` is
doing real work: without it, a match on `[]`, `[_]`, `[_, _]` would have
`limit = 2`, all three constructors would be present, the set would look
complete, and length-3 arrays would slip through.

**Two side conditions that are not bookkeeping.** `covers_specialize` carries
`subpatterns.length ≤ arity` and rest-freeness. `specializeRow` pads *and
truncates*; padding is harmless, truncation is not. A pattern carrying more
sub-patterns than its constructor has fields would be rejected by `matchesAll`
but accepted after truncation -- the matrix would gain coverage it never had.
`lower` never builds one, and the hypothesis records that this is load-bearing
rather than incidental.

## What is not proved

**`useful_sound`**, the converse direction ("no false positives"): that the
compiler reports a non-exhaustive match only when a value really is uncovered.
It needs *inhabitation* -- exhibiting a witness at a missing constructor means
building values at its field types, which is where SPEC §14 rule 14 (recursive
types must be productive) becomes load-bearing rather than stylistic.

This is the expressiveness direction, not the safety one, and it is **false as
the checker stands**: `findings/README.md` §6 records a nested alternation that
makes it reject an exhaustive match. Proving it would first require fixing that.

**The other three theorems** -- type safety, purity, inference -- have no Lean
written for them. Purity in particular should not be started before the
capability hole in `findings/README.md` §4 is resolved, since the theorem as
stated is false.

## Modelling decisions

Three constructors of Rust's `Ty` (`semantics/types.rs`) are deliberately absent:

* **`Ty::Error` is excluded.** `unify` returns `Ok` for `(Error, _)`
  (`semantics/types.rs`), `satisfies` returns `true` (`semantics/inference.rs`), `implements`
  returns `true` (`semantics/types.rs`) -- a declarative system holding such a type
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
`allConstructors` match Rust's `all_ctors` case for case.

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
* **`transform/monomorphize.rs`.** A type error reintroduced during monomorphisation is invisible
  to a pre-monomorphisation proof.
* **`backend/generate.rs` / `backend/javascript.rs`.** The largest unproved surface, with a known gap:
  `Prim::is_bigint()` is `false` for every type and `EXACT_INTEGER_LIMIT` is
  `2^53 - 1`, so `I64`/`I128` arithmetic is inexact above `2^53`. A proof about
  `I128` arithmetic describes a language the JS backend does not implement.
* **Intrinsics.** `semantics/builtins.rs` is axioms in any model of this kind.
* **Diagnostics.** These theorems say an error occurs, never *which* one. The
  reject corpus's exact-output goldens are the right tool, and already exist.

## Findings

`findings/` holds the Stage 0 results -- five hand-written Buri programs,
written to check predictions made by reading the checker. Three confirmed real
defects, including a **falsification of the purity theorem** of SPEC §10.4.
Those did not need Lean, and they are the reason Stage 0 is worth doing before
Stage 5.
