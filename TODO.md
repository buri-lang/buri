# Untested areas of the build-system spec

What `cli/src/docs/*.md` specifies that no test currently pins. Compiled by
reading the seven spec documents against `cli/tests/` (`conformance.rs`,
`corpus.rs`, `example_repo.rs`, `stdlib.rs`), the `reject/` and `crash/`
corpora, and the `#[cfg(test)]` modules in `cli/src/`.

**Legend.** Every item is untested. The tag says why:

- *(untested)* — implemented, and nothing verifies it.
- *(unimplemented)* — no code behind it yet, so the test would be spec-first.

**The structural gap behind most of this — now closed.** The `reject/` corpus
builds each case as a single-package binary with no dependencies, so no
build-graph diagnostic can be expressed in it. `cli/tests/repos/` is the fixture
shape that was missing: whole repositories, one per rule, each with a
`CASE.textproto` manifest naming what the CLI does in it and the output that
produces. See `cli/tests/README.md` for the format and
`cli/tests/harness/case.rs` for the reader.

**Found while building it** — none of these are test gaps:

- [ ] `missing-dep` by method resolution was **unimplemented**, not untested.
      `reached_by_resolution` (`tools.rs:368`) computed the set and it was only
      ever used to *suppress* `unused-dep`; a library reached solely through a
      method call was never reported. Now implemented (`tools.rs`, after the
      import loop) and covered by `repos/build-files/missing_dep_by_method`.
- [ ] **A cycle is reported once per end.** `repos/build-files/dep_cycle`
      records `//lib/a and //lib/b` and then `//lib/b and //lib/a` for one
      cycle. BUILD-FILES.md:389-390 describes one diagnostic per cycle.
- [ ] **`tag-violation` prints `reached by:` for only one of the two tags** —
      whichever sorts second. `repos/tags/forbids_symmetric` shows `"client" is
      carried by //lib/widget` with no path, while `tag_violation` shows the
      path for `server`. The introducing edge is what TAGS.md:191-203 says
      makes the diagnostic useful, and half of it is missing.
- [ ] **`duplicate-source` renders its `= fix:` line misaligned.** The two
      spans have different gutter widths (line 10 and line 5), and the trailing
      `= ...` block is indented for the first while sitting under the second.
      This is the same class of defect the recorded goldens caught before.
- [ ] **`query platforms` on an unsatisfiable target prints nothing and exits
      0.** `repos/tags/unsatisfiable_target/expected/platforms.txt` is empty,
      which is indistinguishable from a command that did nothing.
