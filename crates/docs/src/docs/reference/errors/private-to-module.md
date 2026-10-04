---
title: A private declaration is private to its module
message: {declaration} is private to its module
---
# A private declaration is private to its module

```text
error: field `0` of `Scope` is private to its module [private-to-module]
```

```buri fail code=private-to-module
# from "platform/effect" import { Scope };
# from "ui/signal" import { Signal };

fn peek(n: Signal<Int>): Int {
    n.get(Scope(0))
}
```

Add `export` to the declaration, or go through a method the type provides.

Without `export`, a declaration, field, method or variant of a private enum is
reachable only from its own module.

A struct with any private field can't be constructed outside its module at all,
because writing `Scope(0)` names the hidden field. That makes a private field an
invariant, and it's how the standard library mints a type only from the inside.
Functional update still works, because it never names the hidden fields.
