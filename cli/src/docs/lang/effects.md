## 10. Effects and purity

### 10.1 The model

An **effect** is an interface declared with `effect` instead of `trait`. Its
methods are the operations it grants:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
// core/effect
export effect Alloc {
    fn allocate(self, bytes: Int): Region;
}

export effect Stdout {
    fn print(self, text: Template): ();
    fn println(self, text: Template): ();
}

export effect Fs {
    fn readFile(self, path: Str): Result<Str, IoError>;
    fn writeFile(self, path: Str, body: Str): Result<(), IoError>;
}

// An effect's signature may name types, and those types are declared here
// beside it — `IoError` above, `Request` and `Response` below — rather than in
// the library that wraps the effect, because `core/effect` cannot import a
// module that imports it. The wrapper re-exports them, and that is where a
// program meets them.
export enum Method {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
}

export struct Header {
    export name: Str,
    export value: Str,
}

export struct Request {
    export method: Method,
    export url: Str,
    export headers: [Header],
    export body: [U8],
}

export struct Response {
    export status: Int,
    export headers: [Header],
    export body: [U8],
}

export effect Net {
    fn fetch(self, request: Request): Result<Response, NetError>;
}
```

`core/effect` declares `Alloc`, `Fs`, `Net`, `Clock`, `Rand`, `Env`, `Stdin`,
`Stdout`, `Stderr`, `Proc`, `Tasks`, `Listen`, and `Sockets`. **Only platform
modules may declare effects**; `effect` in ordinary code is a compile error, so
the set of things a Buri program can do to the world is fixed by its platform
rather than open-ended.

`Net.fetch` takes one value and answers one value, and those two types are the
whole of what an HTTP message is in this language — the same `Request` a server
hands a handler, not a second shape for the other direction. Three things follow
from the declarations above and are worth saying out loud:

- **A wire spelling never appears in Buri code.** `GET` is written `.Get`, and
  the three letters live in the platform's implementation. A method the enum
  does not name is a method a program cannot send.
- **Header names are lowercase**, which is what HTTP/2 requires on the wire, so
  looking one up is a comparison rather than a case-insensitive scan.
- **A body is octets.** A body is not necessarily text; decoding it is
  `core/bytes`' job and answers a `Result`, so a body that is not text says so
  where it is read.

`https://` is checked, not merely accepted. The server's certificate is verified
against your machine's own trust anchors — the PEM bundle the platform keeps,
which on macOS is `/etc/ssl/cert.pem` and on Linux one of the four usual paths —
and a certificate that does not check out is a `NetError::Transport` naming what
was wrong and which trust set it was checked against. Setting `SSL_CERT_FILE` to
a PEM bundle **replaces** those anchors, the same way it does for OpenSSL,
`curl` and `git`; it is what a private or corporate authority is for, and on
macOS it is also how a root that lives only in the keychain is reached. There is
no way to turn verification off, and none is planned: a `Net` a program could
ask to trust anybody is not a capability, it is a hole.

An effect is a trait in every other respect — same declaration shape, same
nominal conformance, same `impl`, same bounds. Two rules separate them:

- an effect's implementors are **effect-carrying**, and so may be passed only as
  `self` or `ctx` (Section 10.2);
- **no type may implement both an effect and a trait.** A type is either part of
  the world or part of your data, and the boundary is checked rather than
  assumed. It holds for composites too: an effect-carrying type — one that
  merely *mentions* an effect, such as a `Holder<C>` storing a context —
  satisfies no ordinary bound either, whatever `impl`s its head constructor
  carries. That is what lets Section 10.6 conclude that a `T: Ord` is never a
  context.

A function names the effects it needs as **bounds** on its context parameter:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs };

