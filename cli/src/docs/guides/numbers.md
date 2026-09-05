# Numbers: two names, one set of types

Most code wants to say "a number." Some code — binary formats, checksums,
graphics, FFI — needs an exact width and wants the compiler to hold it there.
`Int` and `Float` are **aliases** (`I64`, `F64`), not a separate tier, so the two
kinds of code interoperate with no conversions at the boundary.

```buri wrap=body
let a = 5; // nothing pins it -> Int
let b: U8 = 200; // the annotation pins it -> U8, not a conversion
let c: [F32] = [1.5]; // literals take their type from context
let bad: U8 = 300; // ERROR: 300 is not representable in `U8`
```

A numeric literal has no type until something constrains it, and only falls back
to `Int`/`Float` when nothing does — so out-of-range literals are caught at
compile time and there are no `5u8` suffixes to learn.

There is **no implicit promotion at all** (`1 + 1.0` is an error), and
conversions are ordinary methods rather than cast operators:

```buri wrap=body
# let small: I32 = 5;
# let big: I64 = 5000;
let exact = small.toI64(); // always exact — returns I64
let maybe = big.toI32(); // may not fit  — returns Result<I32, RangeError>
let wrapped = big.wrapToU8(); // modular      — keeps the low bits, for wire formats
```

Whether a conversion can fail is visible in its return type rather than in the
choice of operator. Overflow is undefined behaviour rather than silent wrapping;
`x.wrappingAdd(y)` and `x.saturatingAdd(y)` are there when wrapping is the
intent, and `x.checkedAdd(y)` when you want to be told.
