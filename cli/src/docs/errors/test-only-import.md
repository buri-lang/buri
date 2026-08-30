---
title: A `testing` module is reachable only from a test
message: this is a test-only module
label: importable only from a test source
note: a path containing a `testing` segment may be imported only from a test source
fix: import it from a file listed in a target's `test.sources`, or drop the import
---
# A `testing` module is reachable only from a test

```text
error: this is a test-only module [test-only-import]
```

## What to do

Import it from a file listed in a target's `test.sources`, or drop the import.

## Why

The restriction is carried by the path rather than by a field, so it is visible
where the import is written and there is nothing to remember to declare. Any
module path with a `testing` segment is covered — `core/testing/assert`,
`//lib/ledger/testing`, `//lib/testing/fakes`.

## A program that provokes it

```buri fail code=test-only-import
from "core/testing/assert/lib.buri" import * as assert;
```
