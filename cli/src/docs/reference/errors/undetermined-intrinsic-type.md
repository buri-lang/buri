---
title: A runtime operation is called at a type the body determines
message: '`{function}` is called at a type nothing determines: {parameters}'
label: nothing here says what this answers
note: a runtime operation is compiled once against no Buri type, so the type argument written at the call is the only record of what the value it answers holds
fix: use the value the call answers at the type it really has, or write the type argument out
---

```buri fail code=undetermined-intrinsic-type
from "core/list" import * as list;

export fn go(): Int {
    let nothing = list.empty();
    7
}
```

Write the argument out, as in `list.empty<Int>()`, or use the answer somewhere
that says what it holds.

A `[T]` crosses into the runtime as two words whatever `T` is, so the compiler
derives the answer's width, and the code that releases what it holds, from the
type argument at the call.

A type parameter that appears only in the answer has nothing else to pin it, so
the checker would resolve it to `()`. A release generated for `()` frees the
block but nothing inside it, so its contents leak.
