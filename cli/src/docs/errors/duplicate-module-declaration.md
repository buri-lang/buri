---
title: A name has one meaning in a module
message: '`{name}` is declared twice in this module'
fix: rename one of them; a name has one meaning in a module
---

```buri fail code=duplicate-module-declaration
struct Point {
    export x: Int,
}

struct Point {
    export y: Int,
}
```
