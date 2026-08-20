# A test name is used once per file

```text
error: this file already has a test called "pads the cents place" [duplicate-test-name]
```

## What to do

rename one of them, so each test in this file has its own title

## Why

a title is how a failing test is named in the report and how `--filter` selects one, so two tests sharing a title in one file cannot be told apart

Two *different* files may use the same title. They are separate modules, a
report names the file each failure came from, and each is reported at its own
line.

## A program that provokes it

```buri fail code=duplicate-test-name role=test
from "core/testing/assert" import * as assert;

test "adds" {
  assert.eq(1 + 1, 2);
}

test "adds" {
  assert.eq(2 + 2, 4);
}
```

Compiled by the test suite, which checks that it still produces `duplicate-test-name` — so
this page cannot describe an error the compiler has stopped emitting.
