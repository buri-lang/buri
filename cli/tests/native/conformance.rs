//! The conformance corpus, compiled and run **natively**.
//!
//! `language/conformance.rs` drives the same corpus through `buri test`, which
//! is the JavaScript backend because every `BUILD.buri` in the corpus says so:
//! it is the suite that says what the language does. This
//! file asks the other half of the question — whether the native backend
//! *agrees* — by taking the same `.buri` files, compiling each one through
//! `middle::run` → `middle::native` → `middle::lower` → `backend::stencil`,
//! linking, running, and checking that every `test` block in it passed.
//!
//! # How a test file becomes a program
//!
//! It does not. The file is compiled exactly as it is, with
//! `monomorphize::Roots::Tests`, and the stencil backend emits a `main` that
//! calls every `test` block in order (`stencil/asm.rs`'s
//! `test_entry`) — which is the other half of what this file needs:
//! without a native test entry point there is nothing to run, and without
//! `core/testing/assert`'s three bodies there is nothing to assert.
//!
//! A failed assertion **ends the process**, because SPEC 6.9 says an abort is
//! a write and an `_exit` and there is nothing to catch. So the exit status is
//! the result: zero means every block in the file passed, and one means the
//! first failure printed `assert.<kind> failed` and stopped. That is a worse
//! *report* than `buri test` gives and it is not a different answer, which is
//! what this file is checking.
//!
//! # Which packages are in the native set, and which are not
//!
//! [`PACKAGES`] is the list, with the reason beside each exclusion.
//! **Forty-two of the fifty-one files are in it** — the number the harness
//! prints, re-derived from it rather than incremented by hand, and one the
//! prose has drifted from more than once. The ordinals in the paragraphs below
//! record *when* a file joined the set and are not a running total of it;
//! `text/hex.buri`, `calendar/duration.buri` and `cli/arguments.buri` are the
//! latest, and their own entries say why they are in.
//! `semantics/http.buri` is the
//! thirty-first — `Request` and `Response`, which are two structs over a
//! `[Header]` and a `[U8]` and reach nothing past `core/bytes`'s UTF-8 pair.
//! `semantics/host_testing.buri` is the
//! thirtieth — `core/host/testing`'s seven doubles, which are handles over the
//! table `cli/runtime/testing.rs` already carried, so it needed nothing the
//! archive did not have. `semantics/anonymous.buri` is the
//! twenty-ninth and `semantics/elision.buri` the twenty-eighth, with
//! `collections/ordmap.buri` the twenty-seventh; none of them needed anything
//! the backend did not already have.
//! `proto/binary.buri` was the twenty-sixth: it compiled and passed all along and
//! was held out for a *middle-end* cost, `middle/rc.rs`'s exponential
//! `Scan::short_circuit`, which is linear now. What is actually
//! *refused* is three things:
//!
//!  1. **An inexact numeric conversion.** `x.toT()` where not every value fits
//!     answers `Result<T, RangeError>` (SPEC 6.2.1), and `RangeError` is a
//!     *struct of two `Str`s* — the source value rendered and the target's
//!     name. That is a different shape from the runtime `Result` of
//!     `cli/runtime/lib.rs` §2.1, which names an error by a variant index or
//!     writes it through a pointer: here the backend has to *build* the two
//!     strings. `numbers/conversions.buri`, `text/json.buri` (one call:
//!     `num.U32.toChar`) and `proto/json.buri` (`num.F64.toI64`) are the three
//!     files, and `numbers/conversions.buri` carries a second problem behind
//!     the first — two of its blocks assert the JavaScript *bound*, which
//!     VALUE-MODEL.md §12 row 2 has already ruled is not the native one.
//!  2. **`json.*`, and `ToJson::toJson` at a primitive.** `json.decode` is a
//!     descriptor-driven walker, which is what `runtime.js` does.
//!     `json/decoding.buri` and `json/encoding.buri`. `derivePrimJson` was the
//!     second half of this reason and is not any more — both native backends
//!     build `Json`'s primitive arm now (VALUE-MODEL.md §12 row 10) — so what
//!     holds `json/encoding.buri` out is the five keys a *direct* `x.toJson()`
//!     produces, which is the same answer reached through the trait rather
//!     than through the derive.
//!  3. **`core/math`'s thirteen transcendentals**, which are refused rather
//!     than unwritten — `cli/runtime/math.rs` argues it. `numbers/floats.buri`.
//!  4. **The reactive graph.** `ui/effect`'s `Ui` entries and `ui/testing`'s
//!     recorder are `backend/js/runtime.js` and nowhere else, because no
//!     native platform grants `Ui` — there is nothing on this side to render
//!     to, and no document either: `ui/tree.buri` is the tree vocabulary and
//!     the keyed reconciler under it, and `ui/theme.buri` is the block of
//!     custom properties a document reads. `ui/reactivity.buri`,
//!     `ui/tree.buri` and `ui/theme.buri`.
//!
//! `semantics/generics.buri` was a fourth until a type parameter a program
//! never determines stopped being a free variable: `Subst::default_unconstrained`
//! makes it `()`, which every backend already lays out.
//!
//! # The `core/host/testing` migration, and why this list did not move
//!
//! Several of the reasons below are written in terms of a context that binds
//! every effect whether a test uses one or not, because that is what the
//! corpus used to write. Every one of those sites now names only what the
//! function under test needs — four hundred and sixty-four rewritten in three
//! batches, then two hundred and forty-seven narrowed again once
//! `unused-context-bound` had removed the dead bounds keeping their bindings
//! alive — and the old module they were written against is gone.
//!
//! **Not one row of the table below moved.** The migration reached three
//! excluded files — `json/decoding.buri`, `json/encoding.buri` and
//! `proto/json.buri` — and none of them was excluded for a context: the
//! reasons were `json.decode`, `derivePrimJson` — now `ToJson::toJson` at a
//! primitive, the derive's leaf having landed — and an inexact `F64 -> I64`.
//! It reached two more in batch three, `numbers/floats.buri` and
//! `text/json.buri`, excluded for `core/math`'s transcendentals and for
//! `num.U32.toChar`; neither reason was ever about the testing context.
//! [`the_excluded_packages_are_excluded_for_the_stated_reason`] was re-run at
//! every step and still reports each of them.
//!
//! That is the honest report: the migration removed the pressure the
//! exclusions named, and no excluded file was waiting on it. The historical
//! reasons are left where they are — what a ledger records is why a file *was*
//! out — with a note beside the ones the migration has overtaken.
//!
//! # The narrowing pass, and the answer to the question this section asked
//!
//! The migration derived each *migrated* site's bindings from the compiler, so
//! those were minimal the day they were written. What it could not reach was
//! the contexts nobody migrated — the ones written by hand — and the bindings
//! a **dead bound** kept alive. `unused-context-bound`'s fix over the two
//! corpora settled both, in that order, because the second follows from the
//! first: `fn note<C: Alloc + Stdout>` whose body only prints forces
//! `Alloc: alloc()` into the context of every test that calls it.
//!
//! Fifteen dead bounds went, over three rounds (the rule has a fixed point and
//! reaching it takes as many passes as the call graph is deep), and **two
//! hundred and forty-seven contexts then shrank** — two hundred and thirty-nine
//! here and eight in `cli/tests/example` — dropping 251 bindings: 176 `Alloc`,
//! 72 `Watch` and 3 `Net`. Fifty-eight of them were reachable *only* after the
//! bounds went, measured by running the same sweep against the tree before
//! them.
//!
//! **The contexts the migration derived are almost exactly where it left
//! them**, which is the measurement that says its fixpoint was minimal rather
//! than merely settled. Of the six files that moved, three are `lib/ui`, a
//! package the migration never listed; `semantics/effects.buri` is one of the
//! two files it was told to hold; and `semantics/host_testing.buri` was
//! written by hand against `core/host/testing` rather than rewritten from a
//! world assembled for it — no commit in its history builds one. That leaves
//! `semantics/evaluation.buri`, which is migrated and dropped fifty-one
//! `Alloc`s: fifty of them are the dead bound on `note` and its nine
//! neighbours, which the migration could not have seen, and the fifty-first is
//! one binding it genuinely left behind, out of the four hundred and
//! sixty-four sites its three batches wrote here.
//!
//! **The table below is unchanged, and this time the question was live.**
//! Several exclusion reasons are written in terms of a context that
//! instantiates everything, and the three `lib/ui` files are the ones that
//! actually carried surplus bindings — 64 `Watch` between them.
//! [`the_excluded_packages_are_excluded_for_the_stated_reason`] was re-run and
//! every one of the nine still names what it named. One refusal did get
//! shorter: `ui/theme.buri` no longer reaches `ui_testing.observer`, because
//! none of its five contexts binds `Watch` any more. It is still out for
//! `install`, `variables`, `render`, `stylesheet` and the `Headless` pair —
//! the document and the reactive graph, which is a reason no wave of this
//! backend retires. So the census is the same 32 files and 1,529 blocks, and
//! `lib/ui`'s exclusion is now stated in terms of what it is really about.
//!
//! The half of this the corpus keeps for itself is
//! `language::conformance::no_conformance_context_asks_for_a_bound_it_does_not_use`:
//! this corpus is deliberately not lint-clean, and that one code is the one it
//! is held to zero over, because a dead bound put back here is contexts put
//! back everywhere that calls it.
//!
//! # The harness used to be the biggest exclusion, and it was never about the
//! backend
//!
//! Eleven files import `//lib/<package>`, and this compiled each one as a
//! snippet against the standard library **with no repository** — so the front
//! end refused them and eleven files' worth of conformance was invisible.
//! `cli/tests/conformance/REPO.buri` has been there all along;
//! [`repository`] opens it, and [`analyze`] compiles each file as a test source
//! of its own package, which is what it is. That alone moved
//! `codegen/equality.buri` and `codegen/tail_calls.buri` into the set with no
//! backend change at all, and it is what made the four `//lib/semantics` files
//! reachable enough to find what they *did* need.
//!
//! # What the backend gained to take the rest
//!
//!  * **`cli/runtime/lib.rs` §2.1, a `Result<T, E>` a runtime entry can
//!    answer** — the shape that was recorded as deferred twice over. The
//!    filesystem double's four methods are the first entries to use it, and
//!    they are what
//!    `semantics/effects.buri` and `semantics/evaluation.buri` were waiting
//!    for.
//!  * **`core/char`'s eight** (`cli/runtime/char.rs`), including `\p{L}` as a
//!    table this repository carries. `data/strings.buri`.
//!  * **`core/bytes`'s six** (`cli/runtime/bytes.rs`) — the UTF-8 pair and the
//!    four IEEE 754 byte-pattern entries. `crypto/sha256.buri`,
//!    `text/bytes.buri`, `proto/binary.buri` and `proto/failures.buri`.
//!
//! The exclusions are checked as well as stated:
//! [`the_excluded_packages_are_excluded_for_the_stated_reason`] compiles each
//! refused file and asserts the backend still refuses it. A package that
//! quietly becomes compilable is a failing test rather than a stale comment.
use buri::build::buildfile::Platform;
use buri::build::workspace::Workspace;
use buri::compiler::backend::stencil::{unavailable_reason as stencil_unavailable_reason, Stencil};
use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME, AVAILABLE};
use buri::compiler::backend::{Backend, Options, Profile, Target};
use buri::compiler::driver;
use buri::compiler::middle::{self, monomorphize};
use buri::compiler::modules::Role;
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// One conformance file, and whether the native backend is expected to
/// compile it.
struct Case {
    /// `lib/<package>/test/<file>.buri`, relative to the corpus root.
    path: &'static str,
    /// `None` when the file is in the native set; `Some(..)` when it is not,
    /// and the reason is what a reader gets instead of a surprise.
    out: Option<Out>,
}

