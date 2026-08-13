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
| `interface` | A library's `lib.buri`, and the `interface` outputs of its deps | `<lib>.bi` — every exported name with its full type |
| `compile` | One target's sources, the `interface` outputs of its deps, the configuration | `<target>.bo` — the compiled module set |
| `link` | A binary's `compile` output and those of its transitive deps | The artifact: an executable, or a `.mjs` |
| `test` | A suite's `compile` output, the target's `compile` output, `test.data` | A pass/fail record and captured output |

Splitting `interface` out from `compile` is the one structural decision here,
and it is the language that makes it cheap. Top-level signatures are mandatory
([`SPEC.md` §9](../SPEC.md)), so a library's interface is derivable by parsing
`lib.buri` and the modules it re-exports from — no inference, no body checking,
no dependency on how anything is implemented. The consequence:

> Editing a function body changes that library's `compile` output and no
> dependent's anything. Editing a signature that `lib.buri` re-exports changes
> the interface, and dependents recheck.

In a repository where most edits are to bodies, most of the graph does not move.

## The sandbox

Every action runs in a sandbox containing exactly its declared inputs, and
nothing else:

- **Filesystem**: a fresh directory holding the input files, read-only, at
  paths relative to the repository root. No access to the source tree, the home
  directory, the system, or anything a previous action wrote that this one did
  not declare.
- **Environment**: empty. Not filtered — empty. `PATH`, `HOME`, `LANG`, `TZ`,
  and every other variable are absent, so nothing can read one.
- **Network**: unavailable. There is nothing to fetch: the toolchain is pinned
  and verified before the build starts, and there are no external repositories.
- **Clock**: actions do not read one. Timestamps in outputs are fixed
  (`1970-01-01T00:00:00Z`); file ordering is sorted; nothing embeds a build date,
  a hostname, or a working directory path.
- **Concurrency**: actions cannot observe each other. There is no shared
  scratch directory and no way to write outside the declared outputs.

A tool that violates the sandbox fails the action rather than degrading to a
non-hermetic build. Hermeticity that is best-effort is hermeticity you cannot
rely on for caching, and caching is why it is here.

`buri run` is the deliberate exception and the only one: it executes a built
artifact outside the sandbox, with the real environment. Building is hermetic;
running a program is the point at which you stop building.

**The language does most of this work already.** There is no `#include` path, no
conditional compilation, no macro that can read a file, no reflection, no build
script, and no ambient I/O — a Buri module's meaning is a function of its own
text and its imports. The sandbox is enforcing a property the language already
has, which is why it can be strict without breaking anything.

## Cache keys

Every action has a key, and the key is a hash of everything that can affect the
output:

```
key = H(
  action_kind,             // interface | compile | link | test
  toolchain.version,
  toolchain.sha256,
  toolchain.flags,         // plus --release/--debug
  configuration,           // every dimension=value pair, sorted
  rule_identity,           // label, rule kind, and the ordered sources paths
  H(content of each input file),
  key(each input action),  // deps enter as their keys, not their contents
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
  action depends on its deps' `interface` actions — so a body edit does not
  propagate.
- **The configuration is in the key.** The same library built for `linux/server`
  and for `js/client` is two entries. Nothing is reused across configurations,
  and nothing is confused between them.

Outputs are content-addressed under `.buri/cache/`, keyed by action key. `buri
build` after a no-op edit is a hash comparison and no compiler invocations.

## What incrementality looks like

Given `//cmd/server` → `//lib/ledger` → `//lib/money`:

| Edit | Reruns |
|---|---|
| A comment in `lib/money/parse.buri` | Nothing. Comments are not in the AST hash. |
| A function body in `lib/money/parse.buri` | `compile(//lib/money)`, `link` of each binary that reaches it. `//lib/ledger` does not recheck. |
| A signature in `lib/money/lib.buri` | `interface(//lib/money)`, then `compile` of `//lib/money`, `//lib/ledger`, `//cmd/server`, then `link`. |
| Adding a file to `sources` | `compile(//lib/money)` and downstream links. The interface is unchanged unless `lib.buri` re-exports from it. |
| Adding a `tag` to `//lib/store` | No compilation at all — the tag check is a graph pass over cached facts. It either passes or fails a link. |
| `toolchain.version` in `REPO.buri` | Everything. This is correct and is why the version is pinned exactly. |
| A test file | That suite's `compile` and `test`. Nothing else, ever — nothing depends on a test. |

The last row is the payoff for tests being unimportable: test sources are always
leaves, so a repository can have any number of them without any of them
appearing in another target's key.

## Reproducibility

Two builds of the same commit in the same configuration produce byte-identical
artifacts. What that requires, beyond the sandbox:

- **Deterministic code generation**: iteration over hash maps is by sorted key;
  monomorphization order follows source order; symbol names are derived from
  labels and module paths rather than from compilation order.
- **Deterministic evaluation semantics**: [`SPEC.md` §8.2](../SPEC.md) specifies
  evaluation order rather than leaving it to the backend, so constant folding
  cannot differ between targets or between runs.
- **No embedded environment**: no paths, no timestamps, no hostname, no user.
  Debug info records repository-relative paths.

`buri build --check-reproducible //cmd/server` builds twice, in separate
sandboxes, and diffs the artifacts. It is a test of the compiler, and it belongs
in the compiler's own CI rather than in yours.

## The cache is local, for now

`.buri/cache/` in the repository root, and it is safe to delete at any time.
`buri clean` does that; needing it is a bug worth reporting, since a
content-keyed cache should not be able to hold a wrong answer.

Remote caching and remote execution are not specified. The reason they are worth
naming here anyway is that the design above is what makes them a transport
change rather than a semantic one: an action key already identifies an action
completely and machine-independently, and an action's inputs are already
enumerated. A remote cache is then a map from key to output blob, with no new
questions about correctness — which is exactly the position you want to be in
before writing one.
