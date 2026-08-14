# Untested areas of the build-system spec

What `build-system/*.md` specifies that no test currently pins. Compiled by
reading the seven spec documents against `cli/tests/` (`conformance.rs`,
`corpus.rs`, `example_repo.rs`, `stdlib.rs`), the `reject/` and `crash/`
corpora, and the `#[cfg(test)]` modules in `cli/src/`.

**Legend.** Every item is untested. The tag says why:

- *(untested)* — implemented, and nothing verifies it.
- *(unimplemented)* — no code behind it yet, so the test would be spec-first.

**The structural gap behind most of this.** The `reject/` corpus builds each
case as a single-package binary with no dependencies
(`conformance.rs:183-190`), so no build-graph diagnostic can be expressed in
it. The example monorepo (`conformance.rs:725`) exercises the happy path and
asserts it lints clean. Between them there is no fixture where a graph rule
*fires* — which is most of what the build system is for. A `tests/graph/`
corpus of small multi-package repositories, each annotated with the diagnostic
it must produce, would close the majority of the list below.

---

## BUILD-FILES.md

- [ ] `undeclared-source` — a `.buri` file no rule lists. *(untested;
      implemented at `tools.rs:248`)*
- [ ] `duplicate-source` — a file listed by two rules. *(untested;
      `tools.rs:224`)*
- [ ] `missing-dep` — use of a library absent from `dependencies`.
      *(untested; `tools.rs:330`)*
- [ ] `missing-dep` **by method resolution** — the case BUILD-FILES.md:376-386
      calls out specifically: no import names `//lib/money`, a method call
      resolves into it, the dep is still required. *(untested)*
- [ ] `unused-dep` — a `dependencies` entry no source uses. *(untested;
      `tools.rs:342`)*
- [ ] `dep-cycle` between packages, printing the cycle in declaration order.
      *(untested; `tools.rs:415`)*
- [ ] `visibility-violation` — the diagnostic at BUILD-FILES.md:346-355,
      including that it names the rule that must change. *(untested;
      `build::check_visibility`)*
- [ ] The two edges that **skip** the visibility check: a test suite reaching
      its target under test, and a binary reaching its co-located library.
      *(untested — the example never has a private library in either position)*
- [ ] `//visibility:private` as the default for a rule that omits
      `visibility`. Every library in the example declares one.
      *(untested end-to-end; `workspace.rs::visibility_forms` covers parsing)*
- [ ] Package with both rules — the negative half of BUILD-FILES.md:299-308:
      overlapping `sources` sets, `main.buri` importing `//tools/report/render`
      instead of `//tools/report`, `lib.buri` importing `//tools/report/main`.
      *(untested; the positive path is `tools/report`)*
- [ ] `lib.buri` missing from a `library`, or listed in `sources`. *(untested)*
- [ ] `testing/lib.buri` required when the block is present; the block required
      when the directory exists; empty `testing {}` accepted. *(untested)*
- [ ] `testing.dependencies` — a `testing/` block with deps of its own, which
      do not become the library's. *(untested; the example's `testing` block
      declares only `sources`)*
- [ ] `artifact_name` on an output. *(untested; native artifacts are
      unimplemented — see below)*

## LIBRARIES.md

- [ ] Importing a name from `//pkg` that `lib.buri` does not re-export.
      *(untested — `reject/reexport_name_not_exported.buri` is the other rule:
      re-exporting a name the source module never exported)*
- [ ] `//pkg/inner` imported from outside `//pkg`, with the diagnostic at
      LIBRARIES.md:80-89. *(untested)*
- [ ] **Methods are filtered by the library surface** — LIBRARIES.md:240-260.
      `a.toCents()` resolves inside `//lib/money` and must not resolve from
      `//cmd/server`, even though `Cents` is on the surface. A headline claim
      of the design, and nothing exercises either direction. *(untested)*
- [ ] `//lib/money/lib` rejected as a path — one spelling per module.
      *(untested)*
- [ ] `//pkg/main` imported from outside that binary's own test sources.
      *(untested)*
- [ ] A module path that is also a package path (`lib/money/cents.buri`
      alongside a `lib/money/cents/` package), rejected by name. *(untested)*
- [ ] `//lib/x/testing` imported from a production source.
      *(untested — `reject/testing_import_in_program.buri` covers only
      `core/testing/assert`, not a repository `testing/` surface)*
