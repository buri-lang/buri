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
never matched. There is deliberately no edit, because where the closer belongs
is exactly what a compiler cannot know — it is the one thing only the person
who wrote the construct can say.

## Why

A construct that is abandoned takes the rest of the file with it: whatever
closer comes next is read as this construct's, the count is off by one from
there on, and the errors that follow are about the miscount rather than about
the program. Naming the opener is what stops that. The parser reads on as though
the closer had been written, so the declarations after the mistake are still
checked and still reported on their own terms.

## A program that provokes it

```buri fail code=unclosed-delimiter
fn seeds(): [Int] {
  [1, 2, 3
}
```
