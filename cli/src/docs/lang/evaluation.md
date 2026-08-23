## 8. Evaluation

### 8.1 Immutability

Every binding is final. There is no assignment operator, no `mut`, no interior
mutability, no aliasing hazard, and therefore no borrow checker and no lifetimes.
"Modifying" a value produces a new one:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let u2 = User { ..u, name: "new" };
```

An implementation is expected to make this cheap through structural sharing and
opportunistic in-place update when a value is provably not shared. That is an
implementation strategy, not a language rule, and it is never observable.

### 8.2 Strictness and order

Buri is strict. Evaluation order is fully specified:

1. `let` bindings in a block are evaluated top to bottom, before the block's
   result expression.
2. Call arguments are evaluated left to right, then the function is applied.
3. Operands of binary operators are evaluated left to right, except for `&&`,
   `||`, and `??`, which short-circuit.
4. `if` evaluates its condition, then exactly one branch.
5. `match` evaluates its scrutinee, then tests arms in order, evaluating each
   guard only when its pattern matched.

This matters more than it usually would: because effects are performed by
ordinary function calls rather than by a monad, **specified evaluation order is
what makes effect sequencing meaningful.** An implementation may reorder or
eliminate work only where the result is indistinguishable, and calls that consume
an effect are never indistinguishable.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let _ = io.println(ctx, "first");
let _ = io.println(ctx, "second");    // guaranteed to print second
```

### 8.3 Recursion and tail calls

Recursion is the only looping construct. Implementations **must** eliminate tail
calls, including mutually recursive ones, so that tail-recursive functions run in
constant stack space. This is what makes `fold`, and every accumulator-passing
helper written on top of it, a real loop rather than a stack hazard.

Non-tail recursion that exhausts the stack aborts.

#### 8.3.1 How, on a target without native tail calls

Native backends can lower a tail call directly. JavaScript cannot — no engine but
JavaScriptCore implements proper tail calls — so the **compiler performs the
elimination itself** rather than relying on the host. Three cases, in increasing
cost:

| Shape | Transformation | Cost |
|---|---|---|
| A function tail-calls itself | rewrite to a loop with parameter rebinding | none |
| A statically known group of functions tail-call each other | merge the group into one function with a dispatch switch | one branch per bounce |
| A tail call through a value of function type | trampoline: return a thunk, drive it from a loop | one allocation per bounce |

The first two cover essentially all Buri code, and both are exact — the emitted
loop is what a hand-written loop would have been. They apply because Buri has no
dynamic dispatch: there are no trait objects and no virtual calls, so the call
graph of direct calls is fully known, and generic calls become direct after
monomorphization.

Only the third case costs anything, and it arises solely when a function *value*
is invoked in tail position. An implementation should apply the cheaper
transformation wherever the callee is statically known, and may specialize a
call site whose function value is known to avoid the trampoline entirely.

One consequence is observable: an abort inside a transformed group reports fewer
stack frames than the source suggests, because those frames no longer exist.
Implementations should preserve source positions through the transformation so
that the reported location is still correct.

### 8.4 Closures

Lambdas capture by value. Since values are immutable, capture is unobservable —
with one exception: **a lambda may not capture an effect-carrying value**, nor
one whose type could be a context at some instantiation (Section 10.6).

---
