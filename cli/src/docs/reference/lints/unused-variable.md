---
title: Every `let` names something the code below it reads
severity: warning
message: "`{name}` is bound and never used"
note: a binding nothing reads is either a computation whose result is dead or a value that was meant to be wired in and was not
fix: use it, delete the `let`, or write `_` in its place
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/unused-variable.md, © 2026 Ivett Ördög, used under the MIT license
---
An unused local is dead weight: every reader has to prove to themselves that it
does not matter. It is usually one of three things.

- A computation whose result nobody consumes. Delete the computation, not just
  the binding.
- A leftover from a refactor that moved the logic elsewhere.
- A value you meant to use and forgot to wire in. That is the real bug.

Decide which it is before deleting. If the right-hand side has side effects you
still need, keep the call but drop the binding. If the value was meant to be
returned or passed on, finish that thread rather than silencing the warning.
