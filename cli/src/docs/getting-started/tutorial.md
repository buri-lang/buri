# Tutorial: a small program, end to end

In this tutorial we will build `convert`, a command line program that turns one
length into another. We will write two libraries, a binary that depends on
them, and the tests that hold all three up — starting from an empty directory
and finishing with a program that runs. It takes about half an hour.

Do [your first program](./first-program.md) before this one: it assumes `buri`
is installed and that you have run `buri test` and `buri run` once.

This is what we are building:

```text
$ buri run //apps/convert -- 26.2 mi km
26.2 mi = 42.16 km
```

## 1. The repository

Make every directory we will need, and move in:

```text
$ mkdir -p convert/libs/units/test convert/libs/convert/test convert/apps/convert
$ cd convert
```

Write `REPO.buri`. Its presence is what makes this directory the repository
root, and what `//` resolves against in every label and every module path:

```textproto schema=repo
# The repository root. Its presence is what `//` resolves against.
lint {
    check_during_build: true
    fail_on_finding: true
}
```

## 2. The library that knows about lengths

Write `libs/units/BUILD.buri`. A directory with a build file in it is a
package, and this one declares a library: its sources, its tests, and who may
depend on it, each listed one path at a time:

```textproto schema=build
library {
    sources: ["units.buri"]
    visibility: ["//libs/convert"]

    test {
        sources: ["test/units.buri"]
    }
}
```

Write `libs/units/units.buri`. This is the whole of what the program knows
about lengths, and none of it can touch the world:

```buri repo=cli/tests/tutorial package=//libs/units
from "core/effect" import { Alloc };
from "core/math" import * as math;
from "core/str" import * as str;

derive Eq, Show for Unit;
/// A length unit. Every conversion goes through metres.
export enum Unit {
    Metres,
    Kilometres,
    Miles,
    Feet,
}

derive Eq, Show for Quantity;
/// A number with its unit attached, so the two cannot drift apart.
export struct Quantity {
    export amount: Float,
    export unit: Unit,
}

derive Eq, Show for ParseError;
/// Each variant carries the word, so a caller can say which one was wrong.
export enum ParseError {
    NotANumber(Str),
    UnknownUnit(Str),
}

export fn parseUnit(word: Str): Result<Unit, ParseError> {
    match (word) {
        "m" => .Ok(.Metres),
        "km" => .Ok(.Kilometres),
        "mi" => .Ok(.Miles),
        "ft" => .Ok(.Feet),
        other => .Err(.UnknownUnit(other)),
    }
}

/// Two words in, a quantity out. `?` gives up on the first failure and hands
/// the caller its error, which is why both lines read as if nothing could go
/// wrong.
export fn parseQuantity(amount: Str, unit: Str): Result<Quantity, ParseError> {
    let n = amount.toFloat().okOr(ParseError.NotANumber(amount))?;
    let u = parseUnit(unit)?;
    .Ok(Quantity { amount: n, unit: u })
}

impl Unit {
    // Exported to the rest of this library and withheld by lib.buri, so it is
    // invisible outside //libs/units.
    export fn metres(self): Float {
        match (self) {
            .Metres => 1.0,
            .Kilometres => 1000.0,
            .Miles => 1609.344,
            .Feet => 0.3048,
        }
    }

    export fn symbol(self): Str {
        match (self) {
            .Metres => "m",
            .Kilometres => "km",
            .Miles => "mi",
            .Feet => "ft",
        }
    }
}

impl Quantity {
    /// No `ctx`, so converting a length cannot read, print or allocate.
    export fn into(self, unit: Unit): Quantity {
        Quantity {
            amount: self.amount * self.unit.metres() / unit.metres(),
            unit: unit,
        }
    }

    /// `Alloc` and nothing else: building a `Str` allocates, and that is all
    /// this does.
    export fn format<C: Alloc>(self, ctx: C): Str {
        let rounded = math.round(self.amount * 100.0) / 100.0;
        str.format(ctx, "${rounded} ${self.unit.symbol()}")
    }
}
```

Write `libs/units/lib.buri`. It is not in `sources` because the rule kind names
it: it is the library's entire public surface, and a name missing from it
cannot be reached from another package, as a function or as a method:

```buri repo=cli/tests/tutorial package=//libs/units
//! The public surface of //libs/units. `metres` is exported by units.buri and
//! withheld here, so it is visible inside this library and nowhere else.

