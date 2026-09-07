---
title: An effect is performed through a function, not a method
message: '`{effect}` is an effect, so `{method}` is not called on a context'
label: called as a method
fix: 'call it through the module that wraps the effect: `io.println(ctx, text)`'
---
# An effect is performed through a function, not a method

```text
error: `Stdout` is an effect, so `println` is not called on a context [effect-method-call]
```

## What to do

Hand the context to the function that performs the operation. Every effect
method has one, and the diagnostic names both it and its module.
`ctx.println("hi")` becomes `io.println(ctx, "hi")`, and `ctx.readFile(p)`
becomes `fs.readText(ctx, p)`.

## Why

A context is the set of things a program may do, and you write it down so a
reader can see what a function reaches for. The receiver is the quietest part of
a call, so `x.f(y)` hides that. Passing the context as an argument puts the
authority where the reader is already looking, and splits the two halves —
*which* effect, and *what* it does — into two names.

It also keeps the vocabulary from being one flat namespace. Method lookup
through a bound searches every effect the bound declares, so `Ui.read` and
`Watch.read` would be ambiguous for anybody binding both. A module-qualified
call cannot be.

Two layers below this line keep the method form: the standard library's wrapper
functions, and the body of an `impl` that *supplies* an effect. The second is
what keeps an attenuating wrapper writable — its `readFile` calls
`self.0.readFile(path)`, reaching only the inner context it was handed.

## A program that provokes it

```buri fail code=effect-method-call wrap=body effects=Stdout
let _ = ctx.println("ready");
```
