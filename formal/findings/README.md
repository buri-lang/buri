# Stage 0 findings

Six hand-written experiments that come before any Lean work. Reading `cli/src/`
and `cli/src/docs/SPEC.md` predicted each one; a build of the toolchain at
commit `1b0a711` then answered it. Reproducers live in `cases/`.

To re-run a case, drop it into a scratch repo as a JS binary package and build it.

```text
REPO.buri               empty; its presence is what makes the directory a root
cmd/<case>/BUILD.buri   binary { outputs: [{ platform: JS }] }
cmd/<case>/main.buri    the case
```

| # | Case | Predicted | Observed | Verdict | Status |
|---|---|---|---|---|---|
| 1 | `pure_abort` | pure call not eliminable | aborts, never prints | **spec bug** | **fixed in SPEC** — §10.4 now conditions the elimination clause on terminating without aborting (`cli/src/docs/language/effects.md`) |
| 2 | — | determinism false across UB | not run | spec bug, by inspection | **fixed in SPEC** — §10.4 now quantifies over *identical* values and excludes undefined behaviour |
| 3 | `self_in_lambda` | false rejection | rejected | **checker bug** | **fixed** — `inference.rs::check_fn` gates the `SelfParam` arm on `is_effect_carrying`, as the `Normal` arm does |
| 4 | `hide_generic` → `launder` → `purity_false` | taint predicate not inductive | **purity theorem falsified** | **design hole** | **resolved by decision** — see "The rule chosen" below; all three cases are now rejected |
| 5 | `principality` | defaulting may precede bound check | accepted; no counterexample constructible today | latent | **pinned** — `builtins.rs::assert_i64_is_trait_maximal` fails the build if any integer type gains a trait `I64` lacks |
| 6 | `nested_or` | (found while formalising) | exhaustive match rejected | **checker bug** | **fixed** — `exhaustiveness.rs`: `specialize`, `default_matrix` and `head_ctors` distribute over an `Or` head |

Regression tests: `cli/tests/conformance/lib/data/test/patterns.buri` ("a
nested or-pattern …", eight tests) and `.../lib/semantics/test/evaluation.buri`
("what a lambda may capture", four tests) cover the false rejections;
`cli/tests/reject/{nested_or_*,lambda_captures_*,lambda_launders_a_context,pure_function_performs_io,effect_carrying_type_fails_a_trait_bound}`
cover the rejections, each with its diagnostic recorded to the byte.

**What this proved.** Reading the spec and the checker source predicted four of
the six findings. The other two showed up only once the algorithm was precise
enough to mechanise, and both were real soundness holes rather than
documentation gaps: finding 4, where the purity theorem was not
under-specified but false, and finding 6, where the exhaustiveness algorithm
did not establish the invariant the Lean model needed.

---

## The rule chosen for finding 4

The checker forbids a lambda from capturing a value whose type could be a
context at some instantiation. Two escapes preserve soundness — an ordinary
trait bound, and a function type — argued in `cli/src/docs/language/effects.md`
§10.6. `inference.rs::satisfies_seen` implements the trait-bound escape and
`Infer::note_capture_risk` the capture check itself. All three reproducers
(`hide_generic`, `launder`, `purity_false`) are rejected as
`lambda-captures-generic` (`cli/src/docs/errors/lambda-captures-generic.md`).
