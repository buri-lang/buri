---
title: A function is one responsibility
severity: warning
message: "`{name}` is {lines} lines long"
note: a body past {limit} lines is almost always carrying more than one responsibility, and the length is the symptom rather than the fault
fix: find the responsibility boundaries in the body and give each one a function of its own
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/oversized-function.md, © 2026 Ivett Ördög, used under the MIT license
---
An array literal's elements and a `match`'s arms count as one line, because a
table is one row per member and has no responsibility boundary to split at.

Work out the responsibilities before you touch anything. Ask yourself:

- Are these separate responsibilities that belong in functions of their own?
- Should this become a type with several methods?
- Can you group cohesive data into a value of its own and cut the local
  variables down?

Do not extract mechanically. Pulling out a `helperA` and a `helperB` just to
clear the threshold hides the smell behind worse names.

If the responsibilities are tangled, *inline* the helpers first so you can see
the whole picture before you redistribute it.

One concrete technique: write what the function does in one short sentence, then
refactor until the code reads as close to that sentence as you can get it. If
you cannot say what it does in one sentence, it almost certainly has more than
one responsibility.
