# A struct pattern mentions every field

```text
error: this pattern does not mention `y` [missing-field-pattern]
```

## What to do

Match `y` too, or end the pattern with `..` to ignore the rest.

## Why

The same reason `match` is exhaustive one level up: adding a field should be a
compile error wherever the type is taken apart, and `..` is how a pattern says
it does not want that.

## A program that provokes it

```buri fail code=missing-field-pattern
struct Point { export x: Int, export y: Int }

fn xOf(p: Point): Int {
  let Point { x } = p;
  x
}
```
