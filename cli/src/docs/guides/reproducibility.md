# Reproducible builds

Two builds of one commit in one configuration produce byte-identical artifacts.

## Verify it

```text
$ buri build //apps/hello --check-reproducible
$ echo $?
0
```

Exit `0` means the artifacts agree. Otherwise it exits `1`, naming the artifact
and the first byte that differs.

It builds every requested binary twice, in two fresh sessions with the cache off
and separate output directories, then compares the bytes. That doubles the
build, so it isn't part of an ordinary one. Run it when a rebuild surprises
you, or in a slow CI job.

## Debug a cache miss

`--explain` prints one line per action: its outcome, the target, the platform,
and the key.

```text
$ buri build //apps/hello --explain
keyed  compile //apps/hello js 07d2690d5c30
keyed  compile //libs/greeting js f291f47ffcc5
run    link //apps/hello js 8ff7ca2c1024
.buri/out/node/apps/hello/hello.mjs (2889 bytes)
run    lint //apps/hello - 3d291ec80592
```

`run` means the action ran, `cached` that an entry served it. `keyed` means it
has a key but no entry of its own: one `link` entry caches a binary's whole
closure, so `compile` always looks like this.

Build again and the keys match:

```text
keyed  compile //apps/hello js 07d2690d5c30
keyed  compile //libs/greeting js f291f47ffcc5
cached link //apps/hello js 8ff7ca2c1024
.buri/out/node/apps/hello/hello.mjs (2889 bytes, cached)
cached lint //apps/hello - 3d291ec80592
```

**A key that moved is the answer.** Edit a function body in `//libs/greeting`
and build again:

```text
keyed  compile //apps/hello js 07d2690d5c30
keyed  compile //libs/greeting js 8ab4448448e8
run    link //apps/hello js 7dddd9ecc705
```

`//libs/greeting` recompiled and the link reran. `//apps/hello` didn't move,
because a body isn't in the interface. Diff the two lists: the first key that
changed names the action whose inputs changed.

If nothing you edited explains it, another input moved: the toolchain (a hash
of the `buri` binary), the build mode, or the platform. Any rebuild of `buri`,
even at the same version, invalidates every entry.

## When the cache is the suspect

`--force` runs the actions and ignores the entries. `buri clean` drops the
cache:

```text
$ buri build //apps/hello --force
$ buri clean
dropped .buri/out and .buri/cache
```

Needing either is worth reporting: the cache is keyed on every input's content,
never a timestamp, so a stale entry is a bug.

## Rebuilding the compiler

A `buri` built from source hashes differently, so it can't be served the
previous build's output. On its first run it drops the old binary's entries
from `.buri/cache/`. No fresh tree, `--force`, or `buri clean` needed.

---

[`hermeticity.md`](../reference/build/hermeticity.md) has the model: the action
kinds, what goes into a key, and why the design rests on reproducibility rather
than a sandbox.
