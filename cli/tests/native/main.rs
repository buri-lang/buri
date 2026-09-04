//! **The native backends**, and the runtime they link against.
//!
//! One binary for the whole domain, with the feature gates at the module
//! declarations below rather than inside the files. A toolchain built without
//! a backend still compiles and runs this suite; the modules that need one
//! are simply not there, which is what "degrades rather than breaks" means for
//! a test.
//!
//! | Module | Gate | Question |
//! |---|---|---|
//! | [`link`] | none | Bytes in, an executable out: object caching, the link key, reproducibility. Objects come from `cc`, because the layer neither knows nor can know which backend made them. |
//! | [`runtime`] | none | The `buri_rt_*` C ABI, driven from C (`driver.c`, beside this file): the archive links, the reference-count header and drop glue behave and leak nothing, every abort message is byte-identical to the JavaScript backend's, and each host capability answers what `core/effect` declares. |
//! | [`float_parity`] | none | Does a native `show` of a `Float` print what a JavaScript one prints, over 3 807 072 doubles? Needs a JavaScript engine and takes seconds; `--skip float_parity` leaves the rest fast. |
//! | [`stencil`] | `backend-stencil` | The same for the copy-and-patch backend: the frame-threaded convention, the hand-written `main`, the constant pool as its own section, and that a refusal is a diagnostic. |
//! | [`conformance`] | `backend-stencil` | The `conformance/` corpus again, compiled natively rather than to JavaScript — the other half of `language::conformance`. Every program in it runs under the runtime's heap check, so "no block was leaked" is an invariant of the corpus rather than of a handful of leak tests. |
//! | [`differential`] | `backend-stencil` | The same corpus through *both* pipelines, compared **per `test` block**: the same verdict on JavaScript and natively, for every block in every package the two can run. |
//! | [`llvm`] | `backend-llvm` | The same for the LLVM backend, plus the attribute discipline read off the optimized IR. |
//! | [`agreement`] | either native backend | VALUE-MODEL.md §12's rows, run under each native backend and compared against JavaScript. |
//!
//! # The net around `middle::rc`, and the bugs it was shown to catch
//!
//! `middle::rc` is a whole-program analysis whose failures are **silent**: a
//! missing decrement leaks, an extra one releases a block something is still
//! reading, and neither shows up in what a program printed until the recycled
//! memory happens to hold something that changes an answer. Five such defects
//! have shipped. Every one was found by a person running a program and
//! noticing that the answer was wrong.
//!
//! So this domain carries four layers, and each one turns a class of that
//! silence into a red test. None of them reads a pass, an IR, a plan or a
//! symbol: what every layer observes is what a compiled program *did* — the
//! bytes it wrote, the status it exited with, and what its allocator was
//! holding when it stopped. That is deliberate, and it is the property that
//! makes the net worth having while the pass under it is rewritten.
//!
//! | | Layer | Where | What it sees |
//! |---|---|---|---|
//! | 1 | **The exit audit** | `cli/runtime/memory.rs`, on for every program this domain runs (`shared::ran`) | a block allocated and never freed — an *under*-decrement |
//! | 2 | **The quarantine** | the same section: freed blocks are poisoned and held, not recycled | a reference operation or a write that reached a freed block — an *over*-decrement |
//! | 3 | **The corpus differential** | [`differential`] | a `test` block whose verdict differs between the reference backend and a native one |
//! | 4 | **The ownership generator** | `cli/tests/fuzz.rs` | all of the above, over programs nobody wrote, in the shapes the five reports name |
//!
//! ## The kill matrix
//!
//! Each row is an experiment: the fix was reverted in a worktree, the net was
//! run, and the tree was restored. **A layer that catches nothing it was built
//! for is a layer that gets fixed or gets written down**, and two rows below
//! are written down.
//!
//! | Defect, re-introduced | 1 audit | 2 quarantine | 3 differential | 4 generator | First red, and how long |
//! |---|---|---|---|---|---|
//! | **#33** `rc.rs`: a projection borrows a tail-shaped base | **yes** | no | no | no | 93 s — 2 agreement rows, on the *exact* leak count |
//! | **#39** `rc.rs`: a match does not keep its scrutinee's root | no | **yes** | no | **yes** | 47 s — `a_match_arms_bindings_survive_a_sibling_field_read` exits `-1` |
//! | **#29a** `tail_calls.rs`: a merged group's slots typed by position | no | no | no | **yes** | 0.1 s — `every_ownership_program_compiles`; 88 s through the agreement row |
//! | **#29b** `rc.rs`: a `Loop`'s entries balanced against each other | no | no | no | no | **not caught** — see below |
//! | **#41** `stencil/lists.rs`: `LOOP_SCRATCH` inside the reserved run | **yes** | no | no | **yes** | 74 s — the `sortBy` row leaks 16 blocks |
//! | synthetic **over**-decrement (a projection stops retaining) | yes | **yes** | **yes** | **yes** | 78 s — 30 native tests, 112 verdict disagreements, 17 use-after-free reports |
//! | synthetic **under**-decrement (a projection retains twice) | **yes** | no | no | **yes** | 47 s — 25 native tests |
//!
//! ### The two rows that are written down rather than fixed
//!
//! **#29b is not caught, and the reason is that the same commit fixed it
//! twice.** `rc.rs` stopped balancing a `Loop`'s entries against each other,
//! *and* `tail_calls` began binding, in the entry that owns it, every slot
//! that entry never reads. With the second half in place the first is
//! redundant: there is no unbound slot left for a balance to release, so
//! putting the balance back changes nothing a program can observe. The
//! experiment reverts one half and the other holds. That is a fact about the
//! fix rather than a hole in the net — but it is also the honest answer to
//! "would this have caught #29b", and the answer is no.
//!
//! **#33 is caught by the audit and not by the quarantine**, which is the
//! opposite of what the report describes. The reason is that the defect's
//! *corruption* half no longer reproduces from that revert alone — the
//! program in the row answers correctly today with the fix taken out — while
//! its counting half does: the row leaks exactly one block with the fix in
//! and none with it out, and `agree_leaking` pins the number in both
//! directions. A change to `rc`'s counting on that shape is red whichever way
//! it moves the count, which is the property a ratchet has and a threshold
//! does not.
//!
//! ### What the matrix does not say
//!
//! Layer 3 fired on one row of seven. That is not a surprise and not a
//! failure: a verdict differs only when a defect changes an *answer*, and four
//! of these seven change only a count. It is in the net for the defect that
//! does change an answer, which is the one a leak check cannot see — and the
//! synthetic over-decrement row is what shows it can.
//!
//! ```text
//! cargo test -p buri --test native                                  # default features
//! cargo test -p buri --test native --features backend-llvm          # and the LLVM half
//! cargo test -p buri --test native -- --skip float_parity           # the fast ones
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "test code. The lint set in `Cargo.toml` pins a promise about the \
              toolchain — that no input panics it — and a harness that drives \
              the toolchain is not the toolchain. A test that unwraps fails on \
              the line that broke, which is what a test is for, and threading \
              `?` through an assertion buys nothing. `clippy.toml` exempts \
              `#[test]` functions already; this covers the helpers around them."
)]

