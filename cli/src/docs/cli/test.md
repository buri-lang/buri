## What it does

Builds the targets you name together with their `test.sources`, runs every
`test` declaration in them, and reports one line per failure and a summary.

A test builds its own context, so a suite decides for itself what the code
under test is allowed to do. That is the whole of the mocking story: a test
double is a struct with methods, bound in a context the way the platform's
implementations are bound in `main`.

Exit status is `0` when every test passed and `1` when any did not — so `buri
test` is usable directly as a gate.

## Watching

With `--watch`, the same invocation is run again every time one of its inputs
changes, until you interrupt it.

There is no file watcher. The build already enumerates every file each suite's
cache key is computed from, so the loop polls *that list* — a few hundred paths
— with one `stat` each, every 150 ms, and runs again once a change has settled
for one further quiet sweep. A save is therefore acted on between 150 and 300 ms
after it lands, and a burst of writes — a formatter rewriting twelve files, a
branch switch — is one run rather than twelve. Neither number is configurable:
this is a sweep interval rather than a debounce, and it is not a value anybody
could choose better.

Two things follow from watching a declared list rather than a directory tree,
and both are ordinarily work a watch mode has to do:

- Nothing the toolchain writes can wake the loop. Build output goes under
  `.buri/`, which is nobody's declared input.
- `.git/`, `target/` and `node_modules/` are not watched, and there is no
  ignore list — they were never in.

**What is watched** is, for every selected target: its closure's entry points,
`sources`, `proto_sources` and `testing/` sources; the suite's own `sources` and
`data`; every `BUILD.buri` in the repository; and `REPO.buri`. The first two are
exactly what the keys are made of, so a change that would not move a key is a
change the loop does not have to see. The last two are what decide *which* keys
exist — a new dependency edge, a new source, a changed tag vocabulary — so each
run opens the repository afresh.

**A new file is not watched until something declares it.** Sources are explicit
lists that `buri gen` maintains, not globs, so a file you have just created is
not an input of anything and appears in no key. What is watched is the
`BUILD.buri` that will name it: run `buri gen`, and the loop sees the build file
change and picks the new source up with everything else. There is no glob
watching here and no directory crawl to make one out of.

**Each run is separated by one line**, carrying the time it was triggered, which
run it is, and the file that moved:

```text
── 14:02:31Z  run 7  lib/money/cents.buri ──────────────────────
```

The time is UTC and says so: the toolchain carries no timezone database and will
not grow one for a header. The screen is never cleared — scrollback holds the
failure you are fixing and the run you are comparing against — and a run that
had nothing to do prints nothing at all, not even the separator. If every suite
came out of the cache and every one passed, the loop is silent, because a watch
mode that prints on every sweep is one you stop reading. `--explain` turns that
inside out and is at its most useful here: one line per suite per run, saying
`cached` or `run`.

**A run that does not build is a state, not an exit.** A `BUILD.buri` that stops
parsing prints its diagnostics and the loop keeps watching — including that file
— so the run that fixes it happens by itself.

Three combinations are refused before anything is opened, each with the reason:

- with `--force`, because forcing turns every cache hit into a run, and the
  cache is the whole of what makes a loop this cheap;
- with `--accept`, because accepting writes to the source tree, and rewriting
  golden files on a timer accepts a regression while you are still reading the
  failure;
- without a terminal on standard output, because a watch loop nothing is
  watching is a hung job — in CI, a build that never finishes. Run `buri test`,
  which is the same selection run once.

Interrupting the loop is how it ends, and the shell will report the interrupt
rather than a verdict: the exit status of a watch session is about the session,
and the state of your suites is on the screen above it. Use plain `buri test`
when you want a status to branch on.
