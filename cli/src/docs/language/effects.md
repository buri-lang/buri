## 10. Effects and purity

### 10.1 The model

An **effect** is an interface declared with `effect` instead of `trait`. Its
methods are the operations it grants:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
// core/effect
export effect Allocator {
    fn allocate(self, bytes: Int): Region;
}

export effect Stdout {
    fn print(self, text: Template): Result<(), IoError>;
    fn println(self, text: Template): Result<(), IoError>;
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

export effect Network {
    fn fetch(self, request: Request): Result<Response, NetError>;
}
```

Not every effect is declared there. `core/fs` is a platform module too, and it
declares the filesystem's two:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
// core/fs
export effect FileSystemRead {
    fn readFile(self, path: Path): Result<Str, IoError>;
    fn fileExists(self, path: Path): Bool;
    fn readDir(self, path: Path): Result<[Str], IoError>;
    fn readFileBytes(self, path: Path): Result<[U8], IoError>;
}

export effect FileSystemWrite {
    fn writeFile(self, path: Path, body: Str): Result<(), IoError>;
    fn writeFileBytes(self, path: Path, body: [U8]): Result<(), IoError>;
    fn appendFile(self, path: Path, body: [U8]): Result<(), IoError>;
    fn renameFile(self, source: Path, destination: Path): Result<(), IoError>;
    fn removeFile(self, path: Path): Result<(), IoError>;
    fn removeDir(self, path: Path): Result<(), IoError>;
    fn makeDir(self, path: Path): Result<(), IoError>;
    fn syncFile(self, path: Path): Result<(), IoError>;
}
```

`core/effect` declares `Allocator`, `Network`, `Clock`, `Random`, `Entropy`, `Environment`,
`Stdin`, `Stdout`, `Stderr`, `Process`, `Tasks`, `Listen`, `Sockets` and
`WebSocketClient`, and `core/fs` declares `FileSystemRead` and `FileSystemWrite`. **Only
platform modules may declare effects**; `effect` in ordinary code is a compile
error. So a program's platform fixes what that program can do to the world.

**The filesystem is two effects because it is two grants.** A program that reads
its configuration has not thereby earned the right to delete it. A
`<C: Allocator + FileSystemRead>` is a promise the compiler keeps: nothing that function
hands `ctx` to can ask for `FileSystemWrite` from a context that does not bind it. The
two live in `core/fs` rather than `core/effect` because their methods name
`Path`, and `core/effect` cannot import a module that imports it. `core/fs`
re-exports `Path`, so `from "core/fs" import { FileSystemRead, Path }` is one import.

`Random` and `Entropy` split the same way. `Random` promises a distribution and
nothing more — the test platform's is seeded, so a failing test reproduces.
`Entropy` promises that somebody who has watched the output cannot predict the
rest. `core/random` is the door onto one, `core/crypto` onto the other.

`Request` and `Response` are the whole of what an HTTP message is in this
language — the same `Request` a server hands a handler. Three things follow:

- **A wire spelling never appears in Buri code.** `GET` is written `.Get`, and
  the three letters live in the platform's implementation. A method the enum
  does not name is a method a program cannot send.
- **Header names are lowercase**, which is what HTTP/2 requires on the wire, so
  looking one up is a comparison rather than a case-insensitive scan.
- **A body is octets.** Decoding it is `core/bytes`' job and answers a `Result`,
  so a body that is not text says so where it is read.

`https://` is checked, not merely accepted. The platform verifies the server's
certificate against your machine's own trust anchors — the PEM bundle it keeps,
which on macOS is `/etc/ssl/cert.pem` and on Linux one of the four usual paths. A
certificate that does not check out is a `NetError::Transport` naming what was
wrong and which trust set it was checked against. Setting `SSL_CERT_FILE` to a
PEM bundle **replaces** those anchors, the same way it does for OpenSSL, `curl`
and `git`; on macOS that is also how you reach a root that lives only in the
keychain. There is no way to turn verification off.

An effect is a trait in every other respect — same declaration shape, same
nominal conformance, same `impl`, same bounds. Two rules separate them:

- an effect's implementors are **effect-carrying**, and so may be passed only as
  `self` or `ctx` (Section 10.2);
- **no type may implement both an effect and a trait.** A type is either part of
  the world or part of your data. It holds for composites too: an
  effect-carrying type — one that merely *mentions* an effect, such as a
  `Holder<C>` storing a context — satisfies no ordinary bound either, whatever
  `impl`s its head constructor carries. That is what lets Section 10.6 conclude a
  `T: Ord` is never a context.

A function names the effects it needs as **bounds** on its context parameter:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Allocator };
# from "core/fs" import { FileSystemRead, Path };

