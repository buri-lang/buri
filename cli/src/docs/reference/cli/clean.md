## What it does

Removes `.buri/out`, the action cache under `.buri/cache`, the object files a
native build stages under `.buri/link/`, and the `out` convenience symlink.
`--outputs` drops `.buri/out` alone. `.buri/link/` comes from the cache, so the
full form drops it too and `--outputs` leaves it.

`.buri/cache` also holds what `buri lint` last found for each target
([`lint.md`](lint.md#what-a-second-run-costs)), so the full form makes your next
lint a cold one.

If you reach for this to fix a build, report it as a bug. Every cache key covers
the content of every input, so a stale entry is a defect rather than a fact of
life.