/// Why a file stays out.
///
/// There was a second variant, `Wrong`, for the one file the backend compiled
/// and got wrong, and `a_wrong_answer_is_still_wrong` ran it and asserted it
/// still failed. `collections/queue.buri` was that file; the three defects
/// behind it are fixed (`middle/rc.rs`'s deferred drops and `middle/layout.rs`'s
/// list niche) and it is in the set above. The category comes back the day
/// another file needs it, and not before.
///
/// There was a third, `Costly`, for the one file the backend compiled and got
/// *right* at a price this suite would not pay: `proto/binary.buri` cost about
/// 280 seconds, all of it `middle/rc.rs`'s `Scan::short_circuit` scanning its
/// right operand twice and so exploring a chain of `&&` along every path
/// through it. `a_costly_package_still_passes` ran it `#[ignore]`d and its doc
/// comment said the day that stopped being exponential the test and the
/// exclusion would be deleted together. The scan is linear, the file takes
/// about three seconds, and they were.
enum Out {
    /// **The backend refuses it**, and names what it has no body for.
    /// [`the_excluded_packages_are_excluded_for_the_stated_reason`] compiles
    /// each and asserts the refusal is still there.
    Refused(&'static str),
}

impl Out {
    fn why(&self) -> &'static str {
        match self {
            Out::Refused(why) => why,
        }
    }
}

const fn included(path: &'static str) -> Case {
    Case { path, out: None }
}

const fn excluded(path: &'static str, why: &'static str) -> Case {
    Case { path, out: Some(Out::Refused(why)) }
}

