---
title: A backslash escape is one of a closed set
message: 'unknown escape `\{escape}`'
note: the escapes are \n \r \t \0 \\ \" \' \$ and \u{{...}}
fix: the escapes are `\n` `\r` `\t` `\0` `\\` `\"` `\'` `\$` and `\u{{...}}`
---

```buri fail code=unknown-escape wrap=body
let s = "\q";
```
