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

      Fixed in `codegen.rs::prim_op`: above 32 bits the operation goes through
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
- [ ] `buri clean --outputs` dropping `.buri/out` only. *(untested — `clean` is
      called for setup in several tests, never asserted)*
- [ ] `buri version` printing the toolchain version and the `REPO.buri` pin.
      *(untested)*
- [x] `buri lsp` — implemented, and recorded as three sessions in
      `repos/lsp/`. `diagnostics`, `hover`, `definition`, `documentSymbol`,
      `formatting`, and completion in the two places that need no type
      information: inside a module path, and inside an import's `{ … }`.
      The case harness grew a `run { stdin: "session.jsonl" }` step for it —
      one JSON request per line, the harness frames them and records the
      decoded responses, so the golden is about what was said rather than how
      many bytes it took.

      Two notes on how it was built rather than what it does:

      - **The overlay for unsaved buffers needed no change to `compile.rs`.**
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
      `editors/tree-sitter-buri/check.sh` parses every `.buri` source in the
      repository with zero `ERROR` and zero `MISSING` nodes, and compiles every
      highlight query. It is not a `cargo test` — it needs the tree-sitter CLI,
      and the toolchain may not depend on an external tool — so
      `corpus.rs::the_editor_integration_is_whole` checks the files are all
      still there and that the queries have exactly one copy.

      **`grammar.ebnf` is stale, found by transliterating it.** Two productions
      describe a language the compiler does not accept:

      - `ImplDecl ::= "impl" ... "{" FnDecl* "}"` — but a method of the type's
        own may be exported, and every `impl` in the standard library and in
        `cli/tests/example` writes `export fn`. `parse.rs:750` reads it.
      - `FnDecl ::= ... Block` — but a declaration may have no body. That is how
        the standard library declares the primitives the runtime supplies
        (`export fn len(self: Str): Int;`, `std/str.buri:18`) and how a trait
        states a method. The EBNF splits the second case out as `MethodSig` and
        does not admit the first at all.

      The tree-sitter grammar accepts what the compiler accepts. The EBNF
      should be corrected to match. *(the grammar is normative, so this is a
      real defect in it, not in the parser)*

- [ ] **`cli/src/docs/grammar.ebnf` and the tree-sitter grammar can drift.**
      `check.sh` proves the tree-sitter grammar accepts the corpus; nothing
      proves the EBNF does, because nothing executes the EBNF. The two are kept
      in step by reading. *(unimplemented, and possibly not worth implementing)*

- [ ] **`editors/zed/extension.toml` pins a placeholder commit.** The grammar
      has to be published as its own git repository before the extension can be
      installed as anything other than a dev extension. *(unimplemented)*

- [ ] No-argument invocation operating on the package containing the working
      directory. *(untested)*
- [ ] No-argument invocation operating on the package containing the working
      directory. *(untested)*
- [ ] "All commands are safe to run concurrently; a file lock serializes cache
      writes" (CLI.md:25). *(untested)*
- [ ] The `out/` convenience symlink pointing at the most recent build.
      *(untested)*
- [ ] `--output=linux/x86_64` selecting one of several outputs. *(untested;
      see the native-backend gap below)*
- [x] **The lint catalogue is complete.** Build-graph rules:
      `undeclared-source`, `duplicate-source`, `missing-dep`, `unused-dep`,
      `dep-cycle`, `platform-violation`, `visibility-violation`,
      `tag-violation`, `unknown-tag`. Style and hygiene rules — new, each with
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
        warning on `let _ = <Result>`, which `infer_expr.rs` already makes a
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
      and none inside either. Unit-tested in `format.rs`: the order is total,
      it is a fixed point, a comment travels with the import it was written
      above, and only the *leading* run moves (an import written after a
      declaration stays put, because moving it across the declaration could
      change what the module means).

- [ ] **`buri format` never wraps a long import clause.** `WIDTH = 88` exists
      (`format.rs:13`) and is applied to other constructs, but `Item::Import`
      prints `from "…" import { … };` on one line whatever its length. A
      35-name import in `conformance/lib/semantics/test/generics.buri` was
      hand-wrapped across six lines; formatting it collapses it to a
      292-column line. Found by running `lint --fix` over the suite and
      checking whether the result was the formatter's own output — it was, so
      this is the formatter's gap rather than the fixer's. *(unimplemented)*

