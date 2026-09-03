---
title: A name is declared once
message: {declaration} is declared twice
---
# A name is declared once

```text
error: variant `Yes` is declared twice [duplicate-declaration]
```

## What to do

Rename one of them.

## Why

A name is how the thing is referred to, and two of them in one scope leaves the
reference with no answer — `match` tells variants apart by name, a call tells
functions apart by name, and neither has a second thing to fall back on.

## A program that provokes it

```buri fail code=duplicate-declaration
enum Choice {
    Yes,
    No,
    Yes,
}
```