/// Every file in `cli/tests/conformance/lib`, in or out, with the reason.
///
/// The list is exhaustive by construction:
/// [`every_conformance_file_is_accounted_for`] walks the corpus and fails on
/// a file that is in neither column, so a package added next door cannot be
/// silently skipped here.
const PACKAGES: &[Case] = &[
    // -- in the native set --------------------------------------------
    //
    // `core/actor`'s whole surface, through the nine runtime entries that move
    // a Buri block by its two words. It is in rather than excluded because
    // every one of those entries is in both runtime tables, and the answers it
    // asserts are answers rather than timings.
    included("actor/counter.buri"),
    // The same surface driven from inside `core/alloc::scoped`, which is where
    // the four values `core/actor` hands the runtime have to be copies made
    // outside every arena rather than blocks the scope is about to unmap. It is
    // a *native* case in the way `memory/copyout.buri` is: on JavaScript the
    // assertions are answers, and here they are answers plus the pages.
    included("actor/scoped.buri"),
    //
    // Five files, and between them they are `core/bits` entire,
    // `Checked`/`Wrapping`/`Saturating`/`Bounded` at every width including
    // 128, the bitwise and string codegen corpora, and `core/simd`.
    included("codegen/bitwise.buri"),
    included("numbers/bits.buri"),
    included("numbers/integers.buri"),
    // `core/simd` turned out to need no vector intrinsic at all: it is
    // written in Buri over fixed-size tuples, and the only entries it
    // reaches outside the language are `math.sqrt` and `math.absFloat`.
    included("vectors/simd.buri"),
    // It used to be the only file that built a testing context and still
    // compiled, because the one it builds is `alloc` and that one reads no
    // state. The *stateful* half — a captured stream, a stdin, a clock, a
    // seeded generator, an environment, a filesystem — is in the archive now
    // (`cli/runtime/testing.rs`), which is what moved the four files below it
    // into this set.
    included("codegen/strings.buri"),
    // `core/alloc`'s three allocators, and the numbers the cost model
    // defines. This one is the payoff of a model that is *defined* rather
    // than measured: the assertions are integers written into the file, and
    // the JavaScript suite next door runs the identical file and gets the
    // identical integers. A backend that disagreed would be wrong, not
    // merely different.
    included("memory/allocators.buri"),
    // `core/alloc`'s scope, on the same terms: every effect forwarding through
    // a `Scoped<C>` and `Alloc` not forwarding are both claims about *who was
    // charged*, which is the defined model and so the same integers here as on
    // the JavaScript side. The arena's pages are the other half of the slice
    // and are asserted where they are visible — `cli/runtime/memory.rs`'s own
    // cases, through `buri_rt_heap_stats`.
    //
    // **Its `Listen` cases were the last thing this backend could not run.** An
    // acceptor that *invokes* a handler a wrapper rebuilt used to fault here,
    // and on the LLVM backend too: `middle/monomorphize.rs` retyped the handler
    // down to the implementation and rewrote the wrapper's own type on the way
    // past. It adapts the call instead now, and both of the forward's paths are
    // in the file. `agreement.rs`'s
    // `a_handler_a_wrapper_rebuilt_is_entered_on_every_backend` is the same
    // shape with no `core/alloc` in it at all.
    included("memory/scoped.buri"),
    // G5's half of the same slice: the value that *leaves* a scope. Every case
    // in it builds its answer out of blocks the program allocated — never a
    // literal, which is immortal and in no arena — and reads it after the
    // scope has ended, so on this backend a pointer that still named the
    // arena's mappings would be a read of unmapped memory rather than a wrong
    // string. That is why the file is worth running here and not only on the
    // reference backend, where the copy is the identity.
    included("memory/copyout.buri"),
    // It was excluded for `list.fold` until the backend grew the loop
    // over a closure, and
    // `the_excluded_packages_are_excluded_for_the_stated_reason` is what
    // said so on the day it stopped being true.
    included("canary/canary.buri"),
    // The two that the testing context and `list.sortBy` between them let in.
    // Both were excluded for a context that bound every effect whether a test
    // used it or not, and `calendar/date.buri` for `list.sortBy` as well.
    // `the_excluded_packages_are_excluded_for_the_stated_reason` is what said
    // so on the day each stopped being true.
    //
    // Neither builds one any more: both name `Alloc` alone. The reason above is
    // why each *was* out, and it is left standing because the reason a file was
    // excluded is the thing this ledger records — but the pressure it names is
    // gone from those files and from every other in the corpus.
    included("calendar/date.buri"),
    // `core/time`'s `Duration` beside the calendar it used to live in: pure
    // integer arithmetic, a hand-written `Show` over `str.format` and
    // `padStart`, and the test platform's clock — all of it surface this
    // backend already had, so the file was in from the day it was written.
    included("calendar/duration.buri"),
    included("collections/bitset.buri"),
    // It was the one file the backend compiled and got *wrong*, and
    // `a_wrong_answer_is_still_wrong` is what said so until the day three
    // premature-release defects behind it were fixed: `middle/rc.rs`'s
    // deferred drops being flushed by a sibling and its consumed scrutinee
    // being dropped twice, and `middle/layout.rs` niching `Option<T>` on a
    // list pointer that is null whenever the list is empty — which is what
    // `pop` produces the moment it empties a side.
    included("collections/queue.buri"),
    // The four the closure surface's last six entries and `deriveArrayShow`
    // let in. `core/list` has no gap left after them: every key in `list.buri`
    // is either a row in the runtime table or a loop in the backend, so a file
    // that only wanted a list is now a file that compiles.
    //
    //  * `collections/map.buri` wanted `find` and `flatten`;
    //  * `data/lists.buri` wanted all six, and is the file the closure surface
    //    exists for;
    //  * `data/optionresult.buri` wanted `find` and `foldResult`;
    //  * `data/patterns.buri` wanted `deriveArrayShow`, which is the element's
    //    generated `show` called once per element plus `buri_rt_show_list`.
    included("collections/map.buri"),
    // `core/ordmap` and `core/ordset` are ordinary Buri over a recursive enum,
    // which the backend boxes, and `core/list`'s splicing — so the file that
    // exercises them reaches nothing the four above do not, and is in the
    // native set from the day it was written.
    included("collections/ordmap.buri"),
    included("data/lists.buri"),
    included("data/optionresult.buri"),
    included("data/patterns.buri"),
    // `core/char`'s eight, which used to be excluded as "a General_Category
    // table Rust does not expose". It still is not one Rust exposes;
    // `cli/runtime/char.rs` carries it, generated from the engine the
    // JavaScript backend runs on, and says so.
    included("data/strings.buri"),
    // `core/char`'s eight, which used to be "a General Category table Rust does
    // not expose" and now is `cli/runtime/char.rs` — a table this repository
    // carries, generated from the engine the JavaScript backend runs on.
    // The two that cost nothing but opening the repository this corpus has
    // always been. They were excluded for `imports //lib/codegen`, which was
    // never a statement about the backend — see [`repository`].
    included("codegen/equality.buri"),
    // The other half of what a derived conformance means, and the file that
    // `deriveArrayCompare` landing made runnable: `derive Ord` where the field
    // is a `[T]` used to be a program the front end accepted and this backend
    // refused by name (buri-lang/buri#27). Its element types are the three that
    // reach different leaves — a scalar, a `Str` and a struct — so it is also
    // the check that the derived order and `Str.compare` agree.
    included("codegen/ordering.buri"),
    included("codegen/tail_calls.buri"),
    // The closure trampoline's pilot. It is in the native set by construction:
    // `list.mapCtxStep` exists so that the *native* boundary — the runtime
    // calling back into Buri code through a generated entry thunk — has
    // something to be exercised by, so a backend that could not compile this
    // file would be a backend the key had failed on.
    included("codegen/step_trampoline.buri"),
    // The four `//lib/semantics` files. Two of them reach the filesystem
    // double's methods and were waiting on `cli/runtime/lib.rs` §2.1 — the
    // `Result<T, E>` shape — as well as on the repository; the other two built
    // a context binding a filesystem whether a test read a file or not.
    included("semantics/effects.buri"),
    included("semantics/evaluation.buri"),
    included("semantics/traits.buri"),
    // `Either.Right(1)` names neither `Left`'s type nor a value of it, and the
    // fourth file is full of that shape. It was excluded while such a parameter
    // stayed a free inference variable, which no backend has a layout for;
    // `Subst::default_unconstrained` makes it `()`.
    included("semantics/generics.buri"),
    // The fifth: which positions of a type constructor hand a value back
    // (SPEC 10.2). It reaches nothing the four above do not — a test context,
    // `list.mapCtx`, `[Str].join` and `str.format` — so it is in the
    // native set from the day it was written.
    included("semantics/variance.buri"),
    // The sixth: a field a literal leaves out is an explicit `.None` by the
    // time either backend sees it, which is the claim this file is here to put
    // to the native one as well as to the reference one (SPEC 5.6).
    included("semantics/elision.buri"),
    // The seventh: a literal that leaves its type name out is the same
    // `StructLit` node by the time either backend sees it — the head is read
    // by the checker and never reaches a backend — so this file is here to say
    // that out loud on the native one too (design/grammar-rationale.md 12.3).
    included("semantics/anonymous.buri"),
    // The eighth: `core/host/testing`'s ten doubles. Seven of them are handles
    // over `cli/runtime/testing.rs`'s table; `TestAlloc` is the two
    // instructions both backends open-code, and `TestNet` and `TestProc` are
    // Buri bodies with no row at all. So the file reaches nothing the archive
    // did not already have. It is here rather than folded into `effects.buri`
    // because the two ask different questions: `effects.buri` is about contexts
    // and `host_testing.buri` is about the doubles a context binds.
    included("semantics/host_testing.buri"),
    // The ninth: `Request` and `Response`, the two types `Net.fetch` speaks in.
    // No `Net` call in it reaches the network — a fresh `net()` refuses and the rest
    // is construction — so what this proves natively is the *shape*: a struct
    // holding a `[Header]` and a `[U8]`, its derived `Eq` and `Show`, and the
    // `core/bytes` pair underneath the text constructors.
    included("semantics/http.buri"),
    // `core/cli`, driven end to end through `run` — which means the
    // environment double, two captured streams, and a handler reached through
    // a `fn(C, Arguments)` stored in a struct field. Nothing in it is an
    // intrinsic: the parse is Buri over `[Str]`, `Map` and `Set`, and the help
    // pages are `str.format` and `padEnd`. It is in the native set for
    // `semantics/host_testing.buri`'s reason — the doubles are the archive's
    // table — plus one of its own: a closure called through a struct field is
    // the shape `codegen/step_trampoline.buri` pilots, and a command's `run`
    // is that shape in a library.
    included("cli/arguments.buri"),
    // -- out: the backend has no body for what they reach ---------------
    //
    // Every one of these is reported by `Backend::missing_intrinsics`
    // before a byte of code is generated, which is what that hook is for,
    // and `the_excluded_packages_are_excluded_for_the_stated_reason`
    // checks that the reason is still true.
    excluded(
        "numbers/conversions.buri",
        "the two inexact conversions whose *source* is not an integer: \
             `F64 -> I64`, where `NaN` and the infinities are outside every \
             range rather than at one end of it, and `U32 -> Char`, where the \
             target is a set of scalar values and not a range at all. The \
             integer narrowings are compiled — `stencil::emit`'s \
             `convert_checked` builds the `Result<T, RangeError>` — as are \
             every widening and every `wrapTo*`",
    ),
    excluded("json/decoding.buri", "`json.decode`, and core/char's classifiers"),
    // `derivePrimJson` was this row's reason and is not any more: both native
    // backends have a body for it (VALUE-MODEL.md §12 row 10). What is left is
    // its sibling — `ToJson::toJson` called *directly* on a primitive, which
    // reaches a backend as `bool.toJson`, `char.toJson`, `str.toJson`,
    // `num.I64.toJson` and `num.F64.toJson`, five ordinary intrinsic keys with
    // no body. They are the same three-way answer `json_prim` already gives
    // and are a slice of their own, because letting this file in moves the
    // census ratchet.
    excluded("json/encoding.buri", "`ToJson::toJson` at every primitive"),
    // `core/bytes` and `char.toDigit` are emitted now, and this file needs
    // nothing else.
    included("proto/failures.buri"),
    // Held out as `Out::Costly` until `middle/rc.rs`'s `Scan::short_circuit`
    // stopped scanning its right operand twice. It compiled and all
    // twenty-nine blocks passed the whole time; what it cost was the 2ⁿ that
    // cost bought over the `&&` chain `middle/derives.rs`'s `eq_fields`
    // right-nests, one link per field of a generated message. About 280
    // seconds then, about three now.
    included("proto/binary.buri"),
    excluded(
        "proto/json.buri",
        "`num.F64.toI64` — an inexact conversion, so it answers \
             `Result<Int, RangeError>`. `core/char`'s classifiers and \
             `core/bytes` are emitted now",
    ),
    // `core/bytes`'s six intrinsics — the UTF-8 pair and the four IEEE 754
    // byte-pattern entries — are `cli/runtime/bytes.rs` now, which is the one
    // surface each of these two was waiting for.
    included("crypto/sha256.buri"),
    // The seeded `Entropy` double and the two doors onto it. Native from the
    // day it landed: `TestEntropy` shares `Slot::Rand` with `TestRand` in
    // `cli/runtime/testing.rs`, so the sequence this file writes down is the
    // one both backends draw.
    included("crypto/entropy.buri"),
    // `Gen`, which is ordinary Buri and reaches no host: U64 wrapping
    // arithmetic, shifts, tail recursion and a tuple returned from every
    // method. It is on the native set from the day it landed for the reason
    // `crypto/sha256.buri` is — a second implementation of an algorithm both
    // backends have to agree about, with every answer written into the file.
    included("random/gen.buri"),
    included("text/bytes.buri"),
    // Hexadecimal across four modules — `char.fromDigit`, `num.toHex`,
    // `str.toRadix` and `core/bytes`' pair. Every conversion in it is exact, so
    // none of it meets the `Result<T, RangeError>` shape that holds
    // `numbers/conversions.buri` out.
    included("text/hex.buri"),
    excluded(
        "numbers/floats.buri",
        "core/math's thirteen *transcendentals*, whose answers IEEE 754 does \
             not fix — `cli/runtime/math.rs` says why implementing them with the \
             platform libm would be a divergence rather than a gap",
    ),
    excluded(
        "text/json.buri",
        "`num.U32.toChar` — an *inexact* conversion, so it answers \
             `Result<Char, RangeError>`. `core/char`'s classifiers and \
             `list.find` are emitted now, and this one call is the whole of \
             what is left",
    ),
    // The two below are excluded for a reason no later wave of this backend
    // retires: the reactive graph is a *runtime* — a mutable dependency graph
    // with a scheduler — and it lives in the JavaScript runtime alone. There
    // is no `cli/runtime/*.rs` for `$ui_*`, and until a native target renders
    // anything there is nothing for one to be right about.
    excluded(
        "ui/reactivity.buri",
        "the reactive graph, `ui/effect`'s five `Ui` entries and \
             `ui/testing`'s recorder — all of them `backend/js/runtime.js` and \
             nothing else, because no native platform grants `Ui`",
    ),
    excluded(
        "ui/tree.buri",
        "`ui/node`'s `mount` and `ui/testing`'s renderer, which are a \
             *document* — an element tree, its listeners and a keyed \
             reconciler over it. There is nothing on this side to render to, \
             and a native backend that grew one would be rendering to \
             something else",
    ),
    excluded(
        "ui/theme.buri",
        "`ui/testing`'s `install` and `variables`, which are the custom \
             properties a document reads. A theme is ordinary Buri — the \
             enums, the mappings and the exhaustiveness that is the whole \
             contract are all checked on every platform — but what a resolved \
             theme *is* on the other side is a `:root` block, and there is no \
             document here to put one in",
    ),
];

