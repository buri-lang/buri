# A lambda may not capture a value that could be a context

```text
error: a lambda may not capture `x`, whose type `T` could be a context [lambda-captures-generic]
```

## What to do

take it as a parameter of the lambda instead — the `fn(C, A) => B` shape a `*Ctx` combinator passes — or return the value rather than a closure over it

## Why

a generic body is checked once, for every instantiation at once (SPEC 13.5), so a type parameter here stands for a context type too — and `fn wrap<T>(x: T, f: fn(T) => ()): fn() => ()` would otherwise launder one into a closure whose type mentions no effect at all

## The longer version

`lambda-captures-effect` is the same rule where the type says so out loud. This
is the case where it does not, and cannot: inside `fn hide<T>(x: T)`, `T` is
opaque, and a body is checked once rather than once per call site. If the rule
asked only "does this type mention an effect?", the answer for `T` would be no,
`hide(ctx)` would be well-typed at the call site, and the closure that came back
would hold a capability behind a type that mentions none. That is the whole
point of the capture rule, so the rule has to be conservative here.

A type parameter escapes the restriction when it carries an **ordinary trait
bound**. A type is either part of the world or part of your data, never both
(Section 10.1), so a `T: Eq` can never be instantiated at a context type — which
is why `xs.any(fn(x) => x == needle)` is still legal inside `impl<T: Eq> [T]`.

Function types escape it too, for a different reason: a closure holds exactly
what it captured, and this rule is what checks that. So `fn compose<A, B, C>(f:
fn(A) => B, g: fn(B) => C): fn(A) => C { fn(x) => g(f(x)) }` is fine.

## A program that provokes it

```buri fail code=lambda-captures-generic
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

fn hide<T>(x: T): fn() => T {
  fn() => x
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let smuggler = hide(ctx);
  let recovered = smuggler();
  let _ = recovered.println("laundered");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `lambda-captures-generic` — so
this page cannot describe an error the compiler has stopped emitting.
