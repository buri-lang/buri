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

## What to do

Drop the `export`.

## Why

Conformance belongs to the type: once `Version` is visible, everything
`impl Equal for Version` supplies is visible with it. Withholding a method the
trait requires would be a conformance that does not hold.

## A program that provokes it

An `impl` block for the type's own methods is the other case, and `export`
means something there.

```buri fail code=impl-method-export
# from "core/order" import { Equal };
export struct Version { export major: Int }

impl Equal for Version {
  export fn equal(self, other: Version): Bool { self.major == other.major }
}
```
