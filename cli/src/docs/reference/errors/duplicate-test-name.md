---
title: A test name is used once per file
message: this file already has a test called {quoted_title}
label: declared again here
note: a title is how a failing test is named in the report and how `--filter` selects one, so two tests sharing a title in one file cannot be told apart
fix: rename one of them, so each test in this file has its own title
---
# A test name is used once per file

```text
error: this file already has a test called "pads the cents place" [duplicate-test-name]
```

## What to do

Rename one of them, so each test in this file has its own title.

## Why

A title is how the report names a failing test and how `--filter` selects one,
so two tests sharing a title in one file cannot be told apart. Two *different*
files may use the same title. They are separate modules, and the report names
the file and the line each failure came from.

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
