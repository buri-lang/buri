---
title: Branches nest shallowly
severity: warning
message: this branch is nested {depth} levels deep
note: the limit is {limit}; an `else if` chain counts as one level, and a lambda body starts again at zero
fix: pull the inner branch into a named function, or replace the ladder with one `match`
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/deep-nesting.md, © 2026 Ivett Ördög, used under the MIT license
---
Deep nesting makes you hold every condition in your head at once. The problem
isn't the indentation; it's a function that branches too much in one place.

Read from the inside out and ask what the innermost block needs. Most enclosing
conditions are guards to check and bail on, not to wrap the real work. Prefer,
in order:

1. **Guard clauses.** Invert a condition and return early. Each one lifts
   everything below it a level.
2. **Replace the structure.** A deep `if`/`else` ladder is often a lookup table,
   or an enum and a `match`.
3. **Extract a helper.** Pull a coherent inner step into a named function.

Don't merge conditions with `&&` just to drop a level; that trades nesting for
an unreadable condition. When the algorithm really is deep, still keep each
function shallow by extracting inner loops into well-named helpers.
