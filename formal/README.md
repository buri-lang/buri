# Mechanising Buri's type system

A Lean 4 formalisation of Buri's type system, aimed at four theorems: type
safety, the purity/capability theorem, exhaustiveness correctness, and
inference soundness.

**Exhaustiveness is done, and so is type safety for the core.** The two
headline results are

* `exhaustive_correct_unbounded` — if the checker accepts a `match`, then for
  every well-typed value of the scrutinee's type, no restriction, some arm the
  programmer wrote fires; and
* `checked_never_stuck` — a closed core term the algorithmic checker accepts
  never reaches a state that is neither a value nor able to step.

The second consumes the first: `match` progress is exactly the place a type
safety proof needs "some arm fires", and that is what the exhaustiveness
theorem was for.

**Purity and inference remain unstarted.** `findings/README.md` §4, the hole
that made the purity theorem false, has since been closed by a language
decision, so Stage 5 is now worth starting; it is not started here.

**Nothing here is on the path to building a `buri` binary.** `formal/` is not a
Cargo workspace member and the toolchain never invokes Lake. The one place the
two meet is `cli/tests/lean_vectors.rs`, which reads checked-in vectors and
needs no Lean.

## Running it

```sh
nix develop                       # provides cargo, bun, and elan
cd formal
lake build                        # must succeed
lake env lean Audit.lean          # must print no `sorryAx`
grep -rn 'sorry\|admit' Buri/     # must find nothing outside doc comments

lake env lean --run Vectors.lean  # regenerates vectors/exhaustiveness.txt
cd ../cli && cargo test --test lean_vectors
```

The toolchain is pinned in `lean-toolchain` (Lean 4.33.0). `elan` reads that
file, which is how Lean projects pin; it does fetch a toolchain tarball on
first use, which is weaker than the hermeticity standard `cli/` holds itself
to. That is a deliberate trade, and it is acceptable only because this
directory is a developer artefact rather than part of the build.

**No Mathlib, and no dependencies at all** — the same policy, for the same
reason, as `cli/Cargo.toml`'s empty `[dependencies]`. Everything used is core
Lean: `List`, `Nat`, `omega`. Mathlib would buy `Finset`, but modelling a
context's bindings as a *set* would silently paper over the duplicate-binding
rule (SPEC §14 rule 33), and `List` + `Nodup` is what the Rust
`Vec<(TraitId, Ty)>` actually is.

## What is proved

Everything below is machine-checked and depends on nothing but Lean's three
standard axioms (`propext`, `Quot.sound`, `Classical.choice`). `Audit.lean`
checks that for 57 results, and is the formal analogue of `conformance.rs`'s
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

```lean
theorem checked_never_stuck (hP : Program.WellFormed S P)
    (h : Expr.check S P [] e t = true) (hsteps : Steps S P e e') :
    Expr.IsValue e' ∨ ∃ e'', Step S P e' e''
```

The exhaustiveness hypotheses are exactly what the compiler establishes before
calling the algorithm: `lower` sizes an array constructor by its sub-pattern
count, `limit` is `max(length_limit) + 1`, and the compiled arms are rest-free
and respect constructor arities. `Expr.check` establishes all four itself, in
`armsOk`.

### Exhaustiveness

| Result | Where | What it says |
|---|---|---|
| **`isUseful` terminates** | `Measure.lean`, `Usefulness.lean` | The algorithm is a total function — no fuel, no partiality |
| `Matrix.weight_specialize_lt` | `Measure.lean` | The load-bearing termination step |
| `specializeRow_matches`, `defaultRow_matches` | `Correct.lean` | Each matrix operation decides coverage, in both directions |
| `Matrix.WellFormed.specialize`, `.defaultMatrix` | `WellFormed.lean` | The invariant survives both steps |
| **`isUseful_false_covers`** | `Complete.lean` | Completeness: a `false` answer means the matrix really covers everything |
| `exhaustive_correct`, `unreachable_correct`, `irrefutable_correct` | `Exhaustive.lean` | The three claims the compiler makes |
| `expandLengths_sound`, `topDisjuncts_sound` | `Expand.lean` | The two pre-passes only narrow what a pattern matches |
| **`Pattern.matches_truncate`** | `Truncate.lean` | No arm can tell a long array from its truncation |
| `useful_not_sound_at_wellFormed` | `Soundness.lean` | Why the converse direction is *still* open — see below |

### Type safety, and the checker

