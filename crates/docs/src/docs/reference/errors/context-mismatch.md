---
title: Two contexts are one type when their bindings match
message: expected {expected}, found {found}
fix: build one context and pass it to both, or bind the same effects to the same types in the same order
---
# Two contexts are one type when their bindings match

```text
error: expected `context { Allocator: HostAllocator, Stdout: HostStdout }`, found `context { Stdout: HostStdout, Allocator: HostAllocator }` [context-mismatch]
```

```buri fail code=context-mismatch
# from "core/io" import * as io;
# from "native" import { NativeHost };
# from "platform/effect" import { Allocator, Stdout };
#
struct Greeter<C> {
    greet: fn(C) => (),
}

fn greeter<C: Stdout>(ctx: C): Greeter<C> {
    Greeter { greet: fn(c) => io.println(c, "hi").ignore() }
}

fn greetWith<C: Stdout>(ctx: C, greeter: Greeter<C>): () {
    (greeter.greet)(ctx)
}

export fn main(host: NativeHost): Result<(), Str> {
    let first = context { Allocator: host.alloc, Stdout: host.stdout };
    let second = context { Stdout: host.stdout, Allocator: host.alloc };
    let _ = greetWith(second, greeter(first));
    .Ok(())
}
```

Each `context { ... }` has a type the compiler generates. Two contexts share it
when they bind the same effects, in the same order, to the same types, so
`context { Clock: clock().at(1) }` and `context { Clock: clock().at(2) }` are
interchangeable.

Order counts because a context's bindings are laid out in the order you write
them.
