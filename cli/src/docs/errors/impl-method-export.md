# An `impl` method is not separately exported

```text
error: an `impl` method is not separately exported [impl-method-export]
```

## What to do

Drop the `export`.

## Why

Conformance belongs to the type. Once `Version` is visible, everything
`impl Eq for Version` supplies is visible with it, so there is nothing left for
`export` to decide — and a method the trait requires that was somehow withheld
would be a conformance that does not hold.

## A program that provokes it

An `impl` block for the type's own methods is the other case, and `export`
means something there.

```buri fail code=impl-method-export
# from "core/order" import { Eq };
export struct Version { export major: Int }

impl Eq for Version {
  export fn eq(self: Version, other: Version): Bool { self.major == other.major }
}
```
