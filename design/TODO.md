# TODO

What is **not** done: open gaps, work deferred with the reason it was deferred,
and the decisions that have to keep being decided the same way.

Completed work is deliberately not recorded here. It is recorded where it can
be checked — in the code, in the test that pins it, in `cli/src/docs/` for
anything a user meets, and in `design/native/` for the native backend's
design. A TODO that narrates what has already landed is a second, drifting copy
of all three, and it is what this file used to be.

**Cite this file by heading, not by line number.** Every section below has a
stable anchor (`design/TODO.md#compile-time`); a line citation goes stale the
next time anything above it is edited, and several already had.

A neighbour: [`STANDARD-LIBRARY.md`](./STANDARD-LIBRARY.md) is why `core/*`
holds what it holds, and what the deliberate absences would cost to close.

---

## Build system and CLI

- **A `main.buri` in a package with no `binary` rule is invisible.** Found
  writing `repositories/cli/gen_never_creates`, and pinned there as a clean
  `lint` run rather than fixed. `gen` is right to leave it alone — it never
  adds a rule — but nothing else mentions it either: `check_sources_declared`
  (`commands/lint.rs`) puts `main.buri` in the `known` set unconditionally, by
  the rule *kind* that names it, and in a library-only package there is no rule
  of that kind. So the file is compiled by nothing, shipped by nothing, and
  reported by nothing.

  The fix is one condition — an entry point is declared by the rule that names
  it only when that rule exists — and a new row in the build-graph table, next
  to `undeclared-source`, saying that a `main.buri` with no `binary` rule (or a
  `lib.buri` with no `library` rule) is a file the build cannot see. The case
  records today's silence, so the fix will show up as a diff there.

- **`artifact_name` on an output is untested.** Native artifacts are built now,
  so this is a test nobody has written rather than a feature nobody has landed.
  `actions::artifact_path` is where it is read.

- **A library's own `platforms:` field is untested**, and so is the
  intersection-down-the-closure rule that goes with it. No library in the
  worked example declares one.

- **Maturity policy is untested.** `experimental` and `stable` are declared in
  the example's `REPO.buri` and carried by no target, so that whole path is
  unexercised.

- **`main.buri` is the only module that may import `core/host`, and its context
  is checked against each output's platform — untested.** A `main` binding
  `Fs: host.fs` under `platform: JS` must be an unresolved name at the entry
  point.

- **The corpus reformat has not been run.** The formatter is ready and proved:
  `corpus::formatting_the_corpus_preserves_what_it_means` formats the whole
  conformance repository into a scratch copy and gets the same assertions, and
  the widest line drops from 309 columns to 180. What is left is the
  coordinated pass that writes it to the checked-in files.

- **The `cli/tests/repositories/` fixtures are not walked by `corpus.rs`,** so
  the sources there are held neither to "every source parses" nor to the
  formatting fixed point. That is deliberate — `repos/cli/format_check` checks in a
  deliberately misformatted file, and future cases will check in ones that must
  not compile — but it means a typo in a fixture is caught only by the case
  that runs it.

## Editors and the language server

- **Nothing tests the LSP against a real editor.** The recorded sessions in
  `cli/tests/repositories/lsp/` prove the server answers; they do not prove a
  client can drive it.

- **`editors/zed/extension.toml` pins a placeholder commit.** The tree-sitter
  grammar has to be published as its own git repository before the extension
  can be installed as anything other than a dev extension.

## Incrementality and caching

- **A dependent does not recheck after a body edit in its dependency.** This
  needs a real `interface` action, and the cheap version is *unsound*:
  `lib.buri` re-exports from sibling modules, so a signature change in
  `parse.buri` changes the interface while leaving `lib.buri` byte-identical,
  and a key that said "unchanged" there would serve a stale answer. A sound
  version needs per-target analysis; `driver::analyze` is whole-closure.
  `buri build --explain` is in place to test it when it lands.

- **The same gap, measured from the other end: a command analyses each target
  from scratch.** `lint //...` on the conformance repository calls
  `driver::analyze` twelve times, and each one re-*checks* the standard library
  modules that target imports. Parsing is shared; checking is not. The fix is
  the same interface-level incrementality — cache a package's checked surface,
  keyed on its sources and its dependencies' surfaces — which is a design
  question about what a package's interface *is*, not a performance patch. The
  cost is O(targets × the closure each imports), so it will be felt long before
  a thousand targets.

  This is also the prerequisite the native backend wants: a native build is
  slow enough that whole-closure recompiles become the first thing anybody
  complains about.

- **Nothing shares work between processes.** The parse cache lives for one
  command. Two `buri lint` runs in a row re-read and re-parse everything; only
  the *action* cache in `.buri/cache` survives, and it caches artifacts rather
  than analysis.

## Compile time

- **Exhaustiveness checking is combinatorial and has no complexity limit.**
  `expand` splits a row on every top-of-column or-pattern, and `expand_lengths`
  multiplies an array-rest pattern by the longest array length any arm
  distinguishes. Both are inherent to the usefulness algorithm. Measured, the
  growth is polynomial rather than exponential on every shape that could be
  constructed for it — reaching 90 ms took a match on a six-column tuple of
  arrays against an eighty-element literal array pattern, which is not a
  program anybody writes.

  It is recorded rather than fixed because the fix is a bail-out, and a
  bail-out in this checker means a `match` that is *not* exhaustive compiles.
  That is a decision about the language, not about how long the compiler takes.

- **The documentation harness loads the whole standard library per snippet.**
  `analyze_snippet_as` calls `load_all_std`, which defeats the lazy loading
  every other entry point gets. Making it lazy passes the whole documentation
  suite and is arguably more faithful, but it did not move the suite's runtime
  at all — so it is a correctness question rather than a performance one, and
  it is left for whoever asks it as one.

