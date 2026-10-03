---
title: Every source file belongs to a library or a binary
severity: warning
message: "{package_path}/{source} belongs to no library or binary, so nothing builds it"
fix: add it to the library's or the binary's `{field}`, or delete it — {how}
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/unused-file.md, © 2026 Ivett Ördög, used under the MIT license
---
Nothing builds or reaches this file, yet every reader has to work out whether it
matters, and "where is this used" searches dead-end on it.

- **It's dead**, say orphaned by a refactor. Delete it, with anything that only
  supported it.
- **It's meant to be reachable**: an entry point, a published export, a script
  something else runs. Add it to the library or binary that should build it.
  Don't silence the finding by ignoring the path.
