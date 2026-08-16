# Stage 0 findings

Results of the five hand-written experiments that precede any Lean work. Each
was predicted by reading `cli/src/` and `SPEC.md`; each was then run against a
build of the toolchain at commit `1b0a711`. Reproducers are in `cases/`.

To re-run: drop a case into a scratch repo as a JS binary package and build it.

```
REPO.buri          toolchain { version: "0.3.0" sha256: "00" }
cmd/<case>/BUILD.buri   binary { outputs: [{ platform: JS }] }
cmd/<case>/main.buri    the case
```

| # | Case | Predicted | Observed | Verdict | Status |
|---|---|---|---|---|---|
| 1 | `pure_abort` | pure call not eliminable | aborts, never prints | **spec bug** | **fixed in SPEC** — §10.4 now conditions the elimination clause on terminating without aborting (`cli/src/docs/lang/effects.md`) |
| 2 | — | determinism false across UB | not run (see below) | spec bug, by inspection | **fixed in SPEC** — §10.4 now quantifies over *identical* values and excludes undefined behaviour |
| 3 | `self_in_lambda` | false rejection | rejected | **checker bug** | **fixed** — `inference.rs::check_fn` gates the `SelfParam` arm on `is_effect_carrying`, as the `Normal` arm does |
| 4 | `hide_generic` → `launder` → `purity_false` | taint predicate not inductive | **purity theorem falsified** | **design hole** | **resolved by decision** — see "The rule chosen" below; all three cases are now rejected |
| 5 | `principality` | defaulting may precede bound check | accepted; no counterexample constructible today | latent, see below | **pinned** — `builtins.rs::assert_i64_is_trait_maximal` fails the build if any integer type gains a trait `I64` lacks |
| 6 | `nested_or` | (found while formalising) | exhaustive match rejected | **checker bug** | **fixed** — `exhaustiveness.rs`: `specialize`, `default_matrix` and `head_ctors` distribute over an `Or` head |

