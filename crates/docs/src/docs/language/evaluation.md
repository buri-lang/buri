## 8. Evaluation

### 8.1 Immutability

Every binding is final: no assignment, no `mut`, no interior mutability, so no
borrow checker or lifetimes. "Modifying" a value builds a new one:

```buri
# struct User {
#     id: Int,
#     name: Str,
# }
#
# fn rename(u: User): User {
    let u2 = User { ..u, name: "new" };
#     u2
# }
```

An implementation should make this cheap by sharing structure, or updating in
place where it can prove a value isn't shared. Nothing can observe which.

### 8.2 Strictness and order

Buri is strict, and evaluation order is fully specified:

- A block evaluates its `let` bindings top to bottom, then its result.
- A call evaluates its arguments left to right, then applies the function.
- A binary operator evaluates its operands left to right; `&&` and `||`
  short-circuit.
- `if` evaluates its condition, then exactly one branch.
- `match` evaluates its scrutinee, then tests arms in order, evaluating a guard
  only when its pattern matched.

Effects are ordinary function calls, not a monad, so **this order is what
sequences effects.** An implementation may reorder or drop work only where the
result is indistinguishable, and a call that consumes an effect never is.

```buri wrap=body run
let _ = io.println(ctx, "first").ignore();
let _ = io.println(ctx, "second").ignore(); // guaranteed to print second
```

```stdout
first
second
```

### 8.3 Recursion and tail calls

Recursion is the only looping construct. Implementations **must** eliminate tail
calls, including mutually recursive ones, so tail-recursive functions run in
constant stack. That makes `fold` and every accumulator-passing helper a real
loop.

Non-tail recursion that exhausts the stack aborts.

#### 8.3.1 How, on a target without native tail calls

Native backends lower a tail call directly. On JavaScript the **compiler
eliminates it**:

| Shape | Transformation | Cost |
|---|---|---|
| A function tail-calls itself | rewrite to a loop with parameter rebinding | none |
| A statically known group of functions tail-call each other | merge the group into one function with a dispatch switch | one branch per bounce |
| A tail call through a value of function type | trampoline: return a thunk, drive it from a loop | one allocation per bounce |

The first two cover nearly all Buri code and emit the loop you'd write by hand.
An implementation should use the cheapest transformation the callee allows, and
may specialize a call site whose function value it knows to drop the trampoline.

An abort inside a transformed group reports fewer stack frames than the source
suggests, because those frames no longer exist. Implementations should carry
source positions through, so the reported location stays correct.

### 8.4 Closures

Lambdas capture by value. Since values are immutable, nothing can observe the
capture, except through the capture rule of Section 10.6.

---