from "//libs/units/units.buri" export {
    format, into, ParseError, parseQuantity, parseUnit, Quantity, symbol, Unit,
};
```

Write `libs/units/test/units.buri`. The suite imports the library by label,
exactly as a dependent does, so it can only assert on what dependents can call:

```buri repo=cli/tests/tutorial package=//libs/units role=test
from "core/effect" import { Alloc };
from "core/host/testing" import { alloc };
from "core/testing/assert" import * as assert;
from "//libs/units" import { ParseError, parseQuantity, Quantity, Unit };

test "two words make a quantity" {
    let marathon = assert.ok(parseQuantity("26.2", "mi"));
    assert.eq(marathon, Quantity { amount: 26.2, unit: Unit.Miles });
}

test "a word that names no unit comes back with the word" {
    assert.eq(
        assert.err(parseQuantity("1", "furlong")),
        ParseError.UnknownUnit("furlong"),
    );
}

test "a marathon is 42.16 kilometres, to two places" {
    let ctx = context {
        Alloc: alloc(),
    };
    let marathon = Quantity { amount: 26.2, unit: Unit.Miles };
    assert.eq(marathon.into(.Kilometres).format(ctx), "42.16 km");
}
```

Run them (timings vary, here and below):

```text
$ buri test //libs/units
3 passed, 0 failed, 0 skipped (0.5s)
```

## 3. The second library: the command line

`//libs/units` knows nothing about a command line, and we are going to keep it
that way. Write `libs/convert/BUILD.buri`. This one has a `dependencies` list,
and the edge it declares is the only reason `//libs/units` is reachable from
here:

```textproto schema=build
library {
    sources: ["convert.buri"]
    dependencies: ["//libs/units"]
    visibility: ["//apps/convert"]

    test {
        sources: ["test/convert.buri"]
    }
}
```

Write `libs/convert/convert.buri`. Everything above `run` is pure; `run` is the
one function that reads the world and writes to it, and its bounds say exactly
which parts of the world it gets:

```buri repo=cli/tests/tutorial package=//libs/convert
from "core/effect" import { Alloc, Env, Stdout };
from "core/env" import * as env;
from "core/io" import * as io;
from "core/str" import * as str;
from "//libs/units" import { ParseError, parseQuantity, parseUnit, Quantity, Unit };

derive Eq, Show for Request;
/// One conversion to perform.
export struct Request {
    export quantity: Quantity,
    export target: Unit,
}

derive Eq, Show for ConvertError;
/// Everything that can go wrong between the command line and the answer.
export enum ConvertError {
    Usage,
    Bad(ParseError),
    CouldNotPrint,
}

impl ConvertError {
    /// No `ctx`: every arm is a literal, and a literal allocates nothing.
    export fn message(self): Str {
        match (self) {
            .Usage => "usage: convert <amount> <from> <to>",
            .Bad(.NotANumber(_)) => "the amount is not a number",
            .Bad(.UnknownUnit(_)) => "the units are m, km, mi and ft",
            .CouldNotPrint => "could not write to standard output",
        }
    }
}

/// //libs/units has its own error type, and `?` does not convert between error
/// types, so the conversion is written once here.
fn bad(e: ParseError): ConvertError {
    ConvertError.Bad(e)
}

/// Three words in, a request out. No `ctx`, so this cannot read the command
/// line itself — it is handed the words.
export fn parseRequest(words: [Str]): Result<Request, ConvertError> {
    match (words) {
        [amount, source, target] => {
            let quantity = parseQuantity(amount, source).mapErr(bad)?;
            let unit = parseUnit(target).mapErr(bad)?;
            .Ok(Request { quantity: quantity, target: unit })
        },
        _ => .Err(.Usage),
    }
}

/// The line the program prints.
export fn describe<C: Alloc>(ctx: C, request: Request): Str {
    let before = request.quantity.format(ctx);
    let after = request.quantity.into(request.target).format(ctx);
    str.format(ctx, "${before} = ${after}")
}

/// The edge: the one function here that reads the world and writes to it.
export fn run<C: Alloc + Env + Stdout>(ctx: C): Result<(), ConvertError> {
    let request = parseRequest(env.args(ctx))?;
    io.println(ctx, describe(ctx, request)).mapErr(fn(e) => ConvertError.CouldNotPrint)
}
```

