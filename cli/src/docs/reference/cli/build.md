## What it does

Compiles the targets you name. A binary produces one artifact per output, under
`.buri/out/<platform>/<package>/`, where `<platform>` is `node`, `web`,
`native/<variant>` or `platform/<name>`.
[Build files](../build/build-files.md#entries) says how each artifact is named.
A library has no artifact of its own, so building one type-checks it:
`buri build //lib/money` asks "is this library correct?"

`--output=<selector>` builds some of them: `node`, `web`,
`native/linux-x86_64`, `native` for every `native` output, or a repository
platform's label, `//platform/cloudflare_worker`.

With no target argument it builds the whole repository: bare `buri build` is
`buri build //...`, from any directory in it.

A JSON file a generator lists is checked against its schema before the
generator runs, and a failure is an error, the same in `buri test` and
`buri lint`. The verdict is cached on the file and every schema it reads, so
editing either one checks it again. See [`guides/json.md`](../../guides/json.md).
A file in a language of your own is checked by its tool's `check` the same way
([`build/tools.md`](../build/tools.md)).

`buri build` on a `tool` rule checks it, the way it checks a library.

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

A build is a set of actions. Each action's key covers a hash of the `buri`
binary, the build mode, the platform and its variant, the entry, and the content of every input. A build reads back any
action whose key is already in the cache rather than running it, so a second
build of an unchanged tree does no work. Keys are content-addressed, so moving
the checkout, or building the same commit on another machine, hits the same
entries.

Cache reads and writes take no lock, so any number of `buri` processes can work
in one repository at once.

## Reproducibility

Two builds of one commit in one configuration produce byte-identical artifacts.
`--check-reproducible` asks that of this repository and exits 1 naming the first
byte that moved if it does not hold. An ordinary build does not run it.
[`hermeticity.md`](../build/hermeticity.md#reproducibility) sets out what makes
it a check rather than a ritual.
