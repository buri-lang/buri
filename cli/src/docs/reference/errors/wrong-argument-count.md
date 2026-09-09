---
title: A call passes exactly the arguments the function declares
message: '`{function}` takes {expected} arguments, but {given} were given'
fix: 'pass `{function}` the following arguments: {signature}'
---
# A call passes exactly the arguments the function declares

## What to do

Line the call up against the signature the fix prints, which is the declaration:
the name, the generics with their bounds, and every parameter with its name and
type. The context is one of those parameters, so a call that forgot it is short
by one. Where the types of the arguments say which parameter was left out, the
fix names it; where the call lines up two ways, or more than one parameter has
nothing to fill it, it names none of them rather than sending you to the wrong
end of the line.

### The parameter, where the types say which

```text
error: `snapshot` takes 5 arguments, but 4 were given [wrong-argument-count]
   = fix: `snapshot` requires a `name` parameter: snapshot<C: Allocator + Ui>(ctx: C, name: Str, root: Node<C>, state: State, themes: [Theme])
```

### The signature alone, where they do not

```text
error: `place` takes 4 arguments, but 2 were given [wrong-argument-count]
   = fix: pass `place` the following arguments: place(x: Int, y: Int, width: Int, height: Int)
```

### The signature again, for an argument too many

```text
error: `midpoint` takes 2 arguments, but 3 were given [wrong-argument-count]
   = fix: pass `midpoint` the following arguments: midpoint(low: Int, high: Int)
```

## A program that provokes it

```buri fail code=wrong-argument-count
fn add(a: Int, b: Int): Int {
    a + b
}

fn go(): Int {
    add(1)
}
```