fn loadConfig<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Config, ConfigError> {
    let text = fs.readText(ctx, at)?;
    parse(ctx, text)
}
```

There is one constraint mechanism in the language. `<T: Ord + Show>` and
`<C: Allocator + FileSystemRead>` are the same feature: a list of interfaces a type parameter
must satisfy.

### 10.2 The `ctx` rule

**An effect-carrying parameter must be `self` or `ctx`** — never any other
name, never any other position, and at most one of each:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Allocator, IoError, Network, Region };
# from "core/fs" import { FileSystemRead, Path };
fn readText<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Str, IoError>    // ok
fn render<C: Allocator>(self, ctx: C): Str                                    // ok
fn allocate(self, bytes: Int): Region                                     // ok
fn sneaky<C: FileSystemRead>(a: Int, handle: C): Bool                             // ERROR
fn twoWorlds<A: FileSystemRead, B: Network>(ctx: A, other: B): ()                     // ERROR

enum Widget<C> { Press(fn(C, Int) => Str), Group([Widget<C>]) }
enum Boxed<C>  { Held(C) }

fn render<C: Allocator>(ctx: C, root: Widget<C>): Str                         // ok
fn peek<C: Allocator>(ctx: C, held: Boxed<C>): Int                            // ERROR
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
stores its `C` and hands it straight back, so it is a second context under
another name. A parameter that occurs in no field at all is data for the same
reason.

`self` has to be allowed because an effect's own methods take the effect as their
receiver (`fn allocate(self, ...)`), and so do the attenuation wrappers of
Section 10.8. Outside those two places, effects arrive through `ctx`.

### An effect is performed by a function, not by a method

**You call an effect's methods through the module that wraps the effect, never on
the value that carries it.** `ctx.println(text)` is `io.println(ctx, text)`;
`ctx.readFile(path)` is `fs.readText(ctx, path)`; `ctx.allocate(n)` is
`alloc.allocate(ctx, n)`. Every method of every declared effect has exactly one
such function. Calling one on a value is `effect-method-call`, which names the
function and the module it comes from.

Passing the context as an argument puts the authority where the reader is already
looking, and it settles an ambiguity: method lookup through a bound searches
every effect the bound declares, so two effects claiming one verb — `Ui.read` and
`Watch.read` are the shipped case — make it ambiguous for everybody who binds
both. A module-qualified call cannot be ambiguous at all.

Two layers are below that line and keep the method form:

- **the standard library**, which is where those wrapper functions are, so its
  bodies are the only thing that reaches an effect at all; and
- **the body of an `impl` that supplies an effect**, which is where the
  operation is implemented. That is what keeps Section 10.8's attenuation wrapper
  writable. `ReadOnly<C>`'s `self.0.readFile(path)` cannot become
  `fs.readText(self.0, at)`, because that wrapper is bounded `Allocator + FileSystemRead`
  where the `impl` carries only `C: FileSystemRead`.

Exactly one construct may hold more than one effect-carrying value: the `context`
expression of Section 11.3. Everywhere else, effects travel through a single
`ctx` parameter or an effect-carrying `self`.

The rule costs a function the ability to take two independent contexts — bundle
them into one type instead — and buys:

> **A function is effectful if and only if it has a `ctx` parameter or an
> effect-carrying `self`.**

Both are fixed positions with fixed names, so you never scan a signature.

### 10.3 Where effects come from

The platform. `core/host` exports one value per effect the platform grants —
`host.alloc`, `host.stdout`, `host.stderr`, `host.stdin`, `host.fs`,
`host.fs`, `host.net`, `host.clock`, `host.rand`, `host.env`, `host.proc`,
`host.tasks`, on a native
platform `host.listen` and `host.sockets`, and on a platform with a document
`host.ui` and `host.watch` — and only the module that exports `main` may import
it. `main` assembles them into the one context the program has:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Allocator, Stdout };
# from "core/fs" import { FileSystemRead };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context {
    Allocator:  host.alloc,
    Stdout: host.stdout,
    FileSystemRead: host.fs,
  };
  ...
}
```

