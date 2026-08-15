# A module path names exactly one file

```text
error: "//cmd/discarded_result" names no file (cmd/discarded_result/lib.buri) [module-not-found]
```

## What to do

create the file the path names, or correct the path — a module path maps to exactly one file, with no search

## A program that provokes it

A module path resolves against the repository, so this one is compiled against
the worked monorepo in `cli/tests/example`, where `//lib/nope` does not exist.

```buri fail code=module-not-found repo=cli/tests/example
from "//lib/nope" import { Nope };

export fn main(): Result<(), Str> {
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces
`module-not-found` — so this page cannot describe an error the compiler has
stopped emitting.
