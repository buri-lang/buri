# Only a platform module declares an effect

```text
error: only a platform module may declare an effect [effect-outside-platform]
```

## What to do

declare it as a plain `trait`, or move it into a platform module

## Why

the set of things a Buri program can do to the world is fixed by its platform rather than open-ended

## A program that provokes it

```buri fail code=effect-outside-platform
effect Mischief {
  fn meddle(self: Self): ();
}
```
