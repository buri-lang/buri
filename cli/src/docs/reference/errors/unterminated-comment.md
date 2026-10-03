---
title: A block comment is closed
message: unterminated block comment
fix: close it with `*/`; block comments nest, so each `/*` needs one
---
# A block comment is closed

```text
error: unterminated block comment [unterminated-comment]
```

```buri fail code=unterminated-comment
/* opened and never closed
export fn area(side: Int): Int { side * side }
```

Nesting lets you comment out code that already holds a comment, but a missing
`*/` swallows the rest of the file. So the error points at where the comment
opened, not where the file ran out.