- [ ] **`test.dependencies` edges are not visibility-checked.** `dep_edges`
      (`workspace.rs:375-388`) walks only rule-level `declared_deps`
      (`workspace.rs:355-361`), so a test suite reaching a library named in
      `test.dependencies` escapes the check — contradicting BUILD-FILES.md:359-360
      ("including a test suite reaching a library named in `test.dependencies`,
      is checked normally"). Not yet covered by a case; triage first.
- [ ] **`name_not_reexported`'s fix is misleading.** It says "add `export` to
      `rawValue`'s declaration", but the declaration is already exported — what
      is missing is the re-export in `lib.buri`.

---

## BUILD-FILES.md

- [x] `undeclared-source` — `repos/build-files/undeclared_source`.
- [x] `duplicate-source` — `repos/build-files/duplicate_source`, in a package
      with both rules.
- [x] `missing-dep` — `repos/build-files/missing_dep`, including that `build`
      does *not* catch it and `lint` does.
- [x] `missing-dep` **by method resolution** — `missing_dep_by_method`. See the
      note above: this needed implementing first.
- [x] `unused-dep` — `repos/build-files/unused_dep`.
- [x] `dep-cycle` between packages — `repos/build-files/dep_cycle`, from both
      ends.
- [x] `visibility-violation` — `repos/build-files/visibility_violation`,
      including that it names the dependency's build file, and that `build`
      enforces it as well as `lint`. Now carries its lint code.
- [x] The two edges that **skip** the visibility check —
      `repos/build-files/visibility_skips`, with the negative twin in the same
      manifest.
- [x] `//visibility:private` as the default — the same case: its library
      declares no `visibility`, and the violation prints
      `//visibility:private (nothing, outside its own package)`.
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

- [x] Importing a name from `//pkg` that `lib.buri` does not re-export —
      `repos/libraries/name_not_reexported`.
- [x] `//pkg/inner` imported from outside `//pkg` —
      `repos/libraries/inner_module_from_outside`.
- [x] **Methods are filtered by the library surface** — LIBRARIES.md:240-260,
      `repos/libraries/method_surface_filter`. Both directions in one case: the
      call resolves inside the library, does not resolve one package over, and
      resolves again once `lib.buri` re-exports it.
- [x] `//lib/money/lib` rejected as a path —
      `repos/libraries/lib_path_spelling`.
- [ ] `//pkg/main` imported from outside that binary's own test sources.
      *(untested)*
- [ ] A module path that is also a package path (`lib/money/cents.buri`
      alongside a `lib/money/cents/` package), rejected by name. *(untested)*
- [x] `//lib/x/testing` imported from a production source —
      `repos/libraries/testing_import_in_production`, with the dependency
      declared under `dependencies` so that only the language can object.
- [ ] `testing/` code never linked into a production artifact. *(untested)*

## TAGS.md

- [x] `tag-violation` — `repos/tags/tag_violation`, the failure the example's
      `cmd/web/BUILD.buri` describes in a comment. Now carries its lint code.
      The reached-by path is printed for one of the two tags only; see the
      findings above.
- [x] `forbids` is **symmetric** — `repos/tags/forbids_symmetric`. `forbids` is
      declared only on `server`, and a `server` binary reaching `client` code
      still fails.
- [x] It is a **union, not a path** — `repos/tags/tag_union_not_path`. Two
      sibling branches that never reach each other, each clean alone.
- [x] `unknown-tag`, with the "did you mean" suggestion —
      `repos/tags/unknown_tag`, both with a near miss and without. The
      suggestion used to **overwrite** the actionable fix (`with_fix` assigns);
      it now appears alongside it, and both forms are recorded.
- [x] `platform-violation` via a tag's `requires` —
      `repos/tags/platform_violation`, with no `client` tag anywhere so the
      forbids rule cannot be what fires. Now carries its lint code.
- [x] The unsatisfiable target error, raised **at the target** —
      `repos/tags/unsatisfiable_target`. There is deliberately no binary in
      that repository; the absence is the test.
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
- [ ] `buri query deps(...)`, `rdeps(...)`, `path(...)`, `sources(...)`.
      `tags(...)` and `platforms(...)` now have recorded goldens
      (`repos/tags/tags_query`) rather than the `.contains()` assertions they
      had. `path` is the one CLI.md says earns its place. *(untested)*
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
- [ ] **Eight of the seventeen lint codes do not exist.** Present:
      `undeclared-source`, `duplicate-source`, `missing-dep`, `unused-dep`,
      `dep-cycle`, `platform-violation`, and — newly attached, each with a case
      in `repos/` recording it — `visibility-violation`, `tag-violation`,
      `unknown-tag`. Absent: `boundary-violation`, `testonly-in-production`
      (both checks run as compile errors and emit no code), plus the whole
      style table — `unreachable-export`, `unused-import`, `unsorted-imports`,
      `discarded-result`, `empty-test-suite`, `test-without-assertion`.
      *(partly unimplemented)*
- [x] `buri build`/`buri test --explain` — one line per action, its key, and
      whether the cache served it. New; `cli/tests/incrementality.rs` reads it.

## HERMETICITY-AND-CACHING.md

- [ ] **The sandbox is neither implemented nor tested.** Nothing in `cli/src`
      isolates an action: the empty environment, the read-only input-only
      filesystem, the unavailable network, the fixed `1970-01-01T00:00:00Z`
      timestamps, and actions being unable to observe each other are all
      unenforced. The largest single gap in this list. *(unimplemented)*
- [x] Most of the incrementality table at HERMETICITY:107-118, via the new
      `--explain` transcript (`cli/tests/incrementality.rs`): a body edit moves
      its own target's key and the `link` above it and leaves a sibling and a
      dependent's own key alone; adding a tag moves nothing and relinks
      nothing; a test-source edit moves the suite's key and no production key;
      a toolchain-pin change moves every key; `--force` turns every hit into a
      run.
- [ ] The one row still open: **a dependent does not recheck after a body edit
      in its dependency** (HERMETICITY:111). This needs a real `interface`
      action, and the cheap version is *unsound* — `lib.buri` re-exports from
      sibling modules, so a signature change in `parse.buri` changes the
      interface while leaving `lib.buri` byte-identical, and a key that said
      "unchanged" there would serve a stale answer. A sound version needs
      per-target analysis; `driver::analyze` is whole-closure
      (`build.rs:71-76`). `--explain` is in place to test it when it lands.
      *(unimplemented)*
- [x] Content-keying, not timestamps: rewriting a file with the bytes it
      already held rebuilds nothing
      (`incrementality::rewriting_a_file_with_its_own_bytes_rebuilds_nothing`).
- [ ] Cache-key composition beyond the unit tests and the above: platform and
      arch, rule identity, and dependencies entering **as keys rather than
      contents**. *(untested)*
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

Adequately covered: a missing `REPO.buri`
(`conformance::outside_a_repository_is_a_bad_invocation` — it cannot be a
repository case, because the point is that there is no repository), an
unparseable build file exiting 2 (`repos/cli/exit_codes`), an unknown field
with a suggestion and a duplicate `tag` name (`buildfile.rs` unit tests).

The exit-code contract itself is now `repos/cli/exit_codes`, which records the
*message* each of the seven invocations prints — the old assertion checked only
the number. `format --check` reporting without rewriting is
`repos/cli/format_check`, with the file's own bytes recorded before and after.

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
      corpus — is untested either way. (`repos/cli/format_check` now covers it
      for one file: format, then build.)
- [ ] `cli/tests/repos/**` is not walked by `corpus.rs`, so the fixture sources
      there are not held to "every source in the repository parses" or to the
      formatting fixed point. That is deliberate — `repos/cli/format_check`
      checks in a deliberately misformatted file, and future cases will check
      in ones that must not compile — but it means a typo in a fixture is
      caught only by the case that runs it.