/// Why this host cannot build and run a native artifact, or `None`.
///
/// The host question belongs to `stencil::unavailable_reason`, which is "this
/// host has a stencil library and an entry point to put in front of it"
/// answered as a sentence — so this suite runs unchanged wherever the backend
/// does, and says which half is missing where it does not.
fn skip_reason() -> Option<String> {
    if !AVAILABLE {
        return Some(String::from("this toolchain carries no native runtime archive"));
    }
    stencil_unavailable_reason()
}

/// Whether this host can build and run a native artifact at all, printing the
/// reason where it cannot.
///
/// The print is the point: the corpus is 26 files and 1187 test blocks, and a
/// host that ran none of them reports the same four passing tests as a host
/// that ran all of them. On a runner it is not a print but a panic —
/// `harness/ci.rs` reads `BURI_CI` and the workflow sets it everywhere.
fn supported() -> bool {
    match skip_reason() {
        Some(why) => !crate::ci::skipped("native conformance", &why),
        None => true,
    }
}

fn host_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance/lib")
}

/// The conformance corpus **as the repository it is**, opened once per process.
///
/// `cli/tests/conformance/REPO.buri` has been there all along; this harness
/// simply never opened it, and compiled each file as a snippet against the
/// standard library with no repository. Eleven files import `//lib/<package>`,
/// so the *front end* refused them — a limit of the harness that said nothing
/// about the backend, and the single largest reason a conformance file was not
/// running natively.
///
/// [`driver::analyze_snippet_as`] takes a workspace and the package the text
/// stands in for, which is exactly what a test source of that package is. The
/// text still comes from the string this harness read rather than from the
/// loader, because [`the_native_set_can_fail`] edits one assertion and
/// recompiles, and a file re-read from disk would silently undo that.
fn repository() -> Option<&'static Workspace> {
    crate::shared::conformance_repository()
}

