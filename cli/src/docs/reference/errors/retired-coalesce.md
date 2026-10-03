---
title: A default for an absent value is `withDefault`
message: '`??` is retired'
note: the operator was a second spelling of a method that already existed, and the only one that needed a right-associative rung of its own in the grammar
fix: write the default as a method call, as in `x.withDefault(0)`
---
# A default for an absent value is `withDefault`

```text
error: `??` is retired [retired-coalesce]
```

```buri fail code=retired-coalesce
fn portOr(port: Option<Int>): Int {
    port ?? 8080
}
```

`Option<T>` and `Result<T, E>` both have `withDefault`:

```buri
fn firstOr(xs: [Int], fallback: Int): Int {
    xs.get(0).withDefault(fallback)
}
```

`a ?? b ?? c` becomes `a.withDefault(b.withDefault(c))`, or
`a.or(b).withDefault(c)` when `a` and `b` are both `Option<T>`.

Unlike `??`, `withDefault` always evaluates its default. When the default is
expensive, write the `match` out, so it runs only in the absent branch:

```buri
fn portOr(port: Option<Int>): Int {
    match (port) {
        .Some(p) => p,
        .None => expensiveDefault(),
    }
}

fn expensiveDefault(): Int {
    8080
}
```