Write `libs/convert/lib.buri`:

```buri repo=cli/tests/tutorial package=//libs/convert
//! The public surface of //libs/convert.

from "//libs/convert/convert.buri" export {
    ConvertError, describe, message, parseRequest, Request, run,
};
```

## 4. A test that hands `run` a world of our own

`run` needs `Env` to read the command line. A test gives it one by writing a
struct with the effect's two methods on it — an effect is an ordinary
interface, so there is no mocking framework and nothing global to stub.

Write `libs/convert/test/convert.buri`:

```buri repo=cli/tests/tutorial package=//libs/convert role=test
from "core/effect" import { Alloc, Env, Stdout };
from "core/host/testing" import { alloc, stdout };
from "core/testing/assert" import * as assert;
from "//libs/convert" import { ConvertError, parseRequest, run };

/// A test double for `Env`: an ordinary struct with the effect's two methods.
struct FixedArgs {
    export words: [Str],
}

impl Env for FixedArgs {
    fn variable(self, name: Str): Option<Str> {
        .None
    }

    fn args(self): [Str] {
        self.words
    }
}

test "too few words is a usage error" {
    assert.eq(assert.err(parseRequest(["1", "km"])), ConvertError.Usage);
}

test "an unknown unit has a line for the user" {
    assert.eq(
        assert.err(parseRequest(["1", "mi", "furlong"])).message(),
        "the units are m, km, mi and ft",
    );
}

test "run reads its arguments and prints one line" {
    let out = stdout();
    let ctx = context {
        Alloc: alloc(),
        Env: FixedArgs { words: ["10", "km", "mi"] },
        Stdout: out,
    };
    assert.ok(run(ctx));
    assert.eq(out.captured(), "10.0 km = 6.21 mi\n");
}
```

The last test never opens a terminal and never reads a real command line:
`FixedArgs` is the arguments, `stdout()` is a captured stream, and `captured()`
reads back what the program wrote to it. Run every suite in the repository:

```text
$ buri test
6 passed, 0 failed, 0 skipped (0.2s, 3 cached)
```

The three cached ones are `//libs/units`, unchanged since we last ran it.

## 5. The binary

Write `apps/convert/BUILD.buri`. A binary declares no `visibility`, because
nothing can depend on a binary:

```textproto schema=build
binary {
    dependencies: ["//libs/convert"]
}
```

Write `apps/convert/main.buri`. This is the only file in the repository allowed
to import `core/host`, and the `context` it builds is the program's entire
effect budget:

```buri repo=cli/tests/tutorial package=//apps/convert role=entry
from "core/effect" import { Alloc, Env, Stdout };
from "core/host" import * as host;
from "//libs/convert" import { run };

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Env: host.env,
        Stdout: host.stdout,
    };

    match (run(ctx)) {
        .Ok(_) => .Ok(()),
        .Err(e) => .Err(e.message()),
    }
}
```

Run it:

```text
$ buri run //apps/convert -- 26.2 mi km
26.2 mi = 42.16 km
$ buri run //apps/convert -- 100 ft m
100.0 ft = 30.48 m
$ buri run //apps/convert -- 5 km ft
5.0 km = 16404.2 ft
```

## 6. Format and lint the whole repository

`buri format` is the one canonical layout — four-space indent, sorted imports,
no options — and it prints the files it rewrote. `buri lint` is the checks
beyond type checking:

```text
$ buri format
$ buri lint
no findings
```

`buri format` printed nothing, because every file above was already laid out
the way it lays a file out.

That is the program: eleven files, two libraries with a build file, a surface,
a source file and a suite each, a binary with a build file and a `main`, and a
repository root over the top of them.

## Where to go next

- [The build system](../guides/build-system.md) — packages, labels,
  dependencies and visibility, in more detail than this tutorial needed.
- [Testing](../guides/testing.md) — fixtures, fault plans, and the rest of
  `core/host/testing`.
- [Restricting effects](../guides/effects.md) — what a `ctx` bound buys, and
  how to hand a callee less of the world than you hold.
- [The standard library](../reference/standard-library.md) — what is in
  `core/*`, and what each module costs.
