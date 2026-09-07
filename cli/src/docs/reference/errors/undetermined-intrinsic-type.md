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

Write the argument out — `list.empty<Int>()` — or use the answer somewhere that
says what is in it.

An operation the runtime supplies has no Buri body. The runtime is compiled
once, against no Buri type at all, and a `[T]` crosses the boundary as two words
whatever `T` is. That makes the type argument at the call site load-bearing: the
compiler generates how wide the answer is, and the walk that releases whatever a
block holds, from it.

A type parameter that appears **only in what the operation answers** has nothing
else to determine it. The checker resolves such a type to `()`, which is right
for a value the body never received, and wrong here, because the runtime handed
back a real value. A release generated for `()` frees the block and lets go of
nothing inside it, so everything the block was carrying leaks.
