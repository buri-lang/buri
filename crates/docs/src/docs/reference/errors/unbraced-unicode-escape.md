---
title: A Unicode escape braces its code point
message: '`\u` must be followed by `{{`, as in `\u{{1F600}}`'
fix: 'brace the code point: `\u{{1F600}}`'
---

```buri fail code=unbraced-unicode-escape wrap=body
let s = "\u0041";
```
