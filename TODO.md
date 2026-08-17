# Untested areas of the build-system spec

What `cli/src/docs/*.md` specifies that no test currently pins. Compiled by
reading the seven spec documents against `cli/tests/` (`conformance.rs`,
`corpus.rs`, `example_repository.rs`, `standard_library.rs`), the `reject/` and `crash/`
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

**Two compiler bugs, found and fixed while writing the collection modules.**
Both had been unreachable because no program in the repository did the thing
that provokes them — which is the argument for a standard library that is
larger than the language's own tests need.

- [x] **A type that recursed through a container hung the compiler.**
      `infer::satisfies` decides whether a derived implementation holds by
      walking the type's components, and for a recursive type that walk reaches
      the type again. The guard was `t.head() == Some(con)` on the immediate
      component — which catches `Node(Tree<T>, ...)` and misses
      `Branch([Rose])`, because an array's head is not a constructor. So
      `a == b` on any type that reached itself through a list, a tuple, or
      another type holding one recursed until the stack ran out. `core/json` is
      exactly that shape, which is how it turned up.

      The guard is now a set of the constructors already on the walk, so
      recursion through *any* container terminates. Pinned by `Rose` in
      `conformance/lib/semantics/{shapes.buri,test/traits.buri}` — deliberately
      non-generic, because a type parameter is undecidable at the declaration
      and takes the other path through the checker.



- [x] **`&`, `|`, `^` and `~` were 32-bit.** They were emitted as the native
      JavaScript operators, which coerce to *32-bit signed* — so on `Int`,
      which is `I64`, everything above bit 31 was silently discarded and the
      result came back negative. `(1 << 40) & (1 << 40)` was `0`;
      `0 | (1 << 31)` was `-2147483648`. Meanwhile `bits.shl` and
      `bits.popCount` were correct 64-bit BigInt, so the two halves of the
      module disagreed with each other.

      Fixed in `compiler/backend/js/generate.rs::prim_op`: above 32 bits the operation goes through
      `$and64`/`$or64`/`$xor64`/`$not64` (and the unsigned forms) in
      `runtime.js`; at 32 bits and below the native operator is exact and
      stays, so ordinary integer code is unchanged. Pinned by
      `conformance/lib/numbers/test/bits.buri`, "the bitwise operators are
      64-bit".

      No golden moved when this landed, which is the part worth noting: not one
      program in the repository used a bitwise operator above bit 31, so
      nothing could have caught it. `core/bitset` is what walked into it.

**Found while building it** — none of these are test gaps:

- [ ] `missing-dep` by method resolution was **unimplemented**, not untested.
      `reached_by_resolution` (`commands/lint.rs`) computed the set and it was only
      ever used to *suppress* `unused-dep`; a library reached solely through a
      method call was never reported. Now implemented (`commands/lint.rs`, after the
      import loop) and covered by `repos/build-files/missing_dep_by_method`.
- [x] **A cycle is reported once per end.** `repos/build-files/dep_cycle`
      recorded `//lib/a and //lib/b` and then `//lib/b and //lib/a` for one
      cycle. BUILD-FILES.md:389-390 describes one diagnostic per cycle.

      Fixed in `check_cycles` (`commands/lint.rs`). A cycle has no first end,
      so neither edge could be preferred on its own terms; what deduplicates is
      the cycle's *membership* — every target mutually reachable with the
      edge's tail — which is the same set whichever edge is walked first. The
      first edge to reach a set not yet reported is the one the diagnostic
      points at. Two goldens lost their second copy, and
      `linting/fix_refuses_a_judgement_call` lost one too, which is how it
      turned out that case was pinning the duplicate as well.
- [x] **`tag-violation` prints `reached by:` for only one of the two tags** —
      whichever sorts second. Both notes are now built by one closure
      (`build/actions.rs`), so the introducing edge is printed for whichever of
      the two the target does not carry itself, and for both when it carries
      neither. `tags/forbids_symmetric` gained the path for `client`;
      `tags/tag_union_not_path` gained one too, and reads as the case's name
      promises now — the two tags arrive down two different paths, and the
      diagnostic shows both.
- [x] **`duplicate-source` renders its `= fix:` line misaligned.** The cause
      was `gutter_width` taking the *widest* line number over all of a
      diagnostic's spans, while `render_snippet` gives each snippet a gutter
      sized to its own. The `= ...` block belongs to the snippet directly above
      it, so it is that one's width it has to match; it now uses the last
      sub-span's, or the primary span's when there is none.
      `diagnostics.rs::the_trailer_lines_up_with_the_snippet_above_it` pins it
      at the unit level. One reject golden moved with it —
      `type_implements_effect_and_trait`, spans on lines 14 and 9 — which is
      the only other case in the repository whose spans straddle ten.
- [x] **`query platforms` on an unsatisfiable target prints nothing and exits
      0.** It now says so, and says why: the target, then one line per distinct
      constraint that ruled a platform out. On stdout and still exit 0, beside
      `path`'s "no path" — it is the answer to the question rather than a
      complaint about the invocation.
      `repos/tags/unsatisfiable_target/expected/platforms.txt` is no longer an
      empty file, which was the whole complaint.
- [x] **`test.dependencies` edges are not visibility-checked.** Implemented:
      `Workspace::test_dep_edges` (`build/workspace.rs`) collects the
      `test.dependencies` of either rule and a library's
      `testing.dependencies`, and `check_visibility` walks it beside
      `dep_edges`.

      Deliberately *not* folded into `dep_edges`: a test dependency is not part
      of what a target ships, so putting it in `closure()` would drag its tags
      into the production tag closure, count against `unused-dep`, and make a
      cycle out of a suite that merely borrows a helper. For the same reason
      the test edges are checked on the target itself rather than across its
      closure — a consumer neither links a library's suite nor could fix it —
      and `//...` reaches every target's own suite anyway.
      `repos/build-files/test_dep_visibility` covers it from both `lint` and
      `test`, ending with the fix the diagnostic printed.
- [x] **`name_not_reexported`'s fix was misleading.** It said "add `export` to
      `rawValue`'s declaration", but the declaration is already exported —
      being exported is exactly why the import got as far as this diagnostic.
      What is missing is the re-export in `lib.buri`, and that is what it says
      now: "re-export `rawValue` from \"//lib/money\"'s `lib.buri`", branching
      on whether the module the name was asked of is a `lib.buri`
      (`compiler/semantics/resolve.rs`). `repos/libraries/name_not_reexported`
      re-recorded, both forms. The sibling diagnostic about a re-export naming
      an unexported name was correct as it stood and did not move.

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
- [x] Package with both rules — the negative half of BUILD-FILES.md:299-308.
      The overlapping `sources` half is `duplicate_source`; the two import
      directions are `repos/build-files/both_rules_boundaries`.

      Recording them turned up a third defect, now fixed: **`main.buri`
      importing a module inside the library beside it was accepted.**
      BUILD-FILES.md:301-305 says the binary reaches the library only through
      `//tools/report`, but the `ModuleKind::Internal` arm in
      `compiler/modules.rs` asked only whether the importer was in the same
      *package* — and in a both-rules package every file is, which is precisely
      the case the rule is about.

      The question it asks now is which *rule* the importing file belongs to
      (`Workspace::rule_of_file`), because that is what the boundary is drawn
      around. Two directions fall out of one comparison, and the case records
      both: a binary source reaching a library-internal module gets
      `internal-import` naming the surface to go through, and a library source
      reaching a binary source gets `internal-import` saying the binary depends
      on the library rather than the other way round. The entry points needed
      no special case — `lib.buri` importing `//tools/report/main` is still
      `binary-entry-import`, and `main.buri` is just another file of the binary
      rule.

      `duplicate_source`'s `main.buri` moved to the surface import with it: it
      had been reaching past the boundary too, and that case is about one file
      listed twice.
- [x] `lib.buri` missing from a `library`, or listed in `sources` —
      `repos/build-files/library_entry_point`.

      Missing is caught: the rule kind names `lib.buri`, so a `library` in a
      package without one gets `module-not-found` naming the file. **Listed in
      `sources` used to be accepted**, because `check_sources_declared` puts
      the three entry points into `known` unconditionally, so a listing of one
      matched itself and nothing asked whether the rule had already named it —
      the redundancy was invisible to the only check that walks the list.

      It is `entry-point-listed` now, raised in the loader
      (`compiler/modules.rs::load_package_source`), which is the one funnel
      every declared source goes through and therefore the one place all three
      names are covered by one check. Being in the loader also means `build`
      and `test` say it and not only `lint`: BUILD-FILES.md:140-144 states it
      as a property of the rule rather than as a hygiene preference. The case
      records `lib.buri`, then the fix, then `main.buri` in a binary — one
      clause, and it reads the same whichever rule kind names the file.
      `buri gen` already agreed: `regenerate.rs` skips the three names when it
      writes a `sources` list, so nothing it generates now fails.
- [x] `testing/lib.buri` required when the block is present; the block required
      when the directory exists; empty `testing {}` accepted —
      `repos/build-files/testing_surface`, all three.

      The block present with no `testing/lib.buri` is caught twice over
      (`module-not-found`, and the missing source). An empty `testing {}` is
      accepted. And **the block required when the directory exists** used to
      hold only by accident: nothing looked at the directory, and what fired
      was `undeclared-source` on the files inside it — so a `testing/`
      directory holding nothing but its own entry point passed with no block at
      all, because `known` always contains `testing/lib.buri`. The surface was
      then invisible: no target compiled it, and `//pkg/testing` resolved to a
      file the build had never heard of.

      `undeclared-testing-surface` asks about the directory now
      (`compiler/modules.rs::check_testing_surface_declared`), which is the
      only thing a file-by-file check could never reach. It is asked once per
      package — by the library rule, or by the binary rule when there is no
      library, so a package with both is not told twice — and it steps over a
      `testing/` that carries its own `BUILD.buri`, since that is a package and
      its files are its own business. The case now ends with the fix: `testing
      {}` put back, and clean.
- [x] `testing.dependencies` — a `testing/` block with deps of its own, which
      do not become the library's. Same case: `//lib/widget` declares
      `//lib/core` under `dependencies` and `//lib/aid` under
      `testing.dependencies`, and `query deps(//lib/widget)` prints `//lib/core`
      and nothing else. Recorded as a golden with a name in it rather than as
      an empty file, so the assertion is that the *right* dep is there and the
      other is not — an empty golden would have passed for the wrong reason.
- [ ] `artifact_name` on an output. *(untested; native artifacts are built now,
      so this is a test nobody has written rather than a feature nobody has
      landed — `actions::artifact_path` is where it is read)*

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
- [x] `//pkg/main` imported from outside that binary's own test sources —
      `repos/libraries/binary_entry_import`, all three positions: the binary's
      own suite imports it and passes, another package's library is refused,
      and one of the binary's own production sources is refused too. Being in
      the package is not what earns the import; being a test source is.

      **Writing it turned up a defect in `Loader::role_for`, now fixed.** What
      a module is imported *as* is a property of the module, and `role_for`
      returned `Role::Source` for every non-`core`, non-`testing` path — so a
      binary's entry point reached through an import was compiled as ordinary
      source, and its `core/host` import and its `context` were both rejected.
      It consults the workspace now and answers `Role::Entry` for a
      `ModuleKind::BinaryEntry`.

      A real build never noticed, because `load_unit` pre-loads a binary's
      entry point as `Role::Entry` before anything can import it — the bug
      needed a compilation that reaches `main.buri` through an import first.
      The documentation harness is exactly that, which is why the evidence is
      a document: testing.md's own example of a binary's suite had been marked
      `ignore` with this bug written out as the reason, and it compiles now.
      That is one off the untested-example ceiling.
- [x] A module path that is also a package path (`lib/money/cents.buri`
      alongside a `lib/money/cents/` package), rejected by name —
      `repos/libraries/module_is_also_a_package`. Implemented already
      (`Workspace::check_package_module_collisions`); what was missing was
      anything that ran it. The case carries the near miss in the same
      repository — a subpackage with no module of that name beside it — so the
      rule cannot quietly become "a subpackage is an error".

      It is exit 2, not 1: a repository where one path means two things is
      unreadable rather than badly written, which is the answer an unparseable
      build file already gets. There is deliberately no closing edit, because
      the fix is to rename a file and a case step edits text inside one.
- [x] `//lib/x/testing` imported from a production source —
      `repos/libraries/testing_import_in_production`, with the dependency
      declared under `dependencies` so that only the language can object.
- [x] `testing/` code never linked into a production artifact —
      `repos/libraries/testing_not_linked`. Every other test of the `testing`
      surface is about what the toolchain *says*; this one is about what it
      *emits*, which is where the claim actually lives.

      A marker string defined in one file — the fake under `testing/` — read
      back out of the binary's own `.mjs`, in debug and again in `--release`.
      On its own an absence passes for the wrong reason (a mistyped path, an
      artifact never written), so the same step also requires the *production*
      marker to be there, and a suite above it asserts on the fake, so the
      string is known to exist and to be reachable from somewhere.

      The case harness grew `file { contains }` and `file { absent }` for it.
      A whole-file golden is the right record of something a person reads; an
      artifact is mostly runtime, so recording one would move on every
      unrelated backend change and bury the one line that is the point. Like
      `exit`, neither is ever blessed.

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
- [x] A test suite inheriting its target's tags and platform restrictions —
      `repos/testing/inherited_policy`. Inheritance is invisible when it agrees
      with you, so the case ends with the edit that makes it disagree: the
      suite asks for a JS run and is refused by a rule in `REPO.buri` about a
      tag the `test` block never mentions.