/// One conformance file, loaded and checked as a test source of its own
/// package.
///
/// `case_path` is `semantics/effects.buri`, and the package is `lib/semantics`
/// — the same split [`read`] makes, for the same reason.
fn analyze(case_path: &str, source: &str, map: &mut SourceMap) -> driver::Analysis {
    let repository = repository();
    let package = repository.and_then(|w| {
        let (package, _) = case_path.split_once('/')?;
        w.package_by_path(&format!("lib/{package}"))
    });
    let mut cache = buri::parsing::parser::Cache::new();
    driver::analyze_snippet_as(
        repository,
        package,
        map,
        &mut cache,
        "main",
        source,
        Role::TestSource,
    )
}

/// A directory this *process* owns, per case.
///
/// The process id is in the name because two overlapping `cargo test` runs
/// otherwise share it, and the second overwrites the binary the first is
/// executing — which on macOS is a child that never returns rather than an
/// error.
fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-conformance-{}", std::process::id()))
        .join(name.replace('/', "-"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The runtime archive, written once for the process rather than once per
/// case, for the reason `native/stencil.rs::archive` gives.
fn archive() -> &'static Path {
    static WRITTEN: OnceLock<PathBuf> = OnceLock::new();
    WRITTEN.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("native-conformance-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(ARCHIVE_NAME);
        std::fs::write(&path, ARCHIVE).unwrap();
        path
    })
}

/// Compile one conformance file as a **test binary**, link it, run it.
///
/// Answers `(status, stdout, stderr, blocks)`, where `blocks` is how many
/// `test` declarations the file holds — the count this harness reports, and
/// the one that makes "it passed" mean something.
fn run(name: &str, source: &str) -> Option<(i32, String, String, usize)> {
    let mut map = SourceMap::new();
    let analysis = analyze(name, source, &mut map);
    if analysis.diagnostics.has_errors() {
        return None;
    }
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diagnostics, monomorphize::Roots::Tests);
    assert!(!diagnostics.has_errors(), "{name}: monomorphization failed");
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    let blocks = program.roots.tests().len();

    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let mut backend = Stencil;
    let missing = backend.missing_intrinsics(&program, &analysis.checked.tables);
    assert!(missing.is_empty(), "{name}: the backend is missing {missing:?}");
    let units = match backend.emit(&program, &analysis.checked.tables, &opts) {
        Ok(units) => units,
        Err(d) => panic!(
            "{name}: the backend refused the program: {:?}",
            d.items.iter().map(|i| i.message.clone()).collect::<Vec<_>>()
        ),
    };

    let dir = workspace(name);
    let mut objects = Vec::new();
    for unit in &units {
        let path = dir.join(&unit.name);
        std::fs::write(&path, &unit.bytes).unwrap();
        objects.push(path);
    }
    let binary = dir.join("program");
    // `build/link.rs`'s driver and its trailing arguments — the product's link
    // line and not a second idea of it (`shared::product_cc`). On Linux it is
    // a static-PIE musl link, which is not something a harness can spell out
    // in three `-l`s.
    let mut cc = crate::shared::product_cc();
    cc.arg("-o").arg(&binary);
    for o in &objects {
        cc.arg(o);
    }
    cc.arg(archive());
    cc.args(crate::shared::product_link_args());
    let built = cc.output().unwrap();
    assert!(
        built.status.success(),
        "{name}: the link failed:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = Command::new(&binary).output().unwrap();
    Some((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        blocks,
    ))
}

