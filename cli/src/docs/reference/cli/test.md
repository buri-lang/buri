## What it does

Builds the targets you name together with their `test.sources`, runs every
`test` declaration in them, and reports one line per failure and a summary.

A test builds its own context, so a suite decides for itself what the code
under test is allowed to do. That is the whole of the mocking story: a test
double is a struct with methods, bound in a context the way the platform's
implementations are bound in `main`.

Exit status is `0` when every test passed and `1` when any did not — so `buri
test` is usable directly as a gate.

## Lint findings

A test run reports the lint catalogue as well where `REPO.buri` asks it to.
`lint { check_during_build: true }` runs the checks `buri lint` runs over the
targets being tested and reports them alongside the verdicts; adding
`fail_on_finding: true` makes a finding an error, and the run fails on one the
way it fails on a failing test. Both default to false, and a repository that
writes neither gets exactly the run described above.

`buri test` is worth opting in on for the same reason `buri build` is, and a
little more so: it is run more often than anything else, and a test suite is
where a helper quietly grows past `oversized-function` first. The fields are
documented in [`repo-config.md`](../build/repo-config.md#lint).

A rule the same block turns off in [`rules`](../build/repo-config.md#rules) is
not reported here either, and a run under a smaller catalogue says which rules
those were.

## Where a suite runs

Natively, on the host, in the development profile. A suite that says otherwise in
`test { platforms }` gets what it asked for, and `--output=js` says it for one
invocation without editing a build file.

Those two are the whole list. Nothing else moves a suite: a suite that named no
platform runs natively or does not run.

Where a native run is not available because this toolchain has no backend for
the host in this profile, no runtime archive, or no C compiler to link with, the
suite is **refused** with `native-run-not-available`, naming the platform and
the profile that were asked for. `--release` is the case worth knowing about:
the release profile routes to LLVM, so a toolchain built without
`backend-llvm` refuses `buri test --release` rather than quietly handing it to
the development backend.

**A program the native backend has no body for is refused too**, naming the
intrinsic and the backend. Both used to be a fallback onto JavaScript with a
line on standard error, and it was the wrong one: the suite then passed on a
backend nobody chose, which is how a named gap turns into a wrong answer rather
than into a report — and the line saying so went to a stream that nobody reads
when a run is green. A suite that belongs on JavaScript says so with
`test { platforms: [JS] }`; anything else is a toolchain bug worth hearing
about.

## Watching

With `--watch`, the same invocation is run again every time one of its inputs
changes, until you interrupt it.

**What is watched** is, for every selected target: its closure's entry points,
`sources`, `proto_sources` and `testing/` sources; the suite's own `sources`;
every `BUILD.buri` in the repository; and `REPO.buri`. That is the
declared list the cache keys are already made of, polled with one `stat` each
every 150 ms, so a save is acted on between 150 and 300 ms after it lands and a
burst of writes is one run rather than twelve. Neither interval is
configurable. Nothing the toolchain writes can wake the loop, because build
output goes under `.buri/`, which is nobody's declared input — and there is no
ignore list for `.git/` or `target/`, because they were never in.

**A new file is not watched until something declares it.** Sources are explicit
lists rather than globs, so a file you have just created is an input of nothing.
What is watched is the `BUILD.buri` that will name it: run `buri gen`, and the
loop sees the build file change and picks the new source up with everything
else.

**Each run is separated by one line**, carrying the time it was triggered, which
run it is, and the file that moved:

```text
── 14:02:31Z  run 7  lib/money/cents.buri ──────────────────────
```

The time is UTC and says so. The screen is never cleared, and a run that had
nothing to do prints nothing at all — not even the separator — because a watch
mode that prints on every sweep is one you stop reading. `--explain` turns that
inside out and is at its most useful here: one line per suite per run, saying
`cached` or `run`.

**A run that does not build is a state, not an exit.** A `BUILD.buri` that stops
parsing prints its diagnostics and the loop keeps watching — including that file
— so the run that fixes it happens by itself.

Two combinations are refused before anything is opened, each with the reason:
`--force`, because forcing turns every cache hit into a run and the cache is
what makes a loop this cheap; and no terminal on standard output, because a
watch loop nothing is watching is a hung job.

Interrupting the loop is how it ends, and the shell reports the interrupt rather
than a verdict. Use plain `buri test` when you want a status to branch on.
