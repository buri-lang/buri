# A private declaration is private to its module

```text
error: field `0` of `Scope` is private to its module [private-to-module]
```

## What to do

add `export` to the field, or go through a method `Scope` provides

## Why

`export` is the whole of visibility: a declaration, a field, a variant or a
method that does not carry it is reachable only from the module that wrote it.
That is one rule, so the code is one code — the same error covers a function
you cannot call, a field you cannot name, and a variant you cannot construct or
match.

The unusual half is that **a struct with any private field cannot be
constructed anywhere else at all**, not merely read: writing `Scope(0)` names
the hidden field. That is what makes a private field an invariant rather than
an inconvenience, and it is how the standard library mints a type only from the
inside. `ui/effect`'s `Scope` is the worked example — a reactive closure is
handed one by the runtime, and no program can build one, which is what
"a closure can never capture a context" rests on.

Functional update still works, because it never names the hidden fields.

## A program that provokes it

```buri fail code=private-to-module
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;
from "ui/effect" import { Scope, Ui, Watch };
from "ui/signal" import { Signal, signal };

fn peek(n: Signal<Int>): Int {
  n.get(Scope(0))
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Ui: host.ui, Watch: host.watch };
  let n = signal(ctx, 1);
  let _ = peek(n);
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces
`private-to-module` — so this page cannot describe an error the compiler has
stopped emitting.
