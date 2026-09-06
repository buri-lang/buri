---
title: A declaration ends with `;`
message: {construct} ends with `;`
fix: write `;` here
---
# A declaration ends with `;`

```text
error: a type alias ends with `;` [missing-terminator]
```

## What to do

Write the `;` where the caret is. The error carries the edit as bytes, so an
editor's quick fix and `buri lint --fix` both write it for you.

## Why

A declaration that binds a value or a name ends with `;`. One that opens a
brace-delimited body does not. So `let`, `type`, `derive`, an import and a
tuple-struct end with `;`, while a `fn`, a `struct` with fields, an `enum`, a
`trait` and an `impl` end with `}`. The rule follows the declaration's last
token, so you remember nothing per keyword.

Inside a block, that same `;` separates one statement from the next and says the
expression's value is discarded rather than returned.

## A program that provokes it

```buri fail code=missing-terminator
type Meters = Float

fn zero(): Meters {
  0.0
}
```
