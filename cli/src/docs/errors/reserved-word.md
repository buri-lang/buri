# Reserved words are not identifiers

```text
error: `return` is a reserved word and may not be used as an identifier [reserved-word]
```

## What to do

pick another name; `return` is not available

## Why

reserved for a future version of Buri; see grammar.ebnf, ReservedWord

## A program that provokes it

```buri fail code=reserved-word
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

fn return(n: Int): Int { n }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `reserved-word` — so
this page cannot describe an error the compiler has stopped emitting.