fn loadConfig<C: Alloc + Fs>(ctx: C, path: Str): Result<Config, ConfigError> {
    let text = fs.readText(ctx, path)?;
    parse(ctx, text)
}
```

There is one constraint mechanism in the language. `<T: Ord + Show>` and
`<C: Alloc + Fs>` are the same feature: a list of interfaces a type parameter
must satisfy.

### 10.2 The `ctx` rule

**An effect-carrying parameter must be `self` or `ctx`** — never any other
name, never any other position, and at most one of each:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, IoError, Net, Region };
fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>       // ok
fn render<C: Alloc>(self, ctx: C): Str                                    // ok
fn allocate(self, bytes: Int): Region                                     // ok
fn sneaky<C: Fs>(a: Int, handle: C): Bool                                 // ERROR
fn twoWorlds<A: Fs, B: Net>(ctx: A, other: B): ()                         // ERROR

enum Widget<C> { Press(fn(C, Int) => Str), Group([Widget<C>]) }
enum Boxed<C>  { Held(C) }

fn render<C: Alloc>(ctx: C, root: Widget<C>): Str                         // ok
fn peek<C: Alloc>(ctx: C, held: Boxed<C>): Int                            // ERROR
```

A type is **effect-carrying** if it is a type variable with an effect
bound, a type that implements an effect, or any type that can hand one of those
back — so a struct that stores a context is effect-carrying too.

**Position decides.** A *function type* is effect-carrying when its **result**
is: `fn(C, A) => B` merely accepts a context, which is the shape the `*Ctx`
combinators of Section 10.6 take, while `fn() => C` produces one. The same
reading applies to a type you declare, at each of its type arguments: an
argument counts only where the constructor can hand that argument back.

`Widget<C>` mentions `C`, but only where a *caller* must supply one: to get a
`C` out of a press handler you would have to pass one in first. `Boxed<C>`
stores its `C` and hands it straight back, so a `Boxed<C>` is a second context
under another name. A parameter that occurs in no field at all — a handle that
is phantom in it — is data for the same reason.

`self` has to be allowed because an effect's own methods take the
effect as their receiver (`fn allocate(self, ...)`), and so do the
attenuation wrappers of Section 10.8. Outside those two places, effects arrive
through `ctx`.

### An effect is performed by a function, not by a method

**An effect's methods are called through the module that wraps the effect,
never on the value that carries it.** `ctx.println(text)` is
`io.println(ctx, text)`; `ctx.readFile(path)` is `fs.readText(ctx, path)`;
`ctx.allocate(n)` is `alloc.allocate(ctx, n)`. Every method of every declared
effect has exactly one such function, and calling one on a value is
`effect-method-call`, which names the function and the module it comes from.

A context is the set of things a program may do, and the point of writing it
down is that a reader can see what a function reaches for. `x.f(y)` hides that:
the receiver is the smallest, quietest part of a call, and an effect performed
through one reads like a method on an ordinary value. Passing the context as an
argument puts the authority where the reader is already looking, and it makes
the two halves — *which* effect, and *what* it does — two names instead of one.
It also settles a question the method form left open: method lookup through a
bound searches every effect the bound declares, so two effects claiming one
verb make that verb ambiguous for everybody who binds both (`Ui.read` and
`Watch.read` are the shipped case), while a module-qualified call cannot be
ambiguous at all.

Two layers are below that line and keep the method form:

* **the standard library**, which is where those wrapper functions are, so its
  bodies are the only thing that reaches an effect at all; and
* **the body of an `impl` that supplies an effect**, which is where the
  operation is implemented — this is what keeps Section 10.8's attenuation
  wrapper writable, and `ReadOnly<C>`'s `self.0.readFile(path)` cannot become
  `fs.readText(self.0, path)`, because that wrapper is bounded `Alloc + Fs`
  where the `impl` carries only `C: Fs`.

The carve-out grants nothing new: an implementor can reach only an inner
context somebody already handed it.

There is exactly one construct in which more than one effect-carrying value may
appear, and it is the `context` expression of Section 11.3 — the place where a
context is assembled out of the implementations that make it up. Everywhere
else, effects travel through a single `ctx` parameter or an
effect-carrying `self`.

The rule costs a function the ability to take two independent contexts — bundle
them into one type instead — and buys the property the chapter rests on:

> **A function is effectful if and only if it has a `ctx` parameter or an
> effect-carrying `self`.**

Both are fixed positions with fixed names, so you never scan a signature.

### 10.3 Where effects come from

