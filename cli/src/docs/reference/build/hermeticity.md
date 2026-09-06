# Hermeticity, actions, and the cache

This design tries to earn one claim: **the same commit, on any machine, produces
byte-identical artifacts, and a build after a one-line edit does the minimum
work that edit implies.** Those are one property, not two. A cache is only safe
when what it caches depends on nothing it did not declare.

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
and the language makes it cheap. Top-level signatures are mandatory
([`language/functions.md` §9](../../language/functions.md)), so the compiler
derives a library's interface by parsing `lib.buri` and the modules it
re-exports from. No inference, no body checking, no dependence on how anything
is implemented. What follows:

> Editing a function body changes that library's `compile` output and nothing
> of any dependent's. Editing a signature that `lib.buri` re-exports changes the
> interface, and dependents recheck.

In a repository where most edits are to bodies, most of the graph does not move.

## Hermeticity is a property of the language

An action is a pure function of its declared inputs. Most build systems have to
*impose* that on tools they did not write, using a filesystem namespace, a
scrubbed environment, and a denied network. That is a sandbox in the
operating-system sense. This build system uses none, and here is why:

- **Every ambient read is an intrinsic.** The language has no ambient I/O.
  Reading the clock, the environment, a file, or a socket goes through a
  `$host_*` intrinsic and nowhere else.
- **Only `main` can name one.** Only the module that exports `main` may import
  `core/host` ([`language/programs.md` §11](../../language/programs.md)). The
  compiler rejects `from "core/host" import …` in a library, an inner module,
  or a test source, with `host-import`, and the reject corpus pins that. No code
  taking part in an action has a *name* for ambient state.
- **A test's capabilities are fakes.** The runner hands a suite a context it
  built itself: an in-memory filesystem holding exactly what the suite gave it,
  one store behind both `FsRead` and `FsWrite`, a clock the test sets, a seeded
  `Rand`, a seeded `Entropy`, and an `Env` of the test's own pairs
  ([`testing.md`](./testing.md)). There is no real capability
  to withhold.
- **The action set is closed.** Four kinds, `interface`, `compile`, `link`, and
  `test`, all of them this toolchain's own code. A repository cannot define a
  fifth. The graph holds no user-supplied program to distrust.

Three of the four kinds never leave this process: the compiler reads declared
files and returns bytes. The fourth, `test`, spawns a JavaScript runtime. That
spawn is **deterministic** rather than confined:

- **An explicit environment.** `env_clear`, then exactly two constants: `TZ=UTC`
  and `SOURCE_DATE_EPOCH=0`. This does not hide the parent's environment from a
  program that could read it, since nothing in an action can. It makes the same
  action produce the same bytes on a machine set to a different time zone or
  carrying a different `LANG`.
- **A frozen clock.** The action's own script replaces `Date.now`,
  `Math.random`, and the host clock intrinsics, so every action observes
  `1970-01-01T00:00:00Z`. That is belt and braces against a runtime regression,
  and it makes a reproducibility check mean something for a suite. Two runs of
  one suite produce the same record, not two records differing in a timing
  field.

`buri run` is the one deliberate exception. It executes a built artifact with
the real environment and the real filesystem. Building is hermetic. Running a
program is where you stop building.

### What is not enforced, and what catches it instead

**The toolchain confines nothing at the operating-system level.** No namespace,
no seccomp filter, no `sandbox-exec` profile, on any platform. The toolchain had
one and dropped it. It bought a second opinion about *toolchain* bugs and
nothing at all about repository code, which has no name for ambient state to
begin with. It bought even that only on macOS, and only for writes and the
network: a profile tight enough to deny reads outside an action's directory also
denies the JavaScript runtime its own binary. So: a partial second opinion, on
one platform, about a class of bug it would catch late and unevenly. Here is
what catches that class instead:

