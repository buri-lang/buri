---
title: A function has finitely many instantiations
message: '`{name}` needs endless instantiations: {growth}'
fix: '{fix}'
---
# A function has finitely many instantiations

```text
error: `nest` needs endless instantiations: `nest<T>` calls `nest<Option<T>>`, which calls `nest<Option<Option<T>>>`, and so on [endless-function]
```

```buri fail code=endless-function
fn nest<T>(x: T, n: Int): Int {
    if (n == 0) { 0 } else { nest(Option.Some(x), n - 1) + 1 }
}
```

Each instantiation of a generic function compiles on its own, so `nest<Int>`
would need `nest<Option<Int>>`, then a bigger one, forever. Where a function
calls itself, directly or through others, pass its own type parameters, like
`count<T>` calling `count<T>`. Swapped is fine too: `swap<B, A>` inside
`swap<A, B>`.
