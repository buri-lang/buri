---
title: A type has finitely many instantiations
message: '`{name}` needs endless instantiations: {growth}'
fix: '{fix}'
---
# A type has finitely many instantiations

```text
error: `Grow` needs endless instantiations: `Grow<T>` contains `Grow<Option<T>>`, which contains `Grow<Option<Option<T>>>`, and so on [endless-type]
```

```buri fail code=endless-type
enum Grow<T> {
    Deeper(Grow<Option<T>>),
    Here(T),
}
```

Each instantiation of a generic type compiles on its own, so `Grow<Int>` would
need `Grow<Option<Int>>`, then a bigger one, forever. Where a type contains
itself, give it its own parameters, like `List<T>` holding a `List<T>`. Swapped
is fine too: `Pair<B, A>` inside `Pair<A, B>`.