The platform. `core/host` exports one value per effect the platform grants —
`host.alloc`, `host.stdout`, `host.stderr`, `host.stdin`, `host.fs`, `host.net`,
`host.clock`, `host.rand`, `host.env`, `host.proc`, `host.tasks`, on a native
platform `host.listen` and `host.sockets`, and on a platform with a document
`host.ui` and `host.watch` — and it is importable only from the module that
exports `main`. `main` assembles them into the one context the program has:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc:  host.alloc,
    Stdout: host.stdout,
    Fs:     host.fs,
  };
  ...
}
```

The form is Section 11.3. What matters here is what it makes true: a program
that never names `host.net` cannot open a socket anywhere in its transitive call
graph — not in a dependency, not in a build script, not by accident, because
nothing anywhere can obtain a value bounded by `Net`. The effect budget is the
set of `host` members reachable from `main`'s context, and a platform that does
not grant an effect simply does not export it, so requesting one is an ordinary
unresolved-name error at the one line that asked for it. Both halves of a grant
are withheld together — the implementation struct as well as the value — so
there is nothing left to construct by name.

`Tasks` — "run this over every item at once" — is granted on `LINUX`, `MACOS`
and `JS`, and withheld from `WEB`, which is the same three as `Fs`, `Stdin`,
`Env` and `Proc`, and is withheld for a reason of the same kind: `parallel`
returns only when the last task has finished, and a page has an interface that a
wait is visible in. A page's concurrency is its event loop.

**A row of that table may name no platform at all**, and an empty set of
platforms is an ordinary value of the field rather than a second mechanism
bolted on beside it. That is what lets a declaration land ahead of the runtime
that will answer it: a signature is the expensive thing to change once programs
are written against it, so `core/effect` declares the effect, `core/host`
declares the implementation struct and the value, and the row grants it nowhere.
Every binding of it is then refused on every target, with the reason rather than
with "no such name", and granting it later is an edit to that one row. No row is
empty today — every effect named above is reachable from somewhere — but the
shape is worth knowing, because it is how the last two arrived.

`Tasks` is the worked example, and it is what the grant table is *for*. `Tasks`
was declared first and granted by nobody — a row with an empty platform list —
so its signature could be written, reviewed and documented before there was a
scheduler to argue with, and every `Tasks: host.tasks` was refused everywhere
with that reason rather than with "no such name". Granting it was an edit to that
one row. Nothing about a program that had been written against the signature
changed, and no second mechanism — no "not implemented" flag, no feature gate —
was ever involved.

`Listen` and `Sockets` — "I accept connections" and "I can write to open
sockets" — came the same way, and they are also the case that shows a platform
list which is neither everything nor the three non-page platforms. They are
granted on `LINUX` and `MACOS`, and nowhere else. Holding a port open is a
native program's authority; a page is served rather than serving, and its host
has no way to accept a connection at all — so `Listen: host.listen` under
`platform: JS` or `platform: WEB` is refused with that reason, and it is a
refusal nothing later is going to lift. The two move together, because being a
server is one authority in two halves: accepting a connection, and writing to
one somebody already accepted.

That pair is also what an empty row was never promising. An empty list says
"nobody grants this today" and never "everybody will": `Listen`'s row gained the
two platforms that can serve and will never gain the other two. The row says who
grants the effect now, and the reason says why — nothing in it was ever a
schedule.

Note what is *not* claimed: an effect is an ordinary interface, so anyone may
write a type that satisfies it (Section 10.9 does). That is not a forgery hole —
a fake `Stdout` still cannot write anything, and what is unforgeable is the
*platform's* implementation. The open interface is what makes testing free.

`Alloc` is the case where that openness is useful outside a test, because it is
the one effect whose implementation grants nothing: `allocate` answers a
`Region`, which is a number nothing reads. So `core/alloc` ships three
implementations — `generalPurpose()`, `arena()`, `fixedBuffer(n)` — and is
importable anywhere rather than only from `main`. Binding one is how a program
asks what it is spending, or refuses to spend more than a budget; it is not how
a program acquires an authority it was not given.

### 10.4 What "pure" means

> **Purity theorem.** If a function has no `ctx` parameter, no
> effect-carrying `self`, captures no effect-carrying value, and constructs no
> context, then any two evaluations on **identical** arguments that **terminate
> without aborting**, in the **absence of undefined behaviour**, produce
> identical results and perform no observable effect — and a call that
> terminates without aborting may be freely cached, reordered, or eliminated.

Each of those three qualifiers is load-bearing, and each is there because the
sentence without it is false:

- **Identical, not equal.** Equality is a trait, and function types do not have
  it (Section 5.11): `f == g` is not a question the language can ask, so "equal
  arguments" has no referent at a function type. The theorem quantifies over the
  *same* values, which is a form that means something at every type.
- **Terminating without aborting.** A pure function may abort — `100 / x` at
  `x = 0` does — and an abort is observable: a message on stderr and a non-zero
  exit status (Section 6.10). Eliminating a call that would have aborted turns
  an aborting program into a running one, which is not a refinement of it.
  Divergence has the same shape. So an implementation may drop a pure call only
  where it can also show the call returns.
- **In the absence of undefined behaviour.** Overflow is undefined (Section
  6.2), and what it does depends on the target: a width wraps and a `BigInt`
  does not. Two evaluations agree only where the program's behaviour is defined
  at all.

Top-level functions capture nothing but other top-level declarations, which are
themselves effect-free, so for a top-level `fn` the theorem reduces to: *is
there a `ctx` parameter?*

The last clause exists because `main` has no parameters and is plainly not pure:
it builds a context and uses it. It is not a hole. A context may be constructed
only in `main`'s body, in a test source, or in a test-only module (Section
11.3), and none of those is a function anybody calls from library code — `main`
is the entry point, and a test source may not be imported. So in all ordinary
code the clause is vacuous, and the useful form of the theorem is unchanged.

Two consequences:

- Purity is not a keyword and not an effect annotation. It is the absence of one
  argument, in a fixed position, with a fixed name.
- The check is shallow and local. You never read a function body, or its
  callees' bodies, to know whether it can touch the world.

### 10.5 Determinism versus effects

`Alloc` is a **resource** effect: it can fail (out of memory) and it costs
something, but it is not observable. Every other effect in `core/effect` is
**observable**.

A function is **deterministic** if its only effect bound is `Alloc`.
`list.map(ctx, f)` is deterministic: it needs to allocate, but it is
referentially transparent. `time.now(ctx)` is not.

Tracking allocation is why `[T]`-returning combinators take a context at all, and
it is what makes "does no I/O" and "does not allocate" separately expressible:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, IoError };
fn sum(self): Int                                                     // pure
fn map<A, B, C: Alloc>(self, ctx: C, f: fn(A) => B): [B]              // deterministic
fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>   // effectful
```

