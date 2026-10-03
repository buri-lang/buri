---
title: Every alternative of an or-pattern must be reachable
message: this alternative is unreachable
note: {covered_by} already covers everything it matches
fix: delete this alternative
---
# Every alternative of an or-pattern must be reachable

```text
error: this alternative is unreachable [unreachable-alternative]
```

```buri fail code=unreachable-alternative
enum Hello {
    World,
    Now(Bool),
}

fn greeting(h: Hello): Str {
    match (h) {
        .Now(_) => "hello world",
        .Now(_) | .World => "now",
    }
}
```

The usual cause is a pattern in the wrong place, and dead text in an arm reads
as handled. An arm with no live alternative is an `unreachable-arm` instead.

Only the arm's top-level `|` alternatives are reported. An alternation nested in
a constructor, as in `.Some(true | false)`, counts toward coverage but isn't
reported branch by branch.
