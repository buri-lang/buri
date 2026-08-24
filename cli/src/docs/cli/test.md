## What it does

Builds the targets you name together with their `test.sources`, runs every
`test` declaration in them, and reports one line per failure and a summary.

A test builds its own context, so a suite decides for itself what the code
under test is allowed to do. That is the whole of the mocking story: a test
double is a struct with methods, bound in a context the way the platform's
implementations are bound in `main`.

Exit status is `0` when every test passed and `1` when any did not — so `buri
test` is usable directly as a gate.

## Where a suite runs

Natively, on the host, in the development profile. A suite that says otherwise in
`test { platforms }` gets what it asked for, and `--output=js` says it for one
invocation without editing a build file.

Where a native run is not available the suite runs on JavaScript instead, with
one line on standard error naming the suite and the reason: this toolchain has
no native backend for the host in this profile, or the suite's program reaches
something the backend has no body for yet, or `--accept` is in play — that mode
rewrites a golden file from the two sides of a failed comparison, and only the
JavaScript runner reports them. The fallback is decided per suite, so one suite
on the frontier does not move the rest of the run.

A platform this toolchain cannot produce a binary for is an *error* when a suite
named it and a fallback when nobody did. That asymmetry is the whole rule: a
platform written down is a request, and the default is a preference.

## Watching

With `--watch`, the same invocation is run again every time one of its inputs
changes, until you interrupt it.

**What is watched** is, for every selected target: its closure's entry points,
`sources`, `proto_sources` and `testing/` sources; the suite's own `sources` and
`data`; every `BUILD.buri` in the repository; and `REPO.buri`. That is the
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

Three combinations are refused before anything is opened, each with the reason:
`--force`, because forcing turns every cache hit into a run and the cache is
what makes a loop this cheap; `--accept`, because rewriting golden files on a
timer accepts a regression while you are still reading the failure; and no
terminal on standard output, because a watch loop nothing is watching is a hung
job.

Interrupting the loop is how it ends, and the shell reports the interrupt rather
than a verdict. Use plain `buri test` when you want a status to branch on.
