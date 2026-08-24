# A method is not a value

```text
error: `area` is a method, and a method is not a value [method-not-a-value]
```

## What to do

call it on a receiver: `x.area()`; to pass it on, wrap it in a lambda: `fn(x) => x.area()`

## A program that provokes it

```buri fail code=method-not-a-value use=errors wrap=body
let sq = Square { side: 3 };
let f = sq.area;
let _ = ctx.println("${f()}");
```
