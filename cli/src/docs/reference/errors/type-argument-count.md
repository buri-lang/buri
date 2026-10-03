---
title: A type takes the arguments it declares
message: '{subject} takes {expected}, but {given}'
fix: supply exactly {expected}
---

```buri fail code=type-argument-count
struct Pair<A, B> {
    export a: A,
    export b: B,
}

fn take(p: Pair<Int>): Int {
    0
}
```

```buri fail code=type-argument-count
fn identity<T>(x: T): T {
    x
}

fn go(): Int {
    identity<Int, Str>(1)
}
```

```buri fail code=type-argument-count
struct Pair<A, B> {
    export a: A,
    export b: B,
}

derive Equal for Pair<Int>;
```

```buri fail code=type-argument-count
fn width(n: Int<Str>): Int {
    0
}
```

```buri fail code=type-argument-count
fn first<T>(xs: T<Int>): Int {
    0
}
```
