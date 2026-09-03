# Hermeticity, actions, and the cache

The claim this design is trying to earn: **the same commit, on any machine,
produces byte-identical artifacts, and a build after a one-line edit does the
minimum work that edit implies.** Those are one property, not two — a cache is
only safe if the thing it is caching depends on nothing it did not declare.

## Actions

A build is a graph of **actions**. An action is a pure function from a declared
set of inputs to a declared set of outputs. There are four kinds:

| Action | Inputs | Outputs |
|---|---|---|
| `interface` | A library's `lib.buri`, and the `interface` outputs of its dependencies | `<lib>.bi` — every exported name with its full type |
| `compile` | One target's sources, the `interface` outputs of its dependencies, the platform | `<target>.bo` — the compiled module set |
| `link` | A binary's `compile` output and those of its transitive dependencies | The artifact: an executable, or a `.mjs` |
| `test` | A suite's `compile` output, the target's `compile` output, the `compile` output of every library the suite's own `dependencies` name | A pass/fail record and captured output |

Splitting `interface` out from `compile` is the one structural decision here,
and it is the language that makes it cheap. Top-level signatures are mandatory
([`language/functions.md` §9](../../language/functions.md)), so a library's
interface is derivable by parsing `lib.buri` and the modules it re-exports from
— no inference, no body checking, no dependency on how anything is implemented.
The consequence:

> Editing a function body changes that library's `compile` output and no
> dependent's anything. Editing a signature that `lib.buri` re-exports changes
> the interface, and dependents recheck.

In a repository where most edits are to bodies, most of the graph does not move.

## Hermeticity is a property of the language

An action is a pure function of its declared inputs. Most build systems have to
*impose* that on tools they did not write, with a filesystem namespace, a
scrubbed environment, and a denied network — a sandbox, in the operating-system
sense. This one does not, and the reason is worth stating plainly rather than
leaving as an absence:

- **Every ambient read is an intrinsic.** There is no ambient I/O in the
  language. Reading the clock, the environment, a file, or a socket happens
  through a `$host_*` intrinsic and nowhere else.
- **Only `main` can name one.** `core/host` is importable only from the module
  that exports `main` ([`language/programs.md`
  §11](../../language/programs.md)). A library, an inner module, and a test
  source that write `from "core/host" import …` are rejected —
  `host-import`, pinned by the reject corpus. So no code that participates in an
  action has a *name* for ambient state.
- **A test's capabilities are fakes.** A suite is handed a context the runner
  builds: an in-memory `Fs` holding exactly what the suite gave it, a clock the
  test sets, a seeded `Rand`, an `Env` of the test's own pairs
  ([`testing.md`](./testing.md)). There is no real capability
  to withhold.
- **The action set is closed.** Four kinds — `interface`, `compile`, `link`,
  `test` — all of them this toolchain's own code, with no way for a repository
  to define a fifth. There is no user-supplied program in the graph to distrust.

Three of the four kinds never leave this process at all: they are the compiler
reading declared files and returning bytes. The fourth, `test`, spawns a
JavaScript runtime, and that spawn is made **deterministic** rather than
confined:

- **An explicit environment.** `env_clear`, then exactly two constants: `TZ=UTC`
  and `SOURCE_DATE_EPOCH=0`. Not to hide the parent's environment from a program
  that could read it — nothing in an action can — but so that the same action
  produces the same bytes on a machine set to a different time zone or carrying
  a different `LANG`.
- **A frozen clock.** `Date.now`, `Math.random`, and the host clock intrinsics
  are replaced in the action's own script, so the instant every action observes
  is `1970-01-01T00:00:00Z`. Belt and braces against a runtime regression, and
  what makes a reproducibility check meaningful for a suite: two runs of one
  suite produce the same record rather than two records differing in a timing
  field.

`buri run` is the deliberate exception and the only one: it executes a built
artifact with the real environment and the real filesystem. Building is
hermetic; running a program is the point at which you stop building.

### What is not enforced, and what catches it instead

**The toolchain confines nothing at the operating-system level.** No namespace,
no seccomp filter, no `sandbox-exec` profile, on any platform. One was built and
then removed: it would have bought a second opinion about *toolchain* bugs and
nothing at all about repository code, which has no name for ambient state to
begin with — and it would have bought that only on macOS, only for writes and
the network, since a profile tight enough to deny reads outside an action's
directory also denies the JavaScript runtime its own binary. A partial second
opinion, on one platform, about a class of bug it would catch late and unevenly.
Here is what catches that class instead:

