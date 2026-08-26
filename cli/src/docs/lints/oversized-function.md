---
title: A function is one responsibility
severity: warning
message: "`{name}` is {lines} lines long"
note: a body past {limit} lines is almost always carrying more than one responsibility, and the length is the symptom rather than the fault
fix: find the responsibility boundaries in the body and give each one a function of its own
---
Functions over 12 lines almost always carry more than one responsibility, and that is the smell to chase — not the line count itself.

Analyse responsibilities first: what distinct concerns does this function handle? Ask: (1) Are these separate responsibilities that belong in different methods? (2) Should this become a class with multiple methods? (3) Can you group cohesive data into objects to reduce local variables?

Avoid mechanical extraction. Pulling out a `helperA` / `helperB` purely to satisfy the threshold often hides the smell behind worse names and leaves the real shape untouched. Find true responsibility boundaries.

If responsibilities are tangled you may need to first *inline* methods to see the whole picture before redistributing. Think of this when reducing line count seems particularly hard — stepping backwards often opens up better possibilities.

A concrete technique: write what the method does in one short sentence. Refactor until the code reads as close to that sentence as possible. If you cannot say what it does in one sentence, it almost certainly has more than one responsibility.

Adapted from [habit-hooks](https://github.com/habit-hooks/habit-hooks) `guides/oversized-function.md`, © 2026 Ivett Ördög, used under the MIT license.