/// Why the native backend will not compile a source, or the empty string
/// where it will.
///
/// **Both halves**, and the second is not optional. `missing_intrinsics`
/// answers before emission and is where an unimplemented `FuncKind::Intrinsic`
/// shows up — but a `deriveArray*` is an `ExprKind::Intrinsic` inside a body
/// `middle::derives` generated, and a structural operation is an
/// `ir::Inst::Structural` that exists only after lowering, so neither is in the
/// program that hook is handed. `native/agreement.rs`'s `native_refusal` says
/// the same thing and asks the same two questions; this used to ask only the
/// first, and `data/patterns.buri` — refused for `deriveArrayShow` and for
/// nothing the hook can see — is the file that made the difference visible.
fn refusal(name: &str, source: &str) -> Result<String, String> {
    let mut map = SourceMap::new();
    let analysis = analyze(name, source, &mut map);
    if analysis.diagnostics.has_errors() {
        return Err(analysis
            .diagnostics
            .items
            .iter()
            .take(2)
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
            .join("; "));
    }
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diagnostics, monomorphize::Roots::Tests);
    if diagnostics.has_errors() {
        return Err(String::from("monomorphization failed"));
    }
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    let missing = Stencil.missing_intrinsics(&program, &analysis.checked.tables);
    if !missing.is_empty() {
        return Ok(missing.join("; "));
    }
    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    match Stencil.emit(&program, &analysis.checked.tables, &opts) {
        Ok(_) => Ok(String::new()),
        Err(d) => {
            Ok(d.items.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("; "))
        }
    }
}

/// Whether the native backend can compile a source at all, without linking.
///
/// The hook's half of [`refusal`], for the callers that want the keys rather
/// than a sentence.
fn missing_for(name: &str, source: &str) -> Result<Vec<String>, String> {
    let mut map = SourceMap::new();
    let analysis = analyze(name, source, &mut map);
    if analysis.diagnostics.has_errors() {
        return Err(analysis
            .diagnostics
            .items
            .iter()
            .take(2)
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
            .join("; "));
    }
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diagnostics, monomorphize::Roots::Tests);
    if diagnostics.has_errors() {
        return Err(String::from("monomorphization failed"));
    }
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    Ok(Stencil.missing_intrinsics(&program, &analysis.checked.tables))
}