## The JavaScript backend

- **A decision tree for nested patterns.** `arm_chain` emits one `if` per arm
  carrying the arm's whole test, so a match on a nested pattern re-tests the
  outer tag on every arm and `to_switch` cannot rescue it — it needs every test
  in the chain to be a bare `disc === lit`. Measured 1.75× on a match with
  eight outer constructors by four inner, and a size win that grows with the
  arity.

  Left undone deliberately: it reorders tests, and Buri's match is
  first-match-wins, so it is the one item on this list where a mistake is a
  miscompile rather than a slower program. It wants to land on its own, against
  `release_and_debug_agree` and the whole conformance corpus, and the sound
  version is a real column-based decision tree in `generate.rs` rather than a
  regrouping of already-emitted JavaScript.

- **Nothing measures how large one emitted function gets.** V8 never optimizes
  a function past 61,440 bytecodes, and a whole-program compiler has three ways
  to grow one quietly: the inliner's per-caller ceiling compounds over its
  rounds, a merged tail-call group fuses an entire mutually recursive
  component, and `main` accumulates every single-use body inlined into it.
  `sizes.txt` records the largest function in the corpus — 843 bytes, nowhere
  near — and the harness fails past 32,768. That is a tripwire, not a
  measurement of a real program; what is missing is the same number for the
  worked monorepo, where the inliner has something to work with.

## The native backend

The design is `design/native/`; the waves it planned have landed. What is open
is one thing:

- **The default has not flipped.** A binary that declares no outputs still gets
  `JS`, and a suite that names no platforms still runs on JavaScript, because
  the native runtime surface is not complete: a program using `core/fs`,
  `core/env`, `json.*`, or any `list.*` entry taking a closure is refused by
  `Backend::missing_intrinsics` rather than mis-run — the right refusal and the
  wrong default.

  The trigger for flipping it is that refusal going quiet across the
  conformance corpus. At that point `selected_outputs`' fallback
  (`build/actions.rs`) and `run_suite`'s (`commands/test.rs`) are one line each.

  `Backend::missing_intrinsics` is also the answer to "what does a second
  backend do about an intrinsic it does not have": the question is asked per
  backend, and the build fails naming the intrinsic and the backend rather than
  emitting something that will not run.

- **`core/host` is still one module rather than one per platform.** It is
  reached only from the module exporting `main`, so the blast radius is small,
  but a platform-specific host is what the effect model was drawn to allow.

## Decisions to keep saying no to

These are settled. They are here because each one was asked for at least once,
and the answer is not obvious from the code.

- **There is no `--shuffle`, and there must not be.** The runner may run a
  suite's tests in any order, and there is no knob to turn that off. A flag
  nothing reads cannot be listed; `repos/testing/filter` pins that `--shuffle`
  and `--shuffle=off` are both exit 2.

- **There is no `buri query --output=proto`.** It was documented once and
  rejected by the parser, and the disagreement was ended by deleting the
  documentation. `repos/query/graph_queries` records the refusal so a reader
  who finds the old claim finds the answer with it.

- **No operating-system sandbox, on any platform.** One was built and removed.
  Hermeticity is enforced by the language, verified by reproducibility
  (`buri build --check-reproducible`), and the toolchain applies no confinement
  at all. The decision record — what confinement would have bought, and why the
  three properties of this language take the ground out from under it — is
  `cli/src/docs/build/hermeticity.md` (`buri docs build/hermeticity`). What was
  kept from the attempt is determinism rather than confinement, and it lives in
  `build/spawn.rs`.

- **No toolchain pin in `REPO.buri`.** `toolchain { version, sha256 }` existed
  and was removed: a pin means something only where a toolchain is fetched, and
  nothing fetches one. Field 1 of `RepoConfig` is reserved so an old file cannot
  be read as something else.

- **No cross-compilation.** `cli/build.rs` builds `libburi_rt.a` for the host
  and for nothing else, so `--output=linux/x86_64` on a macOS host is refused.
  The fix, when someone wants it, is prebuilt runtime archives per triple — a
  packaging problem, not a compiler one. `design/native/ARCHITECTURE.md` §9 is
  the record.

- **`Arena` does not free in bulk, and that is not a backend gap.** What would
  make it real is a scoped context — a language proposal bounding a context's
  lifetime. See `design/STANDARD-LIBRARY.md` §4.

- **Only `allocate(ctx, n)` is counted.** The list, string and closure rows of
  the cost model are charged by definition and reported to no allocator.
  Widening the counted set has to happen on both backends at once or the
  numbers stop agreeing, which is the one property `core/alloc` exists to have.
  See `design/STANDARD-LIBRARY.md` §2.

- **`listBytes(n, stride)` takes its stride as an argument**, because the
  language has no `sizeOf<T>()` for a program to ask with. A `sizeOf<T>()`
  would make it `listBytes<T>(n)`, and that is the language change it waits on.

## Measured and rejected

Recorded so they are not tried again.

- **`$str` of a `Bool` through `String`** — no difference; V8 inlines the
  existing short-circuit.
- **Splitting `Int` into a hi/lo `Int32` pair** — 0.97 ns/op against 0.48 for a
  plain `Number` add, and only reachable with a scalar-replacement pass this
  compiler does not have.
- **`[]` with `push` instead of `new Array(n)` with an index fill in
  `$list_map`** — 13.8 ms against 23.5. The antipattern the general advice
  warns about is `new Array(n).fill(0)`, which is not what this does.
- **Lock contention and interior mutability were looked for and are not
  there.** No `Mutex`, `RwLock`, `RefCell`, `Rc` or `OnceLock` sits on any hot
  path, and diagnostic rendering re-reads no files — every span resolves
  through the `SourceMap` that already holds the text.
