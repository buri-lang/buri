# Why the AI rewrote the standard library

Working notes on the discoverability half of `.notes/stdlib-ideas.md` §13, from
the Claude Code transcripts that produced the monorepo.

**Where the evidence is.** The 177 `.buri` files the survey read are the
_uncommitted_ working tree of `monorepo-buri`, written 2026-09-03 by session
`bb48c073-6679-41fb-a9ca-26fcaa5cd07a` and its sixteen subagents
(`~/.claude/projects/-Users-nick-Documents-GitHub-monorepo-buri/bb48c073.../subagents/*.jsonl`;
line numbers below are JSONL line numbers). The August port (`f1af95af`,
`4de7a589`) was deleted by `f33f98d` and was not Claude's — it was `codex exec`
gpt-5.6 workers supervised by Claude (`f1af95af` L183, L260, L299, L374…). Its
wave logs are gone; the friction reports relayed into the transcript survive and
are cited where they add something.

**The one-line answer.** The AI mostly _did_ consult the docs — 211 `buri docs`
invocations across 15 of 16 subagents. It consulted the modules its brief named,
and almost nothing else. Discovery was not autonomous; it was a reading list.

---

## 1. Findings

### The controlling correlation

The orchestrator's per-agent prompt named a handful of `buri docs` pages. Use of
the library tracked that list almost exactly.

| brief named                        | agent                                                | outcome                                                                                      |
| ---------------------------------- | ---------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `buri docs core/order`             | `a19e4b00` (model), `a481bedb` (query)               | `order.int` used correctly (`a481bedb` L70–73 reads the page, L105 writes `order.int(a, b)`) |
| — (no `core/order`)                | `a480ac37` (storage), `a224933f` (storage_simulator) | `compareInts` / `compareBools` hand-written; `Order.then` open-coded                         |
| `buri docs core/bytes` **(toHex)** | `aad9a35a` (session)                                 | `bytes.toHex` used — `session/identity.buri:17,39`, the only two calls in the repo           |
| — (no `core/bytes`)                | `a224933f`, `a2eea79e` (simulators)                  | `HEX_DIGITS` + `nibble` written twice                                                        |

Supporting counts: 24 distinct `core/*` modules were ever queried, out of 43 the
toolchain ships. `core/order` by 2 agents of 16. The bare `buri docs` index —
which lists every page — was run **once**, by one agent (`a31625fc` L118), in
211 calls. `buri docs search` was used **5 times** in 211.

The instruction was not missing. `DESIGN.md` (read in full by every agent, e.g.
`a480ac37` L6) says: _"use `buri docs <topic>` liberally … `buri docs search
<words>`. Docs are generated from the real stdlib source and are
authoritative."_ It then lists nine modules — and those nine are what got read.

### Sampled incidents

1. **Hex, twice (never asked).** `a224933f` L137/L141 writes `prng.buri` and
   `hex.buri` with a sixteen-`Char` `HEX_DIGITS` table and a `nibble` helper. It
   never once mentions `toHex`. It _was_ in doc-reading mode at that moment — L135
   is `buri docs language/lexical | grep hexadecimal`, looking up hex _literals_.
   It asked about syntax and never asked whether the library renders hex.
   `buri docs search hex` returns `core/bytes.toHex`, `core/bytes.fromHex`,
   `core/crypto.toHex` as the first three hits. Failure mode: **never asked.**

2. **Hex again, by copy (`a2eea79e` L111–112, L142).** The second simulator
   agent `cat`s the first simulator's `hex.buri`, reads it, and pastes it into
   `libs/database/database_simulator/prng.buri`. It treated a sibling package as
   the reference implementation. Failure mode: **copied a sibling** — invisible to
   any single-agent discovery fix.

3. **`padStart` seen and ignored.** `str.padStart`/`padEnd` were printed into
   **seven** agents' context by their own `buri docs core/str` calls
   (`a19e4b00` L73, `a224933f` L103, `a2eea79e` L109, `a31625fc` L110,
   `a480ac37` L91, `a481bedb` L83, `aad9a35a` L128). `a224933f` then wrote
   literal-`\t` column alignment into `violation.buri` at L154, ~50 turns later.
   Zero uses of `padStart`/`padEnd` in 177 files. Failure mode: **looked, saw it,
   did not connect it** — a signature in a wall of signatures, hundreds of turns
   before the code that wanted it.

4. **`.ignore()` deliberately routed around.** The strongest incident.
   `a224933f` L125 runs `grep -rn "ignore()" --include=*.buri .` — which fails as
   a zsh glob error, reading like "no results" — and then `buri docs lint