| Result | Where | What it says |
|---|---|---|
| `NodeKind.fieldTys_ctor` | `Core/Typing.lean` | Constructing a value and destructuring it agree on field types |
| `Typing.erase` | `Core/Typing.lean` | A closed value's erasure is a well-typed `Value` — the bridge into `Patterns/` |
| `Pattern.bind_isSome` | `Core/Bind.lean` | `Pattern.bind` and `Pattern.matches` are the same question |
| `Pattern.bind_typed` | `Core/Typing.lean` | What an arm binds is well typed at `Pattern.binderTypes` |
| `Typing.weaken`, `Typing.subst` | `Core/Subst.lean` | The two structural lemmas, with no lifting lemma anywhere |
| **`progress`**, **`preservation`** | `Core/Safety.lean` | Type safety for the core |
| `Ty.beq_iff` | `Core/Decide.lean` | Decidable type equality, written out — `Ty` is a nested inductive |
| `armsOk_exhaustive`, `armsOk_irrefutable` | `Core/Sound.lean` | The checker's `match`/`let` premises, discharged from the algorithm |
| **`infer_sound`**, `check_sound` | `Core/Sound.lean` | If the algorithm accepts, the declarative judgment holds |

### Five arguments worth reading

**Termination.** `Ctx::useful` is not structurally recursive, for two reasons:
the wildcard-with-complete-constructor-set branch replaces one `_` by `arity`
fresh `_`s, so the pattern vector *grows*; and `specialize` and
`default_matrix` distribute over an or-headed row, so the matrix *gains rows*.
The measure is lexicographic — `(matrix weight + vector weight, vector length)`
— where a row's weight is the **product** of its columns' weights and

    weight(_) = 1     weight(c(p₁..pₙ)) = 1 + ∏ weight(pᵢ)     weight(p₁|..|pₙ) = 1 + Σ weight(pᵢ)

A product, not a sum, because distributing an alternation *copies the rest of
the row*, and only a measure that multiplies through the tail sees that
`Σ weight(aᵢ) < weight(a₁|..|aₙ)` dominates the duplication. The interesting
branch is still the complete one: the vector's weight is unchanged, so the
matrix must shrink instead. It does, because completeness forces some row to be
headed by something other than a wildcard. **The algorithm terminates precisely
because it only expands a wildcard when the matrix has already paid for the
expansion.**

**Arrays.** `allConstructors` reports an array type's constructors as
`array 0 .. array limit` and nothing longer, so the algorithm reasons in a
universe where arrays are bounded. That is sound because `limit` is one more
than the longest length any arm mentions, so no arm can distinguish an array of
length `limit` from a longer one — `Pattern.matches_truncate`. The `+ 1` is
doing real work: without it, a match on `[]`, `[_]`, `[_, _]` would have
`limit = 2`, all three constructors would be present, the set would look
complete, and length-3 arrays would slip through.

**Two side conditions that are not bookkeeping.** `specializeRow_matches`
carries `subpatterns.length ≤ arity` and rest-freeness, both clauses of
`Pattern.WellFormed`. `specializeRow` pads *and truncates*; padding is
harmless, truncation is not. A pattern carrying more sub-patterns than its
constructor has fields would be rejected by `matchesAll` but accepted after
truncation — the matrix would gain coverage it never had. `lower` never builds
one, and the hypothesis records that this is load-bearing rather than
incidental.

**A wildcard is a binder.** The core language reuses `Pattern` verbatim — the
*lowered* form, where `lower` has already erased `PatKind::Bind` to
`Pat::Wild`. So `Pattern.wildcard` is read as binding the value it matches, and
a pattern binds one variable per wildcard, left to right. That is a faithful
over-approximation of Buri, where `x` binds and `_` does not: a body may ignore
a binding. What it buys is that `exhaustive_correct_unbounded` applies to the
core language *directly*, instead of through an erasure between two pattern
types — `Pattern.bind_isSome` is the one lemma joining them, and it is eleven
lines.

**Substitution needs no shifting.** Terms are de Bruijn and what is ever
substituted is a **closed** value, so `Expr.subst` carries a depth and never
lifts. There is not one lifting lemma in this development. Part of that is
hygiene, and part is the language: Buri has no type-variable *binders* either —
generics are instantiated positionally against `Ty.param n` — so `substitute`
is a plain fold with no capture-avoidance obligation. That is the most
expensive part of a typical mechanisation, and Buri's design removes it.

## The bridge to the implementation

A proof about a *model* is worth what the model's fidelity is worth, and
nothing but a test keeps the two from drifting. So the Lean algorithm is
executable, and it is run against the real checker.

**How it works.** `Vectors.lean` fixes a prelude of Buri declarations and,
alongside it, the `Signature` that mirrors them. For each of eight scrutinee
types it holds a pattern pool where every entry carries *both* the Lean
`Pattern` and the Buri surface syntax that lowers to it — that pairing is the
only place the two can drift, and it is a table rather than a translation. It
then enumerates every ordered selection of one, two or three distinct pool
entries, runs `isExhaustive` and the per-arm reachability loop on each, and
writes `(program, verdict)` lines to `vectors/exhaustiveness.txt`.

