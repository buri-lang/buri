## What it does

Compiles the targets you name. A binary produces an artifact under
`.buri/out/<platform>/<package>/`. A library has no artifact of its own, so
building one type-checks it: `buri build //lib/money` asks "is this library
correct?"

With no target argument it builds the whole repository: bare `buri build` is
`buri build //...`, from any directory in it.

## Lint findings

A build reports the lint catalogue too, where `REPO.buri` asks it to. Set
`lint { check_during_build: true }` and the build runs the checks `buri lint`
runs over the targets it is building, reporting them beside the compiler's own
diagnostics. Add `fail_on_finding: true` and a finding becomes an error that
stops the build. Both default to false.

Turn the first one on because this is the command you actually run, and a
structural finding costs least to act on while the shape it is about is still
being made. [`repo-config.md`](../build/repo-config.md#lint) documents both
fields.

A rule the same block turns off in
[`rules`](../build/repo-config.md#rules) is not reported here either. A build
under a smaller catalogue prints which rules were turned off, so a quiet build
is never quiet for a reason nothing on the screen gives.

## Caching

A build is a set of actions. Each action's key covers the toolchain version, the
build mode, the platform, and the content of every input. A build reads back any
action whose key is already in the cache rather than running it, so a second
build of an unchanged tree does no work. Keys are content-addressed, so moving
the checkout, or building the same commit on another machine, hits the same
entries.

A file lock serializes cache writes and reads take none, so any number of `buri`
processes can work in one repository at once.

## Reproducibility

Two builds of one commit in one configuration produce byte-identical artifacts.
`--check-reproducible` asks that of this repository and exits 1 naming the first
byte that moved if it does not hold. An ordinary build does not run it.
[`hermeticity.md`](../build/hermeticity.md#reproducibility) sets out what makes
it a check rather than a ritual.
