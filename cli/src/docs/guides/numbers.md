# Numbers: two names, one set of types

Most code wants to say "a number". Some code needs an exact width and wants the
compiler to hold it there: binary formats, checksums, graphics, FFI. `Int` and
`Float` are **aliases** for `I64` and `F64`, so the two kinds of code meet with
no conversions at the boundary.

```buri wrap=body
let a = 5; // nothing pins it -> Int
let b: U8 = 200; // the annotation pins it -> U8, not a conversion
let c: [F32] = [1.5]; // literals take their type from context
let bad: U8 = 300; // ERROR: 300 is not representable in `U8`
```

A numeric literal has no type until something constrains it, and falls back to
`Int` or `Float` only when nothing does. So the compiler catches out-of-range
literals, and there are no `5u8` suffixes. There is **no implicit promotion at
all** (`1 + 1.0` is an error), and conversions are ordinary methods rather than
cast operators:

```buri wrap=body
# let small: I32 = 5;
# let big: I64 = 5000;
let exact = small.toI64(); // always exact — returns I64
let maybe = big.toI32(); // may not fit  — returns Result<I32, RangeError>
let wrapped = big.wrapToU8(); // modular      — keeps the low bits, for wire formats
```

The return type says whether a conversion can fail. Overflow is undefined
behaviour rather than silent wrapping: reach for `x.wrappingAdd(y)` or
`x.saturatingAdd(y)` when wrapping is the intent, and `x.checkedAdd(y)` when you
want to be told.
