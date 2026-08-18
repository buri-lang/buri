# Reference papers

Every paper consulted while building the Buri toolchain, vendored so the
argument behind a design decision survives a dead link. One PDF per paper,
downloaded from the author's own page, arXiv, or the institutional repository —
nothing here came from behind a paywall.

Cite these from the code and the design docs by relative path
(`reference/<file>.pdf`), the way `cli/src/formatting.rs` and
`design/native/MEMORY.md` already do.

## Vendored papers

| File | Title | Authors | Year | Source | What it informed |
|---|---|---|---|---|---|
| `wadler-prettier-printer.pdf` | *A prettier printer* | Philip Wadler | 1998 | [homepages.inf.ed.ac.uk](https://homepages.inf.ed.ac.uk/wadler/papers/prettier/prettier.pdf) | The `Doc` algebra in `cli/src/formatting.rs` — `Text`/`Concat`/`Line`/`Nest`/`Group` are his combinators and `render`/`fits` are his `best`/`fits`, one pass with one line of lookahead. The formatter was rebuilt on it. |
| `maranget-warnings-for-pattern-matching.pdf` | *Warnings for pattern matching* | Luc Maranget | 2007 | [moscova.inria.fr](http://moscova.inria.fr/~maranget/papers/warn/warn.pdf) | The usefulness algorithm in `cli/src/compiler/semantics/exhaustiveness.rs`, and its Lean re-statement and correctness proof in `formal/Buri/Patterns/Usefulness.lean` and `Exhaustive.lean`. |
| `maranget-compiling-pattern-matching-decision-trees.pdf` | *Compiling Pattern Matching to Good Decision Trees* | Luc Maranget | 2008 | [moscova.inria.fr](http://moscova.inria.fr/~maranget/papers/ml05e-maranget.pdf) | The clause-matrix decision-tree compilation in `cli/src/compiler/middle/decision.rs`, including column selection and the leftmost-refutable-column heuristic that keeps the output deterministic. |
| `jacobs-how-to-compile-pattern-matching.pdf` | *How to compile pattern matching* | Jules Jacobs | 2021 | [julesjacobs.com](https://julesjacobs.com/notes/patternmatching/patternmatching.pdf) | The readable presentation of the above, reached via ReScript's `docs/optimized-pattern-matching.md`; same target, `cli/src/compiler/middle/decision.rs`. |
| `perceus-algorithm.pdf` | *Perceus: Garbage Free Reference Counting with Reuse* | Alex Reinking, Ningning Xie, Leonardo de Moura, Daan Leijen | 2021 | [Microsoft Research](https://www.microsoft.com/en-us/research/publication/perceus-garbage-free-reference-counting-with-reuse/) | The own/borrow elision and the reuse analysis in `cli/src/compiler/middle/rc.rs`; `design/native/MEMORY.md` §5.2–5.3 cites it directly, and the FBIP framing is where §5's growth path comes from. |
| `teeuwissen-reference-counting-reuse-roc.pdf` | *Reference Counting with Reuse in Roc* (MSc thesis) | Jasper Teeuwissen (Utrecht University) | 2023 | [studenttheses.uu.nl](https://studenttheses.uu.nl/handle/20.500.12932/44634) | The same algorithm in a shipped compiler, with the measurements of where reuse pays. Named in `design/native/MEMORY.md` §5.3 as the sanctioned direction (Roc-style Perceus with in-place mutation at refcount 1). |
| `braun-ssa-construction.pdf` | *Simple and Efficient Construction of Static Single Assignment Form* | Matthias Braun, Sebastian Buchwald, Sebastian Hack, Roland Leißa, Christoph Mallon, Andreas Zwinkau | 2013 | [pp.ipd.kit.edu](https://pp.ipd.kit.edu/uploads/publikationen/braun13cc.pdf) | The algorithm `cranelift-frontend`'s `FunctionBuilder` runs, and the reason `design/native/CODEGEN-CRANELIFT.md` §2.1 declines to use it: `middle::ir` is already block-argument SSA, so there is nothing to construct. |
| `leroy-parallel-moves.pdf` | *Tilting at Windmills with Coq: Formal Verification of a Compilation Algorithm for Parallel Moves* | Laurence Rideau, Bernard Serpette, Xavier Leroy | 2008 | [xavierleroy.org](https://xavierleroy.org/publi/parallel-move.pdf) | The theory behind parameter rebinding in `cli/src/compiler/middle/tail_calls.rs`: rewriting a self tail call to a loop *is* a parallel move, and the read-before-overwrite / cycle-breaking rules are this paper's. Shipped in CompCert as `Parmov`. |
| `thivierge-feeley-tail-calls-js.pdf` | *Efficient Compilation of Tail Calls and Continuations to JavaScript* | Eric Thivierge, Marc Feeley | 2012 | [schemeworkshop.org](https://www.schemeworkshop.org/2012/papers/thivierge-feeley-paper-sfp12.pdf) | The measured comparison of trampoline strategies (VM+trampoline vs CPS vs Cheney-on-the-MTA) that argued for the merged dispatch loop in `cli/src/compiler/middle/tail_calls.rs` over a thunk-allocating trampoline, and for a counter-limited bounce as the escape hatch. |
| `vouillon-balat-js-of-ocaml.pdf` | *From bytecode to JavaScript: the Js_of_ocaml compiler* | Jérôme Vouillon, Vincent Balat | 2014 | [irif.fr, via the Internet Archive](https://web.archive.org/web/2id_/https://www.irif.fr/~vouillon/publi/js_of_ocaml.pdf) | The third of the three tail-call designs Buri was choosing between — scratch temporaries only on a proven move cycle, plus the `tc_depth` counter trampoline for mutual recursion. Compared side by side against ReScript's and Elm's in the tail-call research. |
| `ryu-float-to-string.pdf` | *Ryū: Fast Float-to-String Conversion* | Ulf Adams | 2018 | [ACM PLDI'18 open access, via the Internet Archive](https://dl.acm.org/doi/10.1145/3192366.3192369) | The shortest-round-trip digit generation `design/native/VALUE-MODEL.md` §11.4/§12 row 8 requires of the native runtime. `cli/runtime/fmt.rs` documents why v1 leans on `core::fmt` (Grisu3 with a Dragon4 fallback) instead of hand-rolling this, and where a Ryū fast path would drop in. |
| `steele-vigna-spectral-multipliers.pdf` | *Computationally Easy, Spectrally Good Multipliers for Congruential Pseudorandom Number Generators* | Guy L. Steele Jr., Sebastiano Vigna | 2020 | [arXiv:2001.05304](https://arxiv.org/abs/2001.05304) | The 64-bit multiplier `K` in `cli/src/hash.rs`, taken with rustc's `FxHasher` and cited there by name. |
| `xu-kjolstad-copy-and-patch.pdf` | *Copy-and-Patch Compilation* | Haoran Xu, Fredrik Kjolstad | 2021 | [arXiv:2011.13127](https://arxiv.org/abs/2011.13127) | The source of the "Cranelift is slow" numbers that circulate; reading it is what established they were measured against Wasmtime 0.26 and do not describe the pinned version. Backs the debug/release split in `design/native/ARCHITECTURE.md` §4. |
| `tpde-fast-compiler-backend.pdf` | *TPDE: A Fast Adaptable Compiler Back-End Framework* | Tobias Schwarz, Tobias Kamm, Alexis Engelke | 2025 | [arXiv:2505.22610](https://arxiv.org/abs/2505.22610) | The current third-party frame of reference for Cranelift's compile-time/code-quality position against LLVM `-O0`; the other half of the §4 backend-selection argument. |
| `bour-clement-scherer-tail-modulo-cons.pdf` | *Tail Modulo Cons* | Frédéric Bour, Basile Clément, Gabriel Scherer | 2021 | [arXiv:2102.09823](https://arxiv.org/abs/2102.09823) | Research input, not implemented. The design of an *explicit* TRMC annotation (OCaml's `[@tail_mod_cons]`) as the answer to the stack-safety-by-build-configuration trap, recorded as a considered extension to `cli/src/compiler/middle/tail_calls.rs`. |
| `leijen-lorenzen-tail-recursion-modulo-context.pdf` | *Tail Recursion Modulo Context — An Equational Approach* | Daan Leijen, Anton Lorenzen | 2022 | [Microsoft Research (MSR-TR-2022-27)](https://www.microsoft.com/en-us/research/publication/tail-recursion-modulo-context-an-equational-approach/) | Research input, not implemented. The generalisation of the above as implemented in Koka, and the reason the destination-passing shape it needs is compatible with the Perceus reuse already in `middle/rc.rs`. |

## Other sources consulted — not papers, not vendored

Specifications, documentation, source code and blog posts that carried real
weight in a decision. Links only.

**Specifications**

- ECMA-262, *Number::toString* (§6.1.6.1.20) — the float presentation rule
  `cli/runtime/fmt.rs` hand-writes: <https://tc39.es/ecma262/#sec-numeric-types-number-tostring>
- Unicode 6.0.0 ch. 3, conformance — scalar counting for `str.len`:
  <http://www.unicode.org/versions/Unicode6.0.0/ch03.pdf>
- Protocol Buffers language guide and conformance suite — the proto3/editions
  presence rules `cli/src/docs/build/proto.md` covers:
  <https://protobuf.dev/programming-guides/proto3/>
- LLVM Language Reference (`musttail`, calling conventions, `nonnull`,
  attributes): <https://llvm.org/docs/LangRef.html>

**Compiler documentation and source**

- Cranelift: `cranelift/docs/ir.md`, `cranelift/frontend/src/ssa.rs`,
  `cranelift/docs/compare-llvm.md` — <https://github.com/bytecodealliance/wasmtime>
- Chris Fallin, *ægraphs: acyclic e-graphs* (blog post and EGRAPHS 2023 slides) —
  what `opt_level = "none"` skips: <https://cfallin.org/blog/2026/04/09/aegraph/>,
  <https://cfallin.org/pubs/egraphs2023_aegraphs_slides.pdf>
- rustc dev guide (MIR, monomorphization, incremental compilation, interning):
  <https://rustc-dev-guide.rust-lang.org/>
- `rustc-hash` — the `FxHasher` this repo's `cli/src/hash.rs` follows:
  <https://github.com/rust-lang/rustc-hash>
- ReScript compiler `compiler/core/` and `docs/optimized-pattern-matching.md` —
  the optimization catalogue and the pattern-match lineage:
  <https://github.com/rescript-lang/rescript>
- js_of_ocaml manual, *Tail calls* — the `tc_depth = 50` counter trampoline:
  <https://ocsigen.org/js_of_ocaml/latest/manual/tailcall>
- Elm compiler `Generate/JavaScript/`, and elm-optimize-level-2's TCO notes:
  <https://github.com/elm/compiler>,
  <https://github.com/mdgriffith/elm-optimize-level-2>
- PureScript `CodeGen/JS/Optimizer/TCO.hs` and purescript-backend-optimizer:
  <https://github.com/purescript/purescript>,
  <https://github.com/aristanetworks/purescript-backend-optimizer>
- Roslyn red-green trees, and Zig's `InternPool`/`Zir`/`Air` — the two IR shapes
  the front end was weighed against:
  <https://github.com/dotnet/roslyn/blob/main/docs/compilers/Design/Red-Green%20Trees.md>,
  <https://github.com/ziglang/zig>
- Sorbet's `GlobalState`/`NameRef` interning, and Nelson Elhage on why Sorbet is
  fast: <https://blog.nelhage.com/post/why-sorbet-is-fast/>

**Engine behaviour and measurement**

- V8 blog: elements kinds, fast properties, mutable heap numbers, BigInt, the
  React cliff — the inline-cache and representation facts behind the JavaScript
  backend's value model: <https://v8.dev/blog/>
- Nicholas Nethercote, *How to speed up the Rust compiler* series, and the Rust
  Performance Book: <https://nnethercote.github.io/perf-book/>
- Elm bug elm/compiler#2268 — the loop-closure capture bug
  `snapshot_captures` exists to avoid:
  <https://github.com/elm/compiler/issues/2268>

**Named but not consulted as papers**

These are cited by algorithm name in the code, at second hand through the
implementation that already does them correctly, so no PDF is vendored:

- Loitsch, *Printing Floating-Point Numbers Quickly and Accurately with
  Integers* (Grisu3), and Steele & White, *How to Print Floating-Point Numbers
  Accurately* (Dragon4) — what Rust's `core::fmt` runs underneath
  `cli/runtime/fmt.rs`. Steele & White has no author-hosted open copy.
- Baker, *Cheney on the M.T.A.* — named in the tail-call survey as the CPS +
  stack-as-nursery approach that was rejected.
- Ullrich & de Moura, *Counting Immutable Beans* — Lean 4's reference counting;
  reached through the Perceus paper and the Roc thesis, both of which restate it.
