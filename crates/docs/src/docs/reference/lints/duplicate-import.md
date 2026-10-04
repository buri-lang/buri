---
title: A module is imported once
severity: warning
message: "`{module}` is imported twice"
note: two statements for one module drift apart, and the top of the file stops being the whole account of what it borrows
fix: merge the two into one import
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/duplicate-import.md, © 2026 Ivett Ördög, used under the MIT license
---
Merge them into one statement that names everything this file takes from that
module. If the split was deliberate — a type-only import kept separate from a
value import — say that with the language's own type-import syntax rather than
two plain imports that look accidental.
