# A method is looked up in its type's defining module

```text
error: `Square` has no method `area` [no-such-method]
```

## What to do

check the spelling, or declare it in `impl Square { ... }` in that type's own module — a method may not be added from anywhere else

## A program that provokes it

```buri fail code=no-such-method use=errors wrap=body
let _ = ctx.println("${Square { side: 3 }.perimeter()}");
```
