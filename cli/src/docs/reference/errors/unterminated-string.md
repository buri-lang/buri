---
title: A string literal closes on the line it opens
message: unterminated string literal
fix: close it with `"`; a string literal does not span a line break
---

```buri fail code=unterminated-string wrap=body
let s = "unclosed;
```

The quote swallows the rest of the line, so this is the only error you get. Any
separator or closing delimiter on that line is inside the string, and the parser
won't report it missing.
