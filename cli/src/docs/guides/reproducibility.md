# Reproducible builds

Two builds of one commit in one configuration produce byte-identical artifacts.
Here is how to check that on your own tree, and how to find out why a build you
expected to be free was not.

## Verify it

```text
$ buri build //apps/hello --check-reproducible
$ echo $?
0
```

Silence and exit `0` mean the artifacts agree. If they do not, the command exits
`1` naming the artifact and the first byte that differs.

It builds every requested binary twice and compares the bytes: two freshly
opened sessions, the cache off, two separate output directories. Those three
details are what make it a check rather than a rerun.

It doubles every build, so it is not part of an ordinary one. Run it when a
rebuild surprised you, and in whatever job you would put a slow check in.

## Debug a cache miss

`--explain` prints one line per action: what became of it, the target, the
platform, and the key.

```text
$ buri build //apps/hello --explain
keyed  compile //apps/hello js 07d2690d5c30
keyed  compile //libs/greeting js f291f47ffcc5
run    link //apps/hello js 8ff7ca2c1024
.buri/out/js/apps/hello/hello.mjs (2889 bytes)
run    lint //apps/hello - 3d291ec80592
```

`run` means the action ran. `cached` means an entry served it. `keyed` means the
action has a key but no cache entry of its own: one `link` key caches a binary's
whole closure, so that is what `compile` looks like.

Build again and the keys are the same, which is the point:

```text
keyed  compile //apps/hello js 07d2690d5c30
keyed  compile //libs/greeting js f291f47ffcc5
cached link //apps/hello js 8ff7ca2c1024
.buri/out/js/apps/hello/hello.mjs (2889 bytes, cached)
cached lint //apps/hello - 3d291ec80592
```

**A key that moved is the answer.** Edit a function body in `//libs/greeting`
and run it again:

```text
keyed  compile //apps/hello js 07d2690d5c30
keyed  compile //libs/greeting js 8ab4448448e8
run    link //apps/hello js 7dddd9ecc705
```

`//libs/greeting` recompiled and the link ran again. `//apps/hello` did not move
at all, because a body is not in the interface, so a dependent does not recheck.
Diff the two lists: the first key that changed names the action whose inputs
changed.

If nothing you edited explains a key that moved, the input is one of the others
in it: the toolchain version, the build mode, or the platform. A release
invalidates every entry in every repository, because an artifact built by a
different compiler is a different artifact.

## When the cache is the suspect

```text
$ buri build //apps/hello --force
```

`--force` runs the actions and ignores the entries. `buri clean` drops the
cache:

```text
$ buri clean
dropped .buri/out and .buri/cache
```

Needing either is worth reporting: the cache is keyed on the content of every
input, never on a timestamp, so a stale entry is a bug.

## The one trap, and it is not yours

If you **build the compiler from source**, two `buri` binaries built from
different code at the same version compute the same keys, so the first build
after you rebuild the compiler mixes both compilers' output. It is the only
build that does, which makes it easy to dismiss as noise. Compare on a fresh
tree, pass `--force`, or run `buri clean` in between.

---

[`hermeticity.md`](../reference/build/hermeticity.md) has the model: the four
action kinds, what goes into a key, and why reproducibility rather than a
sandbox is what the design rests on.