Fixed-size construction — struct literals, tuples, enum payloads, array literals,
closures, `Template`s — never requires `Alloc`. Only results whose size depends
on runtime data do.

### 10.6 The capture rule

**A lambda may not capture an effect-carrying value.** Effects travel
through the `ctx` parameter only.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
// ERROR: the lambda captures ctx
let texts = paths.map(ctx, fn(p) => fs.readText(ctx, p));

// Thread the context through a *Ctx combinator instead
let texts = paths.mapCtx(ctx, fn(c, p) => fs.readText(c, p));
```

Without this rule, a value of type `fn(Str) => Str` could smuggle a file handle
past a signature with no `ctx` parameter, and the purity theorem would be false.
With it, a function type says everything about what its values can do.

The rule reaches every binding a lambda could close over — parameters, `let`
bindings, names a pattern binds, and the parameters of an *enclosing* lambda.
The context a `*Ctx` combinator hands to `fn(c, p) => ...` is a binding like any
other, so a lambda written inside that one may not capture `c` either.

**And a lambda may not capture a value whose type could be a context.** This is
the same rule at a type parameter, where "carries an effect" cannot be read off
the type — so `fn wrap<T>(x: T, f: fn(T) => ()): fn() => () { fn() => f(x) }` is
rejected, on the capture of `x`.

Nothing in `wrap` mentions an effect, and its body is checked once for every
instantiation at once (Section 13.5), so where the rule runs `T` is opaque. Yet
`wrap(ctx, fn(c) => io.println(c, "hi").ignore())` instantiates it at a context
type and returns a `fn() => ()` holding an effect — the same smuggling,
arriving by the generic route. So a type parameter is treated as though it *were* a context,
unless one of two things says otherwise:

- **An ordinary trait bound.** An effect-carrying type satisfies no ordinary
  bound (Section 10.1), so a `T: Eq` is never a context and
  `xs.any(fn(x) => x == needle)` inside `impl<T: Eq> [T]` is fine. A `T` with no
  bounds, or one bounded only by effects, has no such guarantee.
- **A function type.** A closure holds exactly what this rule let it capture, so
  capturing one is safe whatever its type parameters are: `fn compose<A, B, C>(f:
  fn(A) => B, g: fn(B) => C): fn(A) => C { fn(x) => g(f(x)) }` is legal.

The cost is that a closure-builder over an unconstrained type parameter has to
take the value as a parameter rather than close over it. The alternative is a
purity theorem that is false, which is not an alternative.

The standard library provides `*Ctx` variants (`list.mapCtx`, `list.filterCtx`,
`result.andThenCtx`), and explicit recursion is always available when the
combinator does not fit. This is the sharpest trade-off in the language, and
Section 15 lists it as the first open question.

**A callback declared by an effect is handed a context only if the declaration
names one, and `Self` never names one.** This is the capture rule read from the
other end. An effect method may take a callback — `Tasks.parallel` takes the
step that runs on every item — and that callback cannot close over a context, so
whatever authority it is to have must arrive as its first parameter. Two
different values could arrive there, and the declaration says which:

```buri ignore why="not yet converted to a compiled example: it declares an effect, which only a platform module may do"
export effect Tasks {
    // `ctx` is the caller's whole context, and the step is handed it.
    fn parallel<C, A, B>(self, ctx: C, items: [A], f: fn(C, Int, A) => B): [B];
}