- [ ] `testing/` code never linked into a production artifact. *(untested)*

## TAGS.md

- [ ] `tag-violation` — two tags that forbid each other in one closure, with
      the reached-by path and the introducing edge printed (TAGS.md:191-203).
      The example's `cmd/web/BUILD.buri` describes this failure in a comment;
      no test provokes it. *(untested; `workspace::forbidden_pair` exists)*
- [ ] `forbids` is **symmetric** — declaring it on `server` catches a `client`
      target reaching server code and vice versa. *(untested)*
- [ ] It is a **union, not a path** — client-only and server-only code pulled
      in down two sibling branches that never reach each other. *(untested)*
- [ ] `unknown-tag`, with the "did you mean" suggestion.
      *(untested; `buildfile.rs::duplicate_tags_are_rejected` covers a
      different rule)*
- [ ] `platform-violation` via a tag's `requires` — the JS build of a closure
      containing `server` code (TAGS.md:214-223). Only the positive
      `query platforms(//lib/store)` assertion exists. *(untested)*
- [ ] The unsatisfiable target error, raised **at the target** before any
      binary reaches it (TAGS.md:232-241). *(untested)*
- [ ] A library's own `platforms:` field, and the intersection-down-the-closure
      rule. No library in the example declares one. *(untested)*
- [ ] Maturity policy — `experimental` and `stable` are declared in the
      example's `REPO.buri` and carried by no target, so that whole path is
      unexercised. *(untested)*
- [ ] A test suite inheriting its target's tags and platform restrictions.
      *(untested)*
- [ ] `test { platforms: [...] }` — one run per platform, and a platform the
      target does not admit being an error rather than a skip. *(unimplemented
      — `testrun.rs:165` pins every suite to `Platform::Js`)*

## TESTING.md

- [ ] A test source importing a library-internal module (TESTING.md:115-123).
      The spec calls this "the whole design in one message"; nothing checks it.
      *(untested)*
- [ ] A test source importing another test source. *(untested)*
- [ ] A test source that `export`s, or that something imports. *(untested)*
- [ ] `buri test --accept` — updates only files declared in `test.data`, never
      creates one, prints a diff, leaves the rest of the run unchanged.
      *(flag parses at `cli.rs:89`; behaviour untested)*
- [ ] `--filter=<substring>` on test names. *(untested)*
- [ ] `--shuffle` on by default with the seed printed, and `--shuffle=off`.
      *(untested)*
- [ ] `timeout_seconds` on a suite. *(untested; no example declares one)*
- [ ] Test caching — a suite whose inputs are unchanged reports as cached, and
      `--force` re-runs it. Every test in `conformance.rs` passes `--force` and
      none asserts a cached count. *(untested)*
- [ ] The failure format at TESTING.md:366-372 — target, file, test name,
      actual/expected, source location. The canary asserts only that the output
      contains `FAIL`. *(untested)*

Well covered already, for contrast: the runner-context table (TESTING.md:262-272)
— `captureOut`, `captureErr`, `stdin`, `files`, `readOnly`, `noNet`, `clockAt`,
`advance`, `randSeed`, `envOf` are all exercised by
`cli/tests/conformance/lib/semantics/test/effects.buri`.

## CLI.md

- [ ] **`buri gen` has no test at all.** Every clause of CLI.md:144-219 is
      unverified: rewriting the six managed fields; preserving `tags`,
      `platforms`, `timeout_seconds`, `visibility`, `outputs`, `test.data`,
      `test.platforms` and comments; leaving the file as `buri format` would;
      refusing to create a build file or invent a rule; `gen --check`; the
      four-step rule assignment in a both-rules package, including the error
      when a file is reachable from neither rule or from both. *(untested;
      implemented in `gen.rs` + `tools.rs:443`)*
- [ ] **`buri run` has no test.** Building for the host and executing outside
      the sandbox, `--` argument passthrough. *(untested; `main.rs:174`)*
- [ ] `buri query deps(...)`, `rdeps(...)`, `path(...)`, `sources(...)`. Only
      `tags(...)` and `platforms(...)` are asserted. `path` is the one CLI.md
      says earns its place. *(untested)*
- [ ] `buri query --output=proto`. *(untested)*
- [ ] `buri lint --fix`. *(untested)*
- [ ] `buri clean --outputs` dropping `.buri/out` only. *(untested — `clean` is
      called for setup in several tests, never asserted)*
