# Hermeticity, actions, and the cache

**The same commit, on any machine, produces byte-identical artifacts, and a
build after a one-line edit does only the work that edit implies.** That's one
property, not two: a cache is only safe when what it caches depends on nothing
it didn't declare.

## Actions

A build is a graph of **actions**, each a pure function from declared inputs to
declared outputs:

| Action | Inputs | Outputs |
|---|---|---|
| `generate` | A rule's [`generators`](./generators.md) entries: each tool's artifact, and the contents of every declared input | The modules the tool answered with — text, and an anchor per node saying which input span it came from |
| `interface` | A library's `lib.buri`, and the `interface` outputs of its dependencies | `<lib>.bi` — every exported name with its full type |
| `compile` | One target's sources, its `generate` output, the `interface` outputs of its dependencies, the platform | `<target>.bo` — the compiled module set |
| `link` | A binary's `compile` output and those of its transitive dependencies | The artifact: an executable, or a `.mjs` |
| `test` | A suite's `compile` output, the target's `compile` output, the `compile` output of every library the suite's own `dependencies` name | A pass/fail record and captured output |

Top-level signatures are mandatory
([`language/functions.md` §9](../../language/functions.md)), so the compiler
derives a library's interface by parsing `lib.buri` and the modules it
re-exports from. That makes splitting `interface` from `compile` cheap:

> Editing a function body changes that library's `compile` output and nothing
> of any dependent's. Editing a signature that `lib.buri` re-exports changes the
> interface, and dependents recheck.

## Hermeticity is a property of the language

Most build systems impose purity with a filesystem namespace, a scrubbed
environment, and a denied network. Buri needs none of that:

- **Every ambient read is an intrinsic.** Reading the clock, the environment, a
  file, or a socket goes through a `$host_*` intrinsic and nowhere else.
- **Only `main` holds one.** The production implementations arrive as the
  fields of the host `main` takes, which only the CLI builds
  ([`language/programs.md` §11](../../language/programs.md)). The compiler
  rejects `from "platform/host" import …` anywhere but a platform's
  `platform.buri`, with `host-import-outside-platform`. No code in an action
  has a *name* for ambient state.
- **A test's capabilities are fakes.** The runner hands a suite an in-memory
  filesystem holding exactly what the suite gave it (one store behind both
  `FileSystemRead` and `FileSystemWrite`), a clock the test sets, a seeded
  `Random`, a seeded `Entropy`, and an `Environment` of the test's own pairs
  ([`testing.md`](./testing.md)).
- **The action set is closed.** A repository can't define a sixth kind.
  `generate` runs a program somebody declared, a
  [generator](./generators.md), held to the same rules: every input is in the
  key, and its entry point gets an `Allocator` context and nothing else.

`test` and `generate` each spawn a JavaScript runtime. Both spawns are
**deterministic** rather than confined:

- **An explicit environment.** `env_clear`, then exactly `TZ=UTC` and
  `SOURCE_DATE_EPOCH=0`, so a machine's time zone or `LANG` can't change the
  bytes.
- **A frozen clock, for a suite.** A test's script replaces `Date.now`,
  `Math.random`, and the host clock intrinsics, so it observes
  `1970-01-01T00:00:00Z` and two runs produce the same record. A generator
  needs no such splice: `run` hands it only `Allocator`, `Stdin` and `Stdout`,
  so a generator that reaches for a clock doesn't compile.

`buri run` is the one exception. It runs a built artifact against the real
environment and filesystem. Building is hermetic; running isn't building.

### What is not enforced, and what catches it instead

**The toolchain confines nothing at the operating-system level** — no
namespace, no seccomp filter, no `sandbox-exec` profile. Instead:

| The bug | What catches it |
|---|---|
| A library or test reaching for ambient state | The type system, through `host-import-outside-platform`, the host an entry takes, and the effect bounds on `ctx`. The reject corpus pins both. |
| A test depending on a real clock, `Random`, `Entropy`, or filesystem | It can't. Those capabilities are injected fakes. |
| A toolchain bug that leaks an intrinsic, or a code generator that embeds a path, a hostname, or a date | Two builds of one tree disagreeing. `buri build --check-reproducible` asks, and so does `two_checkouts_of_one_tree_build_identical_bytes` in the toolchain's own suite. |
| A machine's time zone or locale changing what an action produces | The explicit spawn environment and the frozen clock. `build/hermeticity.rs` builds and tests under a perturbed parent environment. |
| A stale cache entry | The key. It holds every input, by content, never timestamps. |

**The language enforces hermeticity, reproducibility verifies it, and the
toolchain applies no operating-system confinement.**

## Cache keys

Every action's key hashes everything that can affect its output:

```
key = H(
  action_kind,             // interface | compile | codegen | link | test
  toolchain_identity,      // the linker's id for this compiler's own binary
  build_mode,              // --release / --debug
  platform, variant, entry, // the only things a build varies along
  rule_identity,           // label, rule kind, and the ordered sources paths
  H(content of each input file),
  key(each input action),  // dependencies enter as keys, not contents
)
```

Each property rules out a class of stale-cache bug:

