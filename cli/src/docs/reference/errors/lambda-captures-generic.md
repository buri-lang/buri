---
title: A lambda may not capture a value that could be a context
message: a lambda may not capture `{name}`, whose type `{type}` could be a context
note: a generic body is checked once, for every instantiation at once (guides/compile-speed.md), so a type parameter here stands for a context type too — and `fn wrap<T>(x: T, f: fn(T) => ()): fn() => ()` would otherwise launder one into a closure whose type mentions no effect at all
fix: take it as a parameter of the lambda instead — the `fn(C, A) => B` shape a `*Ctx` combinator passes — or return the value rather than a closure over it
---
# A lambda may not capture a value that could be a context

```text
error: a lambda may not capture `x`, whose type `T` could be a context [lambda-captures-generic]
```

```buri fail code=lambda-captures-generic
fn hide<T>(x: T): fn() => T {
    fn() => x
}
```

This is `lambda-captures-effect` for a type that can't say whether it's a
context. Otherwise `hide(ctx)` would type-check and return a closure holding a
capability behind a type that mentions none.

Two kinds of type parameter are exempt:

- One with an ordinary trait bound. A `T: Equal` is data, never a context, so
  `xs.any(fn(x) => x == needle)` is fine inside `impl<T: Equal> [T]`.
- A function type, because this rule already checks what that closure captured.
  `fn compose<A, B, C>(f: fn(A) => B, g: fn(B) => C): fn(A) => C { fn(x) => g(f(x)) }`
  is fine.