discarded-result`, which explains that `.ignore()` is _precisely what the lint
   reports_. The agent, held to a "lint clean" gate, then wrote
   `match (io.println(...)) { .Ok(_) => (), .Err(_) => () }` — six times, in four
   files (`database_simulator/cli.buri:94,101`, `storage_simulator/cli.buri:96,103`,
   `storage_simulator/runner.buri:270`). `.ignore()` appears **zero** times in the
   repo; the only occurrence in the tree is a comment in the `buri init` sample.
   Failure mode: **found it and routed around it** — the lint inverted its own
   incentive.

5. **A compiler bug forced hand-written comparators, and they stayed
   hand-written.** `a480ac37` L161–164 reproduces `derive Ord` over an array
   aborting on the native backend, files `bugs/derive-ord-array-native-backend.md`
   at L167, and writes `storage/valuekey.buri` at L169 with `compareInts`,
   `compareBools`, `compareFloats`. Note `compareStrs` _does_ delegate
   (`left.compare(right)`) — it reached for a method on the receiver and found
   one; for `Int` and `Bool` the equivalent lives in another module as a free
   function `order.int`, and that address was never guessed. Failure mode:
   **workaround written without a second lookup**, compounded by _one operation,
   two addresses_ (`a.compare(b)` vs `order.int(a, b)`).

6. **The wrong page, one page away.** `ac80e423` L237 tries `bytes.show(ctx)` in
   an app literally named `apps/frame_hex`. L238: `` `[U8]` has no method `show`
[no-such-method] `` — whose fix line says _"buri docs <module> lists the
   methods it has"_. At L240 it opens `buri docs core/list.join`, and at L242
   hand-rolls a decimal `mapCtx(...).join(ctx, ",")`. `bytes.toHex` was the answer,
   in `core/bytes`. Failure mode: **looked, in the wrong module**, with the
   diagnostic pointing at method lookup rather than at the task.

7. **`Order.then` and `Bool.toInt`, present and unused.** `.then(` : 0 uses;
   open-coded as `match (cmp) { .Equal => next, … }` at
   `storage_simulator/model.buri:174,192`. `Bool.toInt`: `if (b) { 1 } else { 0 }`
   at `storage/codec.buri:283,341`, `database_simulator/steps.buri:106`,
   `storage_simulator/exit.buri:190`. No agent ever ran `buri docs core/bool`
   before writing these. Failure mode: **never asked**.

8. **A diagnostic that hallucinated an absence for us** (August/codex, but the
   diagnostic is unchanged). Calling a member that does not exist through a valid
   namespace reports the _namespace_ missing: `fs.appendBytes(...)` →
   ``there is nothing named `fs` in scope [unresolved-name]``. The worker filed it
   itself (`f1af95af` L480, "Missing module member produces an unresolved namespace
   diagnostic"), with the workaround _"check `buri docs core/fs` manually to
   distinguish a missing member from a missing namespace"_ — and, before working
   that out, built a whole hex-text WAL format around the apparent absence.
   Failure mode: **import friction reading as absence**.

9. **Genuine absences, for the record.** Not every rewrite was avoidable at
   0.3.0: a pure seeded RNG (the splitmix64s), a sorted map, binary/append/rename
   `fs`, path joining, `Show` in a `${}` hole — §2/§4/§8/§9, already moving.

### Tally of the sampled incidents

| failure mode                                    | incidents                                                     | sites in the tree       |
| ----------------------------------------------- | ------------------------------------------------------------- | ----------------------- |
| Never asked — search would have answered        | 4 (hex, `Bool.toInt`, `Order.then`, `order.int`/`order.bool`) | ~13                     |
| Looked, saw it, did not use it                  | 1 (`padStart`/`padEnd`)                                       | 7 agents, ~6 call sites |
| Looked in the wrong module                      | 1 (`[U8].show` → decimal join)                                | 1                       |
| Found it, routed around it (lint incentive)     | 1 (`Result.ignore`)                                           | 6                       |
| Workaround for a compiler bug, no second lookup | 1 (`derive Ord` → comparators)                                | 5                       |
| Copied a sibling package                        | 3 (hex, `valueLabel`, clock fixtures)                         | ~12                     |
| Diagnostic read as absence                      | 1 (`fs.appendBytes`)                                          | whole WAL format        |
| Genuinely absent at 0.3.0                       | 5                                                             | many                    |

**"Did not look" is not the diagnosis — "did not look _for this_" is.** The docs
answered whatever was asked. Nothing prompted the question.

---

## 2. Recommendations, ranked

1. **Ship the reading list, because the reading list is what worked.** The only
   variable that predicted stdlib use was whether the brief named the module. An
   agent will read one index page if told to; it will not enumerate 43 modules on
   a hunch. Put a _stdlib map_ — every `core/*` module, one line each, plus the
   twenty names most likely to be rewritten (`order.int`, `Order.then`,
   `Bool.toInt`, `str.padStart`, `bytes.toHex`, `char.fromDigit`, `num.toHex`,
   `Result.ignore`, `list.sortBy`, `str.toRadix`, `Saturating`, `context Fixture`,
   `.show(ctx)`) — at the top of `buri-types` or a new `buri-stdlib` SKILL in
   `cli/src/docs/reference/skills/`, and make "run `buri docs` (bare) once" the
   first line of it. _Evidence:_ 2/2 agents told to read `core/order` used it;
   0/2 not told did; the bare index was run once in 211 calls.

Nick's decision: we should _not_ make a skill with all standard library functions. Rather, we should update the skills to encourage the AI to explore the standard library and make it easier to read the docs.

2. **Lints that name the library — the only check that fires where the mistake
   is.** `hex-digit-table` (landed this session) would have caught incidents 1 and
   2 outright. Worth adding, in falling confidence: `hand-rolled-comparator` (a
   function returning `Order` whose body is an `if`/`else` chain on `<`/`>` over a
   primitive → `order.int`/`order.bool`/`order.float`); `discarded-result-by-hand`
   (a `match` on a `Result` all of whose arms are `()` → `.ignore()`);
   `order-then-open-coded` (a `match` on an `Order` whose only non-identity arm is
   `.Equal` → `Order.then`); `bool-to-int` (`if (b) { 1 } else { 0 }` in `Int`
   position → `Bool.toInt`). FP risk is real but bounded: the comparator and
   `Order.then` rules key on exact shapes with a known return type and should be
   near-zero-FP; `bool-to-int` must **not** fire inside a `[U8]` literal, where
   `1`/`0` is a wire byte (`storage/codec.buri:283` is legitimate). _Evidence:_
   6 hand-written discards, 2 hex tables, 4 bool-to-ints, 5 comparators — every one
   of them present in a tree that passed `buri lint //...` clean.

Nick's decision (do this): we should have lints to encourage the standard library _so long as there's no false positives_. If there is an opportunity for a false positive, do NOT add the lint.

3. **Fix the `discarded-result` incentive; it is an own-goal.** The rule makes
   `.ignore()` the reported form. An agent whose gate is "lint clean" therefore
   writes the four-line `match` instead, which is _not_ reported — so the rule
   produced exactly the scattered, ungreppable drops it exists to prevent, and
   `.ignore()` has zero uses in 177 files. Either report both forms (see rec 2),
   or move `.ignore()` to a severity a quality gate does not chase, or say on the
   lint page in as many words that the finding is the report working and the
   `match` form is worse. _Evidence:_ `a224933f` L125–126, then six matches.

Nick's decision (do this): in the `discarded-result` text we should mention that doing the four-line match is an anti-pattern and the real solution is to handle the error.

4. **Generated stdlib docs — do it, but not for this.** Honest evaluation: at
   0.3.0 `buri docs search` **already answers** `pad` → `core/str.padStart`,
   `ignore` → `core/result.ignore`, `hex` → `core/bytes.toHex`, `toInt` →
   `core/bool.toInt`. Searchability was not the binding constraint; asking was. The
   generated-docs work is right for correctness and for §1's doc-comment audit, and
   it will not on this evidence change rewrite behaviour. The _one_ search change
   that would have helped: search is **name-shaped, not intent-shaped** —
   `buri docs search "compare ints"` returns `core/proto.packedVarints` and
   `core/str.compare` but never `core/order.int`; `buri docs search fixture`
   never surfaces `context Fixture`. Index doc-comment bodies and module `//!`
   prose, and carry a small concept-alias table (compare/sort → `core/order`;
   pad/align → `core/str`; hex/base16 → `core/bytes`, `core/char`).

Nick's decision (do this): update the search function so it can also be intent-based, where multiple pages could be returned along with the exact CLI command to run to get that page.

5. **`buri init` should write an AGENTS.md.** It writes `.agent/skills/*` and no
   agent instructions at all; this monorepo's root AGENTS.md is one line —
   "Always follow yagni." — and `DESIGN.md` had to restate the docs advice for
   every agent by hand. The file below is the content. _Evidence:_
   `cli/src/commands/init.rs` SCAFFOLD list; monorepo `AGENTS.md`.

Nick's decision (do this): do not write an AGENTS.md file, we should rely on the skills.

6. **"Did you mean `core/list.maxBy`" from the LSP — not feasible for this
   population.** All sixteen agents wrote `.buri` with `cat > file <<'EOF'` from
   Bash. Not one ran an editor; the language server was never in the loop, and
   hover cannot reach a heredoc. The same idea _is_ feasible where agents do
   look: a lint (rec 2), and the diagnostic — `no-such-method` already prints
   ``did you mean `mapErr`?`` (`f1af95af` L682) and names `buri docs <module>`.
   Extending it across modules (``[U8]` has no `show` — `bytes.toHex(ctx, b)`
renders one``) would have caught incident 6.

Nick's decision (do this): turn these into lints if there'll be no false positives.

7. **Make a missing member say so.** `fs.appendBytes` reporting _"there is
   nothing named `fs` in scope"_ tells an agent the module is absent when a member
   is, and at least one hand-rolled subsystem came out of that. Cheap fix, direct
   evidence.

Nick's decision (do this): do that above recommendation

8. **Cross-package duplication is the repo's problem, not the library's — but it
   needs an owner.** Two sibling packages got the same `hex.buri` because two
   concurrent agents each owned one package and neither could create a shared one.
   No discovery affordance reaches this. What does: a brief that names a shared
   `//libs/database/testing` (and `//libs/database/util`) package up front, or a
   `buri lint` duplicate-body check across a repository.

Nick's decision (do this): do not worry about this issue

**Already in flight this session:** `hex-digit-table` and `time-unit-conversion`
(`2fb6ada1`, `5e8e6fee`) — rec 2's first two rules, landed; the Entropy/CSPRNG
surface and pure seeded `Gen` (`8ba38bc8`), which retires the splitmix64s;
scalar-value `Str.compare` (`57a3e7b9`); `??` → `withDefault` (`dad6d06b`);
native `host.fs`/`host.env` (`ab7f6a53`); and the decision to generate the stdlib
docs from `.buri` source (rec 4).
