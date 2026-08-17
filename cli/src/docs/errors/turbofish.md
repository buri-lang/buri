# Type arguments are written without `::`

```text
error: type arguments in an expression are written without `::` [turbofish]
```

## What to do

remove the `::`, as in `list.empty<Int>()`

## Why

Explicit type arguments in an expression used to be written `f::<T>(x)`, the
turbofish, because a bare `<` in expression position was always the comparison
operator and a parser had no way to tell `f<A>(x)` from `(f < A) > (x)`.

It has one now, and it comes from a rule the language already had for its own
reasons: comparison is **non-associative**, so `a < b > c` is not a program
under the comparison reading either. There is no source the two readings both
accept and disagree about, which is what makes `f<T>(x)` safe to read as a
call. The `::` was the price of an ambiguity that turned out not to be one.

Two spellings of one thing would be worse than either, so the old one is an
error rather than a second way to write it. The error carries the edit — delete
the `::` — as bytes, so `buri lint --fix` and an editor's quick fix migrate a
file that still has it.

## A program that provokes it

```buri fail code=turbofish
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/list" import * as list;

fn empty(): [Int] {
  list.empty::<Int>()
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${empty().len()}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `turbofish` —
so this page cannot describe an error the compiler has stopped emitting.
