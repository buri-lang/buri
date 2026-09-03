---
title: A trait or an effect takes no type parameters of its own
message: '`{name}` declares type parameters of its own'
note: an `impl` names a trait without arguments, so there is nowhere to say what the trait's own parameters are bound to
fix: move the parameters onto the methods that need them, as in `fn read<T>(self, id: Int): T;`
---
# A trait or an effect takes no type parameters of its own

```text
error: `Store` declares type parameters of its own [generic-effect-unsupported]
```

## What to do

Move the parameters onto the methods that need them:

```buri
trait Store {
    fn get<T>(self, key: Str): T;
    fn put<T>(self, key: Str, value: T): ();
}
```

A method's own type parameters are supported everywhere a trait or an effect is
— `Show.show<C: Alloc>` and `Ui.memo<T>` are both in the standard library — and
they say the same thing in every case the trait-level version would have.

## Why

A conformance is written `impl Store for Disk { ... }`. There is no place in
that syntax for the trait's arguments, so `Store<Str>` and `Store<Int>` would be
one implementation and there would be nothing to say which. Monomorphization
rebuilds an implementation's type arguments by matching the `impl`'s head
against the receiver; a parameter belonging to the *trait* appears in neither,
and the answer would have to be guessed.

Guessing it is exactly what the compiler used to do — the type arguments were
padded to the declared count with `()` — so the refusal is here, at the
declaration, rather than as a wrong program later.

## A program that provokes it

```buri fail code=generic-effect-unsupported
trait Store<T> {
    fn get(self, key: Str): T;
}
```