| The bug | What catches it |
|---|---|
| A library or test reaching for ambient state | The type system, at compile time, through `host-import` and the effect bounds on `ctx`. The reject corpus pins both. |
| A test depending on a real clock, a real `Rand`, a real `Entropy`, or a real filesystem | It cannot. Those capabilities are injected fakes, and a suite wanting a real one would have to be handed it. |
| A toolchain bug that leaks an intrinsic, or a code generator that embeds a path, a hostname, or a date | Two builds of one tree disagreeing. `buri build --check-reproducible` asks, and so does `two_checkouts_of_one_tree_build_identical_bytes` in the toolchain's own suite. The model rests on this check. |
| A machine's time zone or locale changing what an action produces | The explicit spawn environment and the frozen clock. `build/hermeticity.rs` builds and tests under a perturbed parent environment. |
| A stale cache entry | The key. It holds content, never timestamps, and every input. |

In one sentence: **the language enforces hermeticity, reproducibility verifies
it, and the toolchain applies no operating-system confinement.** A build system
whose language allowed ambient reads would need confinement. This one would be
adding a mechanism to defend a property it already has, and paying for it in
every action, on every platform, forever.

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

Four properties matter, because each rules out a class of stale-cache bug:

- **Content, never timestamps.** Touching a file rebuilds nothing. Checking out
  a branch and checking it back out rebuilds nothing. `git clone` of the same
  commit into a new directory rebuilds nothing.
- **Paths are repository-relative.** Two checkouts in different directories
  produce identical keys, which is what makes a cache shareable at all.
- **Dependencies enter as keys, not contents.** A dependent's key changes only
  when its dependency's *output-determining* inputs change. A `compile` action
  depends on its dependencies' `interface` actions, so a body edit does not
  propagate.
- **The platform is in the key, and tags are not.** The same library built for
  `linux/x86_64` and for `js` is two entries, and the build reuses or confuses
  nothing between them. Tags stay out on purpose. A tag decides whether a build
  is *allowed*, never what it *produces*, so retagging a library invalidates no
  cache entry. That falls out of having no conditional compilation: a source
  file means one thing everywhere.

The cache stores outputs under `.buri/cache/`, content-addressed by action key.
After a no-op edit, `buri build` compares hashes and invokes no compiler.

A native artifact adds one action kind and one directory. `codegen` runs one
action per codegen unit, the object file for one source module's worth of
functions. Its key is the unit's *lowered intermediate representation* rather
than the source it came from. Reformatting a comment produces an identical key
and reuses the object, while a change to a type another module asked to
instantiate never slips past. The build stages the objects a link ran over under
`.buri/link/<link-key>/`, alongside a `manifest` naming each unit, its `codegen`
key, and whether this build or the cache produced the object. That directory
derives from the cache and goes with it: `buri clean` drops it, and `buri clean
--outputs` does not.

The link itself is always full. No shipping linker links incrementally. The two
fast ones say so in their own documentation, and one names reproducibility as
the reason. So "relink only what changed" happens above the linker rather than
inside it. An unchanged unit never recompiles, and a build where no unit's key
moved skips the link entirely, because the link key is the ordered list of the
unit keys.

## What incrementality looks like

Given `//cmd/server` → `//lib/ledger` → `//lib/money`:

| Edit | Reruns |
|---|---|
| A comment in `lib/money/parse.buri` | Nothing. The AST hash excludes comments. |
| A function body in `lib/money/parse.buri` | `compile(//lib/money)`, `link` of each binary that reaches it. `//lib/ledger` does not recheck. |
| A signature in `lib/money/lib.buri` | `interface(//lib/money)`, then `compile` of `//lib/money`, `//lib/ledger`, `//cmd/server`, then `link`. |
| Adding a file to `sources` | `compile(//lib/money)` and downstream links. The interface stays put unless `lib.buri` re-exports from that file. |
| Adding a `tag` to `//lib/store` | No compilation at all. The tag check is a graph pass over cached facts, and it either passes or fails a link. |
| A new toolchain version | Everything. An artifact built by a different compiler is a different artifact. |
| A test file | That suite's `compile` and `test`. Nothing else, ever, because nothing depends on a test. |
| A file in a library named by `test { dependencies }` | The `test` of every suite that names it, plus the `compile` and `link` of anything that depends on it in production. A test dependency sits outside the production closure, so being one moves no artifact's key. It compiles *into* the suite, so it sits in the suite's key. |

