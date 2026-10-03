---
title: Every `let` names something the code below it reads
severity: warning
message: "`{name}` is bound and never used"
note: a binding nothing reads is either a computation whose result is dead or a value that was meant to be wired in and was not
fix: use it, delete the `let`, or write `_` in its place
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/unused-variable.md, © 2026 Ivett Ördög, used under the MIT license
---
An unused local is usually one of three things:

- A result nobody consumes. Delete the computation, not just the binding.
- A leftover from a refactor.
- A value you meant to use and forgot to wire in. That's the real bug.

Decide which before deleting. If the right-hand side has side effects you need,
keep the call and drop the binding. If the value was meant to be returned or
passed on, finish that instead of silencing the warning.
