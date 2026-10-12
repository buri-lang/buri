# Examples that lie

Each fence below claims something its code doesn't do.

```buri
export fn broken(): I64 {
    "not a number"
}
```

```buri use=nowhere
export fn one(): I64 {
    1
}
```

```buri fail code=type-mismatch
export fn fine(): I64 {
    1
}
```

```buri fail code=type-mismatch
export fn unknown(): I64 {
    missing
}
```

```buri fail
export fn quiet(): I64 {
    "text"
}
```

```error
a message nobody prints
```

```buri fail
export fn clean(): I64 {
    1
}
```

```error
a message nobody prints
```

```buri
export fn one(): I64 {
    let unused: I64 = 1; // ERROR: expected a mismatch
    1
}
```

```buri
export fn two(): I64 {
    let unused: I64 = "two"; // ERROR: a message nobody prints
    1
}
```

```buri lint code=todo-comment
export fn three(): I64 {
    3
}
```

```buri run
from "core/io" import * as io;
from "native" import { NativeHost };
# from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, "a latte").mapErr(fn(e) => "no")
}
```

```stdout
a mocha
```

```buri package=//lib/nowhere
export fn four(): I64 {
    4
}
```

```buri repo=missing
export fn five(): I64 {
    5
}
```

A library's own example sees its private names.

```buri package=//lib/money
from "//lib/money" import { latte };

export fn twice(): I64 {
    latte() * 2
}
```

An expression, and a body that prints.

```buri wrap=expr
1 + 1
```

```buri wrap=body ctx=alloc,stdout
let greeting = "hello";
```

A block a later block builds on.

```buri name=base
export fn base(): I64 {
    10
}
```

```buri use=base
export fn more(): I64 {
    base() + 1
}
```

A finding other than the one claimed.

```buri lint code=dead-code
// TODO: price the mocha
export fn mocha(): I64 {
    500
}
```

A lint example and a program that don't compile aren't linted or run.

```buri lint code=todo-comment
export fn cortado(): I64 {
    "390"
}
```

```buri run
export fn main(): I64 {
    "not run"
}
```

```stdout
never
```

Failures that say what they fail with one way each, and say it truly.

```buri fail
export fn flat(): I64 {
    let white: I64 = "a flat white"; // ERROR: expected `I64`
    1
}
```

```buri fail code=type-mismatch
export fn ristretto(): I64 {
    let short: I64 = "short"; // ERROR: expected `I64`
    1
}
```

```buri fail code=type-mismatch
export fn macchiato(): I64 {
    "marked"
}
```