| The bug | What catches it |
|---|---|
| A library or test reaching for ambient state | The type system, at compile time. `host-import` and the effect bounds on `ctx`; the reject corpus pins both. |
| A test depending on a real clock, a real `Rand`, or a real filesystem | It cannot: those capabilities are injected fakes, and a suite that wanted a real one would have to be handed it. |
| A toolchain bug that leaks an intrinsic, or a code generator that embeds a path, a hostname, or a date | Two builds of one tree disagreeing — `buri build --check-reproducible`, and `two_checkouts_of_one_tree_build_identical_bytes` in the toolchain's own suite. This is the check the model rests on. |
| A machine's time zone or locale changing what an action produces | The explicit spawn environment and the frozen clock, checked by building and testing under a perturbed parent environment (`build/hermeticity.rs`). |
| A stale cache entry | The key: content, never timestamps, and every input in it. |

The honest summary is one sentence: **hermeticity is enforced by the language and
verified by reproducibility, and the toolchain applies no operating-system
confinement.** A build system whose language allowed ambient reads would need
one; this one would be adding a mechanism to defend a property it already has,
and paying for it in every action, on every platform, forever.

## Cache keys

Every action has a key, and the key is a hash of everything that can affect the
output:

```
key = H(
  action_kind,             // interface | compile | codegen | link | test
  toolchain_version,       // this compiler's own version
  build_mode,              // --release / --debug
  platform, arch,          // the only things a build varies along
  rule_identity,           // label, rule kind, and the ordered sources paths
  H(content of each input file),
  key(each input action),  // dependencies enter as keys, not contents
)
```

Four properties are worth naming because each rules out a class of stale-cache
bug:

- **Content, never timestamps.** Touching a file rebuilds nothing. Checking out
  a branch and checking it back out rebuilds nothing. `git clone` of the same
  commit into a new directory rebuilds nothing.
- **Paths are repository-relative.** Two checkouts in different directories
  produce identical keys, which is what lets a cache be shared at all.
- **Dependencies enter as keys, not contents.** A dependent's key changes only
  when its dependency's *output-determining* inputs change, and a `compile`
  action depends on its dependencies' `interface` actions — so a body edit does not
  propagate.
- **The platform is in the key, and tags are not.** The same library built for
  `linux/x86_64` and for `js` is two entries, and nothing is reused or confused
  between them. Tags are absent on purpose: a tag decides whether a build is
  *allowed*, never what it *produces*, so tagging a library differently
  invalidates no cache entry. That falls out of there being no conditional
  compilation — a source file means one thing everywhere.

Outputs are content-addressed under `.buri/cache/`, keyed by action key. `buri
build` after a no-op edit is a hash comparison and no compiler invocations.

A native artifact adds one action kind and one directory. `codegen` is one
action per codegen unit — the object file for one source module's worth of
functions — and its key is the unit's *lowered intermediate representation* rather than the source it
came from, so reformatting a comment produces an identical key and the object is
reused, while a change to a type another module requested an instantiation of
does not slip past. The objects a link ran over are staged under
`.buri/link/<link-key>/`, alongside a `manifest` naming each unit, its `codegen`
key, and whether this build produced the object or the cache did. It is derived
from the cache and goes with it: `buri clean` drops it, and `buri clean
--outputs` does not.

The link itself is always full. No shipping linker links incrementally — the two
fast ones say so in their own documentation, and one of them names
reproducibility as the reason — so "relink only what changed" is delivered above
the linker rather than inside it: an unchanged unit is never re-compiled, and a
build in which no unit's key moved skips the link entirely, because the link key
is the ordered list of the unit keys.

## What incrementality looks like

Given `//cmd/server` → `//lib/ledger` → `//lib/money`:

| Edit | Reruns |
|---|---|
| A comment in `lib/money/parse.buri` | Nothing. Comments are not in the AST hash. |
| A function body in `lib/money/parse.buri` | `compile(//lib/money)`, `link` of each binary that reaches it. `//lib/ledger` does not recheck. |
| A signature in `lib/money/lib.buri` | `interface(//lib/money)`, then `compile` of `//lib/money`, `//lib/ledger`, `//cmd/server`, then `link`. |
| Adding a file to `sources` | `compile(//lib/money)` and downstream links. The interface is unchanged unless `lib.buri` re-exports from it. |
| Adding a `tag` to `//lib/store` | No compilation at all — the tag check is a graph pass over cached facts. It either passes or fails a link. |
| A new toolchain version | Everything. An artifact built by a different compiler is a different artifact. |
| A test file | That suite's `compile` and `test`. Nothing else, ever — nothing depends on a test. |
| A file in a library named by `test { dependencies }` | The `test` of every suite that names it, and the `compile` and `link` of anything that depends on it in production. A test dependency is not in the production closure, so it moves no artifact's key by being one — but it is compiled *into* the suite, so it is in the suite's. |