// Not the whole harness: this domain drives the backends in process rather than
// the CLI, so the only thing it wants from `cli/tests/harness/` is the sweep of
// the scratch root — which it wants more than any other binary does, because its
// per-process trees are the ones nothing deletes.
#[path = "../harness/sweep.rs"]
mod sweep;

// The other thing every suite in here wants, and for the same reason the sweep
// is here: `ci::skipped` is what turns this domain's `if !supported()` guards
// from "a quiet pass on a host without a backend" into "a red X on a runner
// that was supposed to have one". Its own doc comment is the argument.
#[path = "../harness/ci.rs"]
mod ci;

// What more than one backend suite needs: the allocation probe, the shape a
// run produces, the conformance corpus as a repository, and the product's own
// link line. One copy, because what those suites assert is that the backends
// agree.
//
// **Not gated on a backend**, though most of it is only read when one is
// built. `runtime` and `float_parity` link a C driver against the runtime
// archive on every host, so they need `product_cc` and `product_link_args`
// too — and a second copy of the link line for the ungated half is exactly the
// duplication that let the Linux flags rot. `#![allow(dead_code)]` at the top
// of the file is what makes the unread rest of it free.
mod shared;

#[cfg(any(feature = "backend-llvm", feature = "backend-stencil"))]
mod agreement;
// The end-to-end tier: whole programs, a real process, a real socket. Gated
// like `agreement` — it needs *a* native backend and does not care which — and
// its module doc is `cli/tests/README.md`'s "The trust ordering" applied.
#[cfg(any(feature = "backend-llvm", feature = "backend-stencil"))]
mod e2e;
#[cfg(feature = "backend-stencil")]
mod conformance;
// The whole corpus through both pipelines, verdict by verdict. Beside
// `conformance` and gated with it, because the native half of the comparison
// is that module's memoized build.
#[cfg(feature = "backend-stencil")]
mod differential;
mod float_parity;
mod link;
#[cfg(feature = "backend-llvm")]
mod llvm;
mod runtime;
#[cfg(feature = "backend-stencil")]
mod stencil;
