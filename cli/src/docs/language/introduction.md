## 1. Introduction

Buri is a strict, purely functional, statically typed language with TypeScript-shaped
syntax, Rust-shaped data declarations, and Roc-shaped ideas about platforms and
effects.

Three ideas define it:

1. **There is no mutation.** Every binding is final. There are no references, no
   borrowing, and no lifetimes. Values are values.
2. **Effects travel through arguments.** The ability to allocate, read a file, or
   open a socket is a *value* of an unforgeable type. A function that was not
   handed one cannot perform that effect. Purity is therefore a property you can
   read off a signature, not a property the compiler asks you to trust.
3. **The grammar is context-free and unambiguous.** Parsing never consults name
   resolution or types. `design/grammar-rationale.md` documents each design decision that pays for
   this, and what was given up to get it.

Version 0.3 is deliberately small: primitives, arrays, tuples, structs, enums,
functions, methods, and traits. Data and behaviour are declared
separately; there is no mutable state, no inheritance, and no dynamic dispatch. A
method is an ordinary function whose first parameter is `self`, and a trait is an
interface satisfied nominally — neither introduces a runtime mechanism.

There are also no loops. Iteration is recursion — guaranteed tail-call
eliminated — or a fold. A `for`/`while` sugar was drafted for this version and
cut; `design/non-goals.md` records why.

### 1.1 A taste

```buri run
# from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;

struct Point {
    x: Float,
    y: Float,
}

enum Shape {
    Circle(Float),
    Rect { width: Float, height: Float },
    Empty,
}

// No context parameter, so this cannot allocate, read, write, or observe
// anything. It is a mathematical function of its argument.
impl Shape {
    fn area(self): Float {
        match (self) {
            .Circle(r) => 3.14159 * r * r,
            .Rect { width, height } => width * height,
            .Empty => 0.0,
        }
    }
}

// `main` builds the one context the program has. Its bindings are the program's
// complete effect budget: neither half of the filesystem is here, so nothing
// this program transitively calls can read a file, let alone write one.
export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
    };

    let shapes = [Shape.Circle(1.0), Shape.Rect { width: 2.0, height: 3.0 }];
    let total = shapes.map(ctx, fn(s) => s.area()).sumFloat();
    let _ = io.println(ctx, "total area: ${total}").ignore();
    .Ok(())
}
```

```stdout
total area: 9.14159
```

---
