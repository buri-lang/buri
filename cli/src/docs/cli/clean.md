## What it does

Removes `.buri/out`, the action cache under `.buri/cache`, and the `out`
convenience symlink.

Reaching for this to fix a build is worth reporting as a bug: the cache is
keyed on the content of every input, so a stale entry is a defect rather than a
fact of life.
