---
title: Every delimiter a construct opens is closed
message: this {construct} is missing its closing {token}
fix: write {token} here
---
# Every delimiter a construct opens is closed

```text
error: this `match` is missing its closing `}` [unclosed-delimiter]
```

## What to do

Close the construct the second caret points at. The diagnostic carries two
spans: the token that is not the closer, and the delimiter that opened and was
never matched. There is deliberately no edit, because only the person who wrote
the construct can say where the closer belongs.

## Why

An abandoned construct takes the rest of the file with it. Whatever closer comes
next is read as this construct's, the count is off by one from there on, and the
errors that follow are about the miscount rather than about the program. Naming
the opener stops that. The parser reads on as though the closer had been
written, so it still checks the declarations after the mistake and still reports
them on their own terms.

## A program that provokes it

```buri fail code=unclosed-delimiter
fn seeds(): [Int] {
  [1, 2, 3
}
```