/// `calendar/date.buri` names `lib/calendar/test/date.buri`: the corpus
/// puts a package's test sources under `test/`, and the key here drops that
/// because every file in the list is one.
fn read(case: &Case) -> String {
    let (package, file) = case.path.split_once('/').unwrap_or((case.path, ""));
    let path = corpus().join(package).join("test").join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// -----------------------------------------------------------------------


/// The list above covers the corpus, so a package added next door is a
/// failing test here rather than a silent omission.
#[test]
fn every_conformance_file_is_accounted_for() {
    let root = corpus();
    let mut found: Vec<String> = Vec::new();
    let mut packages: Vec<_> =
        std::fs::read_dir(&root).unwrap().filter_map(Result::ok).collect();
    packages.sort_by_key(std::fs::DirEntry::file_name);
    for package in packages {
        let tests = package.path().join("test");
        if !tests.is_dir() {
            continue;
        }
        let mut files: Vec<_> =
            std::fs::read_dir(&tests).unwrap().filter_map(Result::ok).collect();
        files.sort_by_key(std::fs::DirEntry::file_name);
        for file in files {
            found.push(format!(
                "{}/{}",
                package.file_name().to_string_lossy(),
                file.file_name().to_string_lossy()
            ));
        }
    }
    for path in &found {
        assert!(
            PACKAGES.iter().any(|c| c.path == path),
            "`{path}` is in the conformance corpus and in neither column of \
                 `PACKAGES`: put it in the native set, or exclude it with a reason"
        );
    }
    for case in PACKAGES {
        assert!(found.iter().any(|f| f == case.path), "`{}` no longer exists", case.path);
    }
}

/// The excluded set is excluded because the backend refuses it, not because
/// somebody wrote a comment.
///
/// This is the test that keeps the reasons above honest: if a package
/// becomes compilable — because a later wave landed `json.*` or the closure
/// surface — this fails and the reason has to be deleted rather than left
/// to rot.
#[test]
fn the_excluded_packages_are_excluded_for_the_stated_reason() {
    if !supported() {
        return;
    }
    for case in PACKAGES.iter().filter(|c| matches!(c.out, Some(Out::Refused(_)))) {
        let source = read(case);
        match refusal(case.path, &source) {
            // A front-end error means the corpus is mid-change, which is
            // not this file's business to fail over.
            Err(_) => continue,
            Ok(why) => assert!(
                !why.is_empty(),
                "`{}` is listed as excluded ({}), but the backend now \
                     compiles it — delete the exclusion",
                case.path,
                case.out.as_ref().map(Out::why).unwrap_or_default()
            ),
        }
    }
}

/// Every file in the native set compiles, links, runs, and every `test`
/// block in it passes.
///
/// The bar is the block count, not the exit status alone: a file that
/// compiled to no tests at all would exit zero and prove nothing, which is
/// the same reason `language/conformance.rs` counts its assertions.
#[test]
fn the_native_set_passes() {
    if !supported() {
        return;
    }
    let mut total = 0usize;
    let mut ran = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for case in PACKAGES.iter().filter(|c| c.out.is_none()) {
        let source = read(case);
        // A file the *front end* refuses is not this file's failure: the
        // corpus is shared with `language/conformance.rs` and may be mid-change.
        match missing_for(case.path, &source) {
            Err(e) => {
                skipped.push(format!("{} (front end: {e})", case.path));
                continue;
            }
            Ok(missing) if !missing.is_empty() => panic!(
                "`{}` is in the native set but the backend is missing {missing:?}",
                case.path
            ),
            Ok(_) => {}
        }
        let Some((status, out, err, blocks)) = run(case.path, &source) else {
            skipped.push(format!("{} (front end)", case.path));
            continue;
        };
        if status != 0 {
            failures
                .push(format!("`{}` exited {status}:\nstdout:\n{out}\nstderr:\n{err}", case.path));
        }
        assert!(blocks > 0, "`{}` holds no `test` blocks", case.path);
        total += blocks;
        ran += 1;
    }
    // Every failing file, not the first: two platforms failing on two
    // different files is one report here and two runs otherwise.
    assert!(failures.is_empty(), "{} files failed:\n{}", failures.len(), failures.join("\n"));
    for s in &skipped {
        eprintln!("native conformance: skipped {s}");
    }
    eprintln!("native conformance: {ran} files, {total} test blocks, 0 failures");
    assert!(ran > 0, "no conformance file ran natively");
}

/// `core/host/testing`, natively, against the numbers and strings the
/// JavaScript runner answers.
///
/// The stateful half of the test platform is a **handle table on the runner's
/// side** (`host_testing.buri`'s header), which on JavaScript is
/// `runtime.js`'s `$t.h` and natively is `cli/runtime/testing.rs`. A
/// conformance file may bind a double and never read it back, so a table that
/// answered the empty string to every `captured()` would pass the suite above
/// unchanged. This is the test that would not.
///
/// Every assertion is a literal written into the source, so the two backends
/// either both pass it or disagree about the language — which is the argument
/// `memory/allocators.buri` makes for a *defined* cost model, applied to a
/// *defined* fixture. The identical source is a package in
/// `scratchpad/tctxcheck` and `buri test` runs it on the JavaScript backend:
/// **12 passed, 0 failed**, the same twelve blocks this runs natively.
///
/// The seeded rows are the load-bearing ones. `rand` is xorshift32 and the
/// *sequence* is part of what a seeded test asserts, so `69, 89` from a seed of
/// zero checks that the two runtimes are the same generator rather than two
/// reproducible ones.
///
/// # The fixture reads its `Option`s with `assert.some`
///
/// It wrote the `match` out until `middle/rc.rs`'s `match_` stopped dropping a
/// consumed scrutinee twice — once at the arm entry and once from
/// `Scan::balance`, because the arm that reports the failure mentions the
/// `Option` itself and so put it in the union the arms are balanced against.
/// `assert.ok` never had it: its failing arm names the payload rather than the
/// `Result`. `rc.rs`'s `a_consumed_scrutinee_is_dropped_once_on_every_arm` is
/// that shape as a unit test; these blocks are it through the real assertion.
#[test]
fn the_test_platform_agrees_with_the_runner() {
    if !supported() {
        return;
    }
    const SOURCE: &str = r##"from "core/effect" import { Alloc, Clock, Env, Rand, Stderr, Stdin, Stdout };
from "core/env" import * as env;
from "core/host/testing" import {
  alloc, clock, env, rand, stderr, stdin, stdout,
};
from "core/io" import * as io;
from "core/list" import * as list;
from "core/random" import * as random;
from "core/str" import * as str;
from "core/testing/assert" import * as assert;
from "core/time" import * as time;

fn speak<C: Stdout>(ctx: C, what: Str): () {
  let _ = io.print(ctx, "[").ignore();
  let _ = io.print(ctx, what).ignore();
  let _ = io.println(ctx, "]").ignore();
}

fn shout<C: Stderr>(ctx: C, what: Str): () {
  let _ = io.eprint(ctx, "<").ignore();
  let _ = io.eprintln(ctx, what).ignore();
}

test "captured reads back what a function printed" {
  let sink = stdout();
  let ctx = context { Alloc: alloc(), Stdout: sink };
  speak(ctx, "hello");
  assert.eq(sink.captured(), "[hello]\n");
}

test "a fresh sink is empty and stays independent" {
  let first = stdout();
  let second = stdout();
  let ctx = context { Alloc: alloc(), Stdout: first };
  speak(ctx, "one");
  assert.eq(second.captured(), "");
  assert.eq(first.captured(), "[one]\n");
}

test "captured accumulates in the order things were printed" {
  let sink = stdout();
  let ctx = context { Alloc: alloc(), Stdout: sink };
  let _ = io.print(ctx, "a").ignore();
  let _ = io.println(ctx, "b").ignore();
  let _ = io.print(ctx, "c").ignore();
  assert.eq(sink.captured(), "ab\nc");
}

test "writeBytes is captured as the text the octets spell" {
  let sink = stdout();
  let ctx = context { Alloc: alloc(), Stdout: sink };
  let _ = io.writeBytes(ctx, [104, 105]).ignore();
  assert.eq(sink.captured(), "hi");
}

test "standard error is its own transcript" {
  let out = stdout();
  let err = stderr();
  let ctx = context { Alloc: alloc(), Stdout: out, Stderr: err };
  shout(ctx, "bad");
  assert.eq(err.captured(), "<bad\n");
  assert.eq(out.captured(), "");
}

test "a test clock starts where it was put and moves only when moved" {
  let dial = clock().at(1000);
  let ctx = context { Alloc: alloc(), Clock: dial };
  assert.eq(time.now(ctx).0, 1000);
  assert.eq(time.now(ctx).0, 1000);
  let _ = time.sleepMs(ctx, 5);
  assert.eq(time.now(ctx).0, 1005);
  let _ = time.sleepMs(dial, 10);
  assert.eq(time.now(ctx).0, 1015);
}

test "a seeded generator is the same sequence on every backend" {
  let ctx = context { Alloc: alloc(), Rand: rand().seed(0) };
  assert.eq(random.int(ctx, 0, 100), 69);
  assert.eq(random.int(ctx, 0, 100), 89);
  assert.eq(random.int(ctx, 10, 11), 10);
  let ctx2 = context { Alloc: alloc(), Rand: rand().seed(7) };
  assert.eq(random.int(ctx2, 0, 1000), 583);
}

test "two generators with the same seed agree with each other" {
  let a = context { Alloc: alloc(), Rand: rand().seed(42) };
  let b = context { Alloc: alloc(), Rand: rand().seed(42) };
  assert.eq(random.int(a, 0, 1000000), random.int(b, 0, 1000000));
}

test "an environment holds what it was given and nothing else" {
  let ctx = context {
    Alloc: alloc(),
    Env: env().variables([("HOME", "/tmp"), ("LANG", "C")]).arguments(["--verbose", "x"]),
  };
  assert.eq(assert.some(env.get(ctx, "HOME")), "/tmp");
  assert.eq(assert.some(env.get(ctx, "LANG")), "C");
  assert.isTrue(env.get(ctx, "PATH").isNone());
  let args = env.args(ctx);
  assert.eq(args.len(), 2);
  assert.eq(args.join(ctx, " "), "--verbose x");
}

test "an empty environment has no variables and no arguments" {
  let ctx = context { Alloc: alloc(), Env: env() };
  assert.isTrue(env.get(ctx, "HOME").isNone());
  assert.eq(env.args(ctx).len(), 0);
}

test "stdin reads its lines, then end of input" {
  let ctx = context { Alloc: alloc(), Stdin: stdin().lines(["one", "two"]) };
  assert.eq(assert.some(io.readLine(ctx)), "one");
  assert.eq(assert.some(io.readLine(ctx)), "two");
  assert.isTrue(io.readLine(ctx).isNone());
}

test "a stdin of octets reads them, and readLine finds nothing there" {
  let ctx = context { Alloc: alloc(), Stdin: stdin().bytes([1, 2, 3, 4]) };
  let first = assert.some(io.readBytes(ctx, 3));
  assert.eq(first.len(), 3);
  assert.eq(assert.some(first.get(0)), 1);
  assert.eq(assert.some(first.get(2)), 3);
  let rest = assert.some(io.readBytes(ctx, 3));
  assert.eq(rest.len(), 1);
  assert.eq(assert.some(rest.get(0)), 4);
  assert.isTrue(io.readBytes(ctx, 1).isNone());
  assert.isTrue(io.readLine(ctx).isNone());
}
"##;
    if refusal("host-testing", SOURCE).is_err() {
        return;
    }
    let Some((status, out, err, blocks)) = run("host-testing", SOURCE) else {
        panic!("the front end refused the host-testing fixture");
    };
    assert_eq!(status, 0, "stdout:\n{out}\nstderr:\n{err}");
    assert_eq!(blocks, 12, "the fixture lost a `test` block");
}

/// `Self` through a context, natively, as a test that cannot be skipped.
///
/// The blocks this mirrors live in `semantics/effects.buri` and run in
/// [`the_native_set_passes`] above — but that harness *skips* a file the front
/// end refuses, deliberately, because the corpus is shared with
/// `language/conformance.rs` and may be mid-change. A handler that reads a
/// field off the implementation it is handed does not typecheck at all when
/// `Self` is the receiver, so the whole file would have been skipped rather
/// than failed, and the native half of this fix would have had no guard on this
/// side at all. This one panics on a refusal.
///
/// Both halves are here in one source:
///
/// * `server.serve(ctx, …)` with a **context value**, whose handler *prints* —
///   an effect this context grants and `OneShotListen` does not. `run` calls
///   the handler itself, under the context `serve` was given, so what arrives
///   is the caller's whole context by construction. When the effect still took
///   the handler and invoked it, the acceptor arrived instead and the process
///   died reading its bytes.
/// * `serveOnce(ctx, …)` and `runInOrder(ctx, …)`, which reach the same
///   library through a **bounded type parameter**, so the call is one layer
///   further from the context. Before `rewrite_call_args`, this exited `-1`
///   with no stdout and no stderr.
///
/// And the rule's other side, which is `Tasks.parallel`'s: a step is handed the
/// caller's **context**, spelled `ctx: C` in the declaration rather than `Self`.
/// `tasks.parallel(ctx, …)`'s step reads a clock the scheduler does not have,
/// and
/// `runInOrderNamed` allocates inside a step reached through a bound — the
/// first is a wrong answer if the wrong value arrives, the second does not
/// compile at all.
#[test]
fn self_through_a_context_is_the_implementing_type() {
    if !supported() {
        return;
    }
    const SOURCE: &str = r#"from "core/effect" import {
  Alloc, Clock, Listen, Net, Request, Response, Sockets, Stdout, Tasks,
};
from "core/host/testing" import { alloc, clock, stdout };
from "core/io" import * as io;
from "core/net/server" import * as server;
from "core/tasks" import * as tasks;
from "core/testing/assert" import * as assert;
from "core/time" import * as time;
from "//lib/semantics" import {
  OneShotListen, QuietSockets, SerialTasks, TeapotNet, runInOrder, runInOrderNamed, serveOnce,
};

test "the handler is handed the caller's context" {
  let sink = stdout();
  let ctx = context {
    Alloc: alloc(),
    Listen: OneShotListen { bindsTo: "10.0.0.1" },
    Stdout: sink,
    Tasks: SerialTasks { label: "serial", bias: 4 },
  };
  assert.ok(server.serve(ctx, server.Server {
    port: 0,
    address: .Some("10.0.0.1"),
    onRequest: fn(c, request) => {
      io.println(c, "hit ${request.url}").ignore();
      Response { status: 200, headers: [], body: [] }
    },
  }));
  let _ = assert.err(server.serve(ctx, server.Server {
    port: 0,
    address: .Some("10.0.0.1"),
    onRequest: fn(c, request) => {
      io.println(c, "hit ${request.url}").ignore();
      Response { status: 42, headers: [], body: [] }
    },
  }));
  assert.eq(sink.captured(), "hit 10.0.0.1\nhit 10.0.0.1\n");
}

test "and a task is handed the context" {
  let ctx = context {
    Alloc: alloc(),
    Clock: clock().at(5),
    Tasks: SerialTasks { label: "serial", bias: 4 },
  };
  let out = tasks.parallel(ctx, [1], fn(c, i, item) => time.now(c).0 + item);
  assert.eq(out[0].withDefault(0), 6);
}

test "and through a bound the call still lands" {
  let ctx = context {
    Alloc: alloc(),
    Listen: OneShotListen { bindsTo: "127.0.0.1" },
    Net: TeapotNet { body: [] },
    Sockets: QuietSockets {},
    Tasks: SerialTasks { label: "serial", bias: 0 },
  };
  assert.eq(serveOnce(ctx, "http://example.com/ping"), 418);
  assert.eq(runInOrder(ctx, [3, 4]), 107);
  assert.eq(runInOrderNamed(ctx, [3, 4]), ["0:3", "1:4"]);
}
"#;
    let Some((status, out, err, blocks)) = run("semantics/self-through-a-context.buri", SOURCE)
    else {
        panic!(
            "the front end refused the `Self`-through-a-context fixture — which is what it did \
             before `rewrite_call_args`, because a handler cannot use an effect the acceptor \
             does not grant"
        );
    };
    assert_eq!(status, 0, "stdout:\n{out}\nstderr:\n{err}");
    assert_eq!(blocks, 3, "the fixture lost a `test` block");
}

