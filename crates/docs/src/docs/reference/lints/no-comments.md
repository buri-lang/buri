---
title: Only doc comments are written
severity: warning
message: this is a comment
note: this repository turned on `no-comments`, which allows only `///` and `//!` doc comments
fix: say it in a name, a type or a test, or make it a doc comment if it documents an API
---
Off unless a repository turns it on:

```textproto schema=repo
lint {
    rules {
        no_comments: true
    }
}
```

Then every `//` and `/* */` comment is a finding, nested blocks included:

```buri lint code=no-comments
// Cents, because floats round.
export fn cents(dollars: I64): I64 {
    dollars * 100
}
```

Doc comments stay allowed, since they're API documentation:

```buri
/// `dollars` in cents, because floats round.
export fn cents(dollars: I64): I64 {
    dollars * 100
}
```

Text inside a string is never a comment. There's no `--fix`: deleting a
comment would lose what it said.