`cli/tests/lean_vectors.rs` reads that file, assembles the vectors into modules
of 64 functions, compiles each module through `driver::analyze_snippet` — the
same entry point the documentation harness uses — and attributes every
`match-not-exhaustive` and `unreachable-arm` diagnostic back to its vector and
arm by byte range. Any *other* diagnostic fails the test, because it means the
pattern pool and the surface syntax have drifted.

**Coverage.** 907 vectors over `Bool`, `Int`, a three-variant enum, an
`Option`-shaped enum, a two-field variant, a struct, a pair, and `[Bool]`;
including nested alternations, array rest patterns, and or-patterns at the top
of a column. 658 exhaustive, 249 not. It runs in about two seconds, and the
vectors are checked in, so the Rust suite never needs Lean.

**It can fail.** `the_bridge_can_detect_a_disagreement` drives two `match`
statements whose answers are not in doubt through the same observation path and
asserts it sees them, so an agreement result is not vacuous.
`the_corpus_covers_the_nested_alternation` asserts the corpus still contains
the shape `findings/README.md` §6 was about, so a future edit to the pool
cannot quietly drop it.

**What it does not do.** It compares verdicts, not diagnostic text — the reject
corpus's exact-output goldens are the right tool for that, and they exist. It
does not exercise the core type checker: `Expr.infer` has no surface syntax to
compile against, because the model's `Expr` is the *post-inference* form.
Bridging that would mean either an elaborator from Buri source to `Expr`, or a
serialisation of `typed::Expr` the Rust side does not have.

## What is not proved

**`useful_sound`**, the converse direction ("no false positives"): that the
compiler reports a non-exhaustive match only when a value really is uncovered.

`findings/README.md` §6 used to be the answer — the theorem was false, because
a nested alternation made the checker reject an exhaustive match. That is fixed
in `exhaustiveness.rs` and the model here is of the fixed algorithm, so that
obstacle is gone. Two remain, and the first is sharper than the old one:

* **`Pattern.WellFormed` is too weak.** It never says the head constructor
  *belongs to* the type — it cannot, because for a nullary constructor the
  check is vacuous. `useful_not_sound_at_wellFormed` (`Soundness.lean`) is a
  machine-checked counterexample: `true`, as a pattern, is well formed at an
  enum type, and the algorithm calls it useful against an empty matrix, where
  no value of that type matches it. `useful_sound` needs a pattern *typing*
  judgment, which is what `semantics/patterns.rs` computes and what the
  progress theorem never needed. (`isUseful_false_covers` is true for ill-typed
  patterns, because a pattern that matches nothing only ever *loses* coverage.)
* **Inhabitation.** Even with pattern typing, the incomplete-constructor-set
  branch needs a *witness*: a value built at a constructor the matrix does not
  mention. That is where SPEC §14 rule 14 (recursive types must be productive)
  stops being stylistic. At a non-enumerable type it additionally needs a
  literal outside a finite set.

**The type-substitution lemma on typing derivations.** `Program.WellFormed`
states that a declared function's body checks *at every instantiation* of its
generics. The natural statement would quantify over the generic signature and
derive the instantiated one; that derivation is an assumption here, not a
theorem. The obstacle is the semantic premises: exhaustiveness at `t` does not
obviously give exhaustiveness at `substitute targs t`, since the two range over
different sets of values.

**Checker completeness.** Only `infer_sound` is proved. The checker is
deliberately conservative in one place: `Pattern.uniformBindersB` demands that
or-pattern alternatives bind *nothing*, where the declarative rule only demands
that they bind the *same* things. `.Some(true | false)` passes; a hypothetical
`.Some(x) | .Other(x)` would not, and Buri would accept it.

**Inference.** `Ty::Var`, `unify`, `default_numerics`, trait obligations. The
model's `Expr` is the post-inference form, so the Rust checker's two
directions collapse into one. Everything `formal/README.md` used to say about
`Ty::Var` being algorithmic is still true and still unstarted.

**Purity.** No Lean is written for it. `findings/README.md` §4 — the reason the
theorem as stated was false — has since been resolved by a language decision,
so it is now worth starting.

**Errors, guards, and intrinsics.** The operational semantics has no aborts
(SPEC §6.10 makes one observable, which is a purity concern, not a safety one),
no guarded arms (a guarded arm covers nothing for exhaustiveness purposes, so a
`match` whose only matching arm is guarded *can* get stuck), and no builtins
(`semantics/builtins.rs` is axioms in any model of this kind).

