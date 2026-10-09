## What it does

Builds the targets you name together with their `test.sources`, runs every
`test` declaration in them, and reports one line per failure and a summary.

A test builds its own context, so a suite decides for itself what the code under
test may do. That is the whole mocking story: a test double is a struct with
methods, bound in a context the way `main` binds the platform's implementations.

Exit status is `0` when every test passed and `1` when any did not, so you can
use `buri test` directly as a gate.

Suites build and run side by side, and print in label order. A suite's own tests
run side by side too, even when you name only that suite. `--jobs` caps how many
run at once:

```sh
buri test --jobs=2
```

The default is one per core. Builds in flight stay within half the machine's
memory, each budgeted by how much source it compiles, but a suite that has
finished building runs beside the others whatever the memory. Set
`BURI_TEST_MEMORY_BYTES` to budget against a different amount of memory. A suite
whose code is unchanged, comments and whitespace aside, reports as cached.

## Every test

`--verbose` lists every suite with how long it took to build, and every test in
it with its verdict and how long it took:

```sh
buri test //... --verbose
```

```text
//apps/web  js  2 tests  48.2 ms  built in 310.4 ms
  ok    page.buri  the title renders       31.0 ms
  ok    page.buri  a click opens the menu  17.2 ms
//lib/money  native  3 tests  12.4 ms  built in 1.8 s
  ok    cents.buri  adding cents carries into dollars        0.8 ms
  FAIL  cents.buri  formatting pads the cents to two digits  1.1 ms
  ok    rates.buri  a rate table is sorted by currency       10.5 ms
//lib/shapes  native  2 tests  cached  412 µs  built with others
  ok    shapes.buri  a square has four equal sides  250 µs
  ok    shapes.buri  an empty shape has no area     162 µs
shared build  95.3 ms

FAIL //lib/money  test/cents.buri  "formatting pads the cents to two digits"
  assert.equal failed
    actual:   "1.5"
    expected: "1.05"
  --> lib/money/test/cents.buri:9:1

6 passed, 1 failed, 0 skipped (0.4s, 2 cached)
```

Suites print in label order, and tests in the order they're declared, whichever
finished first. The test process times each test itself, so building and
starting it isn't counted, and a suite's time is its tests' times added up.
Tests run side by side, so that can be longer than the whole run.

A suite's build time covers the steps it needed alone: checking and compiling
it, then linking its binary or emitting its bundle. Work that served several
suites is counted once, on the `shared build` line, instead of split between
them: loading the sources and the standard library, and one binary built for
several native suites, which `buri test` does whenever it can. A suite with no
step of its own says `built with others`. A binary or bundle the cache puts back
costs what putting it back took.

A cached suite shows the times, build included, from the run that cached it.
`shared build` counts this run's work alone, so a run served from the cache
prints none. A test `--filter` leaves out shows as `skip`, with no time.
Failures print after the list, where they print without the flag.

## Lint findings

A test run reports the lint catalogue too, where `REPO.buri` asks it to.
`lint { check_during_build: true }` runs the checks `buri lint` runs over the
targets being tested and reports them alongside the verdicts. Add
`fail_on_finding: true` and a finding becomes an error, failing the run the way
a failing test does. Both default to false.