- [x] `test { platforms: [...] }` — `repos/testing/suite_platforms`.
      `commands/test.rs` no longer pins the suite to `Platform::Js`: it checks
      policy against every platform the suite will run on and then runs one per
      declared platform. A platform the target does not admit is
      `platform-violation`, an error and not a skip, which is the whole
      difference between a policy and a preference.

      A suite that declares nothing is still checked against the host and
      executed as JavaScript, which is what `buri test` has always done and
      what the `server` tag in the example repository depends on.

      The honest boundary is the last step of that case, and what stands behind
      it has moved: a suite naming a platform this toolchain can build a binary
      for is compiled and run as one, and a suite naming a platform it cannot —
      no backend compiled in, no runtime archive, or a platform that is not this
      host, since there is no cross-compilation — still gets
      `platform-not-implemented` rather than being run through the backend that
      does exist and reported as if it had run natively.

      A native run has no *runner*: a failed assertion aborts the process, so
      the binary runs every `test` block in order and stops at the first
      failure. A clean run reports every test in it; a failing one reports the
      failure and, in a suite of more than one test, says it cannot attribute
      it. That is a worse report and not a wrong answer, and it is what SPEC
      6.10 costs — there is nothing to catch.
      *(landed, wave 3c)*

## TESTING.md

The whole document now has a corpus of its own: `repos/testing`, run by
`repositories.rs::test_suites`.

- [x] A test source importing a library-internal module —
      `repos/testing/internal_import`. It was **not an error**: the internal
      rule in `compiler/modules.rs` only ever fired across packages, so a suite
      reaching into the library it sits beside compiled and passed. It fires
      now, with the spec's message and a fix naming the surface to import and
      the symbols to re-export.

      A module is a test source because a rule lists it in `test.sources` —
      that is the only thing that makes one — so that is what the new check
      asks, rather than asking for the `TestSource` role. A documentation
      snippet is named by its origin rather than by a `//...` path, so it is
      never one, and testing.md's own example of a `testing/` fixture reaching
      its library's internals still compiles.
- [x] A test source importing another test source —
      `repos/testing/test_source_is_not_a_module`. Also not an error before:
      the second suite was loaded as an ordinary module and the failure came
      out as an unrelated pile about exports. Now `test-source-import`, at the
      import line, and the case records both directions — a sibling suite, and
      the library's own source.
