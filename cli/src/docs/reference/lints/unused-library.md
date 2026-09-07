---
title: Every source file belongs to a library or a binary
severity: warning
message: "{package_path}/{source} belongs to no library or binary, so nothing builds it"
fix: add it to the library's or the binary's `{field}`, or delete it — {how}
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/unused-file.md, © 2026 Ivett Ördög, used under the MIT license
---
Nothing in the project imports this file. No production code, no test, no entry
point reaches it. Every reader who opens it has to work out whether it matters,
the tooling still parses and type-checks it, and a search for "where is this
used" keeps landing on a dead end.

Decide which of two things it is. If it is genuinely dead — a module orphaned by
a refactor that moved its callers elsewhere — delete it, along with anything
that existed only to support it. Version control remembers.

If it is *meant* to be reachable — a real entry point, a published package
export, a script an outside process invokes — then the file is not the problem,
the missing connection is. Wire it to the entry point that should have reached
it, or declare it as a root in the tool's config. Do not silence the finding by
ignoring the path.
