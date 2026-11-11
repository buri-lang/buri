# #331: native `core/math` transcendentals

Native builds refused `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `exp`, `ln`, `log2`, `log10`, `cubeRoot` and `power`.

## Design

- Each is a `runtime_table.rs` row and a `buri_rt_math_*` body in `cli/runtime/math.rs`, so stencil and LLVM both reach it with no backend code.
- Twelve call libm through `f64`'s methods. JavaScript's `Math.*` on Bun calls the same libm, so on macOS native and JS match bit for bit.
- `power` is fdlibm's `e_pow.c`, ported to Rust (`buri_rt_math_power`) and to `runtime.js` (`$math_power`), plus ECMA-262's two `NaN` cases. Engines' `Math.pow` differ: node 20 is one unit below node 24 and Bun at `pow(2, 1.5)`, and CI's Linux jobs run node 20. The two ports give identical bits on 601,296 inputs, on node 20, node 24 and Bun, and equal Java's `StrictMath.pow`, which is fdlibm.
- The rejected alternative for the other twelve: port them too. That is bit-identical everywhere, but it's a large port and makes JS slower than `Math.*`.

## Measured libm differences

Run on 500,000 inputs, comparing bits. Scripts in `/tmp/m331` (gone after the session).

| pair | result |
| --- | --- |
| Apple libm vs Bun on macOS | identical; `power` no longer calls either |
| musl (native Linux) vs Bun on Linux (glibc) | 1 to 2 ulp apart on up to 11% of inputs (`atan2`); 0.0002% for `ln` |
| Apple vs musl | up to 3 ulp (`tan`) |
| Bun on Linux `cbrt` | not glibc's; matches musl within 1 ulp |

Apple's libm differs from musl and glibc at common points: `tan(1.0)`, `asin(0.5)`, `acos(0.5)`, `atan(0.5)`, `atan(2.0)`. glibc's `cbrt(-27.0)` is `-3.0000000000000004`.

So conformance values are either fixed by IEEE 754 or ECMA-262, or checked equal on Apple, musl, glibc and Bun on Linux.

## Phases

1. Done in one landing: all thirteen on both back ends, `numbers/transcendentals.buri` on JS, stencil and LLVM, and `numbers/floats.buri` and `random/gaussian.buri` in the native set. Tests that used `math.ln` or `math.sin` as the example native gap now use `ui_node.mount`, which is WEB-only by design. With `numbers` and `random` native, `web` is the only mixed package, so `release.rs` and `split_packages.rs` now check `//lib/web` is among them instead of counting three.
2. Done: `power` became the fdlibm port after CI's node 20 jobs failed on libm-dependent points.
3. Close #331 once CI is green.

## Other native gaps

Found by asking both back ends' `implemented` about every intrinsic key the standard library declares (1022 keys). No key is missing on just one back end.

- `host_testing.Rendered.follow` and `host_testing.Rendered.openInNewTab`: JS bodies only (`runtime.js`); no runtime row, no `cli/runtime/document.rs` body.
- The primitive `fromJson` keys (`bool.fromJson`, `str.fromJson`, `number.<T>.fromJson`, ...) have no body on any back end, JS included. `json.decode` never reaches them; whether a direct call type-checks is unverified.
- Web and UI platform keys (`host.HostUi.*`, `ui_web.*`, `web.*`, `ui_node.mount`) are JS-only by design.
