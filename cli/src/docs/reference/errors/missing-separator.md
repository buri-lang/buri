---
title: A list separates its elements with `,`
message: {construct} ends with `,`
fix: write `,` here
---
# A list separates its elements with `,`

```text
error: a match arm ends with `,` [missing-separator]
```

## What to do

Write the `,` where the caret is. The error carries the edit as bytes, so an
editor's quick fix and `buri lint --fix` both write it for you, and the rest of
the list is read for what it says rather than abandoned.

## Why

Every comma-separated list in the language is separated the same way, with no
exception for an element that happens to end in a brace. A match arm whose body
is a block still ends with `,`, because without it `A => x` followed by
`-1 => y` reads as `x - 1` and where one arm stops becomes a guess (design/grammar-rationale.md 12.12).

One rule, stated once, is also what lets the parser tell a separator that is
missing from a list that has ended: the next element is already under the
cursor, so the mistake is one comma and the diagnostic is one sentence.

## A program that provokes it

```buri fail code=missing-separator
fn pick(n: Int): Int {
  match (n) {
    1 => 1
    _ => 0,
  }
}
```
