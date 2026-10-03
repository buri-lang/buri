---
title: A rest pattern comes last
message: a rest pattern must come last
note: `[first, ..rest]` is legal; `[..init, last]` is not
fix: move `..` to the end, as in `[first, ..rest]`; matching a prefix is what an array pattern does
---
# A rest pattern comes last

```text
error: a rest pattern must come last [rest-pattern-not-last]
```

```buri fail code=rest-pattern-not-last
fn lastOf(xs: [Int]): Int {
  match (xs) {
    [..init, last] => last,
    _ => 0,
  }
}
```

An array pattern matches a prefix and binds the remainder. A rest in the middle
would turn matching into a search.
