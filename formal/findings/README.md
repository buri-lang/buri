# Stage 0 findings

Results of the six hand-written experiments that precede any Lean work. Each
was predicted by reading `cli/src/` and `cli/src/docs/SPEC.md`; each was then run against a
build of the toolchain at commit `1b0a711`. Reproducers are in `cases/`.

To re-run: drop a case into a scratch repo as a JS binary package and build it.

```text
REPO.buri               empty; its presence is what makes the directory a root
cmd/<case>/BUILD.buri   binary { outputs: [{ platform: JS }] }
cmd/<case>/main.buri    the case
```

| # | Case | Predicted | Observed | Verdict | Status |
|---|---|---|---|---|---|
| 1 | `pure_abort` | pure call not eliminable | aborts, never prints | **spec bug** | **fixed in SPEC** — §10.4 now conditions the elimination clause on terminating without aborting (`cli/src/docs/lang/effects.md`) |
| 2 | — | determinism false across UB | not run | spec bug, by inspection | **fixed in SPEC** — §10.4 now quantifies over *identical* values and excludes undefined behaviour |
| 3 | `self_in_lambda` | false rejection | rejected | **checker bug** | **fixed** — `inference.rs::check_fn` gates the `SelfParam` arm on `is_effect_carrying`, as the `Normal` arm does |
| 4 | `hide_generic` → `launder` → `purity_false` | taint predicate not inductive | **purity theorem falsified** | **design hole** | **resolved by decision** — see "The rule chosen" below; all three cases are now rejected |
| 5 | `principality` | defaulting may precede bound check | accepted; no counterexample constructible today | latent | **pinned** — `builtins.rs::assert_i64_is_trait_maximal` fails the build if any integer type gains a trait `I64` lacks |
| 6 | `nested_or` | (found while formalising) | exhaustive match rejected | **checker bug** | **fixed** — `exhaustiveness.rs`: `specialize`, `default_matrix` and `head_ctors` distribute over an `Or` head |

Regression tests: `cli/tests/conformance/lib/data/test/patterns.buri` ("a nested
or-pattern …", eight tests) and `.../lib/semantics/test/evaluation.buri`
("what a lambda may capture", four tests) for the false rejections;
`cli/tests/reject/{nested_or_*,lambda_captures_*,lambda_launders_a_context,pure_function_performs_io,effect_carrying_type_fails_a_trait_bound}`
for the rejections, each with its diagnostic recorded to the byte.

**What this proved.** Four of the six findings were things reading the spec
and the checker source predicted in advance; the other two only surfaced once
the algorithm was made precise enough to mechanise. Finding 4 was the sharpest
case: the purity theorem was not under-specified, it was false, and the
counterexample — a generic closure-builder instantiated at a context type —
came from asking what an unconstrained type parameter actually permits, a
question inspection alone did not force. Finding 6 surfaced the same way, from
asking whether the exhaustiveness algorithm actually established the
well-formedness invariant the Lean model needed; it did not, on a nested
or-pattern. Both were real soundness holes rather than documentation gaps, and
both are now closed with regression coverage — that is the case for the Lean
investment.

---

## The rule chosen for finding 4

The checker forbids a lambda from capturing a value whose type could be a
context at some instantiation, with two soundness-preserving escapes — an
ordinary trait bound, and a function type — argued in full in
`cli/src/docs/lang/effects.md` §10.6. The implementation is
`inference.rs::satisfies_seen` (the trait-bound escape) and
`Infer::note_capture_risk` (the capture check itself); all three reproducers
(`hide_generic`, `launder`, `purity_false`) are rejected as
`lambda-captures-generic` (`cli/src/docs/errors/lambda-captures-generic.md`).
