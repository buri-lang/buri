---
title: A call resolves to a concrete type
message: '{function} is called at a type nothing determines'
fix: annotate the call with type arguments, as in `f<Str>(x)`
---

```buri fail code=undetermined-type
from "core/list" import * as list;

export fn go(): Int {
    let nothing = list.empty();
    7
}
```

Write the argument out, as in `list.empty<Int>()`, or use the answer somewhere
that says what it holds.

A type parameter that appears only in the answer has nothing else to pin it, so
the checker would resolve it to `()`. That matters most for a runtime operation.
A `[T]` crosses into the runtime as two words whatever `T` is, so the compiler
derives the answer's width, and the code that releases what it holds, from the
type argument at the call. A release generated for `()` frees the block but
nothing inside it, so its contents leak.