Section 11.3 has the form. A program that never names `host.net` cannot open a
socket anywhere in its transitive call graph — not in a dependency, not in a
build script, not by accident — because nothing anywhere can obtain a value
bounded by `Network`. The effect budget is the set of `host` members reachable from
`main`'s context. A platform that does not grant an effect does not export it, so
asking for one is `effect-not-on-platform`, reported on the name inside the
braces where the file imported it, and on the member reference where the host
came in as a namespace. Both halves of a grant are refused together, the
implementation struct as well as the value.

The build system decides which platforms the module is checked against, not the
language. The compiler checks `main.buri` against every platform its rule's
`outputs` name, and against every platform its suite names in `test.platforms`,
because a test binary links `main` in. All of them have to compile. Nothing about
an **effect type** is platform-bound: `from "core/fs" import { FileSystemRead }` is legal
everywhere, a page included, because a bound demands an implementation rather
than being one.

The context above reads files and cannot write one: `host.fs` is nowhere in
it, so nothing it reaches can be bounded by `FileSystemWrite`. Binding one half of the
filesystem and not the other is the ordinary case rather than a precaution.

Which platforms grant an effect is a row in a grant table. `Tasks` — "run this
concurrently" — is granted everywhere, `WEB` included: a page's concurrency is
its event loop, and `core/tasks`'s `spawn` is how a program puts a socket, a
retry or a timer on one. `FileSystemRead`, `FileSystemWrite`, `Stdin`, `Environment` and `Process` are the
three platforms that are not a page, because a page has no filesystem, no
standard input, no command line and no process to exit. `Listen` — "I accept
connections" — is granted on `LINUX` and `MACOS` and nowhere else, because
holding a port open is a native program's authority and a page is served rather
than serving.

`Sockets` — "I can write to open sockets" — was granted with it and only with
it, until a page could get a socket without accepting one. `WebSocketClient`
dials one, so both of those are granted everywhere, and the rule that replaced
the pairing is that **`Sockets` is granted wherever a socket can be come by**. A
platform that could obtain a socket and not write on it would be handing out a
handle nothing can use.

**A row may name no platform at all**, which is how a declaration lands ahead of
the runtime that will answer it: `core/effect` declares the effect, `core/host`
declares the implementation struct and the value, and the row grants it nowhere.
Every binding is then refused on every target, with the reason rather than with
"no such name", and granting it later is an edit to that one row. An empty row
says "nobody grants this today" and never "everybody will".

A row also widens. `Tasks` is the one that has: declared and granted by nobody,
then granted on the three platforms that are not a page, and now granted
everywhere. Each move was an edit to that one row, and nothing changed for a
program already written against the signature.

None of this stops anyone writing a type that satisfies an effect, and Section
10.9 does. That is not a forgery hole: a fake `Stdout` still cannot write
anything, and what nobody can forge is the *platform's* implementation. The open
interface is what makes testing free.

`Allocator` is the one effect whose implementation grants nothing: `allocate` answers
a `Region`, which is a number nothing reads. So `core/alloc` ships three
implementations — `generalPurpose()`, `arena()`, `fixedBuffer(n)` — and any
module may import it, not only `main`. Binding one is how a program asks what it
is spending, or refuses to spend more than a budget.

### 10.4 What "pure" means

> **Purity theorem.** If a function has no `ctx` parameter, no
> effect-carrying `self`, captures no effect-carrying value, and constructs no
> context, then any two evaluations on **identical** arguments that **terminate
> without aborting**, in the **absence of undefined behaviour**, produce
> identical results and perform no observable effect — and a call that
> terminates without aborting may be freely cached, reordered, or eliminated.

Each of those three qualifiers is load-bearing, and each is there because the
sentence without it is false:

- **Identical, not equal.** Function types have no `Eq` (Section 5.11), so
  "equal arguments" has no referent at one. The theorem quantifies over the
  *same* values, which means something at every type.
- **Terminating without aborting.** A pure function may abort — `100 / x` at
  `x = 0` does — and an abort is observable (Section 6.9). Divergence has the
  same shape. So an implementation may drop a pure call only where it can also
  show the call returns.
- **In the absence of undefined behaviour.** Overflow is undefined (Section
  6.2), and what it does depends on the target. Two evaluations agree only where
  the program's behaviour is defined at all.

Top-level functions capture nothing but other top-level declarations, which are
themselves effect-free, so for a top-level `fn` the theorem reduces to: *is
there a `ctx` parameter?*