- **Content, never timestamps.** Touching a file, switching branches and back,
  or cloning the same commit into a new directory rebuilds nothing.
- **Paths are repository-relative.** Two checkouts in different directories
  produce identical keys, so a cache is shareable.
- **Dependencies enter as keys, not contents.** `compile` depends on its
  dependencies' `interface` actions, so a body edit doesn't propagate.
- **The platform and the entry are in the key; tags aren't.** One library built
  for `linux/x86_64` and for `js` is two entries. So are a binary's outputs for
  `main` and for `fetch`, because the entry is where dead-code elimination
  starts. A tag decides whether a build is *allowed*, never what it
  *produces*, so retagging invalidates nothing.

Outputs live under `.buri/cache/`, addressed by action key. After a no-op edit,
`buri build` compares hashes and invokes no compiler.

A native artifact adds the `codegen` action: one per codegen unit, the object
file for one source module's functions. Its key is the unit's *lowered
intermediate representation*, not its source, so reformatting a comment reuses
the object while a change to a type another module instantiates never slips
past. The build stages a link's objects under `.buri/link/<link-key>/`, with a
`manifest` naming each unit, its `codegen` key, and whether this build or the
cache produced it. `buri clean` drops that directory; `buri clean --outputs`
doesn't.

No shipping linker links incrementally, so the link itself is always full.
Instead, an unchanged unit never recompiles, and when no unit's key moved the
link is skipped, because the link key is the ordered list of unit keys.

## What incrementality looks like

Given `//cmd/server` → `//lib/ledger` → `//lib/money`:

| Edit | Reruns |
|---|---|
| A comment in `lib/money/parse.buri` | Nothing. The AST hash excludes comments. |
| A function body in `lib/money/parse.buri` | `compile(//lib/money)`, `link` of each binary that reaches it. `//lib/ledger` does not recheck. |
| A signature in `lib/money/lib.buri` | `interface(//lib/money)`, then `compile` of `//lib/money`, `//lib/ledger`, `//cmd/server`, then `link`. |
| Adding a file to `sources` | `compile(//lib/money)` and downstream links. The interface stays put unless `lib.buri` re-exports from that file. |
| Adding a `tag` to `//lib/store` | No compilation. The tag check is a graph pass over cached facts that passes or fails a link. |
| A rebuilt toolchain | Everything, even a rebuild of the same `buri` version. A different compiler makes a different artifact. |
| A test file | That suite's `compile` and `test`. Nothing depends on a test. |
| A golden in `test/__snapshots__`, edited, added or deleted | The `test` of every suite in that package. A `.diff.png` reruns nothing. |
| A file in a library named by `test { dependencies }` | The `test` of every suite that names it, plus the `compile` and `link` of anything depending on it in production. Being a test dependency moves no artifact's key. |

## Reproducibility

Two builds of the same commit in the same configuration produce byte-identical
artifacts. Beyond the deterministic spawn above, that takes:

- **Deterministic code generation**: hash maps iterate by sorted key,
  monomorphization follows source order, and symbol names come from labels and
  module paths, not compilation order.
- **Deterministic evaluation**: [`language/evaluation.md`
  §8.2](../../language/evaluation.md) fixes evaluation order, so constant
  folding can't differ between targets or runs.
- **No embedded environment**: no paths, timestamps, hostname, or user. Debug
  info records repository-relative paths.

The compiler's own suite checks this:
`two_checkouts_of_one_tree_build_identical_bytes` builds one commit in two
directories, compares the artifacts byte for byte, then asks
`--check-reproducible` the same in debug and release. **The model rests on this
check.** A toolchain bug that reads something it shouldn't shows up as two
builds disagreeing, and nowhere else.

`buri build --check-reproducible` asks the same of *your* tree: it builds every
requested binary twice and compares the bytes. It isn't part of `buri build`,
because it doubles the build time for a property the compiler owns.
[Reproducible builds](../../guides/reproducibility.md) covers running it and
reading `--explain` when a rebuild does more work than an edit implies.

## The toolchain in the key

Every key holds the running `buri` binary's identity, not its version, since a
rebuilt `0.3.0` is a different compiler. The identity is the id the linker
derives from the linked bytes (`LC_UUID` on macOS, the GNU build id on Linux),
read from the header, or a hash of the binary where it has none. A rebuilt
compiler can never be served the previous build's entries. On first open it also
drops what the old binary left in `.buri/cache/` and records its identity in
`.buri/cache/.toolchain`, so rebuilding the compiler needs no `rm -rf .buri`
and no `--force`. The backend's identity also carries the LLVM the binary
linked against, and the linker's identity the linker it found.

## The cache is local, for now

You can delete `.buri/cache/` at any time; `buri clean` does that. Needing to is
a bug worth reporting, because a content-keyed cache shouldn't hold a wrong
answer.

Every command is safe to run concurrently, and the cache takes no lock. Each
writer writes its own temporary file and renames it into place, so an entry is
there whole or not at all. Two writers of one key write the same bytes, because
the key is a hash of everything that decides them.

Remote caching and remote execution aren't specified. They'd be a transport
change, not a semantic one: an action key already identifies an action
completely and machine-independently.
