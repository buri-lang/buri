---
title: A private declaration is private to its module
message: {declaration} is private to its module
---
# A private declaration is private to its module

```text
error: field `0` of `Scope` is private to its module [private-to-module]
```

## What to do

Add `export` to the field, or go through a method the type provides.

## Why

`export` is the whole of visibility: a declaration, a field or a method that
does not carry it is reachable only from the module that wrote it. One rule, so
one code — the same error covers a function you cannot call, a field you cannot
name, and a variant of an enum whose own declaration is private.

The unusual half is that **a struct with any private field cannot be
constructed anywhere else at all**, not merely read: writing `Scope(0)` names
the hidden field. That is what makes a private field an invariant rather than
an inconvenience, and it is how the standard library mints a type only from the
inside. `ui/effect`'s `Scope` is the worked example — a reactive closure is
handed one by the runtime, and no program can build one, which is what "a
closure can never capture a context" rests on.

Functional update still works, because it never names the hidden fields.

## A program that provokes it

```buri fail code=private-to-module
# from "ui/effect/lib.buri" import { Scope };
# from "ui/signal/lib.buri" import { Signal };
fn peek(n: Signal<Int>): Int {
  n.get(Scope(0))
}
```
