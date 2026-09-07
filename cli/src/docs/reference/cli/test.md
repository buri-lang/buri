## What it does

Builds the targets you name together with their `test.sources`, runs every
`test` declaration in them, and reports one line per failure and a summary.

A test builds its own context, so a suite decides for itself what the code under
test may do. That is the whole mocking story: a test double is a struct with
methods, bound in a context the way `main` binds the platform's implementations.

Exit status is `0` when every test passed and `1` when any did not, so you can
use `buri test` directly as a gate.

## Lint findings

A test run reports the lint catalogue too, where `REPO.buri` asks it to.
`lint { check_during_build: true }` runs the checks `buri lint` runs over the
targets being tested and reports them alongside the verdicts. Add
`fail_on_finding: true` and a finding becomes an error, failing the run the way
a failing test does. Both default to false, so a repository that writes neither
gets exactly the run described above.

Opt in here for the same reason you opt in on `buri build`, and a little more
so: you run this command more often than anything else, and a test suite is
where a helper quietly grows past `oversized-function` first.
[`repo-config.md`](../build/repo-config.md#lint) documents both fields.

A rule the same block turns off in [`rules`](../build/repo-config.md#rules) is
not reported here either, and a run under a smaller catalogue says which rules
those were.

## Where a suite runs

Natively, on the host, in the development profile. A suite that says otherwise
in `test { platforms }` gets what it asked for, and `--output=js` says it for
one invocation without editing a build file.

Those two are the whole list. Nothing else moves a suite: a suite that named no
platform runs natively or does not run.

Sometimes a native run is not available, because this toolchain has no backend
for the host in this profile, no runtime archive, or no C compiler to link with.
Then it **refuses** the suite with `native-run-not-available`, naming the
platform and the profile you asked for. `--release` is the case worth knowing
about: the release profile routes to LLVM, so a toolchain built without
`backend-llvm` refuses `buri test --release` rather than quietly handing it to
the development backend.

**A program the native backend has no body for is refused too**, naming the
intrinsic and the backend. Both used to fall back onto JavaScript with a line on
standard error, and that was the wrong answer. The suite then passed on a
backend nobody chose, which turns a named gap into a wrong answer rather than
into a report, and the line saying so went to a stream nobody reads when a run
is green. A suite that belongs on JavaScript says so with
`test { platforms: [JS] }`; anything else is a toolchain bug worth hearing
about.

**A `platforms: [JS]` suite cannot paint a snapshot.** There is no painter in
the JavaScript runtime, so a `snapshot` call there fails the test saying so.

## Snapshots

`ui/testing`'s `snapshot` paints a tree and compares the PNG against a golden in
the package's `test/__snapshots__/`. A mismatch fails the test like any other
assertion and writes `<name>.diff.png` beside the golden, showing where the two
disagree.

`--update` records what each `snapshot` painted as its golden instead of
comparing, and clears any diff left from an earlier run:

```sh
buri test //lib/cardlib --update
```

Read the new PNGs in the diff before you commit them — recording a golden is
the whole review. The
[user interfaces guide](../../guides/user-interfaces.md#snapshots) covers what
the painter does and does not do.

## Watching

With `--watch`, `buri test` runs the same invocation again every time one of its
inputs changes, until you interrupt it.

**What it watches**, for every selected target: its closure's entry points,
`sources`, `proto_sources` and `testing/` sources; the suite's own `sources`;
every `BUILD.buri` in the repository; and `REPO.buri`. That is the same declared
list the cache keys are already made of. The loop polls each file with one
`stat` every 150 ms, so it acts on a save between 150 and 300 ms after it lands,
and a burst of writes becomes one run rather than twelve. Neither interval is
configurable. Nothing the toolchain writes can wake the loop, because build
output goes under `.buri/`, which is nobody's declared input. There is no ignore
list for `.git/` or `target/` because they were never in.

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
nothing to do prints nothing at all, not even the separator, because a watch
mode that prints on every sweep is one you stop reading. `--explain` turns that
inside out and is at its most useful here: one line per suite per run, saying
`cached` or `run`.

**A run that does not build is a state, not an exit.** A `BUILD.buri` that stops
parsing prints its diagnostics and the loop keeps watching, that file included,
so the run that fixes it happens by itself.

Two combinations are refused before anything opens, each with the reason.
`--force`, because forcing turns every cache hit into a run, and the cache is
what makes a loop this cheap. And no terminal on standard output, because a
watch loop nobody is watching is a hung job.

Interrupting the loop is how it ends, and the shell reports the interrupt rather
than a verdict. Use plain `buri test` when you want a status to branch on.