Opt in here for the same reason you opt in on `buri build`, and a little more
so: you run this command more often than anything else, and a test suite is
where a helper quietly grows past `oversized-function` first.
[`repo-config.md`](../build/repo-config.md#lint) documents both fields.

A rule the same block turns off in [`rules`](../build/repo-config.md#rules) is
not reported here either, and a run under a smaller catalogue says which rules
those were.

## Where a suite runs

Natively, on the host, in the development profile. A suite that says otherwise
in `test { backends }` gets what it asked for, and `--output=js` or
`--output=native` says it for one invocation without editing a build file. On
`buri test` the selector names a backend. Those two are the whole list: a suite
that named no backend runs natively or does not run.

Sometimes a native run is not available, because this toolchain has no backend
for the host in this profile, no runtime archive, or no C compiler to link with.
Then it **refuses** the suite with `test-run-unavailable`, naming the
platform and the profile you asked for. `--release` is the case worth knowing
about: the release profile routes to LLVM, so a toolchain built without
`backend-llvm` refuses `buri test --release` rather than quietly handing it to
the development backend.

**A program the native backend has no body for is refused too**, naming the
intrinsic and the backend. Falling back onto JavaScript would pass the suite on
a backend nobody chose, turning a named gap into a wrong answer. A suite that
belongs on JavaScript says so with `test { backends: [JS] }`; anything else is
a toolchain bug worth hearing about.

**A `backends: [JS]` suite cannot paint a snapshot.** There is no painter in
the JavaScript runtime, so a `snapshot` call there fails the test saying so.

## Snapshots

`platform/effect/testing`'s `snapshot` paints a tree and compares the PNG against a golden in
the package's `test/__snapshots__/`. A mismatch fails the test like any other
assertion and writes `<name>.diff.png` beside the golden, showing where the two
disagree. A repository from `buri init` ignores `*.diff.png`, so a diff is
something you look at rather than something you commit.

`--update` records what each `snapshot` painted as its golden instead of
comparing, and clears any diff left from an earlier run:

```sh
buri test //lib/cardlib --update
```

It records what it cannot read, so a golden that was never recorded and one
that is half a file both come back as whatever was painted. What it will not
record is a name that is not a file name — that fails under `--update` exactly
as it fails without it, because recording it would write outside the package.

Read the new PNGs in the diff before you commit them — recording a golden is
the whole review. The
[user interfaces guide](../../guides/user-interfaces.md#snapshots) covers what
the painter does and does not do.

## Coverage

`--coverage` counts the lines your tests reach, prints a line per file after
the verdicts, and writes an lcov file:

```sh
buri test //... --coverage
```

```text
3 passed, 0 failed, 0 skipped (0.4s)

coverage
  lib/shapes/shapes.buri  9/15   60.0%
  total                   9/15   60.0%
lcov: .buri/coverage/lcov.info
```

A line counts when a statement, a block's last expression, an `if` branch, a
`match` arm or a function body starts on it. Only your repository's own source
counts: test sources, the standard library and the platforms don't. Paths are
relative to the repository root, and counts add up across suites and backends.

A coverage run builds its own instrumented artifacts and runs every suite, even
one whose verdict is cached, so the counts are always this run's. Verdicts,
output and the exit status are the same as without the flag. `--coverage` and
`--watch` together are refused.

## Watching

With `--watch`, `buri test` runs the same invocation again every time one of its
inputs changes, until you interrupt it.

**What it watches**, for every selected target: its closure's entry points,
`sources`, generator `inputs` and `testing/` sources; the suite's own `sources`;
every `BUILD.buri` in the repository; and `REPO.buri`. That is the same declared
list the cache keys are already made of. The loop polls each file with one
`stat` every 150 ms, so it acts on a save between 150 and 300 ms after it lands,
and a burst of writes becomes one run rather than twelve. Neither interval is
configurable. Nothing the toolchain writes can wake the loop, because build
output goes under `.buri/`, which is nobody's declared input.

**A new file is not watched until something declares it.** Sources are explicit
lists rather than globs, so a file you have just created is an input of nothing.
The loop does watch the `BUILD.buri` that will name it: run `buri gen`, and the
loop sees the build file change and picks the new source up with everything
else.

**One line separates each run**, carrying the time that triggered it, which run
it is, and the file that moved:

```text
── 14:02:31Z  run 7  lib/money/cents.buri ──────────────────────
```

The time is UTC and says so. The loop never clears the screen, and a run with
nothing to do prints nothing at all, not even the separator. `--explain` turns
that inside out and is at its most useful here: one line per suite per run,
saying `cached` or `run`.

**A run that does not build is a state, not an exit.** A `BUILD.buri` that stops
parsing prints its diagnostics and the loop keeps watching, that file included,
so the run that fixes it happens by itself.

Two combinations are refused before anything opens, each with the reason.
`--force`, because forcing turns every cache hit into a run, and the cache is
what makes a loop this cheap. And no terminal on standard output, because a
watch loop nobody is watching is a hung job. The terminal rule is this command's
alone: [`buri run`](./run.md) on a page blocks either way, because it is a
server.

Interrupting the loop is how it ends, and the shell reports the interrupt rather
than a verdict. Use plain `buri test` when you want a status to branch on.
