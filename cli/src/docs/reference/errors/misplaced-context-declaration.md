---
title: A `context` is declared only in an entry or test code
message: a `context` declaration is not allowed here
note: context may be declared only in a main.buri, a test file, or a test-only module
fix: Accept a context variable as an argument, and pass it into the function from a main.buri, a test file, or a test-only module
---

```buri fail code=misplaced-context-declaration
# //libs/print/lib.buri
from "core/alloc" import * as alloc;
from "core/str" import * as str;
from "platform/effect" import { Allocator };

context Budget {
    Allocator: alloc.generalPurpose(),
}

export fn shout(text: Str): Str {
    str.format(Budget(), "${text}!")
}
```

To fix, accept the context as an argument:

```buri
from "core/str" import * as str;
from "platform/effect" import { Allocator };

export fn shout<C: Allocator>(ctx: C, text: Str): Str {
    str.format(ctx, "${text}!")
}
```
