---
title: Branches nest shallowly
severity: warning
message: this branch is nested {depth} levels deep
note: the limit is {limit}; an `else if` chain counts as one level, and a lambda body starts again at zero
fix: pull the inner branch into a named function, or replace the ladder with one `match`
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/deep-nesting.md, © 2026 Ivett Ördög, used under the MIT license
---
Deeply nested blocks make you hold every branch condition in your head at once.
The smell is not the indentation. It is that the function branches too much in
one place.

Read the nesting from the inside out and ask what the innermost block actually
needs. Most of the enclosing conditions are usually *guards*: preconditions to
check and bail on early, not to wrap around the real work.

Prefer, in order:

1. **Guard clauses.** Invert a condition and return early so the happy path
   stays at the top indentation level. Each guard you lift removes one level
   from everything below it.
2. **Replace the structure.** A deep `if/else` ladder is often a lookup table or
   a dispatch in disguise. Sometimes an enum and a `match` is better.
3. **Extract a helper.** When an inner block is a coherent sub-step, pull it
   into a named function. The name documents the intent, and the nesting moves
   into a flat, separately readable unit.

Do not merge conditions with `&&` just to drop a level. That trades vertical
nesting for an unreadable horizontal condition.

If the nesting is genuinely irreducible, extracting the inner loops into
well-named helpers is still the move. Keep each function shallow even when the
algorithm as a whole is deep.
