---
title: An `impl` supplies the signature its trait declares
message: "`{method}` does not have the signature `{trait}` declares"
label: expected {expected}, found {found}
note: a call through a bound is checked against the trait's declaration and dispatched to the `impl`'s method, so the two are one signature
fix: give `{method}` the signature `{trait}` declares
---
# An `impl` supplies the signature its trait declares

```text
error: `size` does not have the signature `Measurable` declares [signature-mismatch]
```

## What to do

Write the method with the parameters, the type parameters and their bounds,
and the return type the trait declared it with. Where the trait wrote `Self`,
an `impl` may write either `Self` or the type it is implementing for — they are
the same type inside the block.

## Why

A method is matched to the slot it fills by name, and nothing else about the
two declarations has to agree for that match to succeed. Everything after it
assumes they agree completely: a caller reaching the method through a bound is
typechecked against the *trait's* declaration, and the code generator
reconstructs the `impl` function's type arguments from the trait's. An `impl`
that took one more parameter, or one fewer type parameter, or a `Str` where the
trait said `Int`, was therefore a promise made in one place and broken in
another — found at some later call site if at all.

A bound is compared as a set, so writing the same bounds in another order is
not a disagreement. Asking for one the trait does not declare is: a caller was
never told to supply it, so the requirement would be discovered at
monomorphization rather than at the call that failed to meet it.

Only the method's own type parameters are counted. The ones on the `impl` head
belong to the block rather than to the method, so `impl<T> Show for [T]`
supplies `show<C>` with one type parameter, not two.

## A program that provokes it

```buri fail code=signature-mismatch
trait Measurable {
  fn size(self): Int;
}

struct Bag { export count: Int }

impl Measurable for Bag {
  fn size(self, scale: Int): Int { self.count * scale }
}
```