The last clause exists because an entry has no context parameter and is plainly
not pure: it builds a context and uses it. Only an entry's body, a test source,
or a test-only module may construct a context (Section 11.3), and library code
calls none of those, so the clause is vacuous in all ordinary code.

Two consequences:

- Purity is not a keyword and not an effect annotation. It is the absence of one
  argument, in a fixed position, with a fixed name.
- The check is shallow and local. You never read a function body, or its
  callees' bodies, to know whether it can touch the world.

### 10.5 Determinism versus effects

`Allocator` is a **resource** effect: it can fail (out of memory) and it costs
something, but it is not observable. Every other effect in `core/effect` is
**observable**.

A function is **deterministic** if its only effect bound is `Allocator`.
`list.map(ctx, f)` is deterministic: it needs to allocate, but it is
referentially transparent. `time.now(ctx)` is not.

Tracking allocation is what makes "does no I/O" and "does not allocate"
separately expressible:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Allocator, IoError };
# from "core/fs" import { FileSystemRead, Path };
fn sum(self): Int                                                      // pure
fn map<A, B, C: Allocator>(self, ctx: C, f: fn(A) => B): [B]               // deterministic
fn readText<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Str, IoError> // effectful
```

Fixed-size construction — struct literals, tuples, enum payloads, array literals,
closures, `Template`s — never requires `Allocator`. Only results whose size depends
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
past a signature with no `ctx` parameter.

The rule reaches every binding a lambda could close over — parameters, `let`
bindings, names a pattern binds, and the parameters of an *enclosing* lambda.
The context a `*Ctx` combinator hands to `fn(c, p) => ...` is a binding like any
other, so a lambda written inside that one may not capture `c` either.

**And a lambda may not capture a value whose type could be a context.** This is
the same rule at a type parameter, where "carries an effect" cannot be read off
the type — so `fn wrap<T>(x: T, f: fn(T) => ()): fn() => () { fn() => f(x) }` is
rejected, on the capture of `x`. Nothing in `wrap` mentions an effect, yet
`wrap(ctx, fn(c) => io.println(c, "hi").ignore())` instantiates it at a context
type and returns a `fn() => ()` holding an effect. So the rule treats a type
parameter as though it *were* a context, unless one of two things says otherwise:

- **An ordinary trait bound.** An effect-carrying type satisfies no ordinary
  bound (Section 10.1), so a `T: Eq` is never a context and
  `xs.any(fn(x) => x == needle)` inside `impl<T: Eq> [T]` is fine. A `T` with no
  bounds, or one bounded only by effects, has no such guarantee.
- **A function type.** A closure holds exactly what this rule let it capture, so
  capturing one is safe whatever its type parameters are: `fn compose<A, B, C>(f:
  fn(A) => B, g: fn(B) => C): fn(A) => C { fn(x) => g(f(x)) }` is legal.

The cost is that a closure-builder over an unconstrained type parameter has to
take the value as a parameter rather than close over it. The standard library
provides `*Ctx` variants (`list.mapCtx`, `list.filterCtx`, `result.andThenCtx`),
and explicit recursion is always available when the combinator does not fit.
`design/non-goals.md` lists this as the first open question.

**A callback an effect declares gets a context only if the declaration names one,
and `Self` never names one.** This is the capture rule read from the other end.
An effect method may take a callback — `Tasks.parallel` takes the step that runs
on every item — and that callback cannot close over a context, so whatever
authority it is to have arrives as its first parameter. Two different values
could arrive there, and the declaration says which:

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

`Self` is the **implementing type** everywhere you write it: in an `impl` head,
in an effect's declaration, and inside a callback's parameter list. It is not the
receiver. Through a `context { … }` value the two differ, because a context
*names* a value that implements the effect rather than being one.

So an effect that wants to hand a callback the **caller's** authority takes the
caller's context as an ordinary `ctx` parameter and spells the callback
`fn(C, …)`. The caller passes the same value twice, once as the receiver and once
as `ctx`: the receiver chooses the implementation, and `ctx` is the authority the
work runs with. Naming it rather than overloading `Self` keeps an effect an
ordinary interface (Section 10.9), so a fake in a test runs its steps exactly as
the shipping implementation does.

A callback whose first parameter is `Self` receives strictly less than its caller
had. An acceptor grants `Listen` and nothing else, so a handler handed one cannot
allocate, print, or start a task. That is the right answer where the callback is
meant to inspect the implementation, and the wrong one for a request handler, so
the declaration writes the choice down per method. `Listen` carries no callback
at all today: the loop that calls a handler lives in `core/net/server`, written
in Buri against the caller's own `C`, so a handler there may allocate, print,
read a clock and start a task.

### 10.7 Calling convention

**Receiver first, context second, everything else after.** Section 10.2 enforces
this. A free function with no receiver takes the context first:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Allocator, IoError };
# from "core/fs" import { FileSystemRead, Path };
export fn map<A, B, C: Allocator>(self, ctx: C, f: fn(A) => B): [B]
export fn readText<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Str, IoError>
```

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
xs.map(ctx, double)
lines.filter(ctx, isLong).sortBy(ctx, order.str)
```

An effect's own operations take the second shape and only the second shape. They
have no receiver a program may name, so they are free functions taking the
context first: `io.println(ctx, text)`, `fs.readText(ctx, path)`. The method form
is not an alternative spelling of them; the compiler refuses it
(`effect-method-call`).

### 10.8 Restricting what propagates

Two forms, giving different guarantees.

**Static confinement.** Bound the callee to fewer effects. It receives the
same value and cannot use, or pass on, anything its bounds do not name:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Allocator, Stdout };
# from "core/fs" import { FileSystemRead };

fn logOnly<C: Stdout>(ctx: C, msg: Str): () {
    let _ = io.println(ctx, msg).ignore();
    // fs.readText(ctx, secrets)           // ERROR: C is not bounded by FileSystemRead
    // dangerous(ctx)                      // ERROR: dangerous needs C: FileSystemRead
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        FileSystemRead: host.fs,
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
# from "core/effect" import { Allocator, IoError, Region };
# from "core/fs" import { FileSystemRead, Path };

// module: safe/readonly
export struct ReadOnly<C>(C);

export fn readOnly<C>(ctx: C): ReadOnly<C> {
    ReadOnly(ctx)
}

// Forwards Allocator...
impl<C: Allocator> Allocator for ReadOnly<C> {
    fn allocate(self, bytes: Int): Region {
        self.0.allocate(bytes)
    }
}

// ...and reading, as an inherent `impl` rather than an `impl FileSystemRead for` — so
// ReadOnly<C> satisfies no effect at all, and a callee holding one cannot pass
// it on as a context.
impl<C: FileSystemRead> ReadOnly<C> {
    export fn readFile(self, at: Path): Result<Str, IoError> {
        self.0.readFile(at)
    }
}
```

