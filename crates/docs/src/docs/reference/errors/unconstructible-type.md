---
title: Every type can be built
message: '`{name}` can never be built: {reason}'
fix: '{fix}'
---
# Every type can be built

```text
error: `Loop` can never be built: it always contains another `Loop` (through `next`) [unconstructible-type]
```

```buri fail code=unconstructible-type
struct Loop {
    next: Loop,
}
```

Building a `Loop` needs a `Loop` already, so give it a way to end: an `Option`
field, or a variant that doesn't hold the type. `enum Wrap { W(Option<Wrap>) }`
is fine, because `.W(.None)` is a value.

An enum with no variants can't be built either, and nor can types that only
hold each other.

```buri fail code=unconstructible-type
enum Never {}

enum Ping {
    Ping(Pong),
}

enum Pong {
    Pong(Ping),
}
```

Where a `Result` can't fail, return the value itself rather than a `Result` whose
error type can't be built.
