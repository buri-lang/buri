---
title: A context is never built from itself
message: 'circular context: {cycle}'
note: building a context runs every call in its body, so one that reaches itself never finishes building
fix: 'break the cycle: spread or call a context that is not on the chain, or write its bindings out'
---
# A context is never built from itself

```text
error: circular context: `Fixture` -> `Fixture` [circular-context]
```

```buri fail code=circular-context role=test
from "platform/effect" import { Clock };
from "platform/effect/testing" import { clock };

context Fixture {
    ..Fixture(),
    Clock: clock(),
}
```

`Fixture()` builds a fresh context each time it's called. A body that calls
`Fixture()`, directly or through other contexts, never finishes. The diagnostic
prints the whole chain, since changing any name on it fixes it.
