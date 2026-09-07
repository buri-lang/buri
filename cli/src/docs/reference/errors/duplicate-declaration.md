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

A name is how you refer to the thing, and two of them in one scope leave the
reference with no answer.

## A program that provokes it

```buri fail code=duplicate-declaration
enum Choice {
    Yes,
    No,
    Yes,
}
```