// The other choice, in the shape `Listen` was once declared with and is not
// any more: `Self` is the acceptor — the type implementing `Listen` — so the
// handler is handed that, and that is all it gets.
export effect Listen {
    fn listen(
        self,
        address: Str,
        port: Int,
        onRequest: fn(Self, Request) => Response,
    ): Result<(), ServeError>;
}
```

`Self` is the **implementing type** everywhere it is written: in an `impl`
head, in an effect's declaration, and inside a callback's parameter list. It is
not the receiver. Through a `context { … }` value the two differ — a context
*names* a value that implements the effect rather than being one — and the
implementation is what `Self` means at every one of those points (Section 10.1).

So an effect that wants to hand a callback the **caller's** authority takes the
caller's context as an ordinary `ctx` parameter and spells the callback
`fn(C, …)`. The caller passes the same value twice, once as the receiver and
once as `ctx`, and the two parameters mean different things: the receiver
chooses the implementation, and `ctx` is what the work is done with.

Naming it rather than overloading `Self` is what keeps an effect an ordinary
interface (Section 10.9). A callback parameter that meant "the caller's context"
would have a type no implementation could name and no implementation could
produce a value of, so no `impl` written in Buri could ever call its own
callback — the effect would be implementable only by the compiler. With `C` in
the signature, a hand-written implementation has both a name for the type and a
value of it, and a fake in a test runs its steps exactly as the shipping
implementation does.

A callback whose first parameter is `Self` receives strictly less than its
caller had: an acceptor grants `Listen` and nothing else, so a handler handed
one cannot allocate, print, or start a task. That is the right answer where the
callback is meant to inspect the implementation, and the wrong one for a request
handler, which is why the choice is written down per method rather than
inferred — and why `Listen` carries no callback at all today. It is four
operations now — bind a listener, accept a request, respond to one, close the
listener — and the loop that calls a handler between the second and the third
lives in `core/net/server`, written in Buri against the caller's own `C`. A
handler there may allocate, print, read a clock and start a task, because the
authority it runs with never crossed the effect boundary to be narrowed.

### 10.7 Calling convention

**receiver first, context second, everything else after** — which is now enforced
rather than merely conventional (Section 10.2). A free function that has no
receiver therefore takes the context first:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, IoError };
export fn map<A, B, C: Alloc>(self, ctx: C, f: fn(A) => B): [B]
export fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>
```

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
xs.map(ctx, double)
lines.filter(ctx, isLong).sortBy(ctx, order.str)
```

An effect's own operations are the second shape and only the second shape: they
have no receiver a program may name, so they are free functions taking the
context first (`io.println(ctx, text)`, `fs.readText(ctx, path)`). The method
form is not an alternative spelling of them — it is refused
(`effect-method-call`).

### 10.8 Restricting what propagates

Two forms, giving different guarantees.

**Static confinement.** Bound the callee to fewer effects. It receives the
same value and cannot use, or pass on, anything its bounds do not name:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, Stdout };

fn logOnly<C: Stdout>(ctx: C, msg: Str): () {
    let _ = io.println(ctx, msg).ignore();
    // fs.readText(ctx, "/etc/passwd")     // ERROR: C is not bounded by Fs
    // dangerous(ctx)                      // ERROR: dangerous needs C: Fs
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
        Fs: host.fs,
    };
    let _ = logOnly(ctx, "starting"); // same value, confined by its bound
    .Ok(())
}
```

