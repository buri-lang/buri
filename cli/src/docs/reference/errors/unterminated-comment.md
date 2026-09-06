---
title: A block comment is closed
message: unterminated block comment
fix: close it with `*/`; block comments nest, so each `/*` needs one
---
# A block comment is closed

```text
error: unterminated block comment [unterminated-comment]
```

## What to do

Close it with `*/`. Block comments nest, so each `/*` needs one.

## Why

Nesting is what lets you comment out a region that already contains a comment.
It costs you this: the lexer counts, so a missing `*/` swallows the rest of the
file instead of stopping at the first one it finds. That is why the error points
at where the comment opened rather than where the file ran out.

## A program that provokes it

```buri fail code=unterminated-comment
/* opened and never closed
export fn area(side: Int): Int { side * side }
```
