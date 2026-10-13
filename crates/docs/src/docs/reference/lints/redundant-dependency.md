---
title: Each dependency is listed once, and never the package itself
severity: warning
message: "{target} {fault}"
fix: delete the entry from `dependencies`
---
An entry for the package itself, or for a dependency already listed, adds no
edge. It only makes the next reader wonder what it was for.

- `dependencies: ["//lib/b", "//lib/b"]` names `//lib/b` twice.
- In `//lib/b`, `dependencies: ["//lib/b"]` names the package to itself.
