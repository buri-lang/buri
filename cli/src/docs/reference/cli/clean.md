## What it does

Removes `.buri/out`, the action cache under `.buri/cache`, the object files a
native build stages under `.buri/link/`, and the `out` convenience symlink.
`--outputs` drops `.buri/out` alone; `.buri/link/` is derived from the cache and
goes with it, so the full form drops it and `--outputs` does not.

`.buri/cache` holds what `buri lint` last found for each target as well as what
the build produced ([`lint.md`](lint.md#what-a-second-run-costs)), so the full
form is also what makes the next lint a cold one.

Reaching for this to fix a build is worth reporting as a bug: the cache is
keyed on the content of every input, so a stale entry is a defect rather than a
fact of life.
