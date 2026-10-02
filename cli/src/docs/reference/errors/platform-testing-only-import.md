---
title: Only an effect's testing surface keeps state
message: this module is only for an effect's test implementation
label: importable only from platform/effect/**/testing
note: core/platforms/testing/state may be imported only from a module under platform/effect/ with a testing segment
fix: keep the state in the effect's testing surface, and import the test implementation it exports
---
# Only an effect's testing surface keeps state

```text
error: this module is only for an effect's test implementation [platform-testing-only-import]
```

## What to do

Import the effect's test implementation instead, such as
`//platform/effect/kv/testing`'s `kv()`.

`core/platforms/testing/state` gives an effect's test implementation a value
that outlives one call. A test checks values it already holds, so it doesn't
need one.

## A program that provokes it

```buri fail code=platform-testing-only-import role=test
from "core/platforms/testing/state" import * as state;
```
