---
title: A string literal closes on the line it opens
message: unterminated string literal
fix: close it with `"`; a string literal does not span a line break
---

```buri fail code=unterminated-string wrap=body
let s = "unclosed;
```

The quote swallows the rest of the line, so this is the only error you get for
it. A separator, terminator or closing delimiter the parser then misses may be
sitting inside the string, and it says nothing about a token you may well have
written.
