---
title: An `impl` method is not separately exported
message: an `impl` method is not separately exported
note: conformance is a property of the type, visible wherever the type is
fix: drop the `export`
---
# An `impl` method is not separately exported

```text
error: an `impl` method is not separately exported [impl-method-export]
```

```buri fail code=impl-method-export
# from "core/order" import { Equal };
export struct Version { export major: Int }

impl Equal for Version {
  export fn equal(self, other: Version): Bool { self.major == other.major }
}
```

Wherever the type is visible, every method of its trait `impl`s is too.
Hiding one would break the conformance. In an `impl` block for the type's own
methods, `export` does mean something.