- [ ] **`buri format` silently deletes comments inside function bodies.**

      ```
      export fn f(): Int {
        // this line does not survive
        let x = 1;
        x
      }
      ```

      `leading_comments` (`format.rs:46`) keys trivia by the byte offset of a
      *declaration*, and `emit_trivia` is called only from `module` and the
      declaration printers — nothing puts a comment back inside a block. This
      is why no `.buri` file in the repository is actually formatted: running
      `buri format` over the corpus destroys it. Found by running it.

      `corpus.rs`'s `formatting_is_a_fixed_point` cannot catch this, because it
      checks `format(format(x)) == format(x)` and both sides have already lost
      the comments. The property that would catch it is the one `token_shape`
      (`format.rs`) was built for, extended to comment trivia — or, more
      simply: that the checked-in corpus is formatted. Neither holds today.
      *(unimplemented)*

- [x] **Every emitted code is documented, and a test says so.**
      `doc_errors.rs` had named this test since it was written and it did not
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

---

## The standard library

Nine modules were added: `core/queue`, `core/bitset`, `core/json`, `core/map`,
`core/set`, `core/date`, `core/simd`, `core/bytes`, `core/crypto`. What they
are and why they are shaped that way is [STANDARD-LIBRARY.md](./STANDARD-LIBRARY.md);
what remains is here.

Each has a conformance package under `cli/tests/conformance/lib/` that *calls*
every exported name, because `cli/tests/stdlib.rs` stops after type checking —
a body-less declaration with no runtime function behind it passes that suite
silently. The suite went from 150 assertions to 1172.

- [ ] **Allocators (`GeneralPurpose`, `Arena`, `FixedBuffer`) wait for the
      native backend, deliberately.** `core/cap` declares
      `effect Alloc { fn allocate(self, bytes: Int): Region }` and
      `$host_HostAlloc_allocate` is `return [Number(n)]` — nothing reads the
      `Region` and nothing counts.

      Accounting was designed and then **not built**, which is the right call
      on this backend: JavaScript has a garbage collector, so an `Arena` would
      reclaim nothing and a `GeneralPurpose` would report a synthetic number
      rather than a measurement. The three types earn their keep when there is
      real memory under them, and that is the native backend's problem.

      What survives for whoever does build it, because it is the non-obvious
      part: **the hook already exists.** Every allocating intrinsic is already
      handed the context and discards it — `$list_map(xs, c, f)`,
      `$str_split(s, c, sep)`, `$list_range(c, a, b)`. Routing it needs no
      change to any signature.

      Two things that were settled while looking at it:

      - **A byte-exact cost model has to be *defined*, not measured**, or the
        numbers are not reproducible across backends and every test that
        asserts one is flaky. Something like: a list of *n* charges `16 + 8n`,
        a string of *n* UTF-8 bytes charges `16 + n`. That makes the model
        observable behaviour and a change to it a breaking change — a
        commitment to state explicitly rather than discover.
      - **No reserved context slot is needed.** The plan called for slot 0 of
        every context to hold the `Alloc` implementation, which changes the
        layout of every context in every program. It is cheaper to find the
        allocator by scanning the context once and caching the answer on the
        array, which changes no generated output at all. A native backend
        knows the layout statically and does neither.

      *(deferred to the native backend, not merely unimplemented)*

- [ ] **`core/json` has no typed encoding**, and cannot have one as a library:
      no reflection, no macros, and `derive` takes a fixed list.

      There is a clean way to get one, and it needs no new language machinery.
      The backend already ships **descriptors** — `[kind, ...]` with `2 struct`
      carrying field names and `3 enum` carrying variant shapes — and
      `$show(v, d)` already walks them to render a value. So `derive ToJson for
      Point;` is implementable exactly as `derive Show` is: add `ToJson` and
      `FromJson` to the derivable list in `check.rs`, register the conformance,
      and write `$json_of(v, d)` / `$json_into(j, d)` in `runtime.js` mirroring
      `$show`. It is also the substrate the `.proto` roadmap below wants.
      *(unimplemented)*

