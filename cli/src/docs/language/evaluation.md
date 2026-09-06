## 8. Evaluation

### 8.1 Immutability

Every binding is final. There is no assignment operator, no `mut`, no interior
mutability, no aliasing hazard, and therefore no borrow checker and no lifetimes.
"Modifying" a value produces a new one:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let u2 = User { ..u, name: "new" };
```

An implementation should make this cheap: share structure, and update in place
where it can prove a value is not shared. That is an implementation strategy
rather than a language rule, and nothing can observe it.

### 8.2 Strictness and order

Buri is strict. Evaluation order is fully specified:

- A block evaluates its `let` bindings top to bottom, before its result
  expression.
- A call evaluates its arguments left to right, then applies the function.
- A binary operator evaluates its operands left to right, except for `&&` and
  `||`, which short-circuit.
- `if` evaluates its condition, then exactly one branch.
- `match` evaluates its scrutinee, then tests arms in order, evaluating each
  guard only when its pattern matched.

This matters more here than it usually would. Effects happen through ordinary
function calls rather than through a monad, so **a specified evaluation order is
what makes effect sequencing mean anything.** An implementation may reorder or
eliminate work only where the result is indistinguishable, and a call that
consumes an effect is never indistinguishable.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let _ = io.println(ctx, "first").ignore();
let _ = io.println(ctx, "second").ignore();    // guaranteed to print second
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

The first two cover essentially all Buri code, and both are exact: the emitted
loop is what a hand-written loop would have been. They apply because Buri has no
dynamic dispatch. There are no trait objects and no virtual calls, so the
compiler knows the whole graph of direct calls, and generic calls become direct
after monomorphization.

Only the third case costs anything, and it arises only when a tail call invokes a
function *value*. An implementation should apply the cheaper transformation
wherever it knows the callee statically, and may specialize a call site whose
function value it knows, dropping the trampoline entirely.

One consequence is observable. An abort inside a transformed group reports fewer
stack frames than the source suggests, because those frames no longer exist.
Implementations should carry source positions through the transformation, so the
reported location is still correct.

### 8.4 Closures

Lambdas capture by value. Values are immutable, so nothing can observe the
capture — with one exception, the capture rule of Section 10.6.

---
