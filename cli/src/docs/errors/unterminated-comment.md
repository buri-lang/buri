# A block comment is closed

```text
error: unterminated block comment [unterminated-comment]
```

## What to do

Close it with `*/`. Block comments nest, so each `/*` needs one.

## Why

Nesting is what lets you comment out a region that already contains a comment.
Its cost is that the lexer counts, so a missing `*/` swallows the rest of the
file rather than ending at the first one it finds — which is why this is
reported where the comment opened rather than where the file ran out.

## A program that provokes it

```buri fail code=unterminated-comment
/* opened and never closed
export fn area(side: Int): Int { side * side }
```