- [x] **Examples in `///` and `//!` comments are compiled and run.** The
      doctest engine already handled prose pages; the missing half was source
      files. `doctest::doc_comments` turns a `.buri` file into a markdown
      document **with the source's own line numbers** — each doc line at its
      own line, everything else blank — so a block's origin is already the
      `.buri` line and there is no map to build, carry, or get wrong. The blank
      lines between doc runs are also what separates one comment's prose from
      the next's, which is what markdown wants anyway.

      Two consequences worth knowing:

      - **`lex.rs` was flattening them.** A doc line was `raw[3..].trim()`,
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
      **It is wrong, and it miscompiles.** `types.rs` renders *every* context
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

## Roadmap: the two features not started

Design notes. Neither is begun, and the sequencing matters more than the
detail — both have a prerequisite that is cheap now and expensive later.

### Native macOS and Linux executables

`build.rs` errors on any non-JS platform; `driver::host_platform()` returns
`Js` unconditionally; the example repository declares `LINUX`/`MACOS` outputs
that nothing builds. `LLVM-tips.md` records the intended direction.

1. **Make the backend an interface before writing a second one.** `codegen.rs`
   and `js.rs` are entangled with `mono::Program`. Extract
   `trait Backend { fn emit(&Program, &Tables, &Options) -> Result<Vec<u8>, Diagnostics> }`
   and make JS the first implementor. Nothing else is safe until this exists,
   and it gets harder every time either file grows.
2. **The value model changes, and it is language-visible.** `runtime.js`
   documents the current one: every integer is a double, a struct is an array,
   an enum is a tag or `[tag, …]`. Native needs sized integers, tagged unions,
   and a struct layout. This is where `I64` stops being a double and where
   `SPEC.md` §6.2's "overflow is undefined" starts meaning something different
   — `num.buri` says so explicitly, and it needs a SPEC amendment rather than a
   quiet divergence.
3. **Memory.** The language has no mutation and no destructors, so native
   either ships a GC or does escape analysis with an arena per `Alloc` scope.
   The allocator work above stops being decorative here — `Region` becomes
   load-bearing — which is an argument for doing it properly first.
4. **`core/host` per platform**, and `check_intrinsics` generalized so
   "missing intrinsic" is a question asked per backend.
5. **A real `link` action per `Output`**, the
   `.buri/out/<platform>/<package>/<artifact>` layout (specified and
   unexercised), `artifact_name`, and `--output=linux/x86_64` selection.
6. **A cross-compilation story, or an explicit refusal.** Refusing is fine.
   Saying nothing is not.

Prerequisite: the interface-level incremental caching gap described under
HERMETICITY above. `driver::analyze` is whole-closure, and a native build is
slow enough that whole-closure recompiles become the thing everyone complains
about first.

### `.proto` import, with binary and JSON serialization

`cli/src/docs/schema/build.proto` already writes the intended surface in its
own header: `from "//proto/foo.proto" import …`. So the syntax is settled and
the work is everything behind it.

1. **A `.proto` *schema* parser**, distinct from `textproto.rs`, which reads
   *values*. proto3 only: messages, enums, fields, `repeated`, `optional`,
   `oneof`, nested types, `import`. No services, no extensions, no `Any`.
2. **A module that is generated rather than read.** `Loader::resolve_module`
   maps a path to a file; a `.proto` path has to map to a synthesized AST. That
   machinery now exists — `Loader::load_source` is what the documentation
   harness compiles fences with — so this is reuse rather than invention.
3. **The mapping has to be decided, not discovered.** `message` → `struct` with
   `Option<T>` for `optional` and `[T]` for `repeated`; `oneof` → `enum`.
   Proto's implicit field presence and its defaults do not survive into a
   language where `Option` is explicit, and that mismatch is a decision to
   write down before any code depends on either answer.
4. **Codecs, both directions of both formats.** If `derive ToJson` lands above,
   the JSON half is largely done — proto's JSON mapping is a variation on it.
   The binary half needs varints and zigzag, which is `core/bytes` work and
   sits naturally beside the hex and base64 already there.
5. **Build integration.** A `.proto` in a package is a source no rule lists,
   which today is `undeclared-source`. It needs a `proto_sources` field in
   `build.proto`, `gen.rs` support, and an `Action::Proto` cache entry.

Land it after `core/json` and `core/bytes` — both now exist — and after
`derive ToJson`. Doing it earlier means building the same machinery twice.
