# An `impl` method is not separately exported

```text
error: an `impl` method is not separately exported [impl-method-export]
```

## What to do

drop the `export`

## Why

conformance is a property of the type, visible wherever the type is

## A program that provokes it

```buri fail code=impl-method-export
// A method of the type's own is exported on its own terms. A method that
// satisfies a trait is not: conformance belongs to the type, so the method is
// visible wherever the type is and there is nothing left for `export` to say.
from "core/effect" import { Alloc, Stdout };
from "core/order" import { Eq };
from "core/host" import * as host;

export struct Version { export major: Int }

impl Eq for Version {
  export fn eq(self: Version, other: Version): Bool { self.major == other.major }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${Version { major: 1 } == Version { major: 1 }}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `impl-method-export` — so
this page cannot describe an error the compiler has stopped emitting.
