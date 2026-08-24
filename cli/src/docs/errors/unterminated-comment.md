# A block comment is closed

```text
error: unterminated block comment [unterminated-comment]
```

## What to do

close it with `*/`; block comments nest, so each `/*` needs one

## Why

Block comments nest, which is what lets you comment out a region that already
contains one. The cost of nesting is that the lexer counts: a missing `*/`
swallows the rest of the file rather than ending at the first one it finds, so
it is reported where the comment opened rather than where the file ran out.

## A program that provokes it

```buri fail code=unterminated-comment
/* opened and never closed
export fn area(side: Int): Int { side * side }
```