The last row is the payoff for tests being unimportable. Test sources are always
leaves, so a repository can hold any number of them without one appearing in
another target's key.

## Reproducibility

Two builds of the same commit in the same configuration produce byte-identical
artifacts. What that requires, beyond the deterministic spawn above:

- **Deterministic code generation**: the compiler iterates hash maps by sorted
  key, monomorphizes in source order, and derives symbol names from labels and
  module paths rather than from compilation order.
- **Deterministic evaluation semantics**: [`language/evaluation.md`
  §8.2](../../language/evaluation.md) specifies evaluation order rather than
  leaving it to the backend, so constant folding cannot differ between targets
  or between runs.
- **No embedded environment**: no paths, no timestamps, no hostname, no user.
  Debug info records repository-relative paths.

Reproducibility is the compiler's property rather than your repository's, so the
compiler's own test suite checks it.
`two_checkouts_of_one_tree_build_identical_bytes` builds the same commit in two
separate directories and compares the artifacts byte for byte. It then asks
`--check-reproducible` the same question in debug and in release.

**This is where the weight of the model above sits.** The toolchain applies no
operating-system confinement. A toolchain bug that read something it should not,
such as a leaked intrinsic or a code generator that embedded a path, a hostname,
or a date, surfaces as two builds of one tree disagreeing. It surfaces nowhere
else. A reproducibility check is not a nicety here. It is the verification this
design chose over a sandbox, and somebody has to run it.

`buri build --check-reproducible` asks the same question of *your* tree. It
builds every requested binary twice and compares the bytes. It is not part of
`buri build`: your repository should not have to remember it, and a build that
checked every time would take twice as long for a property the compiler is
responsible for. [Reproducible builds](../../guides/reproducibility.md) covers
how to run it, and how to read `--explain` when a rebuild does more work than an
edit implies.

## The toolchain in the key

Every action key holds the compiler's own version, so a release invalidates
every entry in every repository. That is correct and deliberate. An artifact
built by a different compiler is a different artifact, and a cache that served
the old one would serve a stale answer nothing else could catch.

`REPO.buri` used to name the toolchain as well. It pinned an exact version and
the SHA-256 of the compiler that had to build the repository, and the CLI
refused with exit `2` before compiling anything. Both halves went into the key.
That pin is gone ([`repo-config.md`](./repo-config.md#what-is-not-here)): a pin
earns its keep where something fetches a toolchain, and nothing fetches one. The
key lost nothing a live repository could vary, because a repository that named a
different toolchain never got as far as computing a key.

### What "version" means here, and the one trap it leaves

The key holds the version string, not a hash of the `buri` binary. Other checks
catch everything a *user* can vary underneath one version. The backend's
identity carries the LLVM the binary was linked against, and the linker's
identity carries the linker it found. For anyone running a released toolchain,
the version is the whole answer.

It is not the whole answer if you **build this compiler from source**. Two
`buri` binaries built from different code at the same version compute the same
keys, so the first build after you rebuild the compiler mixes both compilers'
output. It is the only build that does, which is what makes the trap easy to
dismiss as noise. [The
guide](../../guides/reproducibility.md#the-one-trap-and-it-is-not-yours) has the
way around it.

## The cache is local, for now

The cache lives in `.buri/cache/` at the repository root, and you can delete it
at any time. `buri clean` does that. Needing it is a bug worth reporting,
because a content-keyed cache should not be able to hold a wrong answer.

Every command is safe to run concurrently. Reads take no lock, because the cache
renames an entry into place: it is there whole or not at all. A file lock
serializes writes, and it is held for one write rather than for a whole build.
Two `buri build` processes overlap and meet only when both have an entry to
store. A killed process leaves a lock behind, and the next writer steals it
after thirty seconds. That is safe for the same reason the lock is cheap: an
entry's name is the hash of its contents, so two writers of one key write the
same bytes.

Remote caching and remote execution are not specified. They are worth naming
because the design above makes them a transport change rather than a semantic
one. An action key already identifies an action completely and
machine-independently, and an action already enumerates its inputs. A remote
cache is then a map from key to output blob, raising no new questions about
correctness. That is exactly the position you want to be in before writing one.