Regression tests: `cli/tests/conformance/lib/data/test/patterns.buri` ("a nested
or-pattern …", eight tests) and `.../lib/semantics/test/evaluation.buri`
("what a lambda may capture", four tests) for the false rejections;
`cli/tests/reject/{nested_or_*,lambda_captures_*,lambda_launders_a_context,pure_function_performs_io,effect_carrying_type_fails_a_trait_bound}`
for the rejections, each with its diagnostic recorded to the byte.

---

## The rule chosen for finding 4

Of the four candidates below, the second — *forbid a lambda from capturing a
type-parameter-typed value* — is what the checker now implements, sharpened in
two places so that it costs the corpus almost nothing:

> **A lambda may not capture a value whose type could be a context at some
> instantiation.** The predicate is `is_effect_carrying` with the type-parameter
> case inverted: `Ty::Param(i)` counts as effect-carrying unless it carries an
> **ordinary trait bound**. Function types are exempt outright.

The two sharpenings, and why each is sound:

- **A parameter with an ordinary trait bound escapes.** A type is either part of
  the world or part of your data (SPEC §10.1), so a `T: Eq` can never be
  instantiated at a context type — *provided* that holds for composites too.
  It did not: `struct Holder<C>` with a hand-written `impl<C> Eq for Holder<C>`
  put `Holder<Ctx>` through a `T: Eq` bound. So `satisfies` now answers `false`
  whenever the trait is not an effect and the type is effect-carrying
  (`inference.rs::satisfies_seen`), which is checked at each call site — where
  bounds are already checked, so §13.5 and §13.6 are untouched. Without this the
  escape would have been exactly the hole finding 4 describes, one level up.
- **Function types escape outright.** `is_effect_carrying` taints `fn() => C`
  because of what the *type says*; the capture rule asks what the *value holds*,
  and a closure holds exactly what it captured — which is what this rule checks.
  The argument is inductive rather than structural, and it is what keeps
  `compose` legal.

Two further holes turned up while implementing it, both closed:

- **A lambda's own parameters were never registered as capturable.** So
  `paths.mapCtx(ctx, fn(c, p) => { let g = fn() => fs.exists(c, p); g() })`
  compiled and did I/O from a `fn() => Bool`. This is the *monomorphic* route
  the capture rule was supposed to have closed already. Every binding form now
  funnels through `Infer::note_capture_risk`.
- **Locals bound inside a lambda body counted as captures.** `fn(best, x) =>
  match (best) { .Some(b) => ... }` reported `b` as captured. Harmless while the
  rule only looked at effects; a mass of false rejections once it looked at type
  parameters. Captures are now the referenced locals bound *before* the lambda.

**Measured cost.** The whole corpus — 29 standard-library modules, the
conformance repository (1188 assertions), the worked monorepo, and every
documentation example — needed changes in **one file**: three lambdas in
`core/map`, all three flagged because `Map<K, V>`'s `V` is unbounded and
`[[(K, V)]]` would carry an effect at `V := C`. `insert` and `remove` now splice
with `take`/`push`/`concat` through a new `replaceAt` helper instead of
rebuilding by index (also the cheaper spine copy), and `grow` folds the entries
into their new buckets instead of filtering all entries once per bucket, which
makes every value the inner lambda needs a *parameter*. No conformance program
and no other standard-library module changed.

The other three candidates were rejected as the finding predicts: tainting `fn`
types by their parameters kills the `fn(C, A) => B` combinator shape §10.6
mandates and takes the standard library with it; checking taint at instantiation
needs the body re-checked per call site (§13.5) or an effect summary in the
signature (§13.6); and an explicit "may hold a capability" bound is a new
language feature — a marker trait, spec section, and a bound on every generic
container — for a corpus that turned out to need three lines changed.

---

## 6. A nested or-pattern breaks exhaustiveness -- `cases/nested_or.buri`

Found by mechanising the algorithm, not by reading it: the Lean model needed a
well-formedness invariant, and the question "does `expand` actually establish
this?" turned out to have the answer *no*.

```buri
fn describe(x: Option<Bool>): Int {
  match (x) {
    .Some(true | false) => 1,
    .None => 0,
  }
}
```

```
error: this `match` does not cover `.Some(false)` [match-not-exhaustive]
```

`.Some(false)` plainly matches the first arm. Writing the same match with the
alternation at the top -- `.Some(true) | .Some(false)` -- compiles.

**Root cause.** `expand` (`exhaustiveness.rs`) splits only the alternations it
finds at the *top* of a column:

```rust
let Some(pos) = row.iter().position(|p| matches!(p, Pat::Or(_))) else {
    return vec![row];
};
```

A nested alternation is not at the top of a column, so the row passes through
untouched. `specialize` then peels the constructor off and exposes the `Or` as
the new head -- and both `specialize` and `default_matrix` silently drop
or-headed rows, because neither has a case for one. The row's coverage
disappears, and the wildcard is judged useful.

**Severity.** A false *rejection*, so it costs expressiveness rather than
safety -- the same shape as finding 3. It cannot let a bad program through.

**Fix.** Either make `expand` recurse into sub-patterns, or give `specialize`
and `default_matrix` an `Or` case that distributes over the alternatives. The
second is more local and matches how `useful` already treats an `Or` in the
pattern *vector*.

**Status: fixed** (`cli/src/compiler/semantics/exhaustiveness.rs`), by the
second route — with one addition the finding missed. `head_ctors` had to look
through an `Or` head too: distributing in `specialize` alone leaves the column
looking incomplete, so `useful` takes the `default_matrix` branch, and the
alternatives never get specialized against. All three now distribute.

**A related correction.** An earlier draft of `formal/README.md` claimed the
`Pat::Or` arm of `Ctx::useful` was dead code, on the grounds that `expand` runs
first. That was wrong, and this is the counterexample: the arm is reachable
exactly when a nested alternation is exposed by `specialize`. The Lean
development now models alternations throughout rather than assuming them away.

---

## 4. The purity theorem is false — `cases/purity_false.buri`

This is the headline result and it is worse than predicted. `SPEC.md:1580`:

> "If a function has no `ctx` parameter, no effect-carrying `self`, captures no
> effect-carrying value, and constructs no context, then any two evaluations on
> equal arguments produce equal results, perform no observable effect, and may
> be freely cached, reordered, or eliminated."

```buri
fn wrap<T>(x: T, f: fn(T) => ()): fn() => () {
  fn() => f(x)
}

fn runTwice(f: fn() => ()): Int {
  let _ = f();
  let _ = f();
  7
}
```

`runTwice` has no `ctx` parameter, no effect-carrying `self`, captures nothing,
and constructs no context. It compiles, and it **prints twice**:

```
effect from a pure function
effect from a pure function
runTwice returned 7
```

### Why the checker permits it

Every step is individually sanctioned; the composition is the hole.

1. In `wrap`'s body the lambda `fn() => f(x)` captures `x : T` and
   `f : fn(T) => ()`. The capture rule (`infer_expr.rs:2120-2149`) consults
   `is_effect_carrying` against **`wrap`'s own generics**, where `T` is
   unbounded. `Ty::Param(i)` with no effect bound is untainted
   (`types.rs:561-563`), and `fn(T) => ()` is untainted because **only the
   result counts** (`types.rs:575`). So the lambda is legal.
2. `wrap`'s return type `fn() => ()` is untainted for the same reason, so the
   result flows into any signature at all.
3. At the call site `T := Ctx(κ)`. `check_ctx_rule` (`check.rs:716`) already ran,
   on the pre-instantiation signature. Nothing re-checks.
4. `fn(c) => c.println(...)` is the sanctioned `*Ctx` shape from SPEC §10.6 —
   the capability arrives as a *parameter*, so the capture rule is silent.

This is exactly the scenario SPEC §10.6 names as the reason the capture rule
exists: *"a value of type `fn(Str) => Str` could smuggle a file handle past a
signature with no `ctx` parameter, and the purity theorem would be false."* The
capture rule closes the monomorphic route and leaves the generic one open.

`cases/hide_generic.buri` is the minimal version (`fn hide<T>(x: T): fn() => T`)
and is less severe, because `fn() => Ctx(κ)` *is* tainted at the instantiation,
so a reader of `main` can still see it. `cases/launder.buri` is the first case
where the resulting type mentions nothing effectful.

### What this means for the plan

The syntactic taint predicate is not an inductive invariant, and the gap is not
patchable by tightening the predicate alone — the offending lambda is well-typed
under `wrap`'s own generics, and no information about the instantiation is
available where the rule runs. The candidate fixes each cost something the spec
currently promises:

- **Check taint at instantiation.** Makes the check instantiation-sensitive,
  which collides with §13.5 ("monomorphization is a codegen concern, not a
  checking one") and §13.6 ("no effect inference").
- **Forbid a lambda from capturing any type-parameter-typed value** when the
  lambda escapes. Cheap and local, but bans legitimate generic closure-building.
- **Taint `fn` types by their parameters as well as their result.** Kills the
  `*Ctx` combinator shape §10.6 mandates, since `fn(C, A) => B` becomes tainted.
- **Require an explicit "may hold a capability" bound** on a type parameter
  before a value of that type may be captured. Most expressive, most work.

This wants a decision before Stage 5 (purity) is worth starting, and it makes
the Stage 0 Alloy model (plan item 5) considerably more valuable than budgeted —
it would have found this in an afternoon, and it is the right tool for checking
whether a proposed fix closes the whole family rather than this one instance.

**Status: resolved by decision.** The second candidate, sharpened by an
ordinary-trait-bound escape and a function-type exemption — see "The rule chosen
for finding 4" at the top of this file for the rule, the soundness argument for
each escape, the two further holes it turned up, and the measured cost. The
diagnostic is `lambda-captures-generic`
(`cli/src/docs/errors/lambda-captures-generic.md`).

---

## 1. Pure calls are not eliminable — `cases/pure_abort.buri`

```buri
fn boom(x: Int): Int { 100 / x }
```

`boom` is pure by every criterion §10.4 names. The program runs `let _ = boom(0)`
and then prints; it aborts with `division by zero` and never prints. So the "may
be freely ... eliminated" clause is false: eliminating the call turns an aborting
program into a printing one, and §6.10 makes an abort observable (stderr message,
non-zero exit). Divergence has the same shape.

**Fix:** weaken the clause to a refinement conditioned on
termination-without-abort, as `purity_replaceable`'s `hterm` hypothesis in the
plan does.

**Status: fixed in SPEC.** §10.4 (`cli/src/docs/lang/effects.md`, assembled into
`SPEC.md`) now reads "…that terminate without aborting… and a call that
terminates without aborting may be freely cached, reordered, or eliminated",
with a paragraph naming this case. The program still compiles and still aborts,
which is now what the specification says it does.

## 2. Determinism is false across undefined behaviour

Not run — it follows by inspection. Integer overflow is UB (SPEC §6.2), and
`Prim::is_bigint()` returns `false` for every numeric type with
`EXACT_INTEGER_LIMIT = 2^53 - 1` (`types.rs:125-133`), so `I64`/`I128`
arithmetic is undefined above 2^53 even without overflowing the nominal type.
"Any two evaluations produce equal results" needs "in the absence of undefined
behaviour."

A third correction in the same sentence: *"equal arguments"* has no referent at
function types, because `satisfies` returns `false` for `Ty::Fn(..)` — there is
no `Eq` on closures. The statement must quantify over **identical** values.

**Status: fixed in SPEC.** Both corrections are in §10.4, each with a bullet
saying which sentence it rescues.

## 3. The capture rule fires on a pure `self` — `cases/self_in_lambda.buri`

```buri
struct Report { n: Int }
impl Report {
  export fn above<C: Alloc>(self: Report, ctx: C, xs: [Int]): [Int] {
    xs.filter(ctx, fn(x) => x > self.n)
  }
}
```

```
error: a lambda may not capture `self`, which carries an effect
```

`Report` implements no effect. `infer.rs:58-60` inserts every `self` into
`effect_locals` unconditionally:

```rust
if p.role == ParamRole::Ctx || p.role == ParamRole::SelfParam {
    inf.effect_locals.insert(local);
}
```

`check_ctx_rule` (`check.rs:757`) correctly gates on `is_effect_carrying` for
normal parameters; this path does not. SPEC §10.6 and §14 rule 8 both scope the
rule to **effect-carrying** values.

**Fix:** gate the `SelfParam` arm on
`tables.is_effect_carrying(&p.ty, &info.generics)`, as the `Normal` arm does.

**Why it survived:** across all 43 `.buri` files in `cli/src/compiler/standard_library/sources` and
`cli/tests/conformance`, `self` never appears inside a lambda body — only as a
receiver outside one (`list.buri:64,98,136,142`). A regression test belongs in
the conformance corpus, not `reject/`.

Unlike findings 1 and 4 this is a false *rejection*, so it costs expressiveness
rather than safety. It is also the cheapest to fix.

**Status: fixed** (`cli/src/compiler/semantics/inference.rs::check_fn`). `ctx`
is still marked unconditionally — the `ctx` rule admits nothing else there —
and every other parameter, `self` included, now goes through
`Infer::note_capture_risk`, which gates on the type. Conformance: "a lambda may
capture a `self` that carries no effect" and "a lambda that captures `self` may
outlive the call" in `lib/semantics/test/evaluation.buri`. Reject:
`cli/tests/reject/lambda_captures_effect_self`, where the receiver really does
carry one.

## 5. Principality holds today, but by accident — `cases/principality.buri`

`satisfies` returns `true` for `Ty::Var(_)` (`infer.rs`), so obligations are
discharged only after `default_numerics` has committed a literal to `I64`/`F64`.
The algorithm therefore commits before checking a bound, and a bound that `I64`
fails but another integer type satisfies would be a principality counterexample.

No such bound is constructible today. `builtins.rs:165-190` gives `I64`
`Eq, Ord, Show, Hash, Add, Sub, Mul, Div, Rem, Neg, Bounded, Checked, Wrapping,
Saturating` — a superset of what any unsigned type gets, since unsigned types
lack `Neg`. And §14 rule 22 keeps a user from implementing a trait for a
primitive outside its defining module, so the set cannot be extended.

So principality rests on `I64` being **trait-maximal among integer types** —
which nothing states, nothing tests, and any future `core/num` trait could
break. Worth an assertion in `builtins.rs` rather than a proof.

**Status: pinned.** `Checker::assert_i64_is_trait_maximal`
(`cli/src/compiler/semantics/builtins.rs`) runs at the end of
`register_primitive_methods` — so on every compilation, which is every
conformance, reject, documentation and standard-library test. It reads the real
`impls` table rather than a list, so it cannot drift from what was registered,
and it fails with the reason: defaulting commits a literal to `I64` before any
bound on it is checked, so a trait `U64` has and `I64` lacks would be a
principality counterexample.