## Modelling decisions

Three constructors of Rust's `Ty` (`semantics/types.rs`) are deliberately absent:

* **`Ty::Error` is excluded.** `unify` returns `Ok` for `(Error, _)`
  (`semantics/types.rs`), `satisfies` returns `true` (`semantics/inference.rs`),
  `implements` returns `true` (`semantics/types.rs`) — a declarative system
  holding such a type derives *everything* at it. It is an error-recovery
  artefact whose contract ("a diagnostic was already reported here") is not a
  typing property. What licenses the exclusion is not an argument but a
  Rust-side invariant that needs writing: *if the checker reports no
  diagnostics, no `Ty::Error` survives in any body.* Until that test exists,
  this is an assumption.
* **`Ty::SelfTy` is eliminated at elaboration**, matching what
  `types::substitute(&ty, &args, self_ty)` already does. Same caveat, same
  missing test.
* **`Ty::Var` is algorithmic** and belongs to the inference stage, not the
  declarative system.

Primitives are *not* a separate constructor, because in Rust a primitive is a
`TyCon` reached as `Ty::Con(id, [])`. Keeping that shape is what lets
`allConstructors` match `all_ctors` case for case.

Four decisions about the core language:

* **Every data form is one node.** `variant`, `struct`, `tuple`, `unit`,
  `bool`, a literal and an array literal are all `Expr.node k args`. These are
  exactly the forms the pattern algorithm sees as a `Constructor` with fields,
  so keeping them one node makes `Expr.erase` a one-liner, gives `match` a
  single canonical-forms lemma, and collapses seven near-identical congruence
  rules in the operational semantics into one.
* **`match` and `let` carry semantic premises.** The typing rules require
  "every value of the scrutinee's type is matched by some arm", not
  "`isExhaustive` returned true". That is what makes progress immediate, and it
  is what gives `exhaustive_correct_unbounded` a job: it is the bridge from the
  syntactic condition the checker decides to the semantic one the rule states.
* **`Expr` is post-inference.** See "What is not proved".
* **The model tracks the *fixed* algorithm.** `specialize`, `default_matrix`
  and `head_ctors` distribute over an or-headed row, matching
  `exhaustiveness.rs` after the `findings/README.md` §6 fix. One consequence is
  worth noting: or-freeness is now nowhere a hypothesis, and
  `specializeRow_matches` is a biconditional where the old, dropping version
  needed a side condition for the forward direction.

## Blind spots

What this exercise will not catch, whatever else gets proved:

* **Everything from source bytes to HIR.** Lexing, parsing, name resolution,
  visibility, opacity, method resolution, `derive` expansion, `Self`
  substitution, `?`/`??` desugaring, and the role-based context-construction
  rules. Going through the reject corpus, **roughly 50 of the 83 cases test
  rules that live entirely in that gap**; only about 15 are core-typing rules a
  model like this one can adjudicate, and those 15 are what `Core/` is aimed
  at. That ratio is the most important honest number here.
* **`transform/monomorphize.rs`.** A type error reintroduced during
  monomorphisation is invisible to a pre-monomorphisation proof.
* **`backend/generate.rs` / `backend/javascript.rs`.** The largest unproved
  surface, with a known gap: `Prim::is_bigint()` is `false` for every type and
  `EXACT_INTEGER_LIMIT` is `2^53 - 1`, so `I64`/`I128` arithmetic is inexact
  above `2^53`. A proof about `I128` arithmetic describes a language the JS
  backend does not implement.
* **Intrinsics.** `semantics/builtins.rs` is axioms in any model of this kind.
* **Diagnostics.** These theorems say an error occurs, never *which* one. The
  reject corpus's exact-output goldens are the right tool, and already exist.

## Findings

`findings/` holds the Stage 0 results — hand-written Buri programs, written to
check predictions made by reading the checker. Six entries; all six are now
fixed, in the checker or in the spec. One of them, §6, was found by mechanising
the algorithm rather than by reading it: the Lean model needed a
well-formedness invariant, and the question "does `expand` actually establish
this?" turned out to have the answer *no*. That is the argument for doing
Stage 0 before Stage 5, and for mechanising at all.

## Layout

```
Buri/Syntax/      Ty, Constructor
Buri/Signature.lean   nominal declarations, and generic instantiation
Buri/Dynamics/    Value, HasType, canonical forms
Buri/Patterns/    the usefulness algorithm and its correctness
Buri/Core/        the core language: syntax, typing, semantics, safety, checker
Vectors.lean      the test-vector generator
Audit.lean        the axiom audit — 57 results
```
