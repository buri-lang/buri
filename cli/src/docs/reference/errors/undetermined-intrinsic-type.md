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
once, against no Buri type at all, and the key a backend reaches it by carries
no type arguments — so a `[T]` crosses the boundary as two words whatever `T`
is, and the runtime neither reads an element nor frees a block.

That makes the type argument at the call site load-bearing. Everything the
compiler generates around the value the call answers is generated from it: how
wide the value is, and — where it is a block — the walk that releases whatever
the block holds when the last reference to it goes.

A type parameter that appears **only in what the operation answers** has nothing
else to be determined by: no argument carries it, so if the body never looks
inside the answer either, nothing in the program says what came back. The
checker resolves such a type to `()`, which is right for a value the body never
inspects and never received — and wrong here, because the runtime handed back a
real value. A release generated for `()` frees the block and lets go of nothing
inside it, and everything the block was carrying leaks.

That is not a hypothetical. `core/actor`'s `stop` discards whatever is still in
a mailbox, and a discard loop never opens a message — so nothing determined the
message type, and every undelivered payload leaked. The module pops through a
helper that takes the `Address` now: the address carries the message type, so a
loop that never looks inside a message still drops it at the type it was posted
at.