No copy and no ceremony. Confinement is transitive: `logOnly` cannot hand its
context to anything requiring more, because `C` is opaque at every call site
downstream.

**Attenuation.** Wrap the context in a type that satisfies fewer effects, so
the callee holds a value that genuinely lacks the rest:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, IoError, Region };

// module: safe/readonly
export struct ReadOnly<C>(C);

export fn readOnly<C>(ctx: C): ReadOnly<C> {
    ReadOnly(ctx)
}

// Forwards Alloc...
impl<C: Alloc> Alloc for ReadOnly<C> {
    fn allocate(self, bytes: Int): Region {
        self.0.allocate(bytes)
    }
}

// ...and reading, but there is deliberately no `writeFile`, so ReadOnly<C>
// does not satisfy Fs no matter what C is.
impl<C: Fs> ReadOnly<C> {
    export fn readFile(self, path: Str): Result<Str, IoError> {
        self.0.readFile(path)
    }
}
```

Static confinement is a fact about the type checker; attenuation is a fact about
the value, and survives anything that later escapes the type system. Use the
first by default and the second at trust boundaries.

Note that attenuation narrows the *context*, not one effect out of it. That
is what keeps the `ctx` rule satisfiable: there is still exactly one
effect-carrying parameter.

**The `self.0.readFile(path)` above is the carve-out of Section 10.2, and it
has to be.** A body supplying an effect is where the operation is implemented,
so it is one of the two layers that may still call an effect method on a value.
It cannot delegate to `fs.readText(self.0, path)` instead: that wrapper is
bounded `Alloc + Fs` and this `impl` carries only `C: Fs`, so the bound
mismatch is real rather than cosmetic. The carve-out grants nothing: an
implementor can reach only an inner context somebody already handed it.

### 10.9 Testing

A pure function needs no harness. An effectful one is tested by building a
context out of different implementations — and because effects are ordinary
interfaces, writing one is writing a struct with methods. The call site does not
change, because there was never a global to stub.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, IoError };

struct FakeFs {
    export files: [(Str, Str)],
}

impl Fs for FakeFs {
    fn readFile(self, path: Str): Result<Str, IoError> {
        match (self.files.find(fn(e) => e.0 == path)) {
            .Some(entry) => .Ok(entry.1),
            .None => .Err(.NotFound),
        }
    }

    fn writeFile(self, path: Str, body: Str): Result<(), IoError> {
        .Err(.ReadOnly)
    }
}

// context { Alloc: testing.alloc(), Fs: FakeFs { files: [...] } }
// loadConfig<C: Alloc + Fs> accepts it with no changes anywhere.
```

The harness around that — where tests live, how they are declared, and how they
build a context — is Sections 11.2 and 11.3.
