---
title: A type's arguments are named in full or not at all
message: '`{type}` takes {expected} type arguments'
---

```buri fail code=type-argument-arity
struct Pair<A, B> {
    export a: A,
    export b: B,
}

derive Eq for Pair<Int>;
```
