## What it does

Removes `.buri/out`, the action cache under `.buri/cache`, the object files a
native build stages under `.buri/link/`, and the `out` symlink. `--outputs`
removes only `.buri/out`.

`.buri/cache` also holds `buri lint`'s records
([`lint.md`](lint.md#what-a-second-run-costs)), so your next lint starts cold.

If you need this to fix a build, report a bug. Every cache key covers the
content of every input, so a stale entry is a defect.
