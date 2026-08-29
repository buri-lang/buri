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
//! | [`conformance`] | `backend-stencil` | The `conformance/` corpus again, compiled natively rather than to JavaScript — the other half of `language::conformance`. |
//! | [`llvm`] | `backend-llvm` | The same for the LLVM backend, plus the attribute discipline read off the optimized IR. |
//! | [`agreement`] | either native backend | VALUE-MODEL.md §12's fourteen rows, run under each native backend and compared against JavaScript. |
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

// What more than one backend suite needs: the allocation probe, the shape a
// run produces, and the conformance corpus as a repository. One copy, because
// what those suites assert is that the backends agree.
#[cfg(any(feature = "backend-llvm", feature = "backend-stencil"))]
mod shared;

#[cfg(any(feature = "backend-llvm", feature = "backend-stencil"))]
mod agreement;
#[cfg(feature = "backend-stencil")]
mod conformance;
mod float_parity;
mod link;
#[cfg(feature = "backend-llvm")]
mod llvm;
mod runtime;
#[cfg(feature = "backend-stencil")]
mod stencil;
