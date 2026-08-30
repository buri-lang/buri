# G2: the `lower+macos-arm64-release` budget, amended

**2026-08-30.** The +21.3% on `lower+macos-arm64-release` (LLVM) — measured in
`design/PERFORMANCE.md` §6.6 against the shared-RC dark branch — is **accepted
by Nick**, and that row's budget is amended to the measured range
**+16.6% … +26.3%**. Every other row of `--set=native` is still held to 3%.

## Where the rule lived, and what moved

The 3%-per-native-row rule is **prose in `design/PERFORMANCE.md` §6.6** and
nowhere else: there is no assertion in `cli/benches/compiler.rs`, no threshold
in `cli/benches/pinned/*.txt`, and no per-row budget in any manifest. So the
amendment is three edits to two documents:

- `design/PERFORMANCE.md` §6.6, the budget sentence — the rule now reads "3% on
  every row of `--set=native`, and it is 3% on four of them", with the fifth
  carrying a budget of its own, **amended 2026-08-30**, and a forward pointer to
  the argument.
- `design/PERFORMANCE.md` §6.6, the five-row table — the release row's verdict
  goes from **missed** to **met, against an amended budget**. The other four
  rows are untouched.
- `design/PERFORMANCE.md` **§6.6.1**, a new subsection carrying the argument, in
  the shape MEMORY.md §7.3.1 uses for an amendment: what was accepted, on what
  evidence, and what the amendment is *not*.
- `design/native/VALUE-MODEL.md` §2.1, which claimed the fork cost "under 3 % of
  native release lowering" — flatly contradicted by §6.6's own table. It now
  states +21% and names it an amended budget rather than a met one.

## The three things it was accepted on

1. **Compile time, one backend.** The shared-RC branch adds **two basic blocks
   per reference operation** to the IR handed to `opt`, roughly doubling the
   most common operation the emitter produces, against a `default<O2>` pipeline
   superlinear in block count. The stencil backend's own work is unchanged and
   its row says so: −0.4%.
2. **Two instructions, unshared path.** A load of the word beside the count and
   a bit test, on both instruction sets and both backends, read off the objects
   — and the only path any program compiled today takes.
3. **A profit at run time.** Beside MEMORY.md §5.4's per-thread caches, an
   allocation-heavy program's run time falls **39.8%** (dev) and **64.6%**
   (release); the allocation-free control does not move.

The narrower alternative — hold 3% and leave the row permanently red — was
refused: a budget nothing can meet is a budget nobody reads. The amendment is
scoped to the one row, so the next regression there is measured against this
range rather than against 3%, and a second change of this size is a second
decision.

## Gate

- `cargo test -p buri --no-fail-fast`: **all green**, 0 failed across every
  target (688 lib, 110 native/conformance, and the rest).
- `cargo clippy -p buri --all-targets`: 3 warnings, all pre-existing in Rust
  sources this change does not touch (lib ×2, bench `compiler` ×1). **No new
  warnings** — the change is documentation only.
- `origin/main` merged in; the merge was trivial, no conflicts.
