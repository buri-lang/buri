# Why Buri

A signature in most languages tells you what a function takes and what it
returns, and nothing else. Whether it writes a file, reads the clock, mutates
the array you handed it, or allocates on every call is knowledge you get by
reading the body, and then the bodies of everything the body calls. Most
questions about a piece of code are answered the same way: read further, and
trust that nothing three levels down surprises you.

That is expensive for a person and it does not work at all for an agent, which
writes code from a fraction of a repository, cannot hold the rest in view, and
often cannot run what it wrote. Both readers need the same thing: claims that
are local, and checked rather than believed. Buri is built so the compiler
answers them.

| A question you have to answer constantly | What answers it in Buri |
|---|---|
| Can this function touch the world? | Its first parameter. No `ctx`, no effects — no I/O, no clock, no allocation. |
| Can it change what I passed in? | No. Nothing can; there is no mutation, no reference, no alias. |
| Did I handle every case? | `match` is exhaustive or the program does not compile. |
| Can this be null? | There is no `null`. Absence is `Option`, and opening one is a case you write. |
| Did I drop an error? | `Result` is must-use. Discarding one is a compile error. |
| Is this index in range? | Indexing returns `Option`, so the question is asked at the index. |
| What does this file mean by itself? | All of it. No macros, no reflection, no conditional compilation, no overloads. |

A whole program, with every one of those answers visible in it:

```buri run
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

enum Entry {
    Deposit(Int),
    Withdrawal(Int),
    Correction { was: Int, now: Int },
}

impl Entry {
    // No context parameter, so this cannot allocate, print, read a file, or
    // open a socket. It is a function of its argument and of nothing else, and
    // the signature alone says so.
    fn delta(self): Int {
        match (self) {
            .Deposit(n) => n,
            .Withdrawal(n) => 0 - n,
            .Correction { was, now } => now - was,
        }
    }
}

// `main` takes no arguments. It builds the one context the program has, and
// those two bindings are the whole effect budget: neither half of the
// filesystem is here, so nothing this program transitively calls can read a
// file, let alone write one.
export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
    };

    let ledger = [Entry.Deposit(120), Entry.Correction { was: 10, now: 30 }];
    let balance = ledger.map(ctx, fn(e) => e.delta()).sum();
    let _ = io.println(ctx, "balance: ${balance}").ignore();
    .Ok(())
}
```

```stdout
balance: 140
```

## The priorities

**Safe, fast to run, fast to compile, friendly** — in that order when they
conflict, which they do. Secondarily, one language that targets both a native
binary and JavaScript. Every trade in the design is an application of that
ordering, and each one cost something:

| Goal | What it bought, and what it cost |
|---|---|
| **Safe** | No `null`, no exceptions, no mutation, no aliasing. Exhaustive `match`. Indexing returns `Option`. `Result` is must-use. Effects require an effect value you were handed, in a parameter position the compiler enforces. Out-of-range numeric literals are compile errors. *Costs:* every absence and every failure is a case you write out, and an index is two lines where it used to be one. |
| **Fast to run** | Strict evaluation with fully specified order — no thunks, no space leaks. Monomorphized generics, no dictionaries. Guaranteed tail calls, lowered to loops where the host lacks them. Immutability lets the runtime reuse memory in place when a value is provably unshared. `Alloc` as an effect makes allocation visible at every call site that does it. *Costs:* a function that allocates says so in its signature, and that propagates to its callers. |
| **Fast to compile** | An unambiguous grammar — LR(1) but for one production, where a `<` needs the `>` found before it can be told from a comparison — with no name-resolution or type feedback into the parser, so parsing is one pass and trivially parallel across files. Mandatory top-level signatures make type inference local to each function body, so modules check independently and incrementally. No macros, no reflection, no overload resolution, no row unification. Conformance is nominal and declared in one module, so there is no coherence pass and no instance search — a bound is a table lookup. *Costs:* annotations you would rather infer, and the one concession that method resolution needs the receiver's type, so name resolution and inference interleave. |
| **Friendly** | Errors name a fix rather than a symptom, and each one has a page: `buri docs error <code>` prints the diagnostic, the program that provokes it, and the way out. Lints argue about architecture, not whitespace. The toolchain is one binary that builds, tests, formats, lints, generates build files, and serves this documentation. *Costs:* friendliness loses the other three arguments — parenthesized conditions and a mandatory `else` are here because the parser wants them, not because they read better. |
| **Binary and JS** | Nothing in the semantics assumes a machine word: `Int` is `I64` everywhere, integer overflow is undefined rather than quietly wrapping, and evaluation order is specified rather than left to the backend. The effect model maps onto a browser platform as cleanly as onto a POSIX one — a JS target simply exports a different `core/host`. *Costs:* the 64-bit integer types are `BigInt`s on the JavaScript backend, which is correct and slower than a `number`. |

