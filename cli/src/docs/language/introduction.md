## 1. Introduction

Buri is a strict, purely functional, statically typed language. It has
TypeScript's syntax, Rust's data declarations, and Roc's ideas about platforms
and effects.

- **No mutation.** Every binding is final. No references, borrowing or
  lifetimes.
- **Effects travel through arguments.** The ability to allocate, read a file, or
  open a socket is a *value* of an unforgeable type. A function nobody handed one
  can't perform that effect, so you read purity off a signature.
- **The grammar is context-free and unambiguous.** Parsing never consults name
  resolution or types. `design/grammar-rationale.md` records what that cost.

Version 0.3 has primitives, arrays, tuples, structs, enums, functions, methods,
and traits. Data and behaviour are declared separately. A method is an ordinary
function whose first parameter is `self`; a trait is an interface a type
satisfies nominally. There's no inheritance, no dynamic dispatch, and no loops:
you iterate with tail-call-eliminated recursion or a fold
(`design/non-goals.md`).

### 1.1 A taste

```buri run
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
# from "platform/effect" import { Allocator, Stdout };

struct Point {
    x: Float,
    y: Float,
}

enum Shape {
    Circle(Float),
    Rect { width: Float, height: Float },
    Empty,
}

// No context parameter, so this can't allocate, read, write, or observe
// anything.
impl Shape {
    fn area(self): Float {
        match (self) {
            .Circle(r) => 3.14159 * r * r,
            .Rect { width, height } => width * height,
            .Empty => 0.0,
        }
    }
}

// `main` builds the program's only context: its whole effect budget. No
// filesystem here, so nothing this program calls can read or write a file.
export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
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