The last row is the payoff for tests being unimportable: test sources are always
leaves, so a repository can have any number of them without any of them
appearing in another target's key.

## Reproducibility

Two builds of the same commit in the same configuration produce byte-identical
artifacts. What that requires, beyond the deterministic spawn above:

- **Deterministic code generation**: iteration over hash maps is by sorted key;
  monomorphization order follows source order; symbol names are derived from
  labels and module paths rather than from compilation order.
- **Deterministic evaluation semantics**: [`language/evaluation.md`
  §8.2](../../language/evaluation.md) specifies evaluation order rather than
  leaving it to the backend, so constant folding cannot differ between targets
  or between runs.
- **No embedded environment**: no paths, no timestamps, no hostname, no user.
  Debug info records repository-relative paths.

Reproducibility is a property of the compiler rather than of your repository, so
it is checked in the compiler's own test suite — by
`two_checkouts_of_one_tree_build_identical_bytes`, which builds the same commit
in two separate directories and compares the artifacts byte for byte, and then
asks `--check-reproducible` the same question in debug and in release.

**This is where the weight of the model above sits.** Since the toolchain applies
no operating-system confinement, a toolchain bug that read something it should
not — an intrinsic that leaked, a code generator that embedded a path, a
hostname, or a date — surfaces as two builds of one tree disagreeing, and it
surfaces nowhere else. A reproducibility check is not a nicety here; it is the
verification the design chose over a sandbox, and it has to be run.

`buri build --check-reproducible` asks the same question of *your* tree: it
builds every requested binary twice and compares the bytes. It is not part of
`buri build`, because a repository should not have to remember to run it and a
build that did it every time would take twice as long for a property the
compiler is responsible for. [Reproducible
builds](../../guides/reproducibility.md) is how to run it, and how to read
`--explain` when a rebuild does more work than an edit implies.

## The toolchain in the key

The compiler's own version is in every action key, so a release invalidates
every entry in every repository. That is correct and it is deliberate: an
artifact built by a different compiler is a different artifact, and a cache that
served the old one would be serving a stale answer that nothing else could
catch.

`REPO.buri` used to name the toolchain as well — an exact version and the
SHA-256 of the compiler that had to build the repository, refused with exit `2`
before anything was compiled — and both halves went into the key. That pin was
removed ([`repo-config.md`](./repo-config.md#what-is-not-here)):
a pin earns its keep where a toolchain is fetched, and nothing fetches one. The
key lost nothing a live repository could vary, because a repository that named a
toolchain this was not never got as far as computing a key.

### What "version" means here, and the one trap it leaves

It is the version string, not a hash of the `buri` binary. Everything a *user*
can vary underneath one version is caught separately — the backend's identity
carries the LLVM the binary was linked against, and the linker's identity
carries the linker it found — so for anyone running a released toolchain the
version is the whole answer.

It is not the whole answer for anyone who **builds this compiler from source**.
Two `buri` binaries built from different code at the same version compute the
same keys, so the first build after a rebuilt compiler is a mix of both
compilers' output — and it is the only build that is, which is what makes the
trap easy to dismiss as noise. The way around it is in [the
guide](../../guides/reproducibility.md#the-one-trap-and-it-is-not-yours).

## The cache is local, for now

`.buri/cache/` in the repository root, and it is safe to delete at any time.
`buri clean` does that; needing it is a bug worth reporting, since a
content-keyed cache should not be able to hold a wrong answer.

Every command is safe to run concurrently. Reads take no lock — an entry is
renamed into place, so it is there whole or not at all — and a file lock
serializes writes, held for the length of one write rather than for a build, so
two `buri build` processes overlap and meet only at the moment they both have an
entry to store. A lock left behind by a killed process is stolen after thirty
seconds, which is safe for the same reason the lock is cheap: the name of an
entry is the hash of its contents, so two writers of one key are writing the
same bytes.

Remote caching and remote execution are not specified. The reason they are worth
naming here anyway is that the design above is what makes them a transport
change rather than a semantic one: an action key already identifies an action
completely and machine-independently, and an action's inputs are already
enumerated. A remote cache is then a map from key to output blob, with no new
questions about correctness — which is exactly the position you want to be in
before writing one.