Where the goals pull against each other, the compiler absorbs it rather than
the language. Guaranteed tail calls become loops on a JS target, since no
engine but JavaScriptCore implements them natively
([`language/evaluation.md` §8.3.1](../language/evaluation.md)).
[How Buri compiles fast](../guides/compile-speed.md) states what has to stay
true for the compile-speed goal to keep holding, so a future feature can be
measured against it rather than quietly eroding it.

## Three ideas

### There is no mutation

Every binding is final. No references, no borrowing, no lifetimes, no aliasing
hazards, and no question about who else can see the change you just made.
"Updating" a value produces a new one, and the runtime is expected to make that
cheap through structural sharing and in-place update when a value is provably
unshared — an implementation strategy that is never observable.

### Effects travel as arguments

An **effect** is an interface — declared with `effect` instead of `trait`, and
only by platform modules — and a function names the ones it needs as bounds on
its context parameter:

```buri sig role=platform
# from "core/effect" import { Alloc, IoError };

# struct User(Int);

# enum LoadError {
#     NotFound,
# }

// The real pair, in `core/fs`. Reading and writing are two effects because
// they are two grants: a program that reads its configuration has not thereby
// earned the right to delete it.
effect FsRead {
    fn readFile(self, path: Str): Result<Str, IoError>;
}

effect FsWrite {
    fn writeFile(self, path: Str, body: Str): Result<(), IoError>;
}

fn loadUser<C: Alloc + FsRead>(ctx: C, id: Str): Result<User, LoadError>;
```

The compiler enforces where an effect may arrive: **an effect-carrying
parameter must be `self` or `ctx`**, never any other name or position. So the
question "can this function touch the world?" is answered by reading the first
two parameters and stopping. No type may implement both an effect and a trait,
so the boundary between the world and your data is checked rather than assumed.

The implementations that really do anything live in a module `core/host`, which
only the file exporting `main` may import. `main` takes no parameters: it names
the effects it wants, binds each to one of those implementations, and passes
the result down. A program whose `main` never binds `Net` cannot open a socket
anywhere in its transitive call graph, because nothing anywhere can obtain a
value bounded by `Net`. Giving a callee less is naming fewer bounds, and the
bound is what the callee is confined to:

```buri
# from "core/effect" import { Stdout };
# from "core/fs" import * as fs;
# from "core/fs" import { FsRead, Path };
# from "core/io" import * as io;

/// A caller may hand this the context that also carries `FsRead`. The value is
/// the same one; the bound is what this function can do with it.
fn logOnly<C: Stdout>(ctx: C, msg: Str, at: Path): () {
    let _ = io.println(ctx, msg).ignore();
    let _f = fs.readText(ctx, at); // ERROR: `C` does not satisfy `FsRead`
}
```

There is no copy, no wrapper, and no runtime cost, and the confinement is
transitive: `C` is just as opaque at every call `logOnly` makes. A test builds
its context the same way `main` does, from the test runner's implementations,
so a test double is a struct with methods and there is no mocking framework to
learn.

Purity is therefore not a keyword, not an inferred effect row, and not
something to propagate through signatures. It is the *absence* of one argument:

> If a function has no `ctx` parameter, no effect-carrying `self`, captures no
> effect, and builds no context, then it is deterministic, effect-free, and
> freely cacheable.

The last clause covers `main` and a test, the only two places a context is
built. Neither is a function library code can call, so in ordinary code the
check is still just: *is there a `ctx` parameter?*

Three tiers fall out — pure, deterministic, effectful — and each is visible in
the signature at a glance. Allocation is tracked separately from I/O, so "does
no I/O" and "does not allocate" are separately expressible. The rule that
decides which tier an operation is in is in
[the standard library](../reference/standard-library.md#the-purity-tiers), and
[the effects guide](../guides/effects.md) shows the whole pattern at work.

### The grammar is context-free and unambiguous

Parsing never consults name resolution or the type checker, so a file's syntax
tree does not depend on anything outside it — the property that makes parsing
one parallel pass, and the reason a tool, or a reader, can be certain how a
snippet reads without the rest of the repository. That constraint cost real
ergonomics, and
[`design/grammar-rationale.md`](../../../../design/grammar-rationale.md) lists
all eighteen decisions with what each one gave up: parenthesized `if`
conditions, no expression statements, non-associative comparison, no `<<` or
`>>` token, no cast operator, dot-prefixed variants in patterns, a mandatory
`else`, and the rest.

## The name

Búri, in Norse myth, is the first god — the one the others are descended from.
The language is named for the ambition, not the achievement.

## Where to go next

This repository is the specification and the toolchain that implements it.
`buri docs` serves all of it from the binary, and every example in it is
compiled — `buri docs cli docs` says what that guarantees and how.

[Installing](./installing.md) takes about a minute, and
[your first program](./first-program.md) is the next page.
