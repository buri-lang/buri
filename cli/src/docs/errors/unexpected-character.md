---
title: Every byte of a source file starts a token
message: 'unexpected character `{character}` (U+{code_point})'
fix: delete it; no token in the language starts with it
---

```buri fail code=unexpected-character wrap=body
let n = 1 # 2;
```