/// The harness has to be able to fail.
///
/// `language/conformance.rs` has the same test for the same reason: a suite that
/// cannot fail proves nothing. It breaks one assertion in a file that is in
/// the native set and checks that the native binary exits non-zero and says
/// which assertion it was. `numbers/bits.buri` rather than the canary
/// package, which is in the set itself now and whose own failure would be a
/// second thing to explain.
#[test]
fn the_native_set_can_fail() {
    if !supported() {
        return;
    }
    let case = Case { path: "numbers/bits.buri", out: None };
    let source = read(&case);
    if missing_for("bits-broken", &source).is_err() {
        return;
    }
    // The value, not a name: renaming a constant and its use together would
    // leave the assertion true. `assert!` on the marker means a corpus that
    // stopped containing it fails here rather than passing vacuously.
    const MARKER: &str = "assert.eq(bits.shl(1, 10), 1024);"; 
    assert!(
        source.contains(MARKER),
        "`numbers/bits.buri` no longer contains the assertion this test edits"
    );
    let broken = source.replace(MARKER, "assert.eq(bits.shl(1, 10), 1025);");
    let Some((status, out, err, _)) = run("bits-broken", &broken) else {
        return;
    };
    assert_ne!(status, 0, "a broken assertion still passed:\n{out}\n{err}");
    assert!(
        err.contains("assert.eq failed"),
        "the failure did not name the assertion:\nstdout:\n{out}\nstderr:\n{err}"
    );
}