- [ ] `buri version` printing the toolchain version and the `REPO.buri` pin.
      *(untested)*
- [ ] `buri lsp` — completion inside `from "//`, hover on a label, the
      "add to `dependencies`" code action. *(unimplemented; `main.rs:40`)*
- [ ] No-argument invocation operating on the package containing the working
      directory. *(untested)*
- [ ] "All commands are safe to run concurrently; a file lock serializes cache
      writes" (CLI.md:25). *(untested)*
- [ ] The `out/` convenience symlink pointing at the most recent build.
      *(untested)*
- [ ] `--output=linux/x86_64` selecting one of several outputs. *(untested;
      see the native-backend gap below)*
- [ ] **Eleven of the seventeen lint codes do not exist.** Present:
      `undeclared-source`, `duplicate-source`, `missing-dep`, `unused-dep`,
      `dep-cycle`, `platform-violation`. Absent: `boundary-violation`,
      `testonly-in-production`, `visibility-violation`, `tag-violation`,
      `unknown-tag` (the checks run, but emit no code), plus the whole style
      table — `unreachable-export`, `unused-import`, `unsorted-imports`,
      `discarded-result`, `empty-test-suite`, `test-without-assertion`.
      *(partly unimplemented)*

## HERMETICITY-AND-CACHING.md

- [ ] **The sandbox is neither implemented nor tested.** Nothing in `cli/src`
      isolates an action: the empty environment, the read-only input-only
      filesystem, the unavailable network, the fixed `1970-01-01T00:00:00Z`
      timestamps, and actions being unable to observe each other are all
      unenforced. The largest single gap in this list. *(unimplemented)*
- [ ] The incrementality table at HERMETICITY:107-118 — that a comment change
      reruns nothing, a body edit reruns `compile` but not a dependent's
      typecheck, a `lib.buri` signature edit reruns the interface and every
      dependent, adding a tag recompiles nothing, and a test edit propagates
      nowhere. The `interface`/`compile` split is the design's central
      structural claim and no test observes it. *(untested)*
- [ ] Cache-key composition beyond the two unit tests
      (`cache.rs::tags_are_not_in_the_key`, `::the_build_mode_changes_the_key`):
      toolchain version and sha256, platform and arch, rule identity, and
      dependencies entering **as keys rather than contents**. *(untested)*
- [ ] Content-keying, not timestamps: touching a file, or checking a branch out
      and back, rebuilds nothing. `builds_are_reproducible`
      (`conformance.rs:610`) compares output bytes across two directories but
      never asserts a cache hit. *(untested)*
- [ ] `buri build --check-reproducible` — builds twice in separate sandboxes
      and diffs. *(unimplemented; `cli.rs:78-99` rejects the flag)*
- [ ] Toolchain `sha256` mismatch refusing to run, exit 2. Every scratch
      repository in the test suite writes `sha256: "00"` and nothing verifies
      it. *(untested)*

## REPO-CONFIG.md

- [ ] A `REPO.buri` whose `toolchain.version` or `sha256` does not match the
      running toolchain. *(untested)*
- [ ] The closed platform enum rejecting an unknown `Platform` or `Arch` name.
      *(untested)*

Adequately covered: a missing `REPO.buri` (`conformance.rs:829`), an
unparseable build file exiting 2 (`conformance.rs:824`), an unknown field with
a suggestion and a duplicate `tag` name (`buildfile.rs` unit tests).

---

## Cross-cutting

- [ ] **Only the JS backend exists** (`build.rs:43-47`). The example declares
      `LINUX`/`MACOS` outputs and `conformance.rs:725` runs only `lint` and
      `test` against it, never `build`. So the `link` action, the
      `.buri/out/<platform>/<package>/<artifact>` layout, `artifact_name`, and
      per-output platform checking are unexercised end-to-end.
      *(unimplemented)*
- [ ] **`main.buri` is the only module that may import `core/host`**, and its
      context is checked against each output's platform — a `main` binding
      `Fs: host.fs` under `platform: JS` must be an unresolved name at the
      entry point (BUILD-FILES.md:236-239). *(untested)*
- [ ] `cli/tests/corpus.rs:111` refers to `tests/format_builds.rs`, which does
      not exist. Either the file was dropped or the comment is stale; the
      property it names — that formatting preserves meaning across the whole
      corpus — is untested either way.
