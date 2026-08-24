# A type with no finite value cannot be constructed

```text
error: `Endless` can never be constructed [uninhabited]
```

## What to do

give `Endless` a variant that does not mention itself, the way `.None` terminates an `Option`

## Why

every variant recurses, so building one would need one already

## A program that provokes it

```buri fail code=uninhabited
enum Endless { Node(Endless) }
```
