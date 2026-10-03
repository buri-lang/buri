---
title: A function is one responsibility
severity: warning
message: "`{name}` is {lines} lines long"
note: a body past {limit} lines is almost always carrying more than one responsibility, and the length is the symptom rather than the fault
fix: find the responsibility boundaries in the body and give each one a function of its own
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/oversized-function.md, © 2026 Ivett Ördög, used under the MIT license
---
An array literal's elements and a `match`'s arms count as one line, since a
table has no boundary to split at.

Find the responsibilities before you touch anything:

- Are there separate responsibilities that deserve their own functions?
- Should this become a type with several methods?
- Can cohesive data become its own value, cutting the local variables?

Don't extract mechanically: a `helperA` and `helperB` that only clear the
threshold hide the problem behind worse names. If the responsibilities are
tangled, inline the helpers first to see the whole picture.

Write what the function does in one short sentence, then refactor until the
code reads like it. If you can't write that sentence, it has more than one
responsibility.
