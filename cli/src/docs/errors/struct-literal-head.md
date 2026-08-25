---
title: A struct literal is headed by a type
message: the head of a struct literal must be a type
note: the grammar permits `f(x) {{ a: 1 }}`; the checker does not
fix: name the type, as in `Point {{ x: 1, y: 2 }}`, or `.Variant {{ ... }}` where the expected type is known
---
# A struct literal is headed by a type

```text
error: the head of a struct literal must be a type [struct-literal-head]
```

## What to do

Name the type — `Point { x: 1, y: 2 }` — or write `.Variant { ... }` where the
expected type is known.

## Why

The grammar admits `f(x) { a: 1 }` because it decides shape without consulting
name resolution; the checker is where the head is required to be a type. So
this is a rule the parser deliberately does not enforce, and the diagnostic
arrives one phase later than it looks like it should.

## A program that provokes it

```buri fail code=struct-literal-head
struct Holder { export a: Int }

fn identity(n: Int): Int { n }

fn build(): Int {
  let h = identity(1) { a: 1 };
  h.a
}
```