- [x] A test source that `export`s — `repos/testing/test_source_exports`, both
      forms recorded. The checker already errored; the case pins the words.
      The diagnostic still carries **no code**, so it is the one recorded
      diagnostic in the corpus with an empty `[...]` slot.
      *(`semantics/resolve.rs` was another agent's at the time)*
- [x] `buri test --accept` — `repos/testing/accept_golden`. Implemented in
      `commands/test.rs`: it runs outside the cache in both directions, writes
      the actual value into the declared `data` file whose contents a failing
      `assert.eq` expected, and prints a diff. Each of the three bounds is
      checked by something that fails loudly if crossed — a golden that exists
      and is not declared, a golden that is declared and does not exist, and a
      final ordinary run whose recorded output is what says neither moved.

      The verdict is deliberately unchanged: `--accept` exits 1 on a suite it
      just accepted, because what it changes is the source tree and not whether
      the tests passed.
- [x] `--filter=<substring>` on test names — `repos/testing/filter`. It worked;
      what it did not do was say so. The summary now carries a `skipped` count,
      as TESTING.md:389 shows it, so a filter that matches nothing reads
      differently from a suite that holds nothing.
- [x] `--shuffle` — **there is no such flag, and there must not be.**
      TESTING.md:396-398 is explicit that the runner may run a suite's tests in
      any order and that there is no knob to turn that off. This entry used to
      ask for one; it was wrong. `repos/testing/filter` ends by pinning that
      `--shuffle` and `--shuffle=off` are both exit 2, which the `commands`
      table already guarantees — a flag nothing reads cannot be listed.
- [x] `timeout_seconds` on a suite — `repos/testing/timeout`. Implemented: the
      runner spawns the test binary and kills it at the deadline. The bound is
      the suite's, so the diagnostic is the suite's and no test in it gets a
      result. The case's negative twin gives the spinning function a base case
      and the same declaration lets it through.
- [x] Test caching — `repos/testing/caching` for the count a person reads, and
      `incrementality.rs::a_test_suite_is_cached_and_force_re_runs_it` for the
      `--explain` transcript, which is the sharper question. A second test
      pins that `--filter` and `--accept` are never served from the cache and
      that a filtered run does not leave a partial result behind for the next
      whole one to find.
- [x] The failure format — `repos/testing/failure_format`, and again inside
      `accept_golden` where three failures are recorded at once. Target, file,
      test name, both values, source location, and the summary beneath a blank
      line.

      The location is the `test` declaration's, not the failing assertion's.
      The runner learns where a test is from the compiler and where it failed
      from a JavaScript exception, and only the first of those is a Buri span;
      TESTING.md:387 shows the assertion's line. *(the assertion's own span is
      unimplemented — it needs the span to travel into `core/testing/assert`,
      which is a change to the standard library's signatures)*

Well covered already, for contrast: the runner-context table (TESTING.md:262-272)
— `captureOut`, `captureErr`, `stdin`, `files`, `readOnly`, `noNet`, `clockAt`,
`advance`, `randSeed`, `envOf` are all exercised by
`cli/tests/conformance/lib/semantics/test/effects.buri`.

## CLI.md

- [x] **`buri gen` — three cases in `repositories/cli/`, and four clauses of
      CLI.md:144-219 that turned out to be unimplemented rather than untested.**

      `gen_managed_fields` is the whole contract in one file: a `library` rule
      that starts with nothing but its policy, and comes back with all six
      managed fields derived and `tags`, `platforms`, `visibility`,
      `test.data`, `test.platforms`, `timeout_seconds` and every comment
      saying exactly what they said. It then runs `format --check` (gen leaves
      the file as the formatter would), `gen` again (nothing to do, and it
      prints nothing), `lint //...` and `test` (the derived file is not merely
      stable, it is right), and finally takes `sources` away again so that
      `--check` can report it, exit 1, and write nothing — checked by
      recording the file's own bytes, which is the half the exit code cannot
      show. The last step regenerates and compares against the *same* golden
      as the first.

      `gen_never_creates` is the two refusals, both recorded as an absence
      that survives `buri gen //...`: a directory of `.buri` files with no
      build file stays without one, and a package whose `library {}` sits
      beside a `main.buri` gains no `binary` rule. It ends with the decision
      `gen` refused to make, made by a person, and `gen` filling in the rule
      that appeared.

      `gen_both_rules` is the four-rule assignment, walked backwards because
      rule 4 fires first: the file reachable from both entry points, then the
      file reachable from neither, then the run that places the rest by rules
      2 and 3 while rule 1 holds the two already-listed files still. It ends
      with `lint` and `test`, because a source under the wrong rule is an
      `internal-import` the moment anything uses it.

      Four things were wrong rather than untested, all fixed in
      `build/regenerate.rs`:

      - **Rules 2 and 3 did not exist.** A file in a both-rules package that
        no rule listed was *always* the rule-4 error; reachability from
        `main.buri` and from `lib.buri` was never computed. It is now, off the
        syntax (`reachable`/`imports_of`) rather than off a checked analysis,
        because the file being placed is a file no rule lists yet and the
        loader would not have it. The diagnostic now also says which half of
        rule 4 fired — "reachable from both" and "reachable from neither" are
        different problems with different answers.
      - **`test.dependencies` and `testing.dependencies` were never written.**
        Two of the six fields CLI.md lists as managed. `derive_dependencies`
        now splits its answer by the *role* of the module the import or the
        resolved call sits in, which is what tells a test source from a
        production one, and subtracts the production list from the test one —
        the target under test reaches the suite through itself, so naming its
        dependencies again would be `unused-dep` on the second claim.
      - **A managed field could not create the block it lives in.** CLI.md's
        worked example starts from `library {}` and comes back with a
        `test.sources`; `set_list` walked to the block and gave up when it was
        not there, so that example did not work. It creates the block now, and
        only when there is something to put in it — an empty `test {}` nobody
        wrote is a claim the package has a suite.
      - **`gen` was not a fixed point in one pass.** A test source is loaded
        because a rule lists it, so on the run that *writes* `test.sources`
        the analysis had not seen one and `test.dependencies` came out empty —
        appearing on the second run. A command whose second run differs from
        its first is a command whose `--check` lies. The test files' imports
        are now read off disk and merged with what the analysis found.

      One consequence outside the command: `cli/tests/example`'s
      `lib/ledger` gained the `testing.dependencies: ["//lib/money"]` it had
      always needed and nothing had ever written.

      Two smaller things, deliberate: in a package with both rules the summary
      names the rule (`+ library.sources: …`), because `+ sources:` twice is
      two claims a reader cannot tell apart; and in a both-rules package a
      `test/` file goes to the binary's suite when it imports `//pkg/main` and
      to the library's otherwise, which is the same reachability question read
      from the other end. `buri gen //...` over `cli/tests/example` and over
      the conformance repository changes nothing but formatting and sorting.
- [x] **`buri run` — `repositories/cli/run_passthrough`.** The three claims
      `buri build` cannot make: the program runs and its stdout is the
      command's, everything after `--` reaches the program rather than the
      CLI, and the program's exit code is the command's. The exit code is `3`,
      which is neither success nor either of the two failure codes the CLI
      itself uses, so nothing else could have produced it; the passthrough
      argument list includes `--force`, a flag `buri run` really does take, so
      what is pinned is that after `--` it is argv and not a flag. The program
      also reads a file by a relative path and prints it, which is the
      readable form of "outside the sandbox, with the real filesystem".

      The harness moved for it: `run_in` used to append `--color=never`, which
      after a `--` landed in the program's own argv. It goes before the `--`
      now, so a golden records the product rather than the harness.
- [x] `buri query deps(...)`, `rdeps(...)`, `path(...)`, `sources(...)` —
      `repositories/query/graph_queries`, a new corpus with its own test
      (`repositories.rs::graph_queries`) because a query that has stopped
      working prints a plausible wrong answer rather than failing. The
      repository is CLI.md's own shape: `//cmd/web` reaches `//lib/store` only
      through `//lib/ledger`, and a second binary is there so that the
      *absence* of a path is recorded beside the presence of one. `rdeps` is
      recorded twice, wide and narrow, so it is visibly not `deps` reversed;
      the empty answer for a leaf is recorded next to a three-line one rather
      than alone, because an empty file passes for a broken command too.
- [x] `buri query --output=proto` — **there is no such flag, and the entry was
      stale.** It was documented once and rejected by the parser; the flag
      table in `commands/mod.rs` ended that disagreement in the other
      direction, by deleting it from the documentation, since a flag nothing
      reads cannot be listed. `graph_queries` records the refusal so that a
      reader who finds the old claim finds the answer with it.
- [x] `buri lint --fix` — `repos/linting/fix_applies` and
      `fix_refuses_a_judgement_call`. The flag had been removed entirely; it is
      back, and the two kinds of answer are applied differently: a build file
      that disagrees with the code goes through `gen::regenerate`, an unused
      import is a byte edit. `dep-cycle` and `tag-violation` are refused, and
      the refusing case records both `BUILD.buri` files so a later `--fix` that
      decides to be clever about cycles fails rather than rewriting somebody's
      graph.

      **`--fix` must not run the formatter.** The first version guarded its
      output with `format::source`, which parses *and* reprints — and reprinting
      deletes every comment inside a function body (see the formatter bug
      below), so `--fix` silently destroyed them. It now checks the result
      parses and writes nothing else. `fix_applies` pins this: its `main.buri`
      carries a body comment and a hand-written single-line `context`, and the
      golden is byte-identical apart from the removed name.
- [x] `buri clean --outputs` dropping `.buri/out` only —
      `repositories/cli/clean_outputs`. The difference between `clean` and
      `clean --outputs` is invisible in what they print and invisible in an
      exit code, so it is read twice: off the directories themselves, through
      the case harness's new `path` step, and off the *next build* — which
      reports `cached` when the cache survived and does not when it did not.
      The second reading is the one that matters, because a `--outputs` that
      deleted everything would pass the first.
- [x] `buri version` printing the toolchain version — `repositories/cli/version`,
      plus `conformance.rs::version_works_outside_a_repository` for the clause
      that cannot be a repository case, since a case *is* a repository. The
      pinned output names a version and so moves when the toolchain's does; that
      is deliberate and the diff is the release.

      **Reshaped when the toolchain pin was removed.** The case was
      `version_pin` and pinned three things: the version, the `REPO.buri` pin
      the command reported, and that a mismatched pin stopped `buri build` too.
      The last two went with the feature, so what is left is one golden and the
      claim that opening a repository adds nothing to what `version` prints —
      the absence, recorded where the presence used to be. `--verbose`'s second
      line is the running executable's hash and no golden can hold it, so
      `conformance.rs` asserts that one instead.
- [x] `buri lsp` — implemented, and recorded as three sessions in
      `repos/lsp/`. `diagnostics`, `hover`, `definition`, `documentSymbol`,
      `formatting`, and completion in the two places that need no type
      information: inside a module path, and inside an import's `{ … }`.
      The case harness grew a `run { stdin: "session.jsonl" }` step for it —
      one JSON request per line, the harness frames them and records the
      decoded responses, so the golden is about what was said rather than how
      many bytes it took.

      Two notes on how it was built rather than what it does:

      - **The overlay for unsaved buffers needed no change to `compiler/modules.rs`.**
        `SourceMap::load` already reuses an entry whose name is present, so
        seeding the map with the editor's copy under the name the loader will
        ask for means the loader never reaches the disk.
      - **Scheduling is in the doc, not tuned in the code.** `didChange`
        re-parses one buffer; open and save run the whole front end. Analysing
        per keystroke would mean re-checking the standard library per
        keystroke, because `driver::analyze` is whole-closure.

- [x] **The language server reports the build-graph findings and fixes them.**
      `publishDiagnostics` carries what `buri lint` reports as well as what the
      front end says — an editor showing only type errors was showing the half
      that is easier to notice at a terminal anyway. `codeAction` offers the
      same two kinds of answer `lint --fix` applies, the same way: a finding
      carrying `Diagnostic::edits` becomes a text edit, and one about a build
      file is handed to `buri gen`, which returns the whole file. A finding with
      no mechanical answer offers nothing.
      `repos/lsp/missing_dep_code_action` records both halves.

      The cost, stated in `docs/cli/lsp.md` rather than hidden: the lint checks
      build their own analysis, so a save now costs two. That is the other
      reason none of this happens on a keystroke.

- [ ] **Nothing tests the LSP against a real editor.** The recorded sessions
      prove the server answers; they do not prove a client can drive it.
      *(untested)*

- [x] **Editor integration exists**: `editors/tree-sitter-buri` (grammar plus a
      C external scanner for string interpolation and nestable block comments)
      and `editors/zed` (the extension, which starts `buri lsp` from `PATH`).
      `editors/tree-sitter-buri/check.sh` holds the syntax tree to the
      compiler's own parser over every `.buri` source in the repository, and
      compiles every highlight query. It is not a `cargo test` — it needs the
      tree-sitter CLI, and the toolchain may not depend on an external tool —
      so `corpus.rs::the_editor_integration_is_whole` checks the files are all
      still there and that the queries have exactly one copy.

      **`grammar.ebnf` was stale, found by transliterating it.** Two
      productions described a language the compiler does not accept. Both are
      corrected; the grammar is normative, so the defect was in it and not in
      the parser.

      - `ImplDecl ::= "impl" ... "{" FnDecl* "}"` — a method of the type's own
        may be exported, and every `impl` in the standard library and in
        `cli/tests/example` writes `export fn`. The two forms are now written
        out separately, because the `export` is admitted in exactly one of
        them: an inherent `impl` takes `("export"? FnDecl)*`, a trait `impl`
        takes `FnDecl*`. `parsing/parser.rs` reads it that way — `export` in an
        `impl ... for ...` is `impl-method-export`, since conformance is a
        property of the type and is visible wherever the type is.
      - `FnDecl ::= ... Block` — a declaration may have no body, so it is now
        `... (Block | ";")`. That is how the standard library declares the
        primitives the runtime supplies (`export fn len(self: Str): Int;`,
        `std/str.buri:18`). The `;` form is *syntax*; where it is allowed is a
        separate rule and not a syntactic one — outside a trait, an effect, or
        a bundled standard-library module it parses and is then rejected
        (`parser.rs`, `allow_bodyless`), which is what lets the diagnostic say
        the body is missing rather than that the `;` was unexpected.
        `MethodSig` stays, now documented as the same production under a name,
        because inside a trait or an effect the `;` is required rather than
        merely admitted.

- [x] **`cli/src/docs/grammar.ebnf` and the tree-sitter grammar could drift,
      and they cannot now: `grammar.js` is generated from the EBNF.** There
      were three descriptions of Buri's syntax and no two of them were held
      together by anything but reading. There are two, and the second is the
      implementation.

      `cli/src/documentation/grammar.rs` reads the EBNF and writes
      `editors/tree-sitter-buri/grammar.js`;
      `corpus.rs::the_tree_sitter_grammar_is_generated_from_the_ebnf`
      regenerates it and compares byte for byte, with `BURI_BLESS=1` to record.
      The file stays checked in because an editor installs the grammar without
      the toolchain.

      **The EBNF had to grow, and the constraint was that it must not stop
      being documentation.** `buri docs grammar` serves it verbatim inside a
      fenced block, so everything a generated parser needs beyond a
      context-free grammar is either an ordinary EBNF comment whose first
      character is `@`, or a label: `name=X` on the right-hand side is what a
      syntax tree calls its capture of `X`, which is a fact about the language
      and reads as one. Nothing the productions already state is stated twice —
      a node's name is derived from the production's name, an operator's
      precedence number from its position in the cascade, and its
      associativity from the shape of the production, so `AddExpr ::= AddExpr
      ("+" | "-") MulExpr | MulExpr` *is* "left-associative, level 8" and there
      is nowhere for those to disagree. Token patterns are compiled out of the
      lexical productions, so `[0-9][0-9_]*` is not written anywhere.

      **Two escape hatches, both small and both named.** `@regex` supplies a
      pattern where the lexical grammar states its token in prose — five
      tokens: `IDENT` (whose "minus Keyword" is not a regular expression), the
      character class inside `CHAR`, and the three comment forms, whose
      disambiguation the EBNF states as longest-match and a generated lexer
      needs written out. `@raw` would take a rule in tree-sitter's own terms
      and is used nowhere; it exists so that a future corner has somewhere to
      go other than into the EBNF.

- [x] **Nothing executed the EBNF, and the corpus now does — in both
      directions.** `check.sh` used to parse every source that compiles and
      assert no `ERROR` node. That is the half a corpus of working programs can
      see. The other half is whether the grammar accepts something the language
      does not, and the arbiter for it is the parser itself. `check.sh` pipes
      every Buri source in the repository — 474 of them — through
      `cargo run -p buri --example parse_verdicts`, which answers `parses` or
      `rejects` per file, and then requires an error node in the syntax tree
      exactly where the parser produced a diagnostic.

      **The verdict is asked for, not recorded.** It was a checked-in file for
      about an hour, which was a readout of what the compiler does rather than
      something to compare it against: it went stale on every new test case,
      and it went stale silently in the one direction that matters, which is
      when the compiler's answer changes. The example binary is the seam
      because `buri parse` would be a subcommand nobody outside this
      repository needs; if it ever earns one, that is its name.

      The distinction that makes it work is the *stage*. A case in
      `tests/reject/` that fails type checking is well-formed Buri and its
      syntax tree must be clean; only a parse-stage rejection is a claim about
      syntax. `expected/` under `tests/repositories` is left out, because a
      recorded `buri gen` output is a `BUILD.buri` under a name ending in
      `.buri` and is textproto.

      **It found one, immediately.** The hand-written grammar wrapped every
      method in an `impl` in `impl_method`, so `export fn` inside an
      `impl ... for ...` parsed — which is `impl-method-export`, a diagnostic
      the parser raises. The generated grammar follows the EBNF, where the two
      `impl` forms are written out separately, and rejects it.

      Five files are where the two are *meant* to disagree, listed in
      `check.sh` with a reason each and reported if one of them starts
      agreeing. All five are the same argument: reserved words (`while` is a
      word the lexer refuses, not a keyword), a keyword where a name belongs
      (`fn test(...)`, which tree-sitter's keyword extraction reads as an
      identifier — the mechanism its error recovery is built on), and chained
      comparison (`a < b < c` is not derivable, but a red squiggle is worse
      than the `chained-comparison` message).

- [x] **`Block ::= "{" Stmt+ "}" | "{" Stmt* Expr "}"` was stale**, found by
      generating from it. The parser accepts `{ }` and `{ let x = 1; }`: a
      block with no result expression parses, and having no value is reported
      by the checker, which can say what the block was expected to produce.
      The production is now `"{" Stmt* Expr? "}"`, which is the same shape the
      grammar already uses for `ExprStmt` — the syntax admits it and a rule
      objects, so the diagnostic is about the program rather than about the
      punctuation.

- [ ] **`editors/zed/extension.toml` pins a placeholder commit.** The grammar
      has to be published as its own git repository before the extension can be
      installed as anything other than a dev extension. *(unimplemented)*

- [x] No-argument invocation operating on the package containing the working
      directory — `repositories/cli/cwd_package`. (This was listed twice; it
      is one item.) The case harness grew a `run { cwd: "lib/money" }` knob for
      it, because there is no way to ask the question from the root.

      A scoped command that does nothing proves nothing, so both halves are
      arranged to be visible: `cmd/app` carries an `unused-import`, and every
      no-argument run inside `lib/money` has to stay silent about it while the
      run from the root reports it; and `lib/money`'s build file is missing
      its `sources`, so the no-argument `gen` has something to write and says
      so. `build`, `test`, `lint` and `gen` are all covered, and a run from a
      directory that is not a package records the message that names the fix.
- [x] "All commands are safe to run concurrently; a file lock serializes cache
      writes" (CLI.md:25). **The lock was a claim rather than a lock.**
      `Cache::open` held an open file handle called `_lock` and never took one;
      nothing was serialized, and two writers of one key shared a `.tmp` name.
      There is a lock now (`cache.rs::Lock`), and it is deliberately *narrow*:
      `create_new` on a lock file, held for the length of one `put` and not for
      a build, with reads taking nothing at all because an entry is renamed
      into place and is therefore whole or absent. A lock left by a killed
      process is stolen after thirty seconds — safe for the same reason the
      lock is cheap, since the name of an entry is the hash of its contents and
      two writers of one key are writing the same bytes.
      `hermeticity::two_concurrent_builds_leave_the_cache_intact` runs four
      builds of one repository at once from cold and then asks the cache for
      the answer, because "both processes exited 0" is the half of this that
      was never in doubt.
- [x] The `out/` convenience symlink pointing at the most recent build —
      `repositories/cli/out_symlink`. What is pinned is everything that can
      be: the link exists, it is a link rather than a directory, it names
      `.buri/out/js` relatively, both targets' artifacts are reachable
      through it, `clean` takes it with the directory it points into, and a
      later build puts it back.

      **The boundary was the native-backend one, and it has moved rather than
      closed.** The link points at a *platform* directory, and while JavaScript
      was the only backend every build produced `.buri/out/js`, so two builds of
      two targets could not tell a correct implementation from one that
      hard-codes the string. A native build now writes `.buri/out/macos/...`, so
      the step that tells them apart — a build for one platform followed by a
      build for the other, and the link naming the second — is writable today.
      It is not written: the case is a repository manifest and the platform it
      would have to name is the host's, which a manifest cannot ask for.
- [x] `--output=linux/x86_64` selecting one of several outputs —
      `repositories/cli/output_selection`, and **selecting nothing used to
      exit 0**. `selected_outputs` filters the declared outputs by the
      selector and `cmd_build` looped over the result, so
      `buri build //cmd/app --output=linux/x86_64` on a target that declares
      only JS built nothing and reported success — a build system reporting
      success for work it did not do. It now exits 2 naming the outputs the
      target does declare (`commands/build.rs`, before the build loop): the
      selector is the thing you asked *with*.

      The rest is pinned as far as it goes. A target declaring both JS and
      `LINUX/X86_64` builds the first and is refused for the second, in both
      the `linux/x86_64` and the `linux-x86_64` spelling, and with no selector
      at all it does both — one artifact and one refusal.

      **The refusal is now about the host rather than about the backend**, and
      that makes this case host-dependent: on a Linux x86_64 machine that
      selector names the host, the build succeeds, and the two goldens do not
      hold. What a native artifact *is* is pinned in `incrementality.rs`
      instead, where a test can ask which platform this is; making the
      repository cases host-independent needs the harness to know it too.
- [ ] **A `main.buri` in a package with no `binary` rule is invisible.**
      Found writing `repositories/cli/gen_never_creates`, and pinned there as
      a clean `lint` run rather than fixed. `gen` is right to leave it alone —
      it never adds a rule — but nothing else mentions it either:
      `check_sources_declared` (`commands/lint.rs`) puts `main.buri` in the
      `known` set unconditionally, by the rule *kind* that names it, and in a
      library-only package there is no rule of that kind. So the file is
      compiled by nothing, shipped by nothing, and reported by nothing.

      The fix is one condition — an entry point is declared by the rule that
      names it only when that rule exists — and a new row in the build-graph
      table, next to `undeclared-source`, saying that a `main.buri` with no
      `binary` rule (or a `lib.buri` with no `library` rule) is a file the
      build cannot see. *(untested elsewhere; the case records today's
      silence, so the fix will show up as a diff there)*

- [x] **`buri format` puts the fields of a build file in the schema's order.**
      `textproto::print` used to emit them in the order it read them, so a file
      written binary-first stayed binary-first and `format --check` passed.

      The rule is one rule and it settles what blocked this before: the order is
      **the schema's declaration order**, taken from the same `check_known`
      lists in `buildfile.rs` that decide whether a field is a field at all. It
      needs no special case for `REPO.buri`, because `REPO.buri` has a schema
      too — a build file is data, the order of its fields carries no meaning,
      and the one order nobody has to argue about is the one the schema was
      written in. `library` before `binary` (CLI.md:100) falls out of it, and so
      does `sources` before `dependencies` before `test`.

      Two things the sort does not touch. A field the schema has never heard of
      keeps its place at the end — a formatter that moved or dropped something
      it did not recognise would be worse than one that left it alone — and two
      fields of the same name keep the order they were written in, because that
      order is the only thing about a repeated field that could mean something.

      Build files are formatted at four spaces now, the same as source: a build
      file and the code beside it are read by the same person on the same
      screen. `buri gen` and the language server's code actions write through
      this same printer, so what `gen` leaves behind is what `format --check`
      accepts, and neither can drift from the other.

- [x] **The lint catalogue is complete.** Build-graph rules:
      `undeclared-source`, `duplicate-source`, `entry-point-listed`,
      `undeclared-testing-surface`, `missing-dep`, `unused-dep`,
      `dep-cycle`, `platform-violation`, `visibility-violation`,
      `tag-violation`, `unknown-tag`. The two new ones are the build file
      disagreeing with the package's own files in the two ways the rest of the
      table could not express — a rule naming its entry point twice, and a
      surface on disk that no rule declares — and both have rows in
      `docs/build/cli.md`, where `internal-import`'s row also stopped saying
      "another package" now that the boundary it enforces is the rule's.
      Style and hygiene rules — new, each with
      a case in `repos/linting/` recording both the finding and the edit that
      ends it — `unreachable-export`, `unused-import`, `discarded-result`,
      `empty-test-suite`, `test-without-assertion`.

      Three findings from writing them, none of them test gaps:

      - **`boundary-violation` and `testonly-in-production` never existed as
        codes.** The checks did, as compile errors in
        `compile::check_import_legality`, carrying `internal-import`,
        `binary-entry-import`, and `test-only-import`. Inventing the CLI.md
        names as second codes for the same checks would have given one rule two
        names; the catalogue in `docs/build/cli.md` now names the real ones.
      - **`discarded-result` could not fire as specified.** CLI.md described a
        warning on `let _ = <Result>`, which `compiler/semantics/expressions.rs` already makes a
        hard error (`result-discarded`) and the reject corpus records. It is
        now a warning on `core/result.ignore` — the escape hatch the row's own
        text pointed at.
      - **`unsorted-imports` is not a lint.** Import order is layout, and
        layout is `buri format`'s job. An unsorted import run is a file that has
        not been formatted, not a finding to report. *(the formatter does not
        sort yet — see below)*

      Two true positives in our own fixtures, both fixed: `//lib/store`
      imported `core/str` and never used it (`concat` is a method, and a method
      resolves through its receiver's defining module, not through the
      namespace binding), and `encodeLine`'s `export` reached nobody.

- [x] **`buri format` sorts the leading import run** — `core/*` before `//*`,
      then by path, then by clause, with one blank line between the two groups
      and none inside either. Unit-tested in `formatting.rs`: the order is total,
      it is a fixed point, a comment travels with the import it was written
      above, and only the *leading* run moves (an import written after a
      declaration stays put, because moving it across the declaration could
      change what the module means).

- [x] **`buri format` wraps a long import clause.** Over `WIDTH`, the clause
      breaks onto its own lines, filled and comma-terminated — the shape the
      35-name import in `conformance/lib/semantics/test/generics.buri` was
      hand-wrapped into before the formatter flattened it to 292 columns. A
      re-export wraps the same way, because it is the same line with a
      different keyword. `a_long_import_clause_wraps` checks the shape, that
      no line exceeds the width, and that wrapping is a fixed point.

- [x] **`buri format` keeps comments inside function bodies.** Trivia is no
      longer keyed by a declaration's offset and read once: `Comments` holds
      every comment in the file with the offset of the token it was written
      above, each printer *claims* the ones it is responsible for, and every
      construct sweeps its own span before it closes. So a comment comes back
      above the statement, match arm, field, variant, method, or context
      binding it was written above, above the `}` when it was written last in
      a block, and at the end of the file when it was written below everything.
      A body that would fit on one line stops collapsing when there is a
      comment in it, because a single line has nowhere to put one.

      `source` now refuses its own output when the comments do not survive it,
      the way it already refused output that did not parse — a construct
      nobody thought of leaves the file alone rather than damaging it.

      Three properties, none of which a fixed point can express:
      `formatting::no_comment_is_ever_dropped` over every place a comment can
      be written; `corpus::formatting_keeps_every_comment`, which is
      `token_shape` extended to comment trivia (`Shape::Comment`, `Doc`,
      `ModuleDoc`) and run over the whole corpus; and
      `corpus::formatting_the_corpus_preserves_what_it_means`.

      One thing found on the way and fixed with it: a comment block at the top
      of a file, with a blank line under it, is the *file's* and no longer
      travels down with whichever import happened to sit beneath it when the
      run was sorted (`a_file_header_stays_above_the_sorted_imports`).

- [x] **`buri format` wraps expressions.** The formatter used to wrap
      declarations and import clauses and nothing else, so a hand-wrapped call,
      match arm, array literal, or operator chain collapsed onto one line and
      the corpus came out at 161 columns with 85 lines over `WIDTH`.

      Layout is no longer decided while printing. The tree is converted to a
      `Doc` — Wadler's *A prettier printer*, in the form Prettier generalized
      it — and a second pass lays that out: `text`/`<>`/`line`/`nest`/`group`
      from the paper, plus `SoftLine` and `HardLine`, and `best`/`fits` as the
      paper writes them, one pass and one line of lookahead. Nothing measures
      by rendering a string and looking at it, and nothing writes output it may
      have to take back.

      Four extensions, all of them Prettier's, because the shapes below cannot
      be written in the core algebra: `Fill` (a list that reads *across*, which
      is an import clause and a table of constants), `IfBreak` (the trailing
      comma a broken list gets and a flat one must not have), `BreakParent` (a
      comment, so that whatever encloses it cannot be flat), and `Alt`
      — Prettier's `conditionalGroup` — for the two shapes that want one part
      of a construct to break *while a part enclosing it stays flat*: the
      trailing-argument hug and the method chain broken at the dots. Neither is
      a flatter or more broken version of the other, so `group` cannot choose
      between them. `Alt` measures its first candidate strictly (a forced break
      anywhere inside rules it out) and every later one by its first line, then
      lays the winner out normally so the groups inside it still answer for
      themselves.

      Two things fall out of the algebra that used to be code. A group is
      measured together with whatever the printer already has stacked behind
      it, so the `;` or `,` that trails an expression is counted without any
      printer being told about it. And a comment carries a `BreakParent`, so
      "a construct with a comment in it does not collapse onto one line" stops
      being a question each construct asks itself.

      The shapes, unchanged, each now one document rather than one branch:

      - a call goes **one argument per line** with a trailing comma, unless its
        last argument is a lambda, a block, an array or a struct literal, in
        which case the head stays on its line and that argument **hugs** (an
        `Alt` candidate whose last argument is a group forced to break) —
        `b.mapCtx(ctx, fn(c, x) => {` … `}).join(ctx, "")`. The hug need not be
        the last link of the chain, which is what carries the `.join` out onto
        the closing line the way the repository already wrote it by hand;
      - a chain of two or more method calls breaks **at the dots**, the first
        call staying with what it is called on so that `list.range(…)` does not
        come apart;
      - a run of one operator breaks together, **operator first**, so a long
        condition reads as the list of things it asks;
      - an array of plain values **fills**; an array of expressions, and every
        struct literal, goes one to a line. The line between them is the one the
        corpus drew by hand: a table of constants reads across, a list of
        computations reads down;
      - a `match` puts each arm on its own line and a scrutinee too wide for the
        keyword under it, and an arm body that will not fit beside the `=>`
        takes the line below;
      - once an `if` has to break, **every** branch is on lines of its own —
        keeping `{ a }` beside the condition gives a shape that depends on which
        branch happened to be longest. The whole `if` / `else if` / `else` chain
        is one `group`, which is all that rule is;
      - a signature is a list of parameters and wraps like one, which the widest
        function in the suite was already written as. A body that would fit on
        the line is written as an `IfBreak` on the signature's own group, so a
        signature broken over five lines never has `{ value }` hung off the end
        of it — while a body full of statements still says nothing about
        whether the parameters fit.

      A value with nothing inside it to break — a long name, a string — moves to
      the line below when that is enough, and stays over the margin when it is
      not: there is no shape a formatter can give a 90-column string literal.

- [x] **Blank-line fidelity in comments.** The lexer kept a comment run as a
      list of lines and forgot the gaps, so a section heading and the sentence
      under it came back as one paragraph, and a heading with a blank line under
      it got glued to the declaration it introduces. A comment now carries the
      blank line above it (`lexer::Comment`), a doc run carries the one above
      *it* (`Token::docs_blank`), and `Token::blank_before` narrowed from "a
      blank line somewhere in the trivia" to "the blank line above the run".
      One blank line and never two, which is how the formatter already treated
      the gaps between declarations.

- [x] **A paired-file corpus for the formatter** — `cli/tests/formatting/`, one
      directory per decision the formatter makes, each holding an `input.buri`
      somebody might have typed and the one `expected.buri` it is allowed to
      produce. Prettier's format tests, with the input and the output as two
      files and no third one: a formatter with options has no single right
      answer, and this suite exists to say that this one does.
      `BURI_BLESS=1 cargo test -p buri --test formatting` records, and rewrites
      `expected.buri` only — the question a case asks is fixed and only the
      answer is recorded.

      Six claims, each its own test so a failure names which broke: the output
      matches; **every** output is a fixed point (reported as instability
      rather than as a wrong answer); every comment survives as a set and every
      token as a multiset, modulo the redundant parenthesis, the optional
      trailing comma and the sorted import run; every line is inside the margin
      except in the `width_*` cases, which are named for the atoms that cannot
      break; the `clean_*` cases come out byte-identical to their inputs; and a
      case is two files and no more.

      The shapes it pins were then reviewed as a set, and seventeen of them
      changed — the corpus is what made that review possible, and re-blessing
      it is what made the change safe. Blank lines inside a body collapse to
      one rather than vanishing; a list is on one line or one item to a line,
      with no filling except in an import clause and a `derive`; a function
      body is never on one line and an empty one is `{}`; a lambda body and a
      match arm body that will not fit beside their `=>` are wrapped in braces
      rather than hung under it; a chain breaks at *every* dot or none of
      them, a field access counting as a dot and type arguments not interrupting
      one; the two import groups run together and the names inside a clause
      sort; a comment beside a parameter stays beside it; nothing is
      column-aligned; and a `derive` moves onto the type it is about when that
      type is declared in the same file.

      The indent unit is `INDENT`, one constant that everything derives from,
      and it is four spaces. Nothing else in the file knows a number of spaces.

      Writing it found three bugs, all now fixed.
      - **A block comment grew by two columns every time the file was
        formatted**, and then, once it stopped, stayed behind when everything
        around it was re-indented. Its continuation lines carried the
        indentation they were written with, and the printer added its own on
        top; not adding it left them at columns that no longer meant anything.
        A comment is now moved as a *unit*: the first line goes where the
        printer says and each of the rest keeps the distance from it that it
        was written with, which is the only thing stable under both a reformat
        and a change of indent unit. `lexer::Comment` records the column it was
        written at, and that is the whole of what makes it work.
      - **The one-line collapse had stopped working, and every trailing lambda
        hugged.** `fits`'s `must_be_flat` leaked past the candidate being
        measured into the printer's own stack, where the next thing is nearly
        always a hard line — so the "all of it on one line" candidate of every
        `Alt` was rejected out of hand. It is a question about the candidate,
        not about the line it is being fitted into.

      One shape changed with them: a function whose signature fits but whose
      one-expression body does not no longer breaks the parameter list. The
      body was inside an `IfBreak` on the signature's group, so the signature
      was being measured *with its whole body on the line*; it is two `Alt`
      candidates now.

- [x] **Ready for the corpus reformat; the reformat itself is still to do.**
      Run in memory over the corpus, formatting now takes the widest line from
      **309 columns to 180** and the lines over `WIDTH` from **73 to 15**.
      Every one of the fifteen is an atom the formatter must not rewrite:
      twelve are the text of a hand-written section-heading comment and three
      are a single JSON string literal. Every file re-parses, every file is a
      fixed point, and not one comment is lost.
      `corpus::formatting_the_corpus_preserves_what_it_means` formats the whole
      conformance repository into a scratch copy and gets the same assertions.

      What is left is the coordinated pass that writes it to the checked-in
      files. *(unimplemented)*

- [x] **Every emitted code is documented, and a test says so.**
      `documentation/errors.rs` had named this test since it was written and it did not
      exist; twenty-three codes had no page. It is
      `docs::every_emitted_code_is_documented`, and it accepts **either**
      catalogue, because there are two kinds of diagnostic: a compile error one
      program can provoke earns a page with that program on it, and a
      build-graph finding — `dep-cycle` needs two packages — belongs in the CLI
      reference's tables next to the command that reports it.

      Four new pages (`unresolved-type`, `no-such-module`,
      `module-doc-not-first`, `unterminated-comment`), each with a program that
      provokes it and is compiled by the suite. Four rows added to the
      build-graph table for the module-boundary rules that need a repository:
      `circular-import`, `no-such-module`, `module-outside-repository`,
      `host-import`.
- [x] `buri build`/`buri test --explain` — one line per action, its key, and
      whether the cache served it. New; `cli/tests/incrementality.rs` reads it.

## HERMETICITY-AND-CACHING.md

- [x] **The OS-level sandbox was built and then deliberately removed**, and
      that is the item rather than a step towards one. HERMETICITY-AND-CACHING.md
      now states the decided model: hermeticity is enforced by the language,
      verified by reproducibility, and the toolchain applies no operating-system
      confinement at all.

      The full version existed — a fresh directory per action holding read-only
      copies of its declared inputs, and a `sandbox-exec` profile denying the
      network and confining writes — and building it is what made the trade
      legible. Three facts about this language, together, take the ground out
      from under it:

      - **Every ambient read is a `$host_*` intrinsic**, and `core/host` is
        importable only from the module exporting `main`. A library, an inner
        module and a test source that reach for one are rejected at compile time
        (`host-import`). Nothing that participates in an action has a *name* for
        the environment, the clock, the filesystem, or the network — found the
        hard way, by writing a test that reads an environment variable and being
        told so by the compiler.
      - **A test's capabilities are fakes the runner injects.** There is no real
        capability to withhold from a suite.
      - **The action set is closed.** Four kinds, all of them this toolchain's
        own code, with no way for a repository to define a fifth. There is no
        user-supplied program in the graph to distrust.

      So confinement was never going to catch anything about repository code. It
      would only ever have been a second opinion about *toolchain* bugs — an
      intrinsic that leaked, a code generator that embedded a path — and it was
      a poor one: macOS only, writes and network only, and never reads, because
      a profile tight enough to deny reads also denies the JavaScript runtime
      its own binary and the runtime aborts before the action starts. Linux
      would have wanted user namespaces or `seccomp`, which is privileges or a
      dependency or both. A partial second opinion on one platform is not worth
      a mechanism that has to be maintained, probed, and explained in every
      document that mentions it.

      What catches that class of bug instead is `--check-reproducible` and
      `builds_are_reproducible`: a leaked intrinsic or an embedded path shows up
      as two builds of one tree disagreeing. The verification is the design, not
      the consolation prize.

      **What was kept, because it is determinism rather than confinement**
      (`build/spawn.rs`, renamed from `sandbox.rs` — a module called `sandbox`
      that sandboxes nothing is the same defect as a `_lock` field that never
      locked):

      - `env_clear` and then exactly two constants, `TZ=UTC` and
        `SOURCE_DATE_EPOCH=0`. Not to hide the parent's environment from a
        program that could read it — nothing in an action can — but so that a
        machine set to another time zone does not produce different bytes.
      - The clock frozen at `1970-01-01T00:00:00Z`: `Date.now`, `Math.random`,
        and the host clock intrinsics replaced in the action's own script
        (`spawn::FIXED_CLOCK_JS`), guarded with `typeof` because the minifier
        drops what a suite does not reach. Belt and braces against a runtime
        regression, and what makes a suite's *record* the same bytes twice.
      - **The runtime is resolved to an absolute path before the environment is
        cleared.** A child with no `PATH` cannot find `bun`, and a child with a
        `PATH` does not have an explicit environment. Resolving in the parent is
        the only way to have both.

      `hermeticity::a_perturbed_parent_environment_changes_neither_the_bytes_nor_the_verdict`
      is the test that carries the load now: the same tree built and tested with
      a time zone twelve hours away, a Turkish locale, a junk variable, and a
      hostile `SOURCE_DATE_EPOCH` in the parent, compared against a clean run on
      both the artifact's bytes and the suite's verdict.
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
      (`build/actions.rs:71-76`). `--explain` is in place to test it when it lands.
      *(unimplemented)*
- [x] Content-keying, not timestamps: rewriting a file with the bytes it
      already held rebuilds nothing
      (`incrementality::rewriting_a_file_with_its_own_bytes_rebuilds_nothing`).
- [x] **A test-only dependency was in no key, and the verdict cache served
      stale answers because of it.** `test { dependencies }` and
      `testing { dependencies }` are deliberately outside `Workspace::closure`
      — a test dependency is not a dependency of the thing being shipped, so it
      must not drag its tags into the tag closure or count against
      `unused-dep` — and `test_key` walked the closure. So a helper was
      compiled into the suite and hashed into nothing, and editing it left a
      passing suite passing. Fixed in `actions::test_key`, which now
      contributes each test dependency's own closure, and in `watch::inputs`,
      which mirrors the same edges;
      `incrementality::editing_a_test_only_dependency_re_runs_the_suite` asserts
      it on the *verdict* rather than on the transcript, because a `run` line
      with a stale answer behind it would satisfy a weaker test.
- [x] Cache-key composition: platform and arch, rule identity, and dependencies
      entering **as keys rather than contents**. Both altitudes, because they
      are different claims — `build/cache.rs` asserts them on the `KeyBuilder`,
      where "the platform is in the key" is a statement about the key, and four
      new `incrementality.rs` cases assert them through the CLI, where it is a
      statement about a repository. A builder that composed correctly while
      `action_key` forgot to call it would satisfy only the first.

      The arch is the half that can be moved without a second backend: a JS
      output ignores `arch` when it names the artifact's directory, so adding
      one moves every key and nothing else, which is the shape of the bug this
      rules out. Rule identity is watched by renaming a source to bytes it
      already held. "Dependencies as keys" is watched from outside as the one
      thing it implies — a dependency's key and its dependent's `link` move
      together while every dependent's own `compile` contribution stays exactly
      where it was, which is only possible if what entered was the key.
- [x] `buri build --check-reproducible` — builds twice in separate directories
      and diffs. Two freshly opened sessions, the cache off, two different
      directories; silent and exit 0 on agreement, exit 1 naming the artifact
      and the first differing byte otherwise. All three of those are load
      bearing and the doc says why: a shared session could carry a difference
      across in something memoised, a cache hit would compare an entry with
      itself, and one directory would hide the failure mode this is most likely
      to catch. `repos/hermeticity/reproducible_build` records the green path
      and that a check leaves no artifact behind.

      The red path is unit-tested on the comparison
      (`actions::two_artifacts_that_disagree_report_where`) rather than staged,
      because arranging a genuinely irreproducible build is arranging for the
      system to stop working as designed.
- [x] Toolchain `sha256` mismatch refusing to run, exit 2 — and the version
      half with it, in one place (`build/toolchain.rs`, called from
      `session::open`, so every command that opens a repository checks it and
      none of them has to remember to).

      **Removed by decision, and with it everything below.** The pin is gone
      from `REPO.buri`, from the schema, from `session::open`, from the cache
      key, and from `buri version`; `build/toolchain.rs` is deleted. The
      argument that removed it: a pin is worth its weight where a toolchain is
      *fetched*, because it is what a downloader verifies before unpacking an
      archive — and nothing fetches one. A hash that the same person who
      installed the compiler also writes into `REPO.buri` checks that they agree
      with themselves. What survives is `buri version --verbose`, which prints
      the running executable's hash so that a bug report can name one build of a
      version, and `arguments::VERSION` in every cache key, which is the whole
      of toolchain identity now.

      Kept for the record, because the reasoning was not wrong and would come
      back with a downloader: **what was hashed** was the running executable,
      which is the artifact the release archive would have contained and is the
      stricter of the two — it also catches an executable replaced after it was
      unpacked. And **a `sha256` of nothing but zeros was the sentinel for
      unpinned**, which is the state a repository is in while its compiler is
      built from source. That was the whole escape hatch: no flag and no
      environment variable, because a pin you can turn off from the command line
      is a pin that gets turned off in the one script that matters. That every
      `REPO.buri` in this repository wrote the sentinel, for the whole life of
      the feature, is most of why it is gone.

      One test did not survive the removal:
      `incrementality::changing_the_toolchain_pin_changes_every_key` moved the
      pin between *unpinned* and *pinned to this executable* to show that
      toolchain identity moves every key. A repository has nothing left to move,
      and `VERSION` is a constant compiled into the binary under test, so the
      property is now pinned on the key's composition instead
      (`cache::the_toolchain_version_is_in_every_key`, which rebuilds the key
      field by field). A comment where the test was says so; observing a
      *change* of version would take a second binary to compare against.

## REPO-CONFIG.md

- [x] A `REPO.buri` whose `toolchain.version` or `sha256` does not match the
      running toolchain — exit 2, before anything is compiled. **The pin was
      removed** (see the entry under HERMETICITY-AND-CACHING.md above), so there
      is no refusal left to record and `repos/hermeticity/toolchain_pin` is
      deleted along with `hermeticity::a_pin_on_this_executable_is_satisfied_and_a_pin_on_another_is_refused`.
      What replaces them is one unit test on the reader
      (`buildfile::a_leftover_toolchain_block_is_an_unknown_field`): a
      `REPO.buri` still carrying a `toolchain` block is a file naming a field
      that does not exist, and gets the same diagnostic any other undeclared
      field gets, with no suggestion — `tag` is nowhere near `toolchain`, and a
      suggestion that far away would read as a rename that never happened.
- [x] The closed platform enum rejecting an unknown `Platform` or `Arch` name.
      `repos/hermeticity/closed_platform_enum` records all three places one can
      be written — an output's `platform`, an output's `arch`, and a tag's
      `requires { platforms }` — because a check that fires in two of three is
      a check nobody can rely on. Only the third offers "did you mean", which
      is right: it is the list a person types rather than one `buri gen` writes.

Adequately covered: a missing `REPO.buri`
(`conformance::outside_a_repository_is_a_bad_invocation` — it cannot be a
repository case, because the point is that there is no repository), an
unparseable build file exiting 2 (`repos/cli/exit_codes`), an unknown field
with a suggestion and a duplicate `tag` name (`build/buildfile.rs` unit tests).

The exit-code contract itself is now `repos/cli/exit_codes`, which records the
*message* each of the seven invocations prints — the old assertion checked only
the number. `format --check` reporting without rewriting is
`repos/cli/format_check`, with the file's own bytes recorded before and after.

---

## Cross-cutting

- [x] **The JS backend is no longer the only one.** Cranelift builds the debug
      quadrant and LLVM the release one, `build/link.rs` is the link action, and
      `.buri/out/<platform>/<package>/<artifact>` is a path a native build
      writes to. What is exercised end-to-end is a native `build`, a native
      `run`, and a `test { platforms }` entry naming the host — the last two
      through `incrementality.rs`, which has a half for a toolchain that has no
      native backend and a half for one that does.

      **What is still unexercised** is the *example repository's* own
      `LINUX`/`MACOS` outputs: `conformance.rs` runs `lint` and `test` against
      it and never `build`, and a `buri build //...` there would today be
      refused for the intrinsics the native backend has no implementation of
      (`core/fs`, `core/env`, `json.*`, every `list.*` taking a closure). That
      refusal is the honest state and the reason the default output is still
      `JS`; see the native roadmap below.
- [ ] **`main.buri` is the only module that may import `core/host`**, and its
      context is checked against each output's platform — a `main` binding
      `Fs: host.fs` under `platform: JS` must be an unresolved name at the
      entry point (BUILD-FILES.md:236-239). *(untested)*
- [x] **Formatting preserves meaning across the whole corpus.** The comment at
      `corpus.rs:111` named `tests/format_builds.rs`, which never existed; the
      property it named is now `formatting_the_corpus_preserves_what_it_means`,
      which formats every source in a copy of the conformance repository and
      runs `buri test //...` in both copies. It compares the two runs rather
      than asserting success, so a suite failing for a reason of its own fails
      identically on both sides and the question stays "did formatting change
      the answer" — with a floor under the assertion count so that a corpus
      which compiled to nothing cannot pass. The `golden_javascript` programs
      joined the corpus at the same time; nothing held them to parsing or to
      the formatting fixed point before.
- [ ] `cli/tests/repositories/**` is not walked by `corpus.rs`, so the fixture sources
      there are not held to "every source in the repository parses" or to the
      formatting fixed point. That is deliberate — `repos/cli/format_check`
      checks in a deliberately misformatted file, and future cases will check
      in ones that must not compile — but it means a typo in a fixture is
      caught only by the case that runs it.

---

## How long the compiler takes

Nothing here was a clever optimisation. Every one of them was the compiler
doing a large amount of work that no part of it wanted done, found by sampling
the binary on the two repositories in `cli/tests/` rather than by reading for
things that looked slow — which is the point worth keeping, because the largest
one by far sat in a function whose name gives no hint that it is on any hot
path at all.

Medians over eleven runs of a `--lto` release build, one machine, timed around
the whole process:

| | before | after |
| --- | --- | --- |
| `conformance` `build //...` | 233 ms | 22 ms |
| `conformance` `lint //...` | 501 ms | 63 ms |
| `conformance` `test //...`, cold | 935 ms | 499 ms |
| `conformance` `format --check` | 71 ms | 56 ms |
| `example` `build //...` | 21 ms | 8 ms |
| `example` `lint //...` | 57 ms | 15 ms |
| `example` `test //...`, cold | 186 ms | 151 ms |
| `version --self-check` | 85 ms | 12 ms |

`test`, cold, is the one that moved least, and that is as it should be: three
fifths of it is the toolchain waiting on `node` to run the suites it just
compiled, which is not the compiler's time to save.

- [x] **`Checker::module` handed out a borrow of the checker rather than of
      the modules, and half the compiler's life went into working around it.**
      `Loaded` is behind a `&'a` the checker never writes through, so
      `self.module(id)` could always have returned `&'a ModuleData`. Returning
      `&ModuleData` instead meant every pass that walks a module's items while
      filling in a table — which is every pass — could not hold the items and
      the table at once, and the way each of them settled that was
      `self.module(id).ast.items.clone()`: a deep copy of the module's entire
      syntax tree, every body and every expression, once per pass.

      Worse, `expand_alias` did it too, and `elaborate` calls `expand_alias`
      for *every named type in the program* — so the whole tree was copied once
      per type annotation. That single line was about half the wall time of a
      build. `body_ast` in `inference.rs` was the same shape at function
      granularity: one deep copy of a function's declaration per function
      checked.

      One word in one signature, and the `.clone()`s at seven call sites
      became borrows. `conformance` `build //...` went from 233 ms to 31 ms.

- [x] **The parser ran once per target on files it had already read.** A
      command analyses one target at a time and every target pulls in the
      standard library, so `lint //...` lexed and parsed 204 files to compile
      121 distinct ones. `parser::Cache` keys the parse on `FileId`, which is
      the identity `SourceMap` already uses to decide when a file's *text* is
      re-read — so the two cannot disagree about which revision is in play —
      and the tree is shared with an `Rc` rather than copied, which nothing
      objects to because nothing mutates a tree after parsing. 128 of 204
      parses became lookups.

      The embedded standard library needed one further fix to benefit:
      `load_std` called `SourceMap::add` rather than checking for the module
      first, so every analysis got a *new* `FileId` for text compiled into the
      binary — and a process that analysed a hundred targets accumulated a
      hundred copies of the whole standard library in the map. That is
      `SourceMap::embedded` now.

- [x] **`SourceMap::find` was a linear scan, and `load` calls it for every
      file.** Loading a repository was quadratic in the number of files it has,
      which is invisible at ten and is not at a thousand. It is a `HashMap`
      now; the map is append-only, so the index costs one insert per file.

- [x] **The effect predicates scanned every `impl` in the program, per type
      node.** `is_effect_carrying` and `may_carry_effect` answer "does this
      constructor implement an effect?" once for every type-constructor node
      they walk, and they answered it by walking `impls` — every conformance in
      the compilation, standard library included. So checking one function got
      slower as the repository declared more `impl` blocks *anywhere in it*,
      whether or not that function had heard of them.

      Asked of the effects instead it is one hash lookup each, and a program
      declares a handful of effects and thousands of impls. `add_trait` is the
      only way a trait comes into existence and `is_effect` is fixed there, so
      the list cannot fall out of step. Measured against a repository of *n*
      types each deriving three traits: at *n*=800, 28 ms → 17 ms; at *n*=1600,
      90 ms → 38 ms; at *n*=3200, 327 ms → 121 ms. The old curve is quadratic
      and the gap keeps widening.

- [x] **The tables hash with a multiplicative hash rather than SipHash.**
      `std`'s default is chosen to survive an adversary who picks the keys, and
      nothing here has one — the keys are a program's own names and type ids.
      `hash.rs` is rustc's function, constants and all, at about a hundred
      lines with no dependency; rustc-hash's own test vectors are in the file,
      so it is checkable against the thing it is a copy of. Worth 7–15%
      across the front end.

      The side effect matters more than the speed: `RandomState` seeds itself
      per process, so two runs of the compiler on one input walked its tables
      in two different orders. Nothing was allowed to depend on that — artifacts
      are compared byte for byte — but that was a property maintained by care.
      With a fixed seed a mistake of that kind is at least reproducible, and
      `build --check-reproducible` can see it.

- [x] **The formatter copied a document it was about to throw away.**
      `args_doc` built the plain bracketed form *first* and returned it when
      the call was not hugging, so every call in the program paid for a copy of
      every one of its arguments' documents — and a document is the whole
      subtree. `chain` had the same shape: it built the one-line form from
      clones before asking whether the expression was a chain at all, which
      most are not. Asking first and moving the pieces took `Doc::clone` from
      63% of the formatter to 16%.

      Separately, `comment_shape` called `token_shape` and filtered the tokens
      out of the result — after allocating a `String` for each one. `source`
      does that twice, on its input and on its own output. It is a parameter
      now rather than a filter.

- [x] **`Subst::shallow` copied the type before looking at it.** Following an
      inference variable to what it stands for reads a type; it does not need
      to own one. Every step of `unify`, `resolve` and `occurs` began by
      deep-copying the type it was about to take apart, so `Result<[Str], E>`
      was copied whole at every node, twice per unification step. `shallow_ref`
      is the borrowing form, and `unify` now decides the cases that are decided
      by looking — two primitives, a variable against itself — before any copy
      happens.

### What is still slow, and why it was left

- [ ] **A command still analyses each target from scratch.** `lint //...` on
      the conformance repository calls `driver::analyze` twelve times, and each
      one re-*checks* the standard library modules that target imports. Parsing
      is shared now; checking is not, and half of what is left is that. The fix
      is interface-level incrementality — cache a package's checked surface,
      keyed on its sources and its dependencies' surfaces — which is a design
      question about what a package's interface *is*, not a performance patch,
      and is tracked as its own item elsewhere. The cost is O(targets × the
      closure each target imports), so it is a repository-size problem: it will
      be felt long before a thousand targets.

- [ ] **Exhaustiveness checking is combinatorial and has no complexity limit.**
      `expand` splits a row on every top-of-column or-pattern, and
      `expand_lengths` multiplies an array-rest pattern by the longest array
      length any arm distinguishes. Both are inherent to the usefulness
      algorithm. Measured, the growth is polynomial rather than exponential on
      every shape that could be constructed for it — reaching 90 ms took a
      match on a six-column tuple of arrays against an eighty-element literal
      array pattern, which is not a program anybody writes. It is recorded
      rather than fixed because the fix is a bail-out, and a bail-out in this
      checker means a `match` that is *not* exhaustive compiles. That is a
      decision about the language, not about how long the compiler takes.

- [ ] **The documentation harness loads the whole standard library per
      snippet.** `analyze_snippet_as` calls `load_all_std`, which defeats the
      lazy loading every other entry point gets — and `analyze_snippet`'s own
      comment says its value is being "the same `Loader` and the same `Checker`
      the compiler runs", which loading all thirty modules is not. Making it
      lazy passes the whole documentation suite and is arguably more faithful,
      but it did not move the suite's runtime at all, so it is a correctness
      question rather than a performance one and was left for whoever asks it
      as one.

- [ ] **Nothing shares work between processes.** The parse cache lives for one
      command. Two `buri lint` runs in a row re-read and re-parse everything;
      only the *action* cache in `.buri/cache` survives, and it caches
      artifacts rather than analysis.

Two things that were looked for and are not there, recorded so nobody looks
again: there is no `Mutex`, `RwLock`, `RefCell`, `Rc` or `OnceLock` on any hot
path — until the parse cache there were none in the tree at all — and
diagnostic rendering re-reads no files, because every span resolves through the
`SourceMap` that already holds the text.

---

## The standard library

Nine modules were added: `core/queue`, `core/bitset`, `core/json`, `core/map`,
`core/set`, `core/date`, `core/simd`, `core/bytes`, `core/crypto`. What they
are and why they are shaped that way is [STANDARD-LIBRARY.md](./STANDARD-LIBRARY.md);
what remains is here.

Each has a conformance package under `cli/tests/conformance/lib/` that *calls*
every exported name, because `cli/tests/standard_library.rs` stops after type checking —
a body-less declaration with no runtime function behind it passes that suite
silently. The suite went from 150 assertions to 1172.

- [x] **Allocators (`GeneralPurpose`, `Arena`, `FixedBuffer`) landed, in
      `core/alloc`, on both backends.** They were deferred here on the grounds
      that JavaScript has a garbage collector, so an `Arena` would reclaim
      nothing and a `GeneralPurpose` would report a synthetic number rather
      than a measurement.

      **The premise was half wrong, and the half that was wrong is the
      interesting one.** A count is synthetic only if it is supposed to be a
      measurement. The cost model settled below is *defined* — computed from
      the types, not from what an allocator did — so the same program charges
      the same number on both backends by construction, and there is no
      measurement for JavaScript to be missing. What made the three types real
      was not native memory; it was deciding that the number is a definition.
      Native memory made the *decision* necessary, which is a different thing
      and is why the deferral was still the right call at the time.

      What landed:

      - `core/alloc`, a non-platform module — importable anywhere, because
        `Alloc` is the one effect whose implementation carries no authority.
        `generalPurpose()`, `arena()`, `fixedBuffer(n)`, each with `stats()`
        answering `Stats { allocations, bytes }`, and `FixedBuffer` also
        answering `budget` and `remaining()`. Each carries an `I64` handle into
        a counter table, exactly as `core/testing/context` does, because Buri
        has no mutation to hold a running total with.
      - The model, written down beside `Alloc` in `core/cap` where a reader of
        the effect meets it, and evaluated by `middle::layout`'s `charge_*`.
      - A `FixedBuffer` overrun **aborts**, byte-exactly and with both numbers
        in the message. `cli/tests/crash/alloc_budget_exhausted.buri` pins it
        on JavaScript and `native_cranelift.rs` pins the same sentence
        natively.
      - `cli/tests/conformance/lib/memory/` — one file, run by the JavaScript
        conformance suite and by the native one, asserting the same integers.
        That is the payoff of a defined model, and it is the evidence.

      The two things settled while looking at it both held up:

      - **A byte-exact cost model has to be *defined*, not measured.** It is,
        and it is now a commitment: a change to any row is a breaking change to
        observable behaviour.
      - **No reserved context slot is needed.** None was added. Natively there
        is not even a scan: an allocator that counts is a non-zero-sized
        context member and arrives as an ordinary value.

      What did **not** land, with the reason, because it is the part a reader
      will look for:

      - **The list, string and closure rows are charged by definition and
        reported to no allocator.** Only `allocate(ctx, n)` is counted, on both
        backends. The note above was right that "the hook already exists" —
        every allocating intrinsic is handed the context — and wrong that
        routing it is free. It is not the *signature* that is in the way, it is
        that neither backend can compute the charge where the allocation
        happens: `runtime.js` is untyped and `16 + n * stride(T)` needs
        `stride(T)`, and natively `cranelift/runtime.rs` drops a context
        argument from every `buri_rt_*` call by ABI. Widening the counted set
        has to happen on both at once or the numbers stop agreeing, which is
        the property the module exists to have. `core/alloc` states the
        boundary and the conformance file pins it.
      - **`Arena` does not free in bulk.** It is a separate counter and says
        so. What would make it real is a scoped context — a language proposal,
        not a backend feature (MEMORY.md §4, §7.2).
      - **`listBytes(n, stride)` takes its stride as an argument**, because the
        language has no `sizeOf<T>()` for a program to ask with. A
        `sizeOf<T>()` would make it `listBytes<T>(n)`.

- [x] **`core/json` has a typed encoding.** `derive ToJson for Point;` and
      `derive FromJson for Point;` are on the derivable list, and they needed no
      new language machinery: the backend already ships **descriptors** —
      `[kind, ...]` with `2 struct` carrying field names and `3 enum` carrying
      variant shapes — so `$json_of(v, d)` and `$json_into(j, d)` in
      `runtime.js` are `$show`'s walk with a different destination. One walker
      for the whole program, not one encoder per type. It is also the substrate
      the `.proto` roadmap below wants.

      **The mapping is a decision, so it is written down** — in `core/json`'s
      own source, where a reader of the module meets it, rather than only here.
      Numbers, booleans and strings are the obvious thing; `Char` is a
      one-character string; `()` is `null`; a list *and a tuple* are arrays,
      because a tuple is positional data. The three that had a real choice in
      them:

      - **`Option<T>` is `T`'s encoding, or `null`.** The cost is that
        `Option<Option<T>>` does not round-trip: JSON has one null, so
        `.Some(.None)` and `.None` both write it and both read back `.None`.
        The alternative is a wrapper object around every optional field, which
        is a worse document to make every reader of it pay for.
      - **A positional struct is an array, a one-field one included.**
        Transparency for a newtype was tempting and is wrong: it would make the
        wire format depend on how many fields the type happens to have today,
        so adding a second field would silently change every document already
        written.
      - **An enum is externally tagged** — a variant with no fields is its own
        name as a string, one with named fields is `{"Name": {...}}`, one with
        positional fields is `{"Name": [...]}`. `{"tag": "Name", ...}` reserves
        a key, and a struct variant with a field called `tag` then collides
        with it silently. Externally tagged reserves nothing.

      **Both traits are derived and never written by hand**, which is enforced
      rather than documented. What a derived implementation stands for is the
      type's *shape*, which is what the descriptor carries — so a hand-written
      `impl ToJson for Date` would be called where a `Date` is encoded on its
      own and silently skipped where a `Point` holding one is. An `impl` of
      either is rejected at the `impl`, and no diagnostic anywhere offers
      writing one as the fix.

      Two smaller things worth knowing:

      - **The fold bottoms out in `semantics::builtins`.** `derive ToJson for
        Point` asks whether `Int` satisfies `ToJson`, so the primitives need
        their implementations in the table — and `core/json` loads on import,
        so they are registered exactly when a program could have named either
        trait, and not at all otherwise. `Option` gets one too, marked
        *derived*, so `satisfies` recurses into the payload rather than waving
        it through.
      - **Recursion terminates for the same reason `derive Eq` does.** The
        Rose-shape guard in `infer::satisfies` — a constructor already on the
        walk answers `true` rather than asking again — is what makes
        `derive ToJson for Rose` a fold rather than a hang, and the conformance
        suite recurses through a list *and* through an `Option` of a list to
        say so.

      `json.encode(ctx, x)` is an ordinary Buri function — `value.toJson(ctx)`
      — so it dispatches like any other trait call. `json.decode(ctx, doc)`
      cannot: its trait method takes no `self`, so it is reached through the
      type asked for, the way `num.maxValue<U8>()` reaches `Bounded`. That
      also means **`decode` takes its type from the annotation**: it is generic
      in the type and in the context, a type argument list must name every
      argument, and
      a context type has no name to write (SPEC 11.3), so there is nothing to
      put in the second slot.

- [x] **Examples in `///` and `//!` comments are compiled and run.** The
      doctest engine already handled prose pages; the missing half was source
      files. `doctest::doc_comments` turns a `.buri` file into a markdown
      document **with the source's own line numbers** — each doc line at its
      own line, everything else blank — so a block's origin is already the
      `.buri` line and there is no map to build, carry, or get wrong. The blank
      lines between doc runs are also what separates one comment's prose from
      the next's, which is what markdown wants anyway.

      Two consequences worth knowing:

      - **`parsing/lexer.rs` was flattening them.** A doc line was `raw[3..].trim()`,
        which strips the indentation a fenced block inside a comment depends
        on. It now strips one leading space and trims the end, which is the
        separator coming off rather than the content.
      - **`buri docs test` walks `.buri` files too**, skipping `BUILD.buri` and
        `REPO.buri` (textproto) and any source with no fence at all, which is a
        byte scan.

      Nine executable examples now live in standard library doc comments —
      `queue`, `json`, `map`, `set`, `bitset`, `date`, `simd`, `bytes`,
      `crypto` — each compiled, run, and its output compared, by
      `doctest::standard_library_doc_comments`.

- [x] **The standard library loads lazily.** `load_unit` used to load all
      twenty-nine `core` modules on every command. It now loads
      `stdlib::EAGER_MODULES` — the prelude plus the defining module of every
      built-in type — and everything else arrives on import. `buri lint //...`
      over the example repository went from 0.25s to 0.18s, and the saving
      grows with the library rather than with the program.

      The rule that makes it safe, enforced rather than reviewed: **a lazily
      loaded module may not declare a method on a built-in type.** A method
      needs no import, so `impl [U8]` in `core/bytes` would simply not resolve
      in a program that never imported `core/bytes` — and the error would name
      the call site rather than the cause.
      `stdlib::a_lazily_loaded_module_declares_no_method_on_a_built_in_type`
      fails on it, and `every_module_checks_on_its_own` checks each module the
      way a program reaches it: on top of the eager set and its own imports,
      rather than alongside all twenty-eight others.

- [x] **A monomorphized symbol's hash cannot be name-based, and now says so.**
      Lazy loading moved a symbol in `golden_js` for a program whose source had
      not changed, which is untidy: `mono::name_of` hashes
      `format!("{targs:?}")`, and a `Ty` carries `TyConId`s — indices into a
      table whose contents depend on what the compilation loaded.

      The obvious repair is to hash `types::show` instead — names, not indices.
      **It is wrong, and it miscompiles.** `compiler/semantics/types.rs` renders *every* context
      type as the literal `a context`, because a context type is generated and
      has no name (SPEC 11.3). Two generics instantiated over different contexts
      collide onto one symbol and one body silently replaces the other; the
      conformance suite caught it as a program calling the wrong `Fs`
      implementation. Rendering a context by the effects it binds does not help
      either: two contexts binding the same effects to different implementations
      are still different types, which is exactly what `Ty::Ctx(x) == Ty::Ctx(y)`
      means. The index *is* the identity.

      So the code is unchanged and the comment above `short_hash` — which
      claimed symbols were "derived from labels and module paths rather than
      from compilation order" — now says what is true and why it has to be.
      `golden_js::generics_over_different_contexts_do_not_share_a_symbol` pins
      the invariant, and fails if anyone tries the tidy version again.

      A symbol moving when the toolchain changes what it loads is a real
      wrinkle, and the honest answer is that `golden_js` re-records. Anything
      better needs a name for a context type, which is a language change.

- [x] **`core/list` has `foldResultCtx`.** `foldResult`'s function takes no
      context and a lambda may not capture one, so a fallible fold that
      allocates as it goes could not be written. `core/bytes::fromHex` is the
      one that wanted it and now uses it, which also let its error carry the
      index of the pair that failed rather than of the whole string.



---

## The JavaScript backend

A pass over `backend/` and `middle/` (then `transform/`) informed by reading how ReScript,
js_of_ocaml, Gleam, PureScript and Elm compile the same constructs. Most of what
those compilers do, Buri already did — currying it never had, whole-program
inlining and constant sharing it does better, and mutual tail recursion it
handles where ReScript does not. What the comparison was actually worth was two
miscompiles, both found by writing the program the other compilers' bug trackers
describe and running it.

- [x] **A closure built inside a tail loop captured the slot, not the value.**
      The loop rebinds its parameters in place, which is exact for every read
      inside the iteration that wrote them — and wrong for a closure, which
      outlives the iteration and reads whatever the loop stopped at. A
      four-iteration loop collecting `fn(x) => x + i` returned four copies of
      `+4`. This is Elm's [#2268](https://github.com/elm/compiler/issues/2268),
      which has been open since 2016 and shares Buri's design; ReScript and
      Gleam avoid it by giving each iteration its own binding. `generate::
      snapshot_captures` now does the same, for the slots a closure actually
      captures and no others — a loop with no closure in it keeps the tighter
      output `rebind` already produced, and `tail_self`/`tail_mutual` did not
      move. The failure mode is what makes it worth the note: it appears and
      disappears on edits that have nothing to do with it, because anything
      that defeats the tail-call elimination also fixes the answer. A golden
      file records the right output until someone makes an unrelated change.
- [x] **A tail call under `&&`, `||` or `??` ran on the JavaScript stack.**
      `a && f(x)` *is* `f(x)` when `a` holds, so the right operand of a
      short-circuiting operator is in tail position — which is to say `all`,
      `any` and a linear search, the recursions an immutable language writes
      most often. `&&` and `||` were invisible to the analysis; `??` was
      counted by the analysis and not handled by the emitter, so it got a
      `while (true)` nothing ever continued, which looks like elimination and
      is not. All three overflowed at two million deep. The reason this
      survived is worth keeping: `buri test` runs on Bun, and JavaScriptCore is
      the one engine that implements proper tail calls, so every one of these
      passed. `tail_calls_run_in_constant_stack_on_v8` runs the same artifact
      under node and now covers all three operators.
- [x] **The runtime stopped paying for shapes it does not have.** `&`, `|`, `^`
      and `~` at `Int` built two BigInts and converted back on every call, when
      almost every operand fits in a signed 32-bit integer where JavaScript's
      own operator is exact — 5× on the operation, and the fast path measures
      identical to a native `&`. `str.len()` materialised an array of every
      scalar in the string, when a string with no surrogate in it has one
      scalar per code unit — 95×, and the `charAt` scan that was quadratic with
      an allocation per step is now quadratic with a hundredth of the constant.
      `$hash` and `$show` allocated arrow functions per call to close over an
      accumulator, which is now threaded as an argument. None of this changed a
      single byte of generated code.
- [x] **`derive Eq` compiles at the type instead of walking a descriptor.** A
      two-field struct is `a[0]===b[0]&&a[1]===b[1]`, where it used to be a
      runtime walker asking `typeof` of every element it reached — the only
      megamorphic call site in an artifact. 2.6× on a struct, 3.5× on an enum.
      `Option`'s nesting is carried by a box only the runtime knows how to
      read, so an `Option` keeps the walker; so does an opaque type. The
      identity shortcut is kept deliberately rather than optimized away,
      because it is observable: a struct holding `NaN` is equal to *itself* and
      not to a copy, and both of those are the right answer to different
      questions.
- [x] **Four smaller things.** Local cleanup now descends into lambda bodies,
      so an inlined callee's aliases stop surviving inside every closure. A
      local array literal whose every use is a constant-index read is read
      through to those uses, which removes the intermediate array a functional
      update leaves behind — `structs` lost 43% of its generated code. A
      guarded match whose arms all `return` no longer carries a `while (true)`
      no `continue` can reach. And `x | 0`, `x & x`, `x ^ x` and their
      relatives fold where the width is known, which is where they have to
      fold: `x | 0` is the identity at 64 bits and a truncation in JavaScript.
- [ ] **A decision tree for nested patterns.** `arm_chain` emits one `if` per
      arm carrying the arm's whole test, so a match on a nested pattern
      re-tests the outer tag on every arm and `to_switch` cannot rescue it —
      it needs every test in the chain to be a bare `disc === lit`. Measured
      1.75× on a match with eight outer constructors by four inner, and a size
      win that grows with the arity. Left undone deliberately: it reorders
      tests, and Buri's match is first-match-wins, so it is the one item on
      this list where a mistake is a miscompile rather than a slower program.
      It wants to land on its own, against `release_and_debug_agree` and the
      whole conformance corpus, and the sound version is a real column-based
      decision tree in `generate.rs` rather than a regrouping of already-
      emitted JavaScript.
- [ ] **Nothing measures how large one emitted function gets.** V8 never
      optimizes a function past 61,440 bytecodes, and a whole-program compiler
      has three ways to grow one quietly: the inliner's per-caller ceiling
      compounds over its rounds, a merged tail-call group fuses an entire
      mutually recursive component, and `main` accumulates every single-use
      body inlined into it. `sizes.txt` now records the largest function in the
      corpus — 843 bytes, which is nowhere near — and the harness fails past
      32,768. That is a tripwire, not a measurement of a real program; what is
      missing is the same number for the worked monorepo, where the inliner has
      something to work with.

Three things the comparison suggested and measurement rejected, recorded so
they are not tried again: `$str` of a `Bool` through `String` (no difference —
V8 inlines the existing short-circuit), splitting `Int` into a hi/lo `Int32`
pair (0.97 ns/op against 0.48 for a plain `Number` add, and only reachable with
a scalar-replacement pass Buri does not have), and `[]` with `push` instead of
`new Array(n)` with an index fill in `$list_map` (13.8 ms against 23.5 — the
antipattern the general advice warns about is `new Array(n).fill(0)`, which is
not what this does).

---

## Roadmap: the one feature not started

A design note. The `.proto` half of what used to be two items landed — it is
recorded below the line, decisions and all — so what is left is the native
backend, which has a prerequisite that is cheap now and expensive later.

### Native macOS and Linux executables

Most of this landed, in the waves `design/native/ARCHITECTURE.md` §8 planned.
`build/actions.rs` builds a native output where this toolchain can — a backend
compiled in for the target and profile, a runtime archive for the host, and a
linker — and refuses in the old words where it cannot;
`driver::host_platform()` answers the host's own platform under exactly that
condition; `buri run` executes a native artifact directly, and a `test {
platforms }` entry naming a platform this toolchain can build for is compiled
and run as a native binary.

What has *not* flipped is the default. A binary that declares no outputs still
gets `JS`, and a suite that names no platforms still runs on JavaScript, because
the native runtime surface is not complete: a program using `core/fs`,
`core/env`, `json.*`, or any `list.*` entry taking a closure is refused by
`Backend::missing_intrinsics` rather than mis-run, which is the right refusal
and the wrong default. The trigger for flipping it is that refusal going quiet
across the conformance corpus — at which point `selected_outputs`' fallback
(`build/actions.rs`) and `run_suite`'s (`commands/test.rs`) are one line each.

1. **[landed, wave 0]** Make the backend an interface before writing a second one.
   `compiler/backend/js/generate.rs` and `js/javascript.rs` were entangled with `monomorphize::Program`. Extract
   `trait Backend { fn emit(&Program, &Tables, &Options) -> Result<Vec<u8>, Diagnostics> }`
   and make JS the first implementor — amended to `Vec<Emitted>` so an
   incremental link is representable. See `design/native/ARCHITECTURE.md` §3.
2. **[landed, waves 1b and 3c]** **The value model changes, and it is
   language-visible.** `middle/layout.rs` is the native one — sized integers,
   tagged unions, a struct layout — and `design/native/VALUE-MODEL.md` is the
   argument for it. The SPEC amendment it needed is in: §6.2 now describes
   *both* backends rather than one, §6.2.2 says a `Checked` method's second
   bound is the backend's, and §6.2 promises that a float renders as the
   shortest decimal that round-trips on every backend.

   **What the amendment does not do is fix JavaScript**, and that is deliberate
   rather than pending. The JS backend still has the 2^53 ceiling: making the
   two agree everywhere means `BigInt` for every integer type, which taxes every
   loop counter in every program for a case most never reach (`SPEC.md` §13,
   open question 8, is where that trade is argued). So the differences that
   remain are **documented divergences**, listed in
   `design/native/VALUE-MODEL.md` §12.

   **[landed, wave 4b]** The follow-up was that file's own promise: one test
   that runs a corpus under both backends and holds every row of §12 to "must
   agree" or to an explicit divergence entry. `cli/tests/backend_agreement.rs`
   is it — one row per test, each source compiled through `actions::prepare` and
   `backend::select` twice and compared byte for byte, plus
   `every_row_of_the_table_names_a_test_that_exists`, which reads the table and
   fails on a row whose test is missing. **The table was a claim, and four rows
   of it were wrong:**

   - Row 3 was false. `wrappingMul` was not exact on JavaScript at any width
     where the product leaves 2^53 — `$wrapTo` wrapped a double that had already
     been rounded, so `U32.wrappingMul(0xffffffff, 0xffffffff)` answered 0 where
     the answer is 1. That is a wrong answer at 32 bits rather than a precision
     ceiling, from the surface a checksum is written with. `$wrapOp` now does
     the arithmetic in `BigInt` at the widths where the intermediate can leave
     the exact range, and nowhere else.
   - Rows 2 and 5 describe divergences the toolchain does not have. Both
     backends answer `.None` above the exact range — the native ones narrow the
     bound on purpose, because `conformance/lib/numbers/test/integers.buri`
     states `.None` as a property of the language — and `.Some(.None)` has been
     distinct from `.None` on JavaScript since `$some`/`$val` grew their depth
     counter. **§11.3 and `SPEC.md` §6.2.2 still say `(1 << 60).checkedAdd(1)`
     is `.Some` natively, and that sentence is now the thing that is wrong.**
     Fixing it is an edit to `cli/src/docs/lang/expressions.md`, which is SPEC
     text and is left for whoever owns the amendment — the alternative, making
     the backends match the sentence, breaks the conformance corpus.
   - Two miscompiles fell out of writing it, both native-only and both silent on
     JavaScript. `middle/lower.rs` interned `Str` and `Template` as two types,
     so a `match` whose arms are a literal and an interpolation did not verify —
     VALUE-MODEL.md §3.3 says the two *are* one type. `middle/tail_calls.rs`
     labelled a merged tail-call group's forwarders `()`, so a mutually
     recursive `Bool` came back as nothing: `even(3)` printed the empty string,
     and used as a condition it panicked inside Cranelift.
3. **[landed]** **Memory.** This said native either ships a GC or does escape
   analysis with an arena per `Alloc` scope; `design/native/MEMORY.md` argues
   both are wrong and the answer is non-atomic reference counting with static
   elision, which is what shipped. `Region` did *not* become load-bearing —
   nothing reads one, on either backend — and the allocator work above stopped
   being decorative for a different reason than this predicted: not because
   there is real memory under it, but because the charge became a definition
   the two backends share.
4. **`core/host` per platform**, and `check_intrinsics` generalized so
   "missing intrinsic" is a question asked per backend — `Backend::missing_intrinsics`, wave 0.
5. **[landed, wave 2c]** **A real `link` action per `Output`**, the
   `.buri/out/<platform>/<package>/<artifact>` layout, `artifact_name`, and
   `--output=linux/x86_64` selection. The layout is exercised now rather than
   specified: `.buri/out/macos/cmd/app/app` is a file a native build writes.
6. **[landed, wave 2a]** **A cross-compilation story, or an explicit refusal.**
   It is a refusal, and it is about the runtime archive rather than the
   backends: `cli/build.rs` builds `libburi_rt.a` for the host and for nothing
   else, so `--output=linux/x86_64` on a macOS host is refused
   (`ARCHITECTURE.md` §9). The fix, when someone wants it, is prebuilt runtime
   archives per triple — a packaging problem, not a compiler one.

Prerequisite: the interface-level incremental caching gap described under
HERMETICITY above. `driver::analyze` is whole-closure, and a native build is
slow enough that whole-closure recompiles become the thing everyone complains
about first.

---

## The `.proto` import, landed

Written down here rather than only in the code, because most of it was a
decision and a decision that lives in one function is a decision nobody can
find.

`cli/src/docs/schema/build.proto` wrote the intended surface in its own header,
and that is the surface: `from "//proto/foo.proto" import ...`, the module path
being the schema's own path with its extension on it. One spelling, it is the
file's name, and nothing lands in the source tree — no `_pb.buri` to check in
and no step to forget to run. The mapping and the reasons are
`cli/src/docs/build/proto.md` (`buri docs build/proto`); what follows is the
short form, and the decisions that were decisions rather than transcriptions.

1. **`build/protoschema.rs`** reads schemas, as `build/textproto.rs` reads
   values. proto3 only. `service`, `extend`, `extensions`, `group`, `map<>`,
   `Any`, `required`, `import public` and proto2 are refused **by name**, each
   with the reason and the edit, under `proto-unsupported` — "unsupported"
   without a noun is a message nobody can act on. `option` and `reserved` are
   skipped rather than refused: neither says anything about the shape of a
   message.
2. **`build/protogen.rs`** turns one schema into one module, and
   `Loader::load_proto` puts it through `load_source_in` — the same seam the
   documentation harness compiles a fenced block through. So the real parser
   and the real checker see the generated text, and a mistake in the generator
   is a loud compile error rather than a quiet wrong program.
3. **The mapping, decided.** `optional T` -> `Option<T>`, `repeated T` ->
   `[T]`, `oneof` -> an enum held as an `Option`, nesting flattened with an
   underscore (`Everything_Note`) because Buri has no nested type namespace and
   a bare `Note` would hide where it came from.

   The one the roadmap flagged: **a proto3 singular scalar is `T`, not
   `Option<T>`.** The wire format cannot tell "absent" from "set to zero" — a
   writer omits the default and a reader substitutes it — so an `Option` would
   invent a distinction the bytes do not carry, and `.None` and `.Some(0)`
   would encode identically with one of them not surviving the round trip. A
   singular *message* field is `Option<T>` regardless, which is proto3's own
   exception: there is no default message for an absent one to mean.

   An unrecognised enum number decodes to the zero value rather than failing.
   That is a real loss — proto3 asks a reader to keep the number, and a Buri
   enum has nowhere to keep it — but the alternative makes adding an enum value
   break every reader built before it, which is the thing the rule exists to
   prevent.
4. **Codecs: generated Buri, not a descriptor walk.** The walk `$json_of` does
   was the obvious thing to reuse and it does not fit — the descriptor carries
   field names and variant shapes, and a protobuf message is field *numbers*
   and wire *types*. A runtime walk would have needed a second descriptor
   emitted beside the first, at which point generating Buri is strictly better:
   the codec is checked by the real checker, optimised by the real optimiser,
   and needs no new intrinsic. What is common lives in `core/proto`; the
   varints and zigzag live in `core/bytes` beside hex and base64, doing 64-bit
   arithmetic on two 32-bit halves so a negative `int64` writes the ten bytes
   protoc writes. Four new intrinsics carry the IEEE 754 byte patterns, for the
   same reason `toUtf8` is one.

   JSON is proto3's mapping, not `derive ToJson`'s, and the four differences
   are why it had to be generated too: a 64-bit integer is a string, `bytes` is
   base64, an enum is its value's name, and a `oneof`'s case is an ordinary
   member of the object. One deviation, recorded rather than hidden: members go
   out in schema order with a oneof's case last rather than in field-number
   order, which no conforming reader can notice.
5. **Build integration.** `proto_sources` on both rules, `buri gen` placing a
   schema by the same question it places a source, `undeclared-source` naming
   `proto_sources` in its fix, an `Action::Proto` keyed on the schema's
   contents and reported by `--explain`, and the generated module belonging to
   the declaring rule — so it is internal to it, and `lib.buri` decides which
   of its names leave the library. `unused-import` and `unreachable-export`
   step around a generated module, because both ask a person to make an edit
   and there is no file here to edit. There is deliberately no
   `unused-proto-source`: `gen` writes the field from what is on disk, so a
   lint asking you to remove an entry would be a lint fighting the tool that
   put it there.

Pinned by `conformance/lib/proto` — every field kind, nesting, a oneof, two
schemas importing each other, the wire bytes as hex goldens checked by hand
against the encoding rules, the JSON as document text, unknown-field skip,
and every way a message can fail to be one — and by `repositories/proto`,
which is the build-file half and the two refusal corpora.

Not supported, and not by accident: services, extensions, groups, maps, `Any`,
`required`, public imports, proto2. A `uint64` past 2^53 survives only to the
precision an `Int` has, which is the caveat every double-backed implementation
carries.

### And then measured against protobuf's own conformance suite

Which is the part worth having. `cli/tests/proto/` vendors `conformance.proto`
and a pruned `test_messages_proto3.proto` from protobuf v35.1, builds a testee
that is a Buri binary speaking the runner's length-prefixed protocol, and hands
it to the real C++ `conformance_test_runner`:

```text
CONFORMANCE SUITE PASSED: 988 successes, 1314 skipped, 456 expected failures, 0 unexpected failures.
```

`./run.sh` drives it and prints the recipe for building a runner when one is not
on PATH — nixpkgs does not package it, so `flake.nix` carries the tools to build
one instead. It is deliberately outside `cargo test`, the way
`editors/tree-sitter-buri/check.sh` is; `cli/tests/proto_vectors.rs` replays 163
recorded exchanges through the same testee under cargo, so the pipeline is
covered hermetically even where the runner is not available.

The pruning is the honest part and is stated in three places — the vendored
file's own banner, `cli/tests/proto/README.md`, and every affected test in
`failure_list.txt`. `map<>` and the well-known types were deleted from the copy
rather than skipped by the reader, because a reader that silently drops a
construct is a schema that means something other than what it says. 382 of the
456 expected failures are that pruning; 34 are the `Int`-is-a-double ceiling; the
remaining six are unknown-field retention, unrecognised enum numbers, and two
`core/json` strictness gaps, each named in the list.

**Six real defects, which is what the exercise was for.** Every one is fixed:

1. **A 32-bit field did not truncate.** A `uint32` can arrive carrying 2^33 and
   protobuf reads the low 32 bits; Buri kept the whole number and disagreed with
   every implementation there is. `bytes.readVarint32` reads the low half *as
   such*, because a 64-bit varint has already rounded by the time an `Int` could
   be masked.
2. **A tag was read as 32 bits.** A five-byte varint carries more than 32, and
   the low half of one naming field 2147483649 names field 1 — so a message
   decoded as a *different message* instead of being refused.
3. **NaN and the infinities were written as bare words**, which is not JSON.
4. **A singular message field arriving twice replaced rather than merged**,
   which is what makes a message splittable across two encodings of itself.
5. **`core/json` had no `\uXXXX`** — surrogate pairs included — wrote control
   characters raw into strings, and was missing `\b` and `\f`.
6. **JSON numbers went unchecked**: out-of-range values, leading spaces and
   trailing ones were all accepted, where proto3 JSON rejects a value the field
   cannot hold rather than truncating it.

Two more came out of writing the testee rather than running it: `[packed=false]`
was ignored, and a field's own schema name was not accepted as a JSON key beside
its camelCase one.

### Then required Protobuf Editions, and edition 2026 only

Which changed the headline of the mapping. `syntax = "proto3"` is refused, and
so is proto2, and so is an older edition — `proto-edition`, with the migration
in the `fix`. `REQUIRED_EDITION` in `build/protoschema.rs` is one constant.

**Ground truth from protobuf v35.1, because the ruling asked for it first.**
`EDITION_2026 = 1002` is in `descriptor.proto`'s `Edition` enum — it is real —
but protoc v35.1 *refuses* a file declaring it: "Edition 2026 is later than the
maximum supported edition 2024". So the schemas here declare an edition no
toolchain will compile yet, which costs nothing: the conformance runner never
reads them, and protobuf's own resolution rule gives 2026 a fully determined
feature set from `descriptor.proto` alone. Every wire- and JSON-affecting
feature resolves identically at 2023, 2024 and 2026 — EXPLICIT, OPEN, PACKED,
VERIFY, LENGTH_PREFIXED, ALLOW — and the only defaults introduced since 2023
are two source-retention lints. That is why one constant is the whole of the
requirement.

**Presence flipped, and it is the point.** A singular field is `Option<T>` now:
editions made presence the default and deleted the `optional` label that used to
ask for it. `.None` and `.Some(0)` are two different messages and both survive a
round trip, which under proto3 they could not. `features.field_presence =
IMPLICIT` asks for the old behaviour per file, per message or per field, and
gives back the bare `T`. `LEGACY_REQUIRED` is refused by name.

**Enums are open, so unknown values are kept.** Every generated enum has an
`Unrecognized(Int)` variant, and a number the schema does not name round-trips
through both formats rather than collapsing to the zero value — which is what
the old proto3 mapping did and lost information doing. JSON writes it as a
number, which is what the mapping says. That deleted a whole entry from the
conformance failure list. `CLOSED` is refused: an unrecognised value would
become an unknown *field*, and a generated struct has nowhere to keep one.

`repeated_field_encoding` is honoured both ways — PACKED by default, EXPANDED by
name, and a reader takes either. `message_encoding = DELIMITED`,
`utf8_validation = NONE` and `json_format = LEGACY_BEST_EFFORT` are refused by
name, as is the `option features = { ... }` block form; `enforce_naming_style`
and `default_symbol_visibility` are read past, because they are lints rather
than wire format.

The conformance suite came back **970 succeeded, 1314 skipped, 456 expected to
fail, 0 unexpected** — eighteen fewer successes than under proto3, every one of
them a `Recommended` warning rather than a failure, and all of them the same
thing: the reference implementation is proto3 and the schema under test is
editions, so they disagree about whether a field set to its zero value gets
written. Both are right about their own schema, and it has its own heading in
the failure list. The open-enum change fixed one test outright.

**Two structural defects in the reader, found by an audit and fixed here.**
Both were the same shape — a wrong state that was diagnosed and then used
anyway:

- A `repeated` case inside a `oneof` was reported and then pushed into the
  oneof regardless, so a codec was generated for a field protobuf has no
  meaning for. The fix is not a better diagnostic: a oneof case is now its own
  type, `OneofCase`, with no label field to hold one. The refusal happens at the
  single point a `Field` becomes a case, and the case is still *kept* — the
  error already fails the build, and dropping it would take a second round of
  diagnostics about the same field away from whoever has to fix it.
- `Table::add` used `or_insert`, so two schemas claiming one name were resolved
  by whichever was read first. The two ways that happens are not the same
  mistake and are no longer treated as one: a **fully-qualified** name declared
  twice is `proto-duplicate-type`, reported whether or not anything uses it and
  naming both files; a **short** name two packages both use is ordinary
  ambiguity, so it is recorded and reported as `proto-ambiguous-type` at the
  field that reaches through it, with the qualified name as the fix. A schema
  that says which one it means still compiles, which is what keeps the vendored
  conformance files working.

The testee also needed something the language did not have: **binary standard
input and output**. `Stdin.readLine` reads the stream to its end, so nothing
written with it can answer a request before the other side stops speaking.
`Stdin.readBytes` and `Stdout.writeBytes` are the addition — two effect methods,
two intrinsics, and an in-memory pair in `core/testing/context` — and they are a
capability the language wanted anyway; a conformance harness is only what asked
for it first.