Static confinement is a fact about the type checker; attenuation is a fact about
the value, and survives anything that later escapes the type system. Use the
first by default and the second at trust boundaries.

Attenuation narrows the *context*, not one effect out of it. That is what keeps
the `ctx` rule satisfiable: there is still exactly one effect-carrying
parameter.

**The `self.0.readFile(path)` above is the carve-out of Section 10.2.** A body
supplying an effect is one of the two layers that may still call an effect method
on a value. It cannot delegate to `fs.readText(self.0, path)` instead: that
wrapper is bounded `Allocator + FileSystemRead` and this `impl` carries only `C: FileSystemRead`.

### 10.9 Testing

A pure function needs no harness. You test an effectful one by building a context
out of different implementations, and since effects are ordinary interfaces,
writing one is writing a struct with methods. The call site does not change.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Allocator, IoError };
# from "core/fs" import { FileSystemRead, Path };

struct FakeFs {
    export files: [(Str, Str)],
}

// Seven methods, not sixteen. A double for the half the code under test needs
// restates only that half, which is the other thing splitting the filesystem
// bought.
impl FileSystemRead for FakeFs {
    fn readFile(self, at: Path): Result<Str, IoError> {
        match (self.files.find(fn(e) => e.0 == at.text())) {
            .Some(entry) => .Ok(entry.1),
            .None => .Err(.NotFound),
        }
    }

    fn fileExists(self, at: Path): Bool {
        self.files.any(fn(e) => e.0 == at.text())
    }

    fn readDir(self, at: Path): Result<[Str], IoError> {
        .Err(.NotFound)
    }

    fn readFileBytes(self, at: Path): Result<[U8], IoError> {
        .Err(.NotFound)
    }
}

// context { Allocator: testing.alloc(), FileSystemRead: FakeFs { files: [...] } }
// loadConfig<C: Allocator + FileSystemRead> accepts it with no changes anywhere.
```

Sections 11.2 and 11.3 cover the harness around that: where tests live, how you
declare them, and how they build a context.
