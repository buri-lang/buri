---
title: A type with no finite value cannot be constructed
message: `{name}` can never be constructed
note: every variant recurses, so building one would need one already
fix: give `{name}` a variant that does not mention itself, the way `.None` terminates an `Option`
---
# A type with no finite value cannot be constructed

```text
error: `Endless` can never be constructed [uninhabited]
```

```buri fail code=uninhabited
enum Endless {
    Node(Endless),
}
```

There is no laziness and no null to break the cycle, so the compiler reports it
at the declaration.
