---
title: A program doesn't import the compiler's own modules
message: '"{path}" is internal to the standard library'
note: the compiler loads this module and calls it on a program's behalf
fix: remove the import; `buri docs` lists the modules a program can import
---
# A program doesn't import the compiler's own modules

```text
error: "ui/style/classify" is internal to the standard library [internal-std-import]
```

```buri fail code=internal-std-import
from "ui/style/classify" import * as classify;
```

`ui/style/classify` turns a style the compiler couldn't pin down into the
stylesheet's classes. Hand a node a `Style` and the compiler calls it for you.
