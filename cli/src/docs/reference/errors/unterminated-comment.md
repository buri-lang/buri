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

Close it with `*/`. Nesting lets you comment out a region that already contains
a comment, and the cost is that the lexer counts: a missing `*/` swallows the
rest of the file. That is why the error points at where the comment opened
rather than where the file ran out.

## A program that provokes it

```buri fail code=unterminated-comment
/* opened and never closed
export fn area(side: Int): Int { side * side }
```
