# Contributing

Most of what you need is already written down somewhere in this repository.
This page is the table of contents: the handful of rules that a first change is
likely to trip over, each with a link to the place that explains it in full.

## The environment

`nix develop` is the supported one. The flake's devShell pins LLVM 21, Bun,
`elan`, `lld`, and `mold` on Linux, and sets `LLVM_SYS_211_PREFIX` for you;
[`design/native/BUILD-AND-WATCH.md` §3.1](./design/native/BUILD-AND-WATCH.md)
says why each entry is there.

Without nix, the *default* build needs nothing new: Cranelift is pure Rust, so
`cargo build -p buri` and `cargo test -p buri` work on any machine with a Rust
toolchain and a C compiler. Only two things need packages —
the LLVM backend and a faster linker — and
[§3.4](./design/native/BUILD-AND-WATCH.md) has the per-platform table for both.

## The dependency bar

The admitted set is closed — Cranelift, inkwell, and `target-lexicon` — and
every entry is behind a cargo feature the default build turns off. Adding to it
is a design decision rather than a manifest edit. The bar is stated in full at
the top of the root [`Cargo.toml`](./Cargo.toml):

> A dependency is admissible only if it is a code generator or a platform
> interface that this repository could not reasonably write, it is behind a
> cargo feature the default build can turn off, and its absence degrades the
> toolchain rather than breaking it.

`dependencies_stay_behind_the_bar`, in `cli/tests/language/corpus.rs`, enforces
both halves a file can enforce: the admitted set is closed, and every entry is
`optional = true`. A pull request that adds a crate fails there first, and the
place to argue for it is the pull request, not the test.

## The lints a first change trips

The lint set in [`Cargo.toml`](./Cargo.toml) pins one promise — **no input may
panic the compiler** — and every lint in it is `deny` rather than `warn`,
because a warning is a thing that accumulates. In practice that means no
`unwrap`, no `expect`, no `panic!`, no `v[i]`, no `&s[a..b]`, no unchecked
arithmetic, and no `println!` outside the files whose printing *is* the
command's output.

Two escapes exist and both are deliberate. Where an invariant genuinely cannot
be broken by input, write `#[expect(..., reason = "...")]` naming what upholds
it, or call `diagnostics::ice`, which prints "internal compiler error", says
where to report it, and exits 70 rather than unwinding through a backtrace
nobody can act on. Test code is exempt already: `clippy.toml` turns the lints
off inside `#[test]`, and each file under `cli/tests/` carries the same
exemption at the top for the helpers around them.

## Documentation is not edited where you read it

`README.md` and `cli/src/docs/SPEC.md` are **generated**. They are assembled
from the topic files under [`cli/src/docs/`](./cli/src/docs/), which is the one
place a sentence of user documentation exists. Edit the topic, then

```
cargo run -q -p buri -- docs assemble
```

and commit both. `buri docs assemble --check` fails when the checked-in file
has drifted, so a topic edited without the regeneration is caught in CI.

Every fenced Buri block in every page is compiled by the test suite against the
real standard library — a `run` block is compiled, linked, executed, and its
stdout compared. `buri docs cli docs` explains the fence attributes (`wrap=`,
`sig`, `use=`, `name=`, `// ERROR:`) and what each one is for.

`design/` is the exception: it is working notes for contributors, nothing there
is compiled or served, and [`design/README.md`](./design/README.md) states the
one rule that keeps it from becoming a second copy of the reference.

## The grammar, and the editor grammars

[`cli/src/docs/grammar.ebnf`](./cli/src/docs/grammar.ebnf) is the normative
grammar and the only place Buri's syntax is written down.
`editors/tree-sitter-buri/grammar.js` is generated from it and must not be
edited by hand; regenerate with

```
BURI_BLESS=1 cargo test -p buri --test language corpus::the_tree_sitter_grammar
```

A cargo test regenerates and compares byte for byte, so the two cannot drift.
The other half — that the EBNF is what the compiler actually does — needs the
tree-sitter CLI and so is a script rather than a test:
[`editors/tree-sitter-buri/README.md`](./editors/tree-sitter-buri/README.md)
has `check.sh`, what it proves in both directions, and the five files where the
grammar and the compiler are *meant* to disagree.

## The verification bar

Run this before opening a pull request:

```
cargo test -p buri --no-fail-fast
cargo clippy -p buri --all-targets
cargo bench -p buri --bench compiler --profile validate -- --validate
```

With the LLVM backend in hand, add the three `--features backend-llvm` legs and
the second clippy that
[`cli/tests/README.md`](./cli/tests/README.md)'s *What "the bar" is* section
lists — those four selections are the delta the plain run does not cover, and
running them instead of a second full suite is dedup rather than less coverage.

**The whole bar runs in under five minutes, and that is a policy.** A change
that pushes it over owes either an optimization that brings it back or a
justification written beside the change. Two rules keep it honest: coverage
never pays for it, and the number is measured rather than asserted. The
*five-minute budget* section of `cli/tests/README.md` has the measured table and
the ledger of levers already priced and rejected.

CI runs more than this — everything under both feature sets, on macOS and on
Linux arm64 and x86_64. The sequence above is the local loop.
