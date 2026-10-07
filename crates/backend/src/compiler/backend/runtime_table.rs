//! The `buri_rt_*` table both native backends read: which intrinsic keys have
//! a symbol, and what that symbol's C signature is.
//!
//! `cli/runtime/lib.rs`'s module comment is the contract; this is the
//! transcription of it that generated code is emitted against. It is one table
//! because it describes `cli/runtime`, which is one library. A disagreement
//! between it and the archive is a miscompile that only shows up as a wrong
//! answer at run time, which is why the shapes are named ([`Arg`], [`Ret`])
//! rather than spelled out per symbol as parameter counts.
//!
//! The two backends read the same row differently. The LLVM backend walks
//! [`Entry::args`] to build the C argument list at each argument's own
//! position. The frame-threaded backend (`stencil/rtcall.rs`) flattens the
//! Buri arguments into leaves and appends what [`Entry::extra`] selects. The
//! two agree because every closure-shaped argument is the last one
//! (`a_closure_is_the_last_argument`, below).
//!
//! # A key carries no type arguments
//!
//! `middle::monomorphize` builds an intrinsic key out of the module, the type
//! and the name, and out of nothing else. One key is one row here and one
//! `buri_rt_*` symbol, so **every instantiation of a generic intrinsic reaches
//! the same body**: `list.map` at `[Int]` and at `[Point]` are one call, and
//! the element type does not cross. It cannot — this archive is compiled once,
//! against no Buri type at all.
//!
//! Everything the runtime has to know about an erased type arrives as a value:
//!
//! * the element **stride and retain glue** of [`Arg::Stride`] and
//!   [`Arg::Retain`] — the shape of a `[T]`, which is why `core/list`'s rows
//!   carry them and `core/bytes`'s do not (their element type is fixed at
//!   `U8`);
//! * an **address**, through [`Arg::Spilled`], for an argument whose type is a
//!   bare `T` and so has no leaf list a C signature could name;
//! * a **runtime descriptor**, for an operation whose subject is the *whole*
//!   shape of a type rather than its size — `json.decode` and the test
//!   runner's two, which are `middle::monomorphize`'s `Func::desc` and reach
//!   no row here;
//! * the **entry thunk** of [`Arg::Step`] — a function the backend generated,
//!   which answers "how is one Buri closure called", the one thing
//!   `cli/runtime/list.rs`'s header says C cannot do.
//!
//! An intrinsic that is generic and has none of these is a miscompile with no
//! diagnostic, so the set of keys allowed to be generic is a written list —
//! `GENERIC_INTRINSICS` in `middle/monomorphize.rs` — and a generic intrinsic
//! outside it is an internal error at monomorphization. **A new row here for a
//! generic key needs a row there too.**
//!
//! # Why a table and not a mangling
//!
//! The rule in `lib.rs` §1 would happily produce `buri_rt_list_map` for
//! `list.map`, which does not exist, and a program that used it would get a
//! link error naming a symbol instead of `Backend::missing_intrinsics` naming
//! the operation. So the table decides which keys exist, and the mangler
//! (`runtime_native::symbol_for`) only names the symbol of a key the table
//! has ([`Entry::symbol`]).

use Arg::{
    Bytes, Compute, Dropped, Elems, Equal, List, Press, Release, Retain, Scalar, Spilled, Step,
    Str, Stride, Walk,
};

/// The discriminant a fallible runtime entry returns for its success arm.
///
/// `cli/runtime/lib.rs`'s `BURI_OK`, restated here because the compiler and the
/// runtime are two crates that never link against each other — the archive is
/// `include_bytes!`d, not depended on. The value is `-1` rather than `0` so
/// that an error variant's index is its index, and a backend that gets the
/// sign wrong fails immediately instead of silently reporting the first error
/// arm. `cli/tests/native/runtime.rs`'s C driver holds the two spellings
/// together.
pub const BURI_OK: i32 = -1;

/// One entry in a runtime function's C parameter list.
///
/// `cli/runtime/lib.rs` §2 rule 1: every parameter is a scalar leaf, flattened
/// in declaration order. A `Str` is three parameters and a `[T]` is two.
///
/// Most variants consume one Buri argument and emit its leaves. [`Arg::Stride`],
/// [`Arg::Retain`], [`Arg::Release`] and [`Arg::Equal`] consume **no** Buri
/// argument at all: they are §2 rule 4's "a generic parameter is a pointer and
/// a stride", where the extra words come from `middle::layout` and from the
/// backend's own glue rather than from the call. That is why this is a
/// description of the *C* parameter list walked with a cursor into the Buri
/// one: the two lists have different lengths at every generic entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arg {
    /// `base`, `ptr`, `len` — three parameters.
    Str,
    /// `ptr`, `len` — two. `writeBytes` and `abort` take a byte range without
    /// the owning block, because neither can retain it.
    Bytes,
    /// `ptr`, `len` — two.
    List,
    /// One.
    Scalar,
    /// Zero: a zero-sized `self`, or the operation's **context**, dropped from
    /// the signature (VALUE-MODEL.md §8).
    ///
    /// A context is dropped *whatever it weighs*. `cli/runtime` allocates
    /// through `buri_rt_alloc` and reads no capability (`sources/alloc.buri`'s
    /// header says so), so a `ctx: C` crosses nothing — and which argument that
    /// is is a fact about the **declaration**, which the IR cannot answer:
    /// `list.push(self, ctx, item)` names its second, `list.repeat(ctx, item,
    /// times)` its first. Asking the argument's *type* instead ("is it a
    /// `Ty::Ctx`?") is the same question only while every `C` is instantiated
    /// at a `context { … }` record; a value that *implements* `Allocator`
    /// satisfies `C: Allocator` without being one (SPEC 10.1, 10.8), and one of
    /// those spread to a leaf the C signature has no parameter for and shifted
    /// every argument after it — which links, runs, and dies in `memmove`.
    Dropped,
    /// `ptr`, `len` — two, and the argument's element type is the `T` the
    /// [`Arg::Stride`] and [`Arg::Retain`] that follow it describe. The same two
    /// words [`Arg::List`] emits, kept a separate variant because "which type
    /// is `T`" is the question the generic entries are built around.
    Elems,
    /// One pointer to a stack copy of a Buri argument whose type the runtime
    /// cannot name (§2 rule 4) — `list.push`'s item and `list.repeat`'s. Also
    /// names the element type, which is what makes `list.repeat` — whose only
    /// mention of `T` is the item — work. `host.HostNetwork.fetch`'s `Request`
    /// is the other case: a concrete type with too many words for the
    /// registers.
    Spilled,
    /// `middle::layout`'s element stride, as an immediate. Consumes no Buri
    /// argument.
    Stride,
    /// The per-element retain glue, or null where the element type holds no
    /// counted pointers (`cli/runtime/list.rs`'s header). Consumes no Buri
    /// argument.
    Retain,
    /// The per-value **release** glue, or null where the type holds no counted
    /// pointers. Consumes no Buri argument.
    ///
    /// [`Arg::Retain`]'s mirror, on the rows whose value the runtime *keeps* and
    /// later writes over ([`Extra::Owned`]). Nothing in `core/list` does;
    /// `platform/effect`'s graph does — a signal holds the bytes it was written
    /// until the next write — so the write that replaces them has to let the old
    /// ones go, and only the side that generated the glue knows how.
    Release,
    /// The per-value **equality** glue: `void(frame, a, b, out)`, which writes
    /// a byte through `out` saying whether two values of the type are the same
    /// value. Null where nothing was generated for the type. Consumes no Buri
    /// argument.
    ///
    /// It rides the same rows [`Arg::Release`] does, and answers the question
    /// they ask *before* the store: a signal's rule is that writing a value
    /// equal to the one it holds does nothing, and `==` is structural (SPEC
    /// 7.2) — so the runtime, which has only bytes, cannot decide it. What this
    /// points at is `middle::derives`'s generated comparison behind a C-ABI
    /// thunk (`cli/runtime/ui.rs`'s `Equal`).
    ///
    /// `frame` is a Buri frame the runtime acquired. The LLVM backend's thunk
    /// uses the machine stack and ignores it; the frame-threaded backend runs
    /// the comparison in it.
    Equal,
    // -- the closure trampoline ---------------------------------------------
    /// A **runtime-driven step**: four parameters, from one Buri closure
    /// argument (`backend/intrinsic_keys.rs`'s `step_call`).
    ///
    /// ```text
    ///   entry       the generated C-ABI thunk, `void(state, index, in, out)`
    ///   state       the backend's own record, opaque to the runtime
    ///   in_stride   the source element's stride
    ///   out_stride  the result element's stride
    /// ```
    ///
    /// It consumes the closure and emits none of its words: `{ code, env }` is
    /// the backend's business and reaches the runtime inside `state`. What the
    /// runtime gets is a C function it can call once per element with three
    /// pointers, at every element type there is — [`Arg::Spilled`]'s answer to
    /// "the runtime cannot name `T`" applied to a *call* rather than to a value.
    /// There is no retain glue: the thunk is handed one element at a time and
    /// takes its own count on it.
    ///
    /// Two strides rather than [`Arg::Stride`]'s one because a `map` reads a
    /// `[A]` and writes a `[B]`, and neither is the other's.
    Step,
    /// A **deferred body**: seven parameters, from one Buri closure argument —
    /// a closure the runtime keeps and calls later, rather than during the call
    /// that handed it over.
    ///
    /// ```text
    ///   entry     the generated C-ABI thunk, `void(state, index, in, out)`
    ///   state     the record the backend built, read once and copied
    ///   bytes     how many bytes of it there are
    ///   frame_at  where in the copy to write a working frame, or -1
    ///   stride    how many bytes the body writes through `out`
    ///   release   the release glue for what it writes, or null
    ///   body      the release glue for the record itself, or null
    /// ```
    ///
    /// The thunk is [`Arg::Step`]'s, unchanged: a reactive body is
    /// `fn(Scope) => T`, which is a step of one element whose element is the
    /// scope.
    ///
    /// The other words are what *deferring* costs. A step's record and working
    /// frame are gone when the call returns; a memo runs on the first read and
    /// a watcher on every change. So the runtime **copies** the record and
    /// supplies the frame itself, writing its address at `frame_at` — a number
    /// because the frame-threaded backend's record keeps a frame word and the
    /// LLVM one, on the machine stack, passes `-1`. `stride` and `release` are
    /// [`Arg::Release`]'s pair once more: a memo holds its answer until the next
    /// run replaces it. `body` gives back the count on the closure's
    /// environment, which the graph keeps for the life of the program.
    Compute,
    /// A **walk**: three parameters, from one Buri closure argument — a
    /// `fn(Builder, Node) => ()` the runtime invokes **once**, to walk a whole
    /// tree into the document (`cli/runtime/document.rs`, issue #53).
    ///
    /// ```text
    ///   entry     the generated C-ABI thunk, `void(state, index, in, out)`
    ///   state     the record the backend built — the closure, then a frame
    ///   frame_at  where in it to write a working frame, or -1
    /// ```
    ///
    /// [`Arg::Compute`] with the *keeping* taken out: the walk runs during the
    /// call that handed it over, so the record is used in place rather than
    /// copied, and there is no stride and no release. The thunk is shaped with
    /// the context read out of the record, the builder handle as its index and
    /// the node as its element.
    ///
    /// The runtime writes that context: the document's, before every walk it
    /// drives. A row that drops a context (`mount`) is where the document gets
    /// it, so it writes the context into the record and four more parameters
    /// follow the three:
    ///
    /// ```text
    ///   ctx_at       where in the record the context is
    ///   ctx_bytes    how many bytes it is
    ///   ctx_retain   the retain glue for it, or null
    ///   ctx_release  the release glue for it, or null
    /// ```
    Walk,
    /// A **kept handler**: five parameters, from one Buri closure argument — a
    /// `fn(C, Event) => ()` the runtime keeps on an element and fires later,
    /// when a press or a submit reaches it (`cli/runtime/document.rs`, issue #53
    /// phase 4).
    ///
    /// ```text
    ///   entry     the generated C-ABI thunk, `void(state, index, in, out)`
    ///   state     the record the backend built, read once and copied
    ///   bytes     how many bytes of it there are
    ///   frame_at  where in the copy to write a working frame, or -1
    ///   body      the release glue for the record itself, or null
    /// ```
    ///
    /// [`Arg::Compute`] with the value taken out — no stride and no release,
    /// because a handler answers `()` — but kept, like a body and unlike a walk.
    /// The thunk drops the context and reads the event as its element.
    Press,
}

impl Arg {
    /// How many C parameters this shape emits.
    pub fn leaves(self) -> usize {
        match self {
            Compute => 7,
            Press => 5,
            Step => 4,
            Str | Walk => 3,
            Bytes | List | Elems => 2,
            Scalar | Spilled | Stride | Retain | Release | Equal => 1,
            Dropped => 0,
        }
    }

    /// Whether this shape takes the next Buri argument. The shapes the backend
    /// supplies for itself do not.
    pub fn consumes(self) -> bool {
        !matches!(self, Stride | Retain | Release | Equal)
    }
}

/// What comes back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ret {
    /// Nothing, and the Buri result is `()`.
    Void,
    /// One scalar, at the Buri result's own register shape.
    Scalar,
    /// Nothing, and the call does not come back (SPEC 6.9).
    NoReturn,
    /// One integer of exactly this many bits, which is **not** the
    /// destination's register shape and is narrowed to it at the call site.
    ///
    /// The C boundary has no `i1` and no `i8` enum tag: `buri_rt_str_equal`
    /// returns `u8` and `buri_rt_str_compare` returns `i32`, while `Bool` is an
    /// `i1` and `Order` is whatever width `middle::layout` gave a three-variant
    /// bare tag. Declaring the import at the destination's width instead would
    /// work on both supported platforms by accident — the low byte of `eax` is
    /// the low byte of the value — and would be wrong the first time a target
    /// returned a narrow integer unextended.
    Int(u32),
    /// An aggregate written through a trailing out-pointer (§2 rule 2).
    ///
    /// The pointer is the destination's own slot, so the call writes the value
    /// where it already belongs. `BuriStr` and `BuriList` are `#[repr(C)]`
    /// records of exactly the words `middle::layout` gives `Str` and `[T]`. A
    /// zero-sized result has no out-pointer.
    Out,
    /// An `Option<T>`: an `i32` discriminant, and the payload through a
    /// trailing out-pointer (§2 rule 3).
    ///
    /// [`BURI_OK`] is the success arm and `0` is `.None`. The out-pointer is
    /// the destination **offset to `.Some`'s payload**, so the runtime writes
    /// the payload in place and the backend only has to settle the
    /// discriminant — a tag store for `EnumRepr::Tagged`, nothing at all for
    /// `EnumRepr::Niche`. The runtime never learns whether `middle::layout`
    /// chose a tag or a niche, which is exactly what rule 3 is protecting.
    Sum,
    /// [`Ret::Res`], and the entry **also writes `E`'s message** through one
    /// more trailing out-pointer (`lib.rs` §2.1's message shape).
    ///
    /// A column rather than a fact read off `E`: whether an enum error is
    /// *named by an index* is a property of the type, and whether an entry has
    /// anything to say when it names the payload-carrying one is a property of
    /// the **implementation**. `buri_rt_host_file_system_read_file` and
    /// `buri_rt_host_testing_fs_read_file` answer the same
    /// `Result<Str, IoError>` and have different C signatures, because the
    /// first can meet an `EISDIR` and the second is a map in memory.
    ///
    /// *Where* the message goes is still the type's business:
    /// `runtime_native::error_message_offset` reads it off `E`'s layout, and a
    /// row that claims a message for an `E` with nowhere to put one is emitted
    /// as a plain [`Ret::Res`].
    ///
    /// **The stream writers are deliberately not this.** `HostStdout.println`
    /// can meet an `EPIPE`, but the message out-pointer is an address *into the
    /// destination*, so a function that prints would stop keeping its `Result`
    /// in registers (`native/llvm.rs`'s `a_hot_function_has_no_allocas`).
    /// Printing is the hot path and a stream failure's actionable half is the
    /// variant; a filesystem failure's is often only in the string, because
    /// `ENOTEMPTY` and `EISDIR` have no variant at all.
    ResMsg,
    /// A `Result<T, E>`: an `i32` discriminant, `.Ok`'s payload through a
    /// trailing out-pointer, and an error variant **named by its index**
    /// (`lib.rs` §2.1).
    ///
    /// [`Ret::Sum`] with the failure side carrying information: the
    /// discriminant `0 ..= n` says which variant of `E` failed, of a variant
    /// §2.1 restricts to carrying no fields, so the tag is the whole of it. An
    /// `E` that is not an enum — `bytes.fromUtf8`'s `Utf8Error(Int)` — crosses
    /// whole through a second out-pointer instead.
    ///
    /// The out-pointer is **omitted where `T` is zero-sized**
    /// (`TestFileSystem.writeFile`'s `Result<(), IoError>`), for the reason
    /// [`Ret::Out`] omits it.
    Res,
}

/// What the frame-threaded backend appends after the flattened Buri
/// arguments, read off [`Entry::args`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Extra {
    /// Nothing.
    None,
    /// The element **stride** and **retain glue** ([`Arg::Stride`],
    /// [`Arg::Retain`]).
    Element,
    /// [`Extra::Element`]'s pair with a **release** and an **equality** after
    /// it ([`Arg::Release`], [`Arg::Equal`]): a store the runtime keeps and
    /// later writes over.
    Owned,
    /// [`Arg::Step`]'s four words.
    Step,
    /// [`Arg::Compute`]'s seven words.
    Compute,
    /// [`Arg::Walk`]'s three words.
    Walk,
    /// [`Arg::Press`]'s five words.
    Press,
}

/// One runtime entry both backends can emit a call to.
pub struct Entry {
    /// The intrinsic key `monomorphize` built: `str.slice`, `host.HostStdout.println`.
    pub key: &'static str,
    /// One per Buri parameter, in declaration order, *including* the zero-sized
    /// `self` — so the list can be checked against the IR signature directly —
    /// with the shapes that consume no argument inserted where the C parameter
    /// list has them.
    pub args: &'static [Arg],
    pub ret: Ret,
    /// Whether a generic row's `T` is the **whole value** the call carries
    /// rather than a `[T]`'s element: `platform/effect`'s graph, where a signal
    /// holds one `T` and `T` may be a list like any other.
    ///
    /// A column rather than something a backend works out from the argument
    /// types, because the two readings are indistinguishable in the IR the
    /// moment `T` is *itself* a list. `list.push`'s `[T]` and `Ui.signal`'s `T`
    /// are both an array-typed argument, and a backend that looked for the
    /// first one it could find gave `Signal<[Account]>` the stride and glue of
    /// an `Account` — a store of the wrong width, retained by the wrong walk,
    /// with nothing to say so until the allocator tripped over it at exit.
    pub whole_value: bool,
    /// The key whose archive body the row calls: its own, or another row's
    /// whose C signature is the same ([`via`]).
    pub runs: &'static str,
}

impl Entry {
    /// The exported symbol, per `cli/runtime/lib.rs` §1.
    pub fn symbol(&self) -> String {
        crate::compiler::backend::runtime_native::symbol_for(self.runs)
    }

    /// The shapes of the Buri arguments, one per argument.
    fn consumed(&self) -> impl Iterator<Item = Arg> + '_ {
        self.args.iter().copied().filter(|a| a.consumes())
    }

    /// Whether Buri argument `i` is dropped from the C call ([`Arg::Dropped`]).
    pub fn dropped(&self, i: usize) -> bool {
        self.consumed().nth(i) == Some(Dropped)
    }

    /// The index of the Buri argument passed **by address** ([`Arg::Spilled`]).
    pub fn by_ref(&self) -> Option<usize> {
        self.consumed().position(|a| a == Spilled)
    }

    /// What the frame-threaded backend appends after the flattened arguments.
    pub fn extra(&self) -> Extra {
        let has = |a| self.args.contains(&a);
        if has(Step) {
            Extra::Step
        } else if has(Compute) {
            Extra::Compute
        } else if has(Walk) {
            Extra::Walk
        } else if has(Press) {
            Extra::Press
        } else if has(Release) {
            Extra::Owned
        } else if has(Stride) {
            Extra::Element
        } else {
            Extra::None
        }
    }
}

const fn e(key: &'static str, args: &'static [Arg], ret: Ret) -> Entry {
    Entry { key, args, ret, whole_value: false, runs: key }
}

/// The same row, calling `runs`'s body: one the archive already has under
/// another key, whose arguments flatten the same way.
const fn via(runs: &'static str, entry: Entry) -> Entry {
    Entry { runs, ..entry }
}

/// The same row, carrying **one whole value** rather than a `[T]`'s element
/// ([`Entry::whole_value`]).
const fn v(entry: Entry) -> Entry {
    Entry { whole_value: true, ..entry }
}

/// Every key the archive has a body for, which both backends call.
///
/// Grouped by the module the key names, and in each group by the order
/// `core/<module>` declares them, so that a reader comparing this against
/// `str.buri` or `list.buri` can see at a glance what is absent.
///
/// What is deliberately absent, and why, so that a reader looking for one of
/// these finds the reason rather than an absence:
///
///  * **`str.concat`, `str.format`, `str.length`, `list.length`, `list.empty`.**
///    Open-coded by both backends: an allocation and two copies, a no-op, a
///    masked load, a word the backend already has the address of, and two
///    immediates. `str.concat` is the one whose symbol the archive does export
///    — the frame-threaded backend calls it from the one site that knows its
///    two lengths go **unmasked** (`stencil/rtcall.rs`'s `str_concat`), because
///    VALUE-MODEL.md §3.1's ASCII flag is an input to a concatenation rather
///    than a tag, and the flattening this table drives masks every length.
///  * **`json.*`, and every `list.*` entry taking a closure.**
///    `cli/runtime/list.rs`'s header states why they are not in the archive: a
///    Buri closure's `code` is a thunk at the *flattened* signature of its own
///    element type, so calling one from C would mean synthesizing a parameter
///    list that depends on `T`. Both backends open-code them as loops.
pub const ENTRIES: &[Entry] = &[
    // -- core/str, pure -----------------------------------------------------
    //
    // Every one of these answers a *view* into the receiver's block and increfs
    // its base before doing so (`cli/runtime/text.rs`'s header). That is what
    // makes `slice`, `trim` and `splitOnce` allocation-free, which is what
    // `str.buri:26-45` says by declaring them without an `Allocator` bound.
    e("str.charAt", &[Str, Scalar], Ret::Sum),
    e("str.slice", &[Str, Scalar, Scalar], Ret::Out),
    e("str.trim", &[Str], Ret::Out),
    e("str.trimStart", &[Str], Ret::Out),
    e("str.trimEnd", &[Str], Ret::Out),
    e("str.startsWith", &[Str, Str], Ret::Int(8)),
    e("str.endsWith", &[Str, Str], Ret::Int(8)),
    e("str.contains", &[Str, Str], Ret::Int(8)),
    e("str.indexOf", &[Str, Str], Ret::Sum),
    e("str.splitOnce", &[Str, Str], Ret::Sum),
    e("str.compare", &[Str, Str], Ret::Int(32)),
    e("str.equal", &[Str, Str], Ret::Int(8)),
    e("str.hash", &[Str], Ret::Scalar),
    e("str.toInt", &[Str], Ret::Sum),
    e("str.toFloat", &[Str], Ret::Sum),
    e("str.utf8Length", &[Str], Ret::Scalar),
    // -- core/str, `Allocator`-bounded ------------------------------------------
    e("str.split", &[Str, Dropped, Str], Ret::Out),
    e("str.splitAny", &[Str, Dropped, Str], Ret::Out),
    e("str.lines", &[Str, Dropped], Ret::Out),
    e("str.replace", &[Str, Dropped, Str, Str], Ret::Out),
    e("str.repeat", &[Str, Dropped, Scalar], Ret::Out),
    e("str.toUpper", &[Str, Dropped], Ret::Out),
    e("str.toLower", &[Str, Dropped], Ret::Out),
    e("str.chars", &[Str, Dropped], Ret::Out),
    e("str.fromChars", &[Dropped, List], Ret::Out),
    e("str.fromInt", &[Dropped, Scalar], Ret::Out),
    e("str.fromFloat", &[Dropped, Scalar], Ret::Out),
    e("str.padStart", &[Str, Dropped, Scalar, Scalar], Ret::Out),
    e("str.padEnd", &[Str, Dropped, Scalar, Scalar], Ret::Out),
    // -- core/list ----------------------------------------------------------
    //
    // `len` is open-coded (it is a load) and every entry taking a closure is
    // absent; `cli/runtime/list.rs`'s header says which and why.
    e("list.get", &[Elems, Scalar, Stride, Retain], Ret::Sum),
    e("list.concat", &[Elems, Dropped, Elems, Stride, Retain], Ret::Out),
    // `push(self, ctx, item)` — the item is a `T`, so it goes by address.
    e("list.push", &[Elems, Dropped, Spilled, Stride, Retain], Ret::Out),
    e("list.reverse", &[Elems, Dropped, Stride, Retain], Ret::Out),
    e("list.slice", &[Elems, Dropped, Scalar, Scalar, Stride, Retain], Ret::Out),
    e("list.take", &[Elems, Dropped, Scalar, Stride, Retain], Ret::Out),
    e("list.drop", &[Elems, Dropped, Scalar, Stride, Retain], Ret::Out),
    // `insertAt(self, ctx, index, item)` and its two siblings: `core/map`'s
    // splices with the list first, which flatten to the same C row. They own
    // the list, as `core/map`'s do.
    via("map.insertAt", e("list.insertAt", &[Elems, Dropped, Scalar, Spilled, Stride, Retain, Release, Equal], Ret::Out)),
    via("map.replaceAt", e("list.replaceAt", &[Elems, Dropped, Scalar, Spilled, Stride, Retain, Release, Equal], Ret::Out)),
    via("map.removeAt", e("list.removeAt", &[Elems, Dropped, Scalar, Stride, Retain, Release, Equal], Ret::Out)),
    // `repeat(ctx, item, times)` — likewise, one place earlier.
    e("list.repeat", &[Dropped, Spilled, Scalar, Stride, Retain], Ret::Out),
    e("list.range", &[Dropped, Scalar, Scalar], Ret::Out),
    e("list.join", &[List, Dropped, Str], Ret::Out),
    // `[` + already-rendered elements joined by `, ` + `]`, for
    // `deriveArrayShow`, which `middle::lower` builds as a loop that renders
    // each element and then calls this. No element descriptor: the block is a
    // `[Str]` at every instantiation. No source names the key.
    e("show.list", &[List], Ret::Out),
    // -- core/map -----------------------------------------------------------
    //
    // A node's children, spliced. These own the list (`cli/runtime/splice.rs`),
    // so they carry the element's release for what a splice writes over.
    e("map.insertAt", &[Dropped, Elems, Scalar, Spilled, Stride, Retain, Release, Equal], Ret::Out),
    e("map.replaceAt", &[Dropped, Elems, Scalar, Spilled, Stride, Retain, Release, Equal], Ret::Out),
    e("map.removeAt", &[Dropped, Elems, Scalar, Stride, Retain, Release, Equal], Ret::Out),
    // -- core/bytes ---------------------------------------------------------
    //
    // Six of `bytes.buri`'s surface, and the rest of that module is Buri:
    // hexadecimal, base64, varints and zigzag are arithmetic over a `[U8]`.
    // These six are the two conversions whose answer is the platform's
    // representation — the UTF-8 encoding of a string, and the IEEE 754 byte
    // pattern of a `Float`.
    //
    // `Extra::None` at every one of them, including the three answering a
    // `[U8]`: the element type is fixed at `U8`, so there is no `T` for the
    // stride-and-glue pair of `lib.rs` §2 rule 4 to describe, and
    // `cli/runtime/value.rs`'s `list_of_bytes` knows the stride is one.
    e("bytes.toUtf8", &[Dropped, Str], Ret::Out),
    // `Result<Str, Utf8Error>` — §2.1's *second* error shape. `Utf8Error(Int)`
    // is a struct, so there is no variant index to name it with and the value
    // crosses through its own out-pointer.
    e("bytes.fromUtf8", &[Dropped, List], Ret::Res),
    e("bytes.f64ToBytes", &[Dropped, Scalar], Ret::Out),
    e("bytes.f64FromBytes", &[List, Scalar], Ret::Sum),
    e("bytes.f32ToBytes", &[Dropped, Scalar], Ret::Out),
    e("bytes.f32FromBytes", &[List, Scalar], Ret::Sum),
    // -- core/character -----------------------------------------------------
    //
    // Eight of `character.buri`'s nine. `toU32` is the ninth and is not here:
    // a `Char` **is** a `U32`, so it is a representation change the backend
    // open-codes, and `isAlphanumeric` is written in Buri over two of these.
    //
    // Every one of them is one comparison or one table lookup, and a call is
    // more instructions than the answer — which is the argument for open-coding
    // that `str.concat` wins and this loses. `isAlpha` is a binary search over
    // six hundred ranges of Unicode data, `isUpper` is two full case mappings,
    // and open-coding *those* in two backends is two places for the data to
    // drift. So all eight go through the archive together rather than four of
    // them here and four there, and `cli/runtime/character.rs` is the one
    // place the answers live.
    e("character.isDigit", &[Scalar], Ret::Int(8)),
    e("character.isAlpha", &[Scalar], Ret::Int(8)),
    e("character.isSpace", &[Scalar], Ret::Int(8)),
    e("character.isUpper", &[Scalar], Ret::Int(8)),
    e("character.isLower", &[Scalar], Ret::Int(8)),
    e("character.toUpper", &[Scalar], Ret::Scalar),
    e("character.toLower", &[Scalar], Ret::Scalar),
    e("character.toDigit", &[Scalar, Scalar], Ret::Sum),
    // -- core/crypto --------------------------------------------------------
    //
    // Sealing and the three signature checks, in `cli/runtime/crypto.rs` behind
    // the `crypto` feature (`runtime_native::crypto_intrinsic`). Every argument
    // is a `[U8]`, so `core/bytes`'s `Extra::None` reasoning holds.
    e("crypto.chacha20Poly1305Seal", &[Dropped, List, List, List, List], Ret::Out),
    e("crypto.chacha20Poly1305Open", &[Dropped, List, List, List, List], Ret::Sum),
    e("crypto.ecdsaP256Sha256Verify", &[List, List, List], Ret::Int(8)),
    e("crypto.ed25519Verify", &[List, List, List], Ret::Int(8)),
    e("crypto.rsaPkcs1Sha256Verify", &[List, List, List, List], Ret::Int(8)),
    // -- core/math, the exactly-specified half --------------------------------
    //
    // Nine of twenty-two. `cli/runtime/math.rs` says why the other thirteen are
    // not here, and the short version is that IEEE 754 does not fix a
    // transcendental's answer, so V8 and the platform libm differ in the last
    // bit — which a rendered `Float` shows.
    e("math.squareRoot", &[Scalar], Ret::Scalar),
    e("math.absoluteFloat", &[Scalar], Ret::Scalar),
    e("math.floor", &[Scalar], Ret::Scalar),
    e("math.ceiling", &[Scalar], Ret::Scalar),
    e("math.truncate", &[Scalar], Ret::Scalar),
    e("math.round", &[Scalar], Ret::Scalar),
    e("math.isNan", &[Scalar], Ret::Int(8)),
    e("math.isInfinite", &[Scalar], Ret::Int(8)),
    e("math.isFinite", &[Scalar], Ret::Int(8)),
    // -- the text streams ---------------------------------------------------
    //
    // `Result<(), IoError>` on all five, which is [`Ret::Res`] with the
    // out-pointer omitted — `()` occupies no bytes — so the C signature is the
    // arguments, a trailing `out_err`, and an `i32` discriminant. The same
    // shape the filesystem's writers have, and for the same reason: a stream a program
    // cannot write to is a failure the program can act on, and a signature
    // saying `()` was claiming otherwise.
    e("host.HostStdout.print", &[Dropped, Str], Ret::Res),
    e("host.HostStdout.println", &[Dropped, Str], Ret::Res),
    e("host.HostStdout.writeBytes", &[Dropped, List], Ret::Res),
    e("host.HostStderr.eprint", &[Dropped, Str], Ret::Res),
    e("host.HostStderr.eprintln", &[Dropped, Str], Ret::Res),
    // -- the filesystem, both halves of it ------------------------------------
    //
    // Ten operations, and until they landed the native backend had none of
    // them: a binary that bound the filesystem was refused before code
    // generation, one key at a time, while `cli/runtime/host.rs` had a body for
    // every one (buri-lang/buri#36). What was missing was never the body and
    // never the shape of the *arguments* — it was the shape of the **error**.
    //
    // All ten answer `Result<T, IoError>`, and `IoError`'s seventh variant is
    // `Other(Str)`: the one a real filesystem answers for every kind the other
    // six do not name. `lib.rs` §2.1 restricted the variant the discriminant
    // names to carrying no fields, so a row here would have made every
    // unclassified failure `.Other("")` — a failure that says nothing, where
    // the JavaScript backend says `EISDIR`. §2.1's message shape is what these
    // rows wait on, and it is split between the two places it belongs: *where*
    // the message goes is read off `IoError`'s layout by
    // `runtime_native::error_message_offset`, and *whether an entry has one* is
    // [`Ret::ResMsg`], the column below — which is why these eleven carry it and
    // the five stream writers above deliberately do not.
    //
    // `self` is `HostFileSystem`, an empty struct, so it flattens to nothing and no row
    // here drops a context: neither `FileSystemRead` nor `FileSystemWrite` declares a
    // context parameter, and the allocation these do is `buri_rt_alloc`'s.
    //
    // **One host type for two effects**, which is what keeps these keys — and
    // therefore `lib.rs` §1's symbol rule — to one family. A
    // `HostFileSystemRead` would mangle `readFile` to
    // `buri_rt_host_file_system_read_read_file`, and the filesystem being two
    // grants is a fact about a *context* rather than about the platform, which
    // has one.
    //
    // **A `Path` argument is the same three C parameters a `Str` was**, and
    // that is rule 1 of `lib.rs` §2 rather than a coincidence: a parameter is
    // flattened into its scalar leaves, and a one-field struct wrapping a `Str`
    // has the `Str`'s three. So the split of `FileSystem` into `FileSystemRead` and `FileSystemWrite`
    // and the move from `Str` to `Path` changed the *keys* in this column and
    // not one symbol or one signature in `cli/runtime/host.rs`.
    //
    // `fileExists` is the one that is not a `Result` — it answers `Bool` and
    // cannot fail — which is why it sits with the scalars below and not here.
    e("host.HostFileSystem.readFile", &[Dropped, Str], Ret::ResMsg),
    e("host.HostFileSystem.readDir", &[Dropped, Str], Ret::ResMsg),
    e("host.HostFileSystem.readFileBytes", &[Dropped, Str], Ret::ResMsg),
    e("host.HostFileSystem.writeFile", &[Dropped, Str, Str], Ret::ResMsg),
    e("host.HostFileSystem.writeFileBytes", &[Dropped, Str, List], Ret::ResMsg),
    e("host.HostFileSystem.appendFile", &[Dropped, Str, List], Ret::ResMsg),
    e("host.HostFileSystem.renameFile", &[Dropped, Str, Str], Ret::ResMsg),
    e("host.HostFileSystem.removeFile", &[Dropped, Str], Ret::ResMsg),
    e("host.HostFileSystem.removeDir", &[Dropped, Str], Ret::ResMsg),
    e("host.HostFileSystem.makeDir", &[Dropped, Str], Ret::ResMsg),
    e("host.HostFileSystem.syncFile", &[Dropped, Str], Ret::ResMsg),
    // `metadata`'s `.Ok` is a **struct** rather than a `Str` or a list, which
    // costs no column: `Ret::Out`'s pointer is the destination's own slot, so
    // the entry writes `Metadata`'s three fields where they already belong and
    // `cli/runtime/host.rs`'s `BuriMetadata` is the layout transcribed —
    // `net.rs`'s `BuriRequest` one level down.
    e("host.HostFileSystem.metadata", &[Dropped, Str], Ret::ResMsg),
    e("host.HostFileSystem.readRange", &[Dropped, Str, Scalar, Scalar], Ret::ResMsg),
    e("host.HostFileSystem.realPath", &[Dropped, Str], Ret::ResMsg),
    e("host.HostFileSystem.copyFile", &[Dropped, Str, Str], Ret::ResMsg),
    // -- Env, and Stdin beside it -------------------------------------------
    //
    // Four rows and no new shape between them, which is what made them the
    // other half of the same gap: `variable` is an `Option<Str>` and `args` a
    // `[Str]`, `readLine` an `Option<Str>` and `readBytes` an `Option<[U8]>` —
    // all four expressible by the `Ret::Opt` and `Ret::Out` this table has had
    // since it was written. They were absent because no slice had wired the
    // host surface up, and a program cannot read its own arguments without
    // them.
    //
    // `self` is empty at all four, so the C call of `args` is the out-pointer
    // and nothing else.
    e("host.HostEnvironment.variable", &[Dropped, Str], Ret::Sum),
    e("host.HostEnvironment.arguments", &[Dropped], Ret::Out),
    // Three more of the same two shapes: two `Str`s and a `[(Str, Str)]`,
    // which is `[Header]`'s layout and so is `list_of_headers`' block.
    e("host.HostEnvironment.currentDirectory", &[Dropped], Ret::Out),
    e("host.HostEnvironment.allVariables", &[Dropped], Ret::Out),
    e("host.HostEnvironment.operatingSystemName", &[Dropped], Ret::Out),
    // Starting a program. `self` is `HostSpawn`, an empty struct, so the C call
    // is a `Command` encoded into four flat arguments — two `[Str]`s, a `Bool`
    // and a `[U8]`, which is nine leaves with the two out-pointers and so
    // inside `backend/stencil/abi.rs`'s register budget. `Output`'s three
    // fields leave through the one out-pointer `Ret::ResMsg` gives, as
    // `Metadata` does.
    e("host.HostSpawn.spawnProcess", &[Dropped, List, List, Scalar, List], Ret::ResMsg),
    e("host.HostStdin.readLine", &[Dropped], Ret::Sum),
    e("host.HostStdin.readBytes", &[Dropped, Scalar], Ret::Sum),
    // -- the scalar capabilities --------------------------------------------
    // `Tcp`'s four. A dial answers a handle and a read answers octets, both
    // `Result<_, IoError>` with `.Other(Str)` on the error side — so both are
    // `Ret::ResMsg`, exactly as `host.HostFileSystem`'s eleven are and for the same
    // reason: a socket meets failures `IoError` has no variant for, and the
    // sentence is the only actionable half of one. `tcpWrite`'s `.Ok` is `()`
    // and so has no payload out-pointer, and `tcpClose` answers nothing at all.
    e("host.HostTcp.tcpConnect", &[Dropped, Str, Scalar], Ret::ResMsg),
    e("host.HostTcp.tcpRead", &[Dropped, Scalar, Scalar], Ret::ResMsg),
    e("host.HostTcp.tcpWrite", &[Dropped, Scalar, List], Ret::ResMsg),
    e("host.HostTcp.tcpClose", &[Dropped, Scalar], Ret::Void),
    // `Request` goes by address: its nine words plus two out-pointers would not
    // fit the stencil backend's ten argument registers. `NetError`'s `BadUrl`
    // and `Transport` carry the sentence `Ret::ResMsg` gives a place to.
    e("host.HostNetwork.fetch", &[Dropped, Spilled], Ret::ResMsg),
    e("host.HostFileSystem.fileExists", &[Dropped, Str], Ret::Int(8)),
    e("host.HostClock.nowMilliseconds", &[Dropped], Ret::Scalar),
    e("host.HostClock.sleepMilliseconds", &[Dropped, Scalar], Ret::Void),
    // A reading off a clock that only goes forward, in nanoseconds. `Ret::Scalar`
    // like `nowMilliseconds` and for the same reason: one `i64` out, nothing in but the
    // dropped `self`.
    e("host.HostClock.monotonicNanoseconds", &[Dropped], Ret::Scalar),
    e("host.HostRandom.nextInt", &[Dropped, Scalar, Scalar], Ret::Scalar),
    e("host.HostRandom.nextFloat", &[Dropped], Ret::Scalar),
    // The one row here whose symbol may not be in the archive: it is behind the
    // runtime's `crypto` feature, and `runtime_native::crypto_intrinsic` is
    // what turns a toolchain built without it into a refusal naming the
    // operation rather than a link error naming this symbol. A row here is a
    // claim that the *runtime* implements the key, which it does; whether this
    // toolchain's copy carries it is the feature file's question and is asked
    // separately, exactly as `host.HostListen.*` is.
    e("host.HostEntropy.bytes", &[Dropped, Scalar], Ret::Out),
    e("host.HostProcess.exitWith", &[Dropped, Scalar], Ret::NoReturn),
    // `allocate(self, bytes) -> Region`. `self` is `HostAllocator`, an empty
    // struct, so it flattens to nothing and the C call is the one `i64`; the
    // result is `struct Region(I64)`, whose single leaf is what makes
    // [`Ret::Scalar`] right where `host_testing.stdout`'s
    // `struct TestStdout(I64)` needs [`Ret::Out`] — the difference is the *C*
    // signature, and `buri_rt_host_allocator_allocate` returns an `i64` rather
    // than a struct.
    //
    // MEMORY.md §7 is the body: `HostAllocator` is zero-sized and unbounded, so
    // the charge is the request and the accounting is the caller's. The row is
    // here rather than open-coded next to `TestAllocator.allocate` because the
    // archive already has the body and the LLVM backend already calls it — two
    // backends reaching one definition of a *defined* cost model, which is what
    // §7.1 means by "the same number on both backends".
    e("host.HostAllocator.allocate", &[Dropped, Scalar], Ret::Scalar),
    // -- Tasks --------------------------------------------------------------
    //
    // `parallel(self, ctx, items, f)`. `self` is `HostTasks`, an empty struct,
    // so it flattens to nothing; `ctx` is the caller's whole context and is the
    // one this row drops at index 1, because it is dropped from the C call
    // and read into the step's state record instead; `items` is the `[A]` the
    // runtime walks, which is what the strides of [`Extra::Step`] describe; `f`
    // crosses as the entry thunk and the state record rather than as
    // `{ code, env }`.
    //
    // Two of the four arguments carry no bytes across and they are dropped for
    // different reasons: `self` because it is empty, `ctx` because the runtime
    // reads no capability. Only the second is a rule — a `TestTasks` receiver is
    // a live handle and crosses — which is why the row drops an index rather
    // than a width.
    //
    // The body is in `cli/runtime/rt.rs` behind feature `net`, which is why
    // `runtime_native::net_intrinsic` names the `host.HostTasks.*` family: a
    // toolchain built without the reactor refuses this key with a sentence
    // before code generation rather than with a missing symbol from `cc`.
    e("host.HostTasks.parallel", &[Dropped, Dropped, Elems, Step], Ret::Out),
    // -- Listen, and Sockets beside it --------------------------------------
    //
    // Seven operations and no closure among them: the accept loop is
    // `core/net/server`'s, in Buri, so nothing here is runtime-driven and none
    // of these rows is an [`Extra::Step`]. A socket's loop and a socket's
    // state are Buri's too, which is why F7 cost two more ordinary rows and no
    // shape at all. That is the whole reason `Listen`
    // costs seven ordinary rows where `Tasks` costs one with a trampoline
    // behind it — and it is why F3 could put a *worker per handler* on the
    // thread pool without touching the trampoline at all: the fan-out is
    // `Tasks.parallel`'s, one row up, and these seven neither know nor care how
    // many callers they have.
    //
    // Six of them are `Result<_, ServeError>`, and `ServeError` is a
    // **struct** — so those take §2.1's *second* shape: the error crosses whole
    // through an out-pointer of its own and the discriminant says only that it
    // failed. `bytes.fromUtf8` is the other row with that shape, and
    // `NetError`'s payload-carrying variants are why `ServeError` was declared
    // a struct rather than a ninth and tenth `NetError` variant.
    //
    // **`listenAccept` and `listenRequest` are two rows and not one** because
    // only the first of them waits: the accept is where a server spends its idle
    // life and the read is of something the acceptor is already holding.
    // `effect Listen` carries the argument.
    //
    // `self` is `HostListen`, an empty struct, so it flattens to nothing and
    // no row here drops a context: none of the seven takes a context.
    // `listenRespond`'s `Response` flattens into its leaves by §2 rule 1, and
    // `Listener` and `Request` come back through an out-pointer whole.
    //
    // The bodies are in `cli/runtime/net.rs`, which is why
    // `runtime_native::net_intrinsic` names the `host.HostListen.*` family: a
    // toolchain built without the network refuses these keys with a sentence
    // before code generation rather than with a missing symbol from `cc`.
    e("host.HostListen.listenBind", &[Dropped, Str, Scalar, List, Scalar, Scalar], Ret::Res),
    e("host.HostListen.listenAccept", &[Dropped, Scalar], Ret::Res),
    e("host.HostListen.listenRequest", &[Dropped, Scalar], Ret::Res),
    e("host.HostListen.listenRespond", &[Dropped, Scalar, Scalar, List, List], Ret::Res),
    e("host.HostListen.listenClose", &[Dropped, Scalar], Ret::Void),
    // The two that turn a connection into a socket and then read it.
    // `listenUpgrade` answers a bare `Int` — the socket — so its `.Ok` is a
    // scalar out-pointer like `listenAccept`'s; `listenReceive` answers a
    // `Received`, which is a struct and comes back whole, exactly as `Request`
    // does one row up. Neither is a closure either: the socket's loop is
    // `core/net/server`'s in Buri, and a socket's state never crosses.
    e("host.HostListen.listenUpgrade", &[Dropped, Scalar], Ret::Res),
    e("host.HostListen.listenReceive", &[Dropped, Scalar], Ret::Res),
    // The socket half. `()` on all three, because a frame is enqueued rather
    // than delivered and "did this arrive" was never answerable — the message
    // goes to the socket's own outbound buffer and leaves when that socket's
    // worker next pumps it. A handle naming no open socket is one that has
    // already gone, which is the same answer, so the three are total.
    e("host.HostSockets.socketSendText", &[Dropped, Scalar, Str], Ret::Void),
    e("host.HostSockets.socketSendBytes", &[Dropped, Scalar, List], Ret::Void),
    e("host.HostSockets.socketPing", &[Dropped, Scalar], Ret::Void),
    e("host.HostSockets.socketClose", &[Dropped, Scalar, Scalar, Str], Ret::Void),
    // -- WebSocketClient, the other way to come by a socket -----------------
    //
    // Two rows, and they are `listenUpgrade` and `listenReceive` from the
    // client's end. `connectSocket` answers a `Connected` — a struct, so §2.1's
    // second shape again: the socket and the `101`'s three fields come back
    // through the out-pointer whole and `core/net/websocket` builds the
    // `Response` a program wanted. `connectReceive` answers the *same*
    // `Received` a server's socket does, because the socket a client dialled
    // and the socket a server accepted are one value in one table in
    // `cli/runtime/net.rs` — which is also why the three `Sockets` rows above
    // needed nothing added to write on a client socket.
    //
    // `self` is `HostWebSocketClient`, an empty struct, so it flattens to
    // nothing; the URL flattens to its three `Str` leaves by §2 rule 1.
    e("host.HostWebSocketClient.connectSocket", &[Dropped, Str], Ret::Res),
    e("host.HostWebSocketClient.connectReceive", &[Dropped, Scalar], Ret::Res),
    // -- core/alloc's counters ----------------------------------------------
    //
    // Four scalars in, one scalar out, and no context anywhere in them: the
    // handle *is* the allocator here, so these are the one part of the `Allocator`
    // story that needs no argument this ABI drops (`runtime_call`, above).
    // `charge` is the one that can end the process, and it is `Ret::Scalar`
    // rather than `Ret::NoReturn` because it returns on every request that
    // fits.
    e("alloc.newCounter", &[Scalar], Ret::Scalar),
    e("alloc.charge", &[Scalar, Scalar], Ret::Scalar),
    e("alloc.count", &[Scalar], Ret::Scalar),
    e("alloc.total", &[Scalar], Ret::Scalar),
    // -- `core/alloc`'s scope (G4) ------------------------------------------
    //
    // The same shape as the four counters above and for the same reason: the
    // handle *is* the arena, so no context reaches these and there is nothing
    // for this ABI to drop. `arenaRelease` answers the bytes it gave back
    // rather than `()`, because a scalar out is the row this table has and
    // `scoped` discards it.
    e("alloc.arenaCreate", &[], Ret::Scalar),
    e("alloc.arenaAllocate", &[Scalar, Scalar], Ret::Scalar),
    e("alloc.arenaRelease", &[Scalar], Ret::Scalar),
    e("alloc.arenaCount", &[Scalar], Ret::Scalar),
    e("alloc.arenaTotal", &[Scalar], Ret::Scalar),
    // G5's pair: the arena the platform allocator serves out of, for this
    // thread and for the dynamic extent of `scoped`'s body. `arenaEnter`
    // answers the arena that was active before, so nesting is the caller's
    // local and not a stack in the runtime.
    e("alloc.arenaEnter", &[Scalar], Ret::Scalar),
    e("alloc.arenaLeave", &[Scalar], Ret::Scalar),
    // -- core/actor's mailbox, state and reply slots (F6) --------------------
    //
    // Nine rows, and not one of them carries a stride, a glue or a descriptor
    // — which is the whole reason `core/actor` is shaped the way it is. Every
    // value that crosses is a **one-element `[T]`**, and a `[T]` is `ptr` and
    // `len` whatever `T` is (VALUE-MODEL.md §4), so the message, the state and
    // the answer are two words each and the runtime holds them without ever
    // learning what is inside. That is a *fifth* way an erased type can be
    // carried, beside `Extra::Element`'s stride, `Entry::by_ref`'s address,
    // `Func::desc`'s descriptor and `Extra::Step`'s thunk, and
    // `middle/monomorphize.rs`'s `GENERIC_INTRINSICS` is where it is argued.
    //
    // The context is argument `0` on every row: `core/actor` declares these as
    // `fn <name><C: Tasks, …>(ctx: C, …)` — a module function with the
    // authority in its bound, `core/list`'s shape — so argument 0 is the
    // context and the C signature has no parameter for it.
    //
    // `Ret::Opt` on four of them, and the `.None`s are all one sentence: there
    // is no actor, or there is nothing there yet. `Ret::Res` on the two a
    // `sendMessage` reads a reason from, `mailboxPush` and `stateTake`: their
    // `.Err` is a `SendError` named by its index, `Stopped`, `TimedOut` or
    // `WouldDeadlock`. The payload is an `Int` for the two that answer a depth
    // and a `[T]` for the four that answer a block; both are written through
    // the trailing out-pointer at the success arm's own offset, so a niche
    // `Option<[T]>` is settled by the non-null block pointer the runtime wrote
    // — and `core/actor`'s `Carried<T>` is what guarantees that pointer is
    // non-null, since a zero-stride element would have made the block empty
    // and the niche `.None`.
    //
    // The bodies are in `cli/runtime/rt.rs` behind feature `net`, so
    // `runtime_native::net_intrinsic` names the `actor.*` family too: a
    // toolchain built without the reactor refuses these keys with a sentence
    // before code generation rather than with a missing symbol from `cc`.
    e("actor.mailboxOpen", &[Dropped, List, Scalar], Ret::Scalar),
    e("actor.mailboxPush", &[Dropped, Scalar, List], Ret::Res),
    e("actor.mailboxPop", &[Dropped, Scalar], Ret::Sum),
    e("actor.mailboxClose", &[Dropped, Scalar], Ret::Sum),
    e("actor.stateTake", &[Dropped, Scalar], Ret::Res),
    e("actor.statePut", &[Dropped, Scalar, List], Ret::Sum),
    e("actor.replyOpen", &[Dropped], Ret::Scalar),
    e("actor.replyPut", &[Dropped, Scalar, List], Ret::Sum),
    e("actor.replyTake", &[Dropped, Scalar], Ret::Sum),
    // Takes no context: whether any arena holds pages is a fact about the
    // process, and `core/actor` asks it before skipping a copy.
    e("actor.scopesLive", &[], Ret::Scalar),
    // -- core/tasks's scopes (F8) --------------------------------------------
    //
    // Ten rows, the nine above read a second time: a spawned task crosses as a
    // one-element `[Carried<fn(C) => ()>]`, the runtime holds the block and
    // hands it back, and nothing about the closure is described. So no stride,
    // no glue, no descriptor and no entry thunk — which is what makes
    // background work land without [`Extra::Step`] growing a second shape. The
    // task is *entered* by `core/tasks::running`, in Buri, through
    // `Tasks.parallel`.
    //
    // The context is argument `0` on every row for the actor block's reason:
    // `core/tasks` declares these as `fn <name><C: Tasks, ...>(ctx: C, ...)`,
    // so argument 0 is the context and the C signature has no parameter for
    // it.
    //
    // `scopeRound` answers a `[Int]` and carries no [`Extra::Element`], which
    // is `list.range`'s row exactly: the element type is fixed, so there is no
    // `T` for a stride to describe. The `Bool`s are [`Ret::Scalar`] and
    // cross as a `u8`, which is `str.startsWith`'s shape.
    //
    // The bodies are in `cli/runtime/rt.rs` behind feature `net`, so
    // `runtime_native::net_intrinsic` names this family too.
    e("tasks.scopeOpen", &[Dropped], Ret::Scalar),
    e("tasks.scopePush", &[Dropped, Scalar, List], Ret::Sum),
    e("tasks.scopeRound", &[Dropped, Scalar], Ret::Out),
    e("tasks.scopeTaskAt", &[Dropped, Scalar, Scalar], Ret::Sum),
    e("tasks.scopeEnter", &[Dropped, Scalar], Ret::Scalar),
    e("tasks.scopeLeave", &[Dropped, Scalar], Ret::Scalar),
    e("tasks.scopeBeside", &[Dropped, Scalar], Ret::Scalar),
    e("tasks.scopeClaim", &[Dropped, Scalar], Ret::Scalar),
    e("tasks.scopeSpare", &[Dropped, Scalar], Ret::Scalar),
    e("tasks.scopeRan", &[Dropped, Scalar], Ret::Scalar),
    // `core/tasks`'s timers. Unlike a spawned task, a timer's body is the
    // runtime's to call, so it crosses as a kept handler ([`Extra::Press`]):
    // `fn(C, Int) => ()`, with the context written into the record beside the
    // closure and the timer's handle as the element. The runtime fires it
    // where the program waits and, once `main` has returned, until none is
    // pending (`cli/runtime/rt.rs`'s timers).
    e("tasks.timerStart", &[Dropped, Scalar, Press], Ret::Scalar),
    e("tasks.timerStop", &[Dropped, Scalar], Ret::Void),
    // -- platform/effect/testing's stateful half -----------------------------------
    //
    // `platform/effect/testing`'s names, over one handle table.
    // `cli/runtime/testing.rs`'s header is the argument for these being in the
    // archive rather than open-coded: each names a slot in one mutable table,
    // which is `runtime.js`'s `$t.h` written for a language that has statics.
    //
    // Every **constructor** is `Ret::Out` and not `Ret::Scalar`, and that is
    // the one non-obvious row here. `struct TestStdout(I64)` is a struct, and
    // `middle/layout.rs` gives every struct `Repr::Aggregate` however few
    // fields it has — so the result is an aggregate and §2 rule 2 puts it
    // through an out-pointer. Declaring it as returning one word would agree
    // with the archive by accident on both supported targets and be an ABI
    // disagreement nothing diagnoses.
    //
    // `TestFileSystem`'s eleven methods answer a `Result<T, IoError>`, which was the
    // shape this table had no `Ret` for; §2.1 is that shape and [`Ret::Res`] is
    // the row for it. `host.HostFileSystem.readFile` is still absent, and for a
    // different reason: the archive has a body for it and this table has no
    // row, which is a gap rather than a shape.
    //
    // `alloc` and `TestAllocator.allocate` are open-coded and are named in
    // [`the_unimplemented_surface_is_not_claimed`].
    //
    // `proc` and `TestProcess.exitWith` are absent and are not named there either,
    // for `TestNetwork.fetch`'s reason rather than the allocator's: both are Buri
    // bodies, so no key reaches this table to be missing from it. `TestProcess`
    // records nothing because nothing can read it back.
    e("host_testing.stdout", &[], Ret::Out),
    e("host_testing.stderr", &[], Ret::Out),
    // The five writers answer `Result<(), IoError>` here too, and always
    // `.Ok(())`: a captured stream is a buffer the runner owns, so there is
    // nothing to fail. The shape is the effect's, not the implementation's.
    e("host_testing.TestStdout.print", &[Scalar, Str], Ret::Res),
    e("host_testing.TestStdout.println", &[Scalar, Str], Ret::Res),
    e("host_testing.TestStdout.writeBytes", &[Scalar, List], Ret::Res),
    e("host_testing.TestStdout.captured", &[Scalar], Ret::Out),
    e("host_testing.TestStderr.eprint", &[Scalar, Str], Ret::Res),
    e("host_testing.TestStderr.eprintln", &[Scalar, Str], Ret::Res),
    e("host_testing.TestStderr.captured", &[Scalar], Ret::Out),
    e("host_testing.stdin", &[], Ret::Out),
    e("host_testing.TestStdin.lines", &[Scalar, List], Ret::Out),
    e("host_testing.TestStdin.bytes", &[Scalar, List], Ret::Out),
    e("host_testing.TestStdin.readLine", &[Scalar], Ret::Sum),
    e("host_testing.TestStdin.readBytes", &[Scalar, Scalar], Ret::Sum),
    // The stream's log, read back. A log is state the runner keeps, so it is
    // here for the reason the handle table itself is.
    e("host_testing.TestStdin.calls", &[Scalar], Ret::Out),
    // `TestFileSystem`'s twenty-two, and every one of them takes a **handle** rather
    // than a `TestFileSystem`. That value is a handle and a fault plan since the plan
    // landed, and an argument crosses as its leaves — so a row taking `self`
    // would be handed three values where it expects one, which is the crash
    // `TestNetwork.calls` found first. The eleven filesystem methods are Buri bodies
    // over these rows; `host_testing.buri` says why the plan is in the program.
    //
    // `snapshot` is `Ret::Out` over a `[(Str, Str)]` — one block of two-`Str`
    // elements, which is the layout `str.splitOnce` already writes through an
    // out-pointer, one element wide. The three builders and `newFs` answer an
    // `I64` and so are `Ret::Scalar`, `newNet`'s shape rather than `clock`'s:
    // what they answer is the handle, and the value around it is built in Buri.
    e("host_testing.newFs", &[], Ret::Scalar),
    e("host_testing.fsFiles", &[Scalar, List], Ret::Scalar),
    e("host_testing.fsFilesBytes", &[Scalar, List], Ret::Scalar),
    e("host_testing.fsReadOnly", &[Scalar], Ret::Scalar),
    e("host_testing.fsRead", &[Scalar, Str], Ret::Res),
    e("host_testing.fsSnapshot", &[Scalar], Ret::Out),
    e("host_testing.fsCalls", &[Scalar], Ret::Out),
    e("host_testing.fsReadFile", &[Scalar, Str], Ret::Res),
    e("host_testing.fsWriteFile", &[Scalar, Str, Str], Ret::Res),
    e("host_testing.fsFileExists", &[Scalar, Str], Ret::Int(8)),
    e("host_testing.fsReadDir", &[Scalar, Str], Ret::Res),
    e("host_testing.fsReadFileBytes", &[Scalar, Str], Ret::Res),
    e("host_testing.fsWriteFileBytes", &[Scalar, Str, List], Ret::Res),
    e("host_testing.fsAppendFile", &[Scalar, Str, List], Ret::Res),
    e("host_testing.fsRenameFile", &[Scalar, Str, Str], Ret::Res),
    e("host_testing.fsRemoveFile", &[Scalar, Str], Ret::Res),
    e("host_testing.fsRemoveDir", &[Scalar, Str], Ret::ResMsg),
    e("host_testing.fsMakeDir", &[Scalar, Str], Ret::Res),
    e("host_testing.fsSyncFile", &[Scalar, Str], Ret::Res),
    e("host_testing.fsMetadata", &[Scalar, Str], Ret::Res),
    // The one of the four that can say something: a negative offset or count is
    // `.Other` with a sentence, and the sentence is the one the JavaScript
    // double writes.
    e("host_testing.fsReadRange", &[Scalar, Str, Scalar, Scalar], Ret::ResMsg),
    e("host_testing.fsRealPath", &[Scalar, Str], Ret::Res),
    e("host_testing.fsCopyFile", &[Scalar, Str, Str], Ret::Res),
    // -- the fault plan's promise -------------------------------------------
    //
    // The plan itself never crosses. It is a list of Buri values holding an
    // `IoError`, and §2.1 cannot name an error variant that carries a field, so
    // matching is the `Equal` the `Call` records derive and happens in
    // `host_testing.buri`. What crosses is the half a program cannot keep:
    // `fsWithPlan`/`netWithPlan` mint the plan, `addFsFault`/`addNetFault` say
    // what each entry would read like in a failure message, `noteFault` records
    // that one fired, and `test.leave` below reports the rest. `noteFsCall` is
    // the twelfth way into a log: a call the plan failed never reaches the row
    // that would have recorded it, and it is still a call.
    e("host_testing.fsWithPlan", &[Scalar], Ret::Scalar),
    e("host_testing.addFsFault", &[Scalar, Str, Str, Str], Ret::Void),
    e("host_testing.addNetFault", &[Scalar, Str], Ret::Void),
    e("host_testing.faultFails", &[Scalar, Scalar, Scalar, Str], Ret::Void),
    e("host_testing.noteFault", &[Scalar, Scalar], Ret::Void),
    e("host_testing.noteFsCall", &[Scalar, Str, Str, Str], Ret::Void),
    e("host_testing.netRebind", &[Scalar], Ret::Scalar),
    e("host_testing.netWithPlan", &[Scalar], Ret::Scalar),
    // -- the call log's remaining four --------------------------------------
    //
    // `spelled` is an `FsCall` constructor's decode and not a filesystem
    // operation at all: a test writing a call down performs no effect, so it
    // has no context to reach `bytes.fromUtf8` with.
    //
    // The other three are `TestNetwork`'s. `net()` and `TestNetwork.fetch` are Buri
    // bodies and have no row — the absent-key list below says why — but the
    // *log* is state, so the handle naming it is minted here
    // (`alloc.newCounter`'s shape), written by `recordFetch` once the responder
    // has answered, and read back by `netCalls`. `recordFetch` takes `Request`
    // flattened by §2 rule 1, which is `buri_rt_host_network_fetch`'s argument list
    // without its answer; `netCalls` takes the handle rather than the `TestNetwork`,
    // because that value carries the responder too and an argument crosses as
    // its leaves.
    e("host_testing.spelled", &[List], Ret::Out),
    e("host_testing.newNet", &[], Ret::Scalar),
    e("host_testing.recordFetch", &[Scalar, Scalar, Str, List, List, Scalar], Ret::Void),
    e("host_testing.netCalls", &[Scalar], Ret::Out),
    // `tcp()`'s seven. Its shape is `TestStdin`'s rather than `TestNetwork`'s —
    // what a test writes down is a script and what it reads back is a log, and
    // there is no responder to keep in the program — so the handle names all of
    // it and nothing here needs a plan. `recordTcpRead` is `Ret::Opt` because
    // the one failure the double has is a stream it never minted, which carries
    // nothing.
    e("host_testing.newTcp", &[], Ret::Scalar),
    e("host_testing.tcpStream", &[Scalar, List], Ret::Scalar),
    e("host_testing.recordTcpConnect", &[Scalar, Str, Scalar], Ret::Scalar),
    e("host_testing.recordTcpRead", &[Scalar, Scalar, Scalar], Ret::Sum),
    e("host_testing.recordTcpWrite", &[Scalar, Scalar, List], Ret::Int(8)),
    e("host_testing.recordTcpClose", &[Scalar, Scalar], Ret::Void),
    e("host_testing.tcpCalls", &[Scalar], Ret::Out),
    // -- tasks(): the order the work happens in ------------------------------
    //
    // `parallel` is the **second** key of the closure trampoline in this table
    // and the reason the double is worth having: it reaches its steps through
    // the same entry thunk `host.HostTasks.parallel` reaches them through, so a
    // program tested against this is tested against the boundary that ships.
    // `self` is a `TestTasks`, a handle, so it is a scalar where the real one is
    // `Arg::Dropped` — the runtime has to be able to ask which order this run
    // schedules in. The other rows are the ordering builders, the log, and the
    // plan's two halves.
    e("host_testing.TestTasks.parallel", &[Scalar, Dropped, Elems, Step], Ret::Out),
    e("host_testing.tasks", &[], Ret::Out),
    e("host_testing.TestTasks.anyOrder", &[Scalar], Ret::Out),
    e("host_testing.TestTasks.everyOrder", &[Scalar], Ret::Out),
    e("host_testing.TestTasks.seed", &[Scalar, Scalar], Ret::Out),
    e("host_testing.TestTasks.calls", &[Scalar], Ret::Out),
    e("host_testing.TestTasks.runs", &[Scalar], Ret::Scalar),
    e("host_testing.TestTasks.orders", &[Scalar], Ret::Scalar),
    e("host_testing.TestTasks.replan", &[Scalar], Ret::Out),
    e("host_testing.TestTasks.addFault", &[Scalar, Scalar, Scalar, Str], Ret::Void),
    e("host_testing.clock", &[], Ret::Out),
    e("host_testing.TestClock.at", &[Scalar, Scalar], Ret::Out),
    e("host_testing.TestClock.nowMilliseconds", &[Scalar], Ret::Scalar),
    e("host_testing.TestClock.sleepMilliseconds", &[Scalar, Scalar], Ret::Void),
    e("host_testing.TestClock.monotonicNanoseconds", &[Scalar], Ret::Scalar),
    e("host_testing.rand", &[], Ret::Out),
    e("host_testing.TestRandom.seed", &[Scalar, Scalar], Ret::Out),
    e("host_testing.TestRandom.nextInt", &[Scalar, Scalar, Scalar], Ret::Scalar),
    e("host_testing.TestRandom.nextFloat", &[Scalar], Ret::Scalar),
    e("host_testing.entropy", &[], Ret::Out),
    e("host_testing.TestEntropy.seed", &[Scalar, Scalar], Ret::Out),
    e("host_testing.TestEntropy.bytes", &[Scalar, Scalar], Ret::Out),
    e("host_testing.env", &[], Ret::Out),
    e("host_testing.TestEnvironment.variables", &[Scalar, List], Ret::Out),
    e("host_testing.TestEnvironment.withArguments", &[Scalar, List], Ret::Out),
    e("host_testing.TestEnvironment.variable", &[Scalar, Str], Ret::Sum),
    e("host_testing.TestEnvironment.arguments", &[Scalar], Ret::Out),
    e("host_testing.TestEnvironment.currentDirectory", &[Scalar], Ret::Out),
    e("host_testing.TestEnvironment.allVariables", &[Scalar], Ret::Out),
    e("host_testing.TestEnvironment.operatingSystemName", &[Scalar], Ret::Out),
    // The spawn double is a log and nothing else — the scripted answer holds an
    // `IoError`, which §2.1 cannot hand back across a row, so it stays in the
    // program and `spawnProcess` is a Buri body. `TestNetwork`'s arrangement.
    e("host_testing.newSpawn", &[], Ret::Scalar),
    e("host_testing.recordSpawn", &[Scalar, List], Ret::Void),
    e("host_testing.spawnCalls", &[Scalar], Ret::Out),
    // `sockets()` — a socket with no network behind it. Seven rows: the double,
    // the mint, the three `Sockets` methods and the two readers. `sent` and
    // `isOpen` take the **handle** rather than the `TestSockets`, in
    // `netCalls`'s shape but for the plainer reason: they are Buri bodies,
    // because `sent` builds a `Message` out of the flat record this side writes
    // and `isOpen` unwraps a `Socket`. `cli/runtime/lib.rs` §2.1's division —
    // a runtime writes a struct and the program builds the enum — is the same
    // one `Received` is on.
    e("host_testing.sockets", &[], Ret::Out),
    e("host_testing.socketsOpen", &[Scalar], Ret::Scalar),
    e("host_testing.socketsSent", &[Scalar], Ret::Out),
    e("host_testing.socketsIsOpen", &[Scalar, Scalar], Ret::Scalar),
    e("host_testing.TestSockets.socketSendText", &[Scalar, Scalar, Str], Ret::Void),
    e("host_testing.TestSockets.socketSendBytes", &[Scalar, Scalar, List], Ret::Void),
    e("host_testing.TestSockets.socketClose", &[Scalar, Scalar, Scalar, Str], Ret::Void),
    // `sockets().dialling(messages)` — a client with a script instead of a
    // network. Three rows: the mint and the client's two effect methods.
    //
    // The client is minted by a `TestSockets` rather than minting sockets of
    // its own, and that is the whole reason it needs no `Sockets`
    // implementation: the socket it answers is one of *that* double's, so a
    // program's `socket.send` is recorded by `sent()` and its `close` shows up
    // in `isOpen`. One double writes and one double reads, which is the same
    // division `effect Sockets`' header draws.
    e("host_testing.socketsDialling", &[Scalar, List], Ret::Scalar),
    e("host_testing.TestWebSocketClient.connectSocket", &[Scalar, Str], Ret::Res),
    e("host_testing.TestWebSocketClient.connectReceive", &[Scalar, Scalar], Ret::Res),
    // The one key here that no Buri declaration produces: `middle::monomorphize`
    // emits it after every `test` body, so that "a fault whose call never
    // happens fails the test" is checked once for all three backends rather than
    // three times in three test-binary entry points. Its twin
    // `buri_rt_test_enter` is called from those entry points instead, because it
    // is the *runner's* protocol — which block to run — and this is the
    // *program's* rule.
    e("test.leave", &[Scalar], Ret::Void),
    // The other half of the same lowering, emitted after it: whether to run this
    // body again. `TestTasks.everyOrder` reruns the body once per completion
    // order, and answering yes here is how — the body calls itself, so the
    // reruns are one tree on all three backends rather than a loop in each of
    // three entry points.
    e("test.replay", &[Scalar], Ret::Scalar),
    // `buri test --coverage`'s probe, which `middle::coverage` puts in front of
    // every line it counts. No declaration produces it either.
    e("coverage.hit", &[Scalar], Ret::Void),
    // -- the reactive graph, and the snapshot it paints ----------------------
    //
    // `cli/runtime/ui.rs` holds the graph and `cli/runtime/snapshot.rs` the
    // painter's entry. Seven of these are what a *snapshot* reaches —
    // `rootScope`, `Scope.read`, `headless`, `Headless`'s `signal`, `read` and
    // `write`, and `paint`. The rest is what a `platform/effect/testing` **suite** reaches:
    // the graph's two computations, the observer that reads a cell from
    // outside every computation, and the recorder that says when a computation
    // ran.
    //
    // `Ui.memo` and `Ui.watch` are what [`Extra::Compute`] was added for: both
    // take a Buri closure the runtime keeps and calls later, which is
    // [`Extra::Step`]'s thunk with a lifetime problem to answer. A snapshot
    // still reaches neither — `ui/node`'s `describe` reads props under
    // `rootScope`, and an untracked read subscribes nothing.
    //
    // `platform/effect/testing`'s renderer is here now — `render`, `Rendered`'s methods, and,
    // since #53 phase 5, `install`, `variables` and `stylesheet`. The last three
    // are not a document to build but two artifacts to surface: `stylesheet` is
    // the compiler's extracted sheet, and `install`/`variables` are the `:root`
    // block `ui/theme` resolves, both handed over as text. Their rows sit with
    // `ui_theme.installDoc`/`ui_theme.variables` and `host_testing.stylesheet`.
    //
    // A snapshot's **themes** are here too, and they are not a document:
    // `ui/theme`'s `document` flattens the list to text under its own
    // `rootScope`, and `installThemes` hands that text over for the paint that
    // follows. Two rows, both monomorphic, and a `Theme` never crosses.
    //
    // Five of them are generic and each carries §2 rule 4's pair. The type is
    // the value the call carries whole rather than a `[T]`'s element, which is
    // what [`v`] says: `signal` and `write` name it in a `by_ref`
    // argument, and the three `read`s name it in the result. **A `T` that is
    // itself a list is why that has to be a column** — `Signal<[Account]>` is
    // an array-typed argument at `signal` exactly as `list.push`'s receiver is,
    // and a backend that guessed gave the cell an `Account`'s width and an
    // `Account`'s glue.
    //
    // `signal` and `write` carry a third and a fourth word — the release and
    // the equality — and are the only rows in this table that do. A cell keeps
    // what it was written, so the write that replaces the bytes gives the old
    // ones back and the exit walk gives the last ones back; and the write only
    // replaces them at all when the new value is not the one already there,
    // which is `==` at the cell's type rather than a comparison of its bytes.
    // See [`Extra::Owned`]. `signal` takes the equality and uses neither it nor
    // the release: a fresh cell replaces nothing. One shape for both keys,
    // because `Extra::Owned` is one emission rule.
    //
    // `Ret::Out` on both `read`s although a `T` is often a scalar. One key is
    // one C signature, and `read` at `Str` and at `Bool` is one key — so the
    // value comes back through a pointer at every instantiation rather than in
    // a register at some of them.
    e("ui_node.rootScope", &[], Ret::Out),
    // `ui/theme`'s own untracked scope, for the walk that flattens a theme
    // list to the document a snapshot's painter reads. A second row rather
    // than a second name for `ui_node.rootScope`, because the symbol a key
    // produces is the key's own (§1's rule) — and a private one per module is
    // what keeps a `Scope`, which grants reading the graph, out of any public
    // signature.
    e("ui_theme.rootScope", &[], Ret::Out),
    // The theme artifact `platform/effect/testing` reads (#53 phase 5). `installDoc` resolves
    // a theme list the caller has flattened to the document `ui/theme`'s
    // `document` builds — the chain following and the `:root`/`body`/scheme
    // blocks are `cli/runtime/ui.rs`'s, unchanged — stores it, and answers the
    // block. `variables` answers whatever the last install left. A switching
    // theme is the caller's business: `platform/effect/testing`'s `install` registers a
    // watcher that flattens through the tracked scope and installs again, so no
    // closure crosses here. Both answer a `Str`.
    e("ui_theme.installDoc", &[Str], Ret::Out),
    e("ui_theme.variables", &[], Ret::Out),
    v(e("effect.Scope.read", &[Scalar, Scalar, Stride, Retain], Ret::Out)),
    e("host_testing.headless", &[], Ret::Out),
    v(e("host_testing.Headless.signal", &[Scalar, Spilled, Stride, Retain, Release, Equal], Ret::Scalar)),
    v(e("host_testing.Headless.read", &[Scalar, Scalar, Stride, Retain], Ret::Out)),
    e("host_testing.observer", &[], Ret::Out),
    v(e("host_testing.Observer.read", &[Scalar, Scalar, Stride, Retain], Ret::Out)),
    v(e("host_testing.Headless.write", &[Scalar, Scalar, Spilled, Stride, Retain, Release, Equal], Ret::Void)),
    e("host_testing.Headless.memo", &[Scalar, Compute], Ret::Scalar),
    e("host_testing.Headless.watch", &[Scalar, Compute], Ret::Void),
    // The virtual clock `after` schedules on under `headless()` (#256).
    // `headlessTimerStart` is `tasks.timerStart`'s shape: a kept handler with
    // the double written into the record beside it and the handle as the
    // element. `elapse` fires what came due, each on the graph's own turn.
    e("host_testing.headlessTimerStart", &[Dropped, Scalar, Press], Ret::Scalar),
    e("host_testing.Headless.unschedule", &[Scalar, Scalar], Ret::Void),
    e("host_testing.elapse", &[Scalar], Ret::Void),
    e("host_testing.installThemes", &[Str], Ret::Void),
    // `core/platforms/testing/state`: a whole `T` per handle, in the shape of
    // `Headless`'s `signal`, `read` and `write` above. `stateNew` and
    // `statePut` carry the release so the runtime can give the value back at
    // exit; `stateTake` moves the value out, so it needs no retain but takes
    // the pair anyway for `read`'s one C shape.
    v(e("platforms_testing_state.stateNew", &[Spilled, Stride, Retain, Release, Equal], Ret::Scalar)),
    v(e("platforms_testing_state.stateRead", &[Scalar, Stride, Retain], Ret::Out)),
    v(e("platforms_testing_state.stateTake", &[Scalar, Stride, Retain], Ret::Out)),
    v(e("platforms_testing_state.statePut", &[Scalar, Spilled, Stride, Retain, Release, Equal], Ret::Void)),
    // `stylesheet()` — the extracted sheet, a compile artifact `buri test`
    // writes beside the binary and hands over the way it hands over the snapshot
    // directory (#53 phase 5). The same string the JavaScript backend splices in
    // as `$ui_sheet`, so a suite asserting what a class means shares.
    e("host_testing.stylesheet", &[], Ret::Out),
    e("host_testing.paint", &[Str, Str, Str], Ret::Void),
    // The recorder: how a computation says that it ran. A reactive body holds
    // a `Scope`, which grants reading the graph and nothing else, so it cannot
    // write a signal and it cannot print — the log lives on this side, exactly
    // as `platform/effect/testing`'s captured stdout does.
    e("host_testing.recorder", &[], Ret::Out),
    e("host_testing.Recorder.record", &[Scalar, Str], Ret::Void),
    e("host_testing.Recorder.recorded", &[Scalar], Ret::Out),
    e("host_testing.Recorder.note", &[Scalar, Scalar], Ret::Scalar),
    e("host_testing.Recorder.noted", &[Scalar], Ret::Out),
    // -- the renderer, and the document it builds (issue #53) ----------------
    //
    // `render` is not here: it is a Buri body now, `Rendered(mount(ctx, root,
    // renderInto))`, so the walk crosses as an ordinary argument the backend
    // materialises rather than a body-less intrinsic this table would name.
    // What is here is the mount it reaches, the builders the walk emits to, and
    // the readers a `Rendered` answers.
    //
    // `mount` drops its context, passes `root` by address — a pointer to the
    // one `Node` the walk destructures and this side never reads — and takes
    // the walk as its last argument ([`Extra::Walk`]). `renderInto` never
    // crosses whole: only its `{ code, env }` does, inside the record, and the
    // dropped context beside it, which the document keeps for the walks and
    // handlers it drives later.
    e("host_testing.mount", &[Dropped, Spilled, Walk], Ret::Scalar),
    // The builders the walk emits to. `emitElement` takes the element name and
    // its scene declarations, both `Str`; `emitText` a run; `exitElement`
    // closes the open element. The builder handle is a `Builder`, one word.
    e("ui_node.emitElement", &[Scalar, Str, Str], Ret::Void),
    e("ui_node.exitElement", &[Scalar], Ret::Void),
    e("ui_node.emitText", &[Scalar, Str], Ret::Void),
    // The reactive builders (#53 phase 3). `openText` mints a run and answers
    // its handle, `patchText` writes over that run — together the native
    // `$tree_bind`. `enterDynamic` opens a region and answers its handle, and
    // `rebuildRegion` clears it and re-walks a subtree into the gap — the native
    // `$tree_dynamic`. Its last argument is the walk `renderInto`, an
    // [`Extra::Walk`] like `mount`'s, so the runtime supplies and drops the
    // context the watcher could not capture, with the node at index 2 by
    // address. `reactive` is the renderer's own `watch`: the same deferred body
    // [`Extra::Compute`] carries for `Ui.watch`, registered with no `Ui` in hand
    // because the runtime owns the graph.
    e("ui_node.openText", &[Scalar], Ret::Scalar),
    e("ui_node.patchText", &[Scalar, Scalar, Str], Ret::Void),
    // `openElement` emits-and-enters like `emitElement` but answers the index a
    // reactive style patches its body by; `patchBody` writes over that body,
    // the element kept — together a `$tree_styles` bind for the scene.
    e("ui_node.openElement", &[Scalar, Str, Str], Ret::Scalar),
    e("ui_node.patchBody", &[Scalar, Scalar, Str], Ret::Void),
    e("ui_node.reactive", &[Compute], Ret::Void),
    e("ui_node.enterDynamic", &[Scalar], Ret::Scalar),
    // `beginRegion` clears a region and points the builder at its gap and
    // `endRegion` restores it — `rebuildRegion` split open, for a reactive
    // widget that emits inline under its own watcher rather than walking a node.
    e("ui_node.beginRegion", &[Scalar, Scalar], Ret::Void),
    e("ui_node.endRegion", &[Scalar], Ret::Void),
    e("ui_node.rebuildRegion", &[Scalar, Scalar, Spilled, Walk], Ret::Void),
    // The keyed list (#53 phase 4). `enterEach` opens the two markers and the
    // row owner; `reconcile` is driven from the list's watcher with the keys the
    // watcher computed and the row body last, an [`Extra::Walk`] like a region
    // rebuild's — but with nothing spilled, because the body builds its own node
    // from `rowAt` rather than being handed one.
    e("ui_node.enterEach", &[Scalar], Ret::Scalar),
    e("ui_node.reconcile", &[Scalar, Scalar, List, Walk], Ret::Void),
    // The event arms (#53 phase 4). `registerPress` keeps a button's or a form's
    // `fn(C, Event) => ()` on the open element — an [`Extra::Press`], the kept
    // two-parameter handler; `registerValue` stores a field's or a toggle's bound
    // signal so `fill`/`flip` can write it; `markSubmit` flags the button whose
    // press submits its form.
    e("ui_node.registerPress", &[Scalar, Press], Ret::Void),
    // `registerOutside` keeps an `onPressOutside`'s handler on the document
    // paired with the open element — an `ep` like `registerPress`, kept in the
    // graph so the subtree's disposal takes the listener.
    e("ui_node.registerOutside", &[Scalar, Press], Ret::Void),
    // `registerFollow` keeps a route link's plain-click handler and its
    // destination on the anchor, so `follow` fires it; a kept two-parameter
    // handler like `registerPress`, with the destination string ahead of it.
    e("ui_node.registerFollow", &[Scalar, Str, Press], Ret::Void),
    e("ui_node.registerValue", &[Scalar, Scalar], Ret::Void),
    // `registerSelection` stores a field's caret/selection signal so `select`
    // can write it an `(anchor, focus)` pair — a plain handle like `registerValue`.
    e("ui_node.registerSelection", &[Scalar, Scalar], Ret::Void),
    // `registerLabel` keeps a button's accessible name on it, so `press` finds
    // a children-button by the label a reader hears rather than its glyphs.
    e("ui_node.registerLabel", &[Scalar, Str], Ret::Void),
    e("ui_node.markSubmit", &[Scalar], Ret::Void),
    // A file picker (#209). `registerPick` keeps its handler in a slot of its
    // own, an `ep` like `registerPress`, so `press` never fires it; the three
    // `offered` readers answer the file `pickFile` left on the document, and
    // `offerFile`/`deliverFile` are `pickFile`'s two halves.
    e("ui_node.registerPick", &[Scalar, Press], Ret::Void),
    e("ui_node.offeredName", &[Scalar], Ret::Out),
    e("ui_node.offeredType", &[Scalar], Ret::Out),
    e("ui_node.offeredBytes", &[Scalar], Ret::Out),
    e("host_testing.offerFile", &[Scalar, Str, Str, List], Ret::Void),
    e("host_testing.deliverFile", &[Scalar, Str], Ret::Void),
    // The readers, over the reconciled document rather than the string:
    // `markup` and `text` answer a `Str` through an out-pointer, `count` and
    // `identity` an `Int`.
    e("host_testing.Rendered.markup", &[Scalar], Ret::Out),
    e("host_testing.Rendered.text", &[Scalar], Ret::Out),
    e("host_testing.Rendered.count", &[Scalar, Str], Ret::Scalar),
    e("host_testing.Rendered.identity", &[Scalar, Str, Scalar], Ret::Scalar),
    // The event dispatch (#53 phase 4), each addressing the reconciled document
    // by label or index and mutating a signal a watcher then sees: `press` fires
    // the stored handler (and submits an enclosing form for a submit button),
    // `fill` and `flip` write the bound signal, `submit` fires the form's handler
    // under the implicit-submission rule. All answer `()`.
    e("host_testing.Rendered.press", &[Scalar, Str], Ret::Void),
    e("host_testing.Rendered.fill", &[Scalar, Str, Str], Ret::Void),
    // `select` moves the caret and selection: it writes the field's `(Int, Int)`
    // selection signal the way `fill` writes its value.
    e("host_testing.Rendered.select", &[Scalar, Str, Scalar, Scalar], Ret::Void),
    e("host_testing.Rendered.flip", &[Scalar, Str], Ret::Void),
    e("host_testing.Rendered.submit", &[Scalar, Scalar], Ret::Void),
    // The pointer (#220). `registerPointer` keeps one of an element's three
    // pointer handlers under its phase, an `ep` like `registerPress`; the four
    // readers answer the `PointerAt` the dispatch in flight set, and
    // `pointerDown`/`pointerMove`/`pointerUp` are that dispatch.
    e("ui_node.registerPointer", &[Scalar, Scalar, Press], Ret::Void),
    e("ui_node.pointerX", &[Scalar], Ret::Scalar),
    e("ui_node.pointerY", &[Scalar], Ret::Scalar),
    e("ui_node.pointerOverRow", &[Scalar], Ret::Scalar),
    e("ui_node.pointerRow", &[Scalar], Ret::Out),
    e("host_testing.Rendered.pointerDown", &[Scalar, Str, Scalar, Scalar], Ret::Void),
    e("host_testing.Rendered.pointerMove", &[Scalar, Str, Scalar, Scalar], Ret::Void),
    e("host_testing.Rendered.pointerUp", &[Scalar, Str, Scalar, Scalar], Ret::Void),
];

/// The entry for a key, or `None` where the archive has no body for it.
pub fn entry(key: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.key == key)
}

// ---------------------------------------------------------------------------
// The symbols this backend emits without an intrinsic key behind them
// ---------------------------------------------------------------------------

/// `buri_rt_abort(msg, len)` — `-> !`. Two parameters and no `base`: an abort
/// message is a static, and the runtime exits before anything could own it
/// (`abort.rs:52`).
pub const ABORT: &str = "buri_rt_abort";
/// `buri_rt_abort_div_zero()` — `-> !`. Division by zero (SPEC 6.2), with the
/// message shared between the two backends so `cli/tests/crash/` pins one
/// string.
pub const ABORT_DIV_ZERO: &str = "buri_rt_abort_div_zero";
/// `buri_rt_abort_shift()` — `-> !`. A `core/bits` shift count outside
/// `0 ..< bits`, with the message shared between the two backends and with
/// `$shiftCount` (`runtime.js:925`) so one string is pinned.
pub const ABORT_SHIFT: &str = "buri_rt_abort_shift";
/// `buri_rt_abort_unreachable()` — `-> !`, for `Profile::defensive_aborts`.
pub const ABORT_UNREACHABLE: &str = "buri_rt_abort_unreachable";
/// `buri_rt_abort_assert(kind, len)` — `-> !`, a failing `core/testing/assert`.
///
/// `(ptr, len)` and no `base`: the assertion kind is a `Str` the entry only
/// reads, so `lib.rs` §2 rule 1's third word is not passed and no count changes
/// hands — the process ends before either could matter.
pub const ABORT_ASSERT: &str = "buri_rt_abort_assert";
/// `buri_rt_test_enter(index) -> i32` — whether this process is to run the
/// `test` block at `index`, and a note of which one it is for the record an
/// abort writes. `cli/runtime/testing.rs` states the protocol.
pub const TEST_ENTER: &str = "buri_rt_test_enter";
/// `buri_rt_test_leave(index)` — the end of a `test` block: every fault the
/// block planned has happened, or the block fails now with the ones that did
/// not. Reached through the table above rather than from `test_entry_point`,
/// because `middle::monomorphize` emits the call inside the body's own function
/// and so all three backends get it from one place.
pub const TEST_LEAVE: &str = "buri_rt_test_leave";
/// `buri_rt_test_fail_compared(kind, len, actual, len, expected, len)` — `-> !`,
/// a failed comparison with both values already rendered by the `Show`
/// `middle::derives` generated at their type.
/// `buri_rt_test_replay(index) -> u8` — whether to run this `test` body again,
/// which `TestTasks.everyOrder` answers yes to once per completion order.
/// Emitted by `middle::monomorphize` immediately after [`TEST_LEAVE`], and
/// reached through the table for the same reason.
pub const TEST_REPLAY: &str = "buri_rt_test_replay";
pub const TEST_FAIL_COMPARED: &str = "buri_rt_test_fail_compared";
/// `buri_rt_test_fail_expected(kind, len, shown, len)` — `-> !`, `failExpected`
/// with its one value rendered.
pub const TEST_FAIL_EXPECTED: &str = "buri_rt_test_fail_expected";
/// `buri_rt_alloc(payload) -> *mut u8`.
pub const ALLOC: &str = "buri_rt_alloc";
/// `buri_rt_free(p)`.
pub const FREE: &str = "buri_rt_free";
/// `buri_rt_incref(p)` — the **shared** arm of `incref` (MEMORY.md §5.1).
///
/// The unshared arm is open-coded and always will be; this is reached only from
/// the fork on `cap`'s bit 63, which nothing sets, and it is a call because the
/// atomic sequence behind it is cold, is written once in the runtime, and is
/// twelve instructions this backend would otherwise put in front of the
/// optimizer at every reference operation in the program — measured at a
/// median +46 % of native release lowering, against +21 % for the call
/// (`design/PERFORMANCE.md` §6.6).
pub const INCREF: &str = "buri_rt_incref";
/// `buri_rt_decref(p, drop_glue)` — the **shared** arm of `decref`, and the
/// free that follows it. `drop_glue` is null for a type holding no references.
pub const DECREF: &str = "buri_rt_decref";
/// `buri_rt_argv_init(argc, argv)` — the emitted `main`'s first statement.
pub const ARGV_INIT: &str = "buri_rt_argv_init";
/// `buri_rt_flush()` — required before every return path from `main`.
pub const FLUSH: &str = "buri_rt_flush";
/// `buri_rt_main_returned()` — `main` answered `.Ok(())`, or answered nothing:
/// fire the timers still pending as they come due, then flush. The success
/// arm's flush, so a program that failed ends at once (`cli/runtime/rt.rs`'s
/// timers).
pub const RETURNED: &str = "buri_rt_main_returned";
/// `buri_rt_frames_are_per_thread()` — the artifact's one statement about
/// itself, made once at startup (`cli/runtime/lib.rs` §6).
///
/// **The LLVM backend makes it and the frame-threaded one does not**, and that
/// is a fact about where a Buri frame lives rather than a difference of opinion
/// about scheduling. In LLVM a Buri function is an ordinary LLVM function and
/// its locals are `alloca`s, so a thread's 512 KiB stack is its own and the
/// runtime may run two `Tasks.parallel` steps at once. In the stencil backend a
/// program has one Buri stack — the `buri$stencil$stack` block its `main`
/// guards — and a step runs in a frame the *call site* set aside, so two of
/// them would share it. Saying nothing is the safe answer, which is why only
/// the LLVM backend emits the call.
pub const FRAMES_PER_THREAD: &str = "buri_rt_frames_are_per_thread";
/// `buri_rt_values_may_cross_tasks()` — the artifact's other statement about
/// itself, made once at startup (`cli/runtime/lib.rs` §6).
///
/// It says **this artifact's values may cross a task boundary, and its
/// reference operations read [`SHARED_MASK`]**. The release backend makes it,
/// for a program `middle::rc::crosses_tasks` says can reach a task boundary.
/// It is not the same fact as [`FRAMES_PER_THREAD`]: that one is about *where
/// a frame lives*, a property of the backend, and this one is about *whether a
/// block can be reached from two threads*, a property of the program.
///
/// It marks nothing itself. The runtime begins sharing at its first fan-out,
/// and from then on every block counts as marked, so every reference operation
/// takes G2's atomic arm and no uniquely-owned in-place write fires. Before
/// that, a program that only could fan out pays nothing for it
/// (buri-lang/buri#243). Silence is the safe answer: the runtime's fan-out is
/// gated on this statement, so an entry point that lost it runs its tasks one
/// at a time. A backend whose fork reads only the block's bit must not make
/// it, because a block allocated before sharing began carries no bit.
pub const VALUES_MAY_CROSS_TASKS: &str = "buri_rt_values_may_cross_tasks";
/// `uint64_t buri_rt_shared_mask` — `middle::layout::CAP_SHARED_FLAG` once
/// the runtime has begun sharing, and `0` before. A data symbol, not a
/// function.
///
/// A program that makes the [`VALUES_MAY_CROSS_TASKS`] statement ORs it into
/// the `cap` its reference-count fork and its `Str` uniqueness probe test, so
/// a block allocated before sharing began counts as marked after it.
pub const SHARED_MASK: &str = "buri_rt_shared_mask";

/// The intrinsic keys that can run Buri code on a second thread: the runtime's
/// two fan-outs, `Tasks.parallel` and a scope running its tasks beside its
/// body. Every other key in `middle::rc::crosses_tasks` hands a value to a
/// table, and `core/actor` steps on whichever thread drives it.
///
/// An omission here costs speed and not soundness: a fan-out the runtime
/// reaches in a program that didn't make the [`VALUES_MAY_CROSS_TASKS`]
/// statement runs its steps in order.
pub const FANS_OUT: [&str; 2] = ["host.HostTasks.parallel", "tasks.scopeBeside"];

/// Whether `program` can run Buri code on two threads at once, so its counts
/// must read [`SHARED_MASK`] and its entry point makes the
/// [`VALUES_MAY_CROSS_TASKS`] statement.
///
/// Narrower than `ir::Program::crosses_tasks`. A program that reaches
/// `core/actor` and no [`FANS_OUT`] key has crossings but never a second
/// thread, so it keeps the plain fork: a mask read on every count was 6.5% of
/// the instructions of buri-lang/buri#243's program, which starts no thread.
pub fn shares_counts(program: &crate::compiler::middle::ir::Program) -> bool {
    use crate::compiler::middle::ir::{Body, Inst};
    let fans_out = |key: &str| FANS_OUT.contains(&key);
    program.crosses_tasks
        && program.funcs.iter().any(|f| match &f.body {
            Body::Runtime(key) => fans_out(key),
            Body::Code(code) => code.blocks.iter().any(|b| {
                b.insts
                    .iter()
                    .any(|i| matches!(i, Inst::CallIntrinsic { key, .. } if fans_out(key)))
            }),
        })
}

/// `void *buri_rt_copy_block(void *p, void (*glue)(void *))` — a fresh block
/// holding the same bytes, with `rc == 1` and nothing shared with the original.
///
/// G5's half of the copy out of a scope. The *type*-dependent half is the
/// per-type walk each backend generates (`Op::Copy` in both
/// backends' `walk_rc`); this is the block-dependent half, and
/// the division is where it is because this archive is compiled once against no
/// Buri type and a walk knows no header.
pub const COPY_BLOCK: &str = "buri_rt_copy_block";

/// `void buri_rt_copy_str(void *value)` — [`COPY_BLOCK`] for a `Str`, whose
/// `ptr` points *into* its block and has to be rebased onto the copy
/// (VALUE-MODEL.md §3).
pub const COPY_STR: &str = "buri_rt_copy_str";
/// `buri_rt_i128_divmod(a_lo, a_hi, b_lo, b_hi, signed, quot, rem)`.
pub const I128_DIVMOD: &str = "buri_rt_i128_divmod";
/// `buri_rt_i128_checked(op, a_lo, a_hi, b_lo, b_hi, signed, out) -> i32` and
/// `buri_rt_i128_saturating(op, ...)`, where `op` is `0` add, `1` sub, `2` mul,
/// `3` div.
///
/// A call rather than open-coded for [`I128_DIVMOD`]'s reason: the overflow test
/// both backends use at 64 bits is a widening multiply, which neither has at
/// `i128`, and a hand-rolled 128-bit one in two code generators is two places to
/// get it wrong. The `Entry` beside the first exists so that
/// `Unit::call_sum` — which translates an `i32` discriminant into whatever
/// `middle::layout` chose for the `Option` — can be driven by something that is
/// not a table row: this operation has no intrinsic key of its own, it is the
/// 128-bit arm of `number.I128.checkedAdd`.
pub const I128_SATURATING: &str = "buri_rt_i128_saturating";

/// The shape `buri_rt_i128_checked` answers in, as an [`Entry`] so the
/// sum-returning call path can be reused. `args` is empty because the argument
/// list is built at the call site from a 128-bit pair rather than from a Buri
/// signature, and the key is the one [`Entry::symbol`] mangles to that name.
pub const I128_CHECKED_ENTRY: Entry =
    e("i128.checked", &[], Ret::Sum);

/// `buri_rt_str_scalar_len(ptr, byte_len) -> u64` — the slow half of
/// `str.length`, called only where the ASCII flag is clear.
pub const STR_SCALAR_LEN: &str = "buri_rt_str_scalar_len";

/// `buri_rt_str_written(base)` — the in-place arm of the open-coded
/// `str.concat` is about to write into `base`'s block, so any scalar index the
/// runtime keeps of the block's bytes is dropped (`cli/runtime/scalars.rs`).
pub const STR_WRITTEN: &str = "buri_rt_str_written";

// -- rendering: a template hole, and `derivePrimShow` ------------------------
//
// These have no intrinsic *key* — a template hole arrives as
// `ir::Inst::Structural { op: Show }` and `derivePrimShow` arrives with the
// primitive only in the IR type of its argument — so they are named here rather
// than in [`ENTRIES`], which is keyed on something neither of them has. Every
// one writes its `Str` through a trailing out-pointer (`cli/runtime/lib.rs` §2
// rule 2), which is [`Ret::Out`]'s shape without [`Ret::Out`]'s table row.

/// `buri_rt_str_from_int(i64, out)` — decimal. The template-hole rendering of
/// every integer of 64 bits or fewer, after a widening by the *source's*
/// signedness.
pub const SHOW_INT: &str = "buri_rt_str_from_int";
/// `buri_rt_show_f32(f32, out)` / `buri_rt_show_f64(f64, out)` — the
/// shortest-round-trip formatter, which has to be one body because it has to
/// produce the same bytes as JavaScript (VALUE-MODEL.md §12).
pub const SHOW_F32: &str = "buri_rt_show_f32";
pub const SHOW_F64: &str = "buri_rt_show_f64";
/// `buri_rt_show_i128(lo, hi, out)` / `buri_rt_show_u128(lo, hi, out)` — a pair
/// of `u64`s, low half first, for `I128_DIVMOD`'s reason.
pub const SHOW_I128: &str = "buri_rt_show_i128";
pub const SHOW_U128: &str = "buri_rt_show_u128";
/// `buri_rt_char_to_str(u32, out)` — a `Char` as its own UTF-8, which is what a
/// template hole asks for.
pub const CHAR_TO_STR: &str = "buri_rt_char_to_str";
/// `buri_rt_show_char(u32, out)` — `'c'`, quoted and escaped. `derivePrimShow`
/// only; a template hole is [`CHAR_TO_STR`].
pub const SHOW_CHAR: &str = "buri_rt_show_char";
/// `buri_rt_show_str(ptr, len, out)` — `JSON.stringify`. `derivePrimShow` only;
/// a template hole of a `Str` is the string itself and is not a call at all.
pub const SHOW_STR: &str = "buri_rt_show_str";

// -- hashing: `derivePrimHash` ----------------------------------------------
//
// FNV-1a over UTF-16 code units, which is the one thing about hashing that has
// to be shared rather than open-coded: `hash()` is observable, and the two
// backends must answer the same number as `runtime.js` (`cli/runtime/hash.rs`).

/// `buri_rt_mix(h: u64, x: u32) -> u64` — one 32-bit word into the
/// accumulator. Every `Bool` and every integer goes through this, truncated.
pub const MIX: &str = "buri_rt_mix";
/// `buri_rt_hash_f64(h: u64, x: f64) -> u64`. An `F32` is promoted first, so
/// that `1.5f32` and `1.5f64` hash alike as they do on JavaScript, where there
/// is one number type.
pub const HASH_F64: &str = "buri_rt_hash_f64";
/// `buri_rt_hash_char(h: u64, c: u32) -> u64`.
pub const HASH_CHAR: &str = "buri_rt_hash_char";
/// The FNV-1a offset basis, and the accumulator `$hash` starts from.
///
/// Transcribed rather than reached: it is a Rust `const` in
/// `cli/runtime/hash.rs` and not an exported symbol, so a backend that wants to
/// start a hash has to write it down. `middle::derives`'s `HASH_SEED` and
/// `stencil/emit.rs`'s `HASH_SEED` are the same number for the same reason, and
/// `cli/runtime/hash.rs`'s own tests pin it — which is what keeps four copies
/// of one constant from drifting into four different hashes.
pub const HASH_SEED: u64 = 0x811c_9dc5;
/// `buri_rt_hash_str(h: u64, base, ptr, len) -> u64` — **not** the mangler's
/// answer for `str.hash`, which would be `buri_rt_str_hash`, and therefore
/// deliberately not an [`ENTRIES`] row: the archive groups it with the other
/// hashes rather than with `core/str`.
pub const HASH_STR: &str = "buri_rt_hash_str";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::backend::intrinsic_keys::step_call;
    use crate::compiler::backend::runtime_native::symbol_for;

    /// The examples `cli/runtime/lib.rs` §1 and VALUE-MODEL.md §10 spell out,
    /// checked directly against the rule [`Entry::symbol`] applies.
    #[test]
    fn the_symbol_is_the_rule_applied_to_the_key() {
        assert_eq!(symbol_for("host.HostFileSystem.readFile"), "buri_rt_host_file_system_read_file");
        assert_eq!(symbol_for("host.HostProcess.exitWith"), "buri_rt_host_process_exit_with");
        assert_eq!(symbol_for("host.HostStdout.println"), "buri_rt_host_stdout_println");
        assert_eq!(symbol_for("str.splitOnce"), "buri_rt_str_split_once");
        // The one that does not collapse, because the repetition is real:
        // `memory.rs` exports it under this name.
        assert_eq!(symbol_for("host.HostAllocator.allocate"), "buri_rt_host_allocator_allocate");
    }

    /// **The two thread-stack entries are symbols with no row, and that is
    /// the answer rather than an omission.**
    ///
    /// `buri_rt_stack_acquire` and `buri_rt_stack_release` are called by the
    /// frame-threaded backend's thread door, by name, out of a hand-written
    /// shim (`stencil/asm.rs`), and never by the LLVM backend, whose frames are
    /// machine frames. [`ENTRIES`] is keyed by *intrinsic key*, and these two
    /// have none — no Buri expression names them and none should.
    #[test]
    fn the_thread_stack_entries_have_symbols_and_no_row() {
        use crate::compiler::backend::task_thread;
        for symbol in [task_thread::STACK_ACQUIRE, task_thread::STACK_RELEASE] {
            assert!(symbol.starts_with("buri_rt_"), "{symbol} is not a runtime symbol");
            assert!(
                !ENTRIES.iter().any(|e| e.symbol() == symbol),
                "{symbol} gained a row: neither backend's door reaches it through the table"
            );
        }
    }

    /// A key the archive has no body for must not be in the table. If one of
    /// these gains a symbol, this test is the reminder to add its shape rather
    /// than to let the mangler invent it.
    #[test]
    fn the_unimplemented_surface_is_not_claimed() {
        for absent in [
            // Open-coded by both backends; see [`ENTRIES`].
            "str.concat",
            "str.format",
            "str.length",
            "list.length",
            "list.empty",
            // Outside the archive for the reasons [`ENTRIES`]'s comment gives.
            "list.map",
            "list.fold",
            "list.sortBy",
            "list.zip",
            "list.flatten",
            "json.decode",
            "json.encode",
            // These arrive qualified by their primitive (`derivePrimShow.U8`)
            // and so have no key of their own.
            "derivePrimShow",
            "derivePrimHash",
            // `platform/effect/testing`'s `net()` needs no row at all:
            // `TestNetwork` carries its responder as a value and
            // `TestNetwork.fetch` is a Buri body that calls it. A responder is a
            // `{ code, env }` pair the archive has no way to invoke.
            //
            // Open-coded, and named here so that "it has no symbol" and "the
            // backend cannot compile it" stay two different statements: the
            // allocator is two instructions on both native backends.
            "host_testing.alloc",
            "host_testing.TestAllocator.allocate",
            // Buri bodies: a `TestFileSystem` is a handle and a plan, and
            // `newFs` is the row that mints the handle.
            "host_testing.fs",
            "host_testing.TestFileSystem.readFile",
            "host_testing.TestFileSystem.faults",
            // `TestTasks.faults` is a Buri body over `replan` and `addFault`,
            // for the reason its two twins are: a plan is walked one entry at a
            // time, and the walk is the program's.
            "host_testing.faults",
            "host_testing.TestTasks.faults",
        ] {
            assert!(entry(absent).is_none(), "{absent}");
        }
    }

    /// `str.concat` has a body in the archive and deliberately no row, which
    /// is the one case where those two are not the same question. [`ENTRIES`]'
    /// own comment is the reason; this is the assertion that it stays true in
    /// both directions.
    #[test]
    fn str_concat_has_a_symbol_and_no_row() {
        assert_eq!(symbol_for("str.concat"), "buri_rt_str_concat");
        assert!(entry("str.concat").is_none());
    }

    /// `host.HostNetwork.fetch` hands its `Request` over by address and has a
    /// place for `NetError`'s sentence.
    #[test]
    fn host_net_fetch_passes_its_request_by_address() {
        let fetch = entry("host.HostNetwork.fetch").expect("a row for fetch");
        assert_eq!(fetch.symbol(), "buri_rt_host_network_fetch");
        assert_eq!(fetch.args, &[Dropped, Spilled]);
        assert_eq!(fetch.by_ref(), Some(1));
        assert_eq!(fetch.ret, Ret::ResMsg);
    }

    /// The shapes the backend supplies for itself emit a parameter and consume
    /// no argument, and everything else consumes exactly one. Both backends
    /// walk the C list with a cursor into the Buri list on precisely this
    /// invariant.
    #[test]
    fn only_the_generic_extras_consume_no_argument() {
        for shape in [Str, Bytes, List, Scalar, Dropped, Elems, Spilled, Step, Compute, Walk, Press] {
            assert!(shape.consumes(), "{shape:?}");
        }
        for shape in [Stride, Retain, Release, Equal] {
            assert!(!shape.consumes(), "{shape:?}");
            assert_eq!(shape.leaves(), 1);
        }
    }

    /// `stride` and `retain` travel together (`cli/runtime/lib.rs` §2 rule 4),
    /// a release never travels without them, and the equality rides with the
    /// release. A row with one and not the other would be a call with a
    /// parameter missing, which the C boundary does not diagnose.
    #[test]
    fn the_generic_words_come_in_pairs() {
        for e in ENTRIES {
            let count = |a: Arg| e.args.iter().filter(|x| **x == a).count();
            assert!(count(Stride) <= 1, "{}", e.key);
            assert_eq!(count(Stride), count(Retain), "{}", e.key);
            assert!(count(Release) <= count(Retain), "{}", e.key);
            assert_eq!(count(Equal), count(Release), "{}", e.key);
        }
    }

    /// Every closure-shaped argument is the **last** Buri argument, and a step
    /// is where [`step_call`] says the closure is.
    ///
    /// That is what lets the frame-threaded backend, which flattens the other
    /// arguments and appends the closure's words after them, emit the same C
    /// signature the LLVM backend writes at the closure's own position.
    #[test]
    fn a_closure_is_the_last_argument() {
        let mut checked = 0usize;
        for e in ENTRIES {
            let buri: Vec<Arg> = e.consumed().collect();
            for (at, arg) in buri.iter().enumerate() {
                if !matches!(arg, Step | Compute | Walk | Press) {
                    continue;
                }
                assert_eq!(at + 1, buri.len(), "{}: the closure is not last", e.key);
                assert_eq!(
                    e.args.last(),
                    Some(arg),
                    "{}: something follows the closure in the C list",
                    e.key
                );
                checked += 1;
            }
            match (buri.iter().position(|a| *a == Step), step_call(e.key)) {
                (None, None) => {}
                (Some(at), Some(call)) => assert_eq!(at, call.func, "{}", e.key),
                (Some(_), None) => panic!("{} has a step and no `step_call` row", e.key),
                (None, Some(_)) => panic!("{} is runtime-driven and has no `Arg::Step`", e.key),
            }
        }
        // Two steps, the graph's two deferred bodies and the renderer's
        // `reactive`, three walks, five kept handlers and the two timers.
        assert_eq!(checked, 15);
    }

    /// The module a key's first segment names, for the keys whose operations
    /// are *declared* in Buri.
    ///
    /// `test.leave` and `test.replay` are absent because they are not: both
    /// runner hooks are built by `middle::monomorphize::leaving` and no
    /// declaration spells them.
    const DECLARED_IN: &[(&str, &str)] = &[
        ("actor", "core/actor"),
        ("alloc", "core/alloc"),
        ("bytes", "core/bytes"),
        ("character", "core/character"),
        ("crypto", "core/crypto"),
        ("host", "platform/host"),
        ("host_testing", "platform/effect/testing"),
        ("list", "core/list"),
        ("math", "core/math"),
        ("str", "core/str"),
        ("tasks", "core/tasks"),
    ];

    /// Every `fn <name>` in `source`, answered as the index of its `ctx`
    /// parameter — `None` where it has none.
    ///
    /// A set rather than one answer because the name is looked up without its
    /// owner: `Str.repeat` and `list.repeat` are two declarations of `repeat`,
    /// and they are in two modules, so within one module the answers agree or
    /// the lookup was not specific enough to assert on. The caller checks that.
    fn declared_ctx(source: &str, name: &str) -> Vec<Option<usize>> {
        let mut out = Vec::new();
        for (at, _) in source.match_indices("fn ") {
            // `asfn foo` is not a declaration of `foo`.
            if source
                .get(..at)
                .and_then(|before| before.chars().next_back())
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            let Some(rest) = source.get(at.saturating_add(3)..) else { continue };
            let Some(tail) = rest.strip_prefix(name) else { continue };
            // `fn splitOnce` must not answer for `fn split`, so the character
            // after the name has to end it.
            if tail.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            // Past the generics, if any, to the parameter list's own `(`.
            let Some(open) = tail.find('(') else { continue };
            let Some(head) = tail.get(..open) else { continue };
            if head.contains(')') || head.contains('{') {
                continue;
            }
            let Some(after) = tail.get(open.saturating_add(1)..) else { continue };
            let mut depth = 0usize;
            let mut params: Vec<String> = Vec::new();
            let mut piece = String::new();
            for c in after.chars() {
                match c {
                    // `>` closes a generic argument list and also ends the `=>`
                    // of a function type; the saturating decrement is what lets
                    // one loop read both without counting arrows.
                    '(' | '[' | '<' => {
                        depth = depth.saturating_add(1);
                        piece.push(c);
                    }
                    ')' if depth == 0 => break,
                    ')' | ']' | '>' => {
                        depth = depth.saturating_sub(1);
                        piece.push(c);
                    }
                    ',' if depth == 0 => params.push(std::mem::take(&mut piece)),
                    _ => piece.push(c),
                }
            }
            if !piece.trim().is_empty() {
                params.push(piece);
            }
            out.push(
                params.iter().position(|p| p.split(':').next().is_some_and(|n| n.trim() == "ctx")),
            );
        }
        out
    }

    /// Every **declaration's** `ctx` parameter is [`Arg::Dropped`], checked
    /// against the declaration rather than against a second list.
    ///
    /// This is the test that would have caught the bug [`Arg::Dropped`]'s
    /// comment describes: a context is dropped whatever it weighs, so which
    /// argument it is has to come from the one place that cannot be wrong
    /// about it — the signature.
    #[test]
    fn every_declared_context_is_dropped() {
        let module = |path: &str| {
            crate::compiler::standard_library::MODULES
                .iter()
                .find(|m| m.path == path)
                .map(|m| m.source)
        };
        let mut checked = 0usize;
        let mut contexts = 0usize;
        for entry in ENTRIES {
            let Some((_, path)) = DECLARED_IN
                .iter()
                .find(|(prefix, _)| entry.key.split('.').next() == Some(prefix))
            else {
                continue;
            };
            let source = module(path).unwrap_or_else(|| panic!("no module at {path}"));
            let name = entry.key.rsplit('.').next().unwrap_or(entry.key);
            let found = declared_ctx(source, name);
            // `str.equal` and `str.hash` are `semantics/builtins.rs`'s, declared
            // on every primitive rather than written in `core/str` — so there
            // is nothing here to read, and neither takes a context.
            if found.is_empty() {
                continue;
            }
            assert!(
                found.iter().all(|a| *a == found[0]),
                "{}: two declarations of `{name}` in {path} disagree about `ctx`",
                entry.key
            );
            if let Some(at) = found[0] {
                assert!(entry.dropped(at), "{}: argument {at} is the context", entry.key);
                contexts += 1;
            }
            checked += 1;
        }
        // A scan that matched nothing would pass every assertion above.
        assert!(checked > 140, "only {checked} rows were read against a declaration");
        assert_eq!(contexts, 56);
    }

    /// The two places a context sits, by example, so that the indices are
    /// legible without opening `core/list`.
    #[test]
    fn a_receiver_shifts_the_context_by_one() {
        let dropped = |key: &str, at: usize| entry(key).is_some_and(|e| e.dropped(at));
        assert!(dropped("list.push", 1));
        assert!(dropped("list.repeat", 0));
        assert!(dropped("str.fromInt", 0));
        assert!(dropped("str.split", 1));
        // `get` takes no context — it allocates nothing.
        assert!(!dropped("list.get", 1));
        assert!(!dropped("list.get", 0));
    }

    // -- the host surface, against the effect that declares it ---------------

    /// One standard-library module's text.
    ///
    /// Two modules declare effects that this table has rows for: `platform/effect`,
    /// and `core/fs`, which declares `FileSystemRead` and `FileSystemWrite` because their
    /// methods name a `Path` and `core/path` names `Allocator`.
    fn module_source(path: &str) -> &'static str {
        crate::compiler::standard_library::MODULES
            .iter()
            .find(|m| m.path == path)
            .unwrap_or_else(|| panic!("`{path}` is a module"))
            .source
    }

    /// The method names one `effect` block declares, in declaration order.
    ///
    /// A scan of the source rather than a second list: a method added to the
    /// effect and forgotten here would be exactly the gap this is checking for.
    fn effect_methods(module: &str, effect: &str) -> Vec<String> {
        let source = module_source(module);
        let body = source
            .split(&format!("export effect {effect} {{"))
            .nth(1)
            .unwrap_or_else(|| panic!("no `effect {effect}` in `{module}`"))
            .split("\n}")
            .next()
            .unwrap_or_else(|| panic!("`effect {effect}` never closes"));
        let mut out = Vec::new();
        for line in body.lines() {
            let Some(rest) = line.trim().strip_prefix("fn ") else { continue };
            let Some(name) = rest.split('(').next() else { continue };
            out.push(name.to_string());
        }
        out
    }

    /// **Every operation of the filesystem, `Environment` and `Stdin` has a row.**
    ///
    /// This is buri-lang/buri#36 as an assertion. `cli/runtime/host.rs` had a
    /// body for all sixteen and the tables had a row for one of them
    /// (`fileExists`), so a native binary that bound the filesystem was refused
    /// before code generation while the same program ran on JavaScript.
    ///
    /// Read off the effect rather than listed here, so that the next operation
    /// is covered by the commit that declares it rather than by somebody
    /// remembering this test.
    #[test]
    fn every_operation_of_the_host_file_and_environment_effects_has_a_row() {
        let mut checked = 0usize;
        for (module, effect, host) in [
            ("core/fs", "FileSystemRead", "HostFileSystem"),
            ("core/fs", "FileSystemWrite", "HostFileSystem"),
            ("platform/effect", "Environment", "HostEnvironment"),
            ("platform/effect", "Stdin", "HostStdin"),
        ] {
            let methods = effect_methods(module, effect);
            assert!(
                methods.len() >= 2,
                "the scan found only {} operations of `{effect}`",
                methods.len()
            );
            for method in methods {
                let key = format!("host.{host}.{method}");
                assert!(
                    entry(&key).is_some(),
                    "`{key}` is declared on `effect {effect}` and has no row, so a native \
                     program binding it is refused before code generation"
                );
                checked += 1;
            }
        }
        assert!(checked >= 16, "only {checked} operations were checked");
    }

    /// **Which entries carry §2.1's message, as a written list.**
    ///
    /// A column can be wrong in a way a type cannot — nothing links this table
    /// to `cli/runtime/host.rs`'s parameter lists, because the archive is
    /// `include_bytes!`d rather than linked against — so the judgement is
    /// written down here and not left to a reader to reconstruct.
    ///
    /// Two claims, and the second is the one worth the test. Every fallible
    /// operation of the filesystem carries a message, because `ENOTEMPTY` and `EISDIR`
    /// have no `IoError` variant at all and the string is the only place the
    /// failure says which it was. `Tcp`'s three fallible operations are on the
    /// same list for the same reason: a reset connection, a broken pipe and a
    /// host with no route to it are three failures `IoError` names none of. **The five stream writers do not**, and
    /// `cli/runtime/host.rs`'s `reported` is the other half of that: those five
    /// take no out-pointer, so a row that gained one here would hand the archive
    /// an argument it has no parameter for. The reason they were left out is a
    /// measurement rather than a preference — the pointer is an address into the
    /// destination, so a function that prints stops keeping its `Result` in
    /// registers, which `cli/tests/native/llvm.rs`'s
    /// `a_hot_function_has_no_allocas` is the assertion about.
    #[test]
    fn only_the_filesystem_and_one_double_carry_a_message() {
        let mut carrying: Vec<&str> =
            ENTRIES.iter().filter(|e| e.ret == Ret::ResMsg).map(|e| e.key).collect();
        carrying.sort_unstable();
        assert_eq!(
            carrying,
            vec![
                "host.HostFileSystem.appendFile",
                "host.HostFileSystem.copyFile",
                "host.HostFileSystem.makeDir",
                "host.HostFileSystem.metadata",
                "host.HostFileSystem.readDir",
                "host.HostFileSystem.readFile",
                "host.HostFileSystem.readFileBytes",
                "host.HostFileSystem.readRange",
                "host.HostFileSystem.realPath",
                "host.HostFileSystem.removeDir",
                "host.HostFileSystem.removeFile",
                "host.HostFileSystem.renameFile",
                "host.HostFileSystem.syncFile",
                "host.HostFileSystem.writeFile",
                "host.HostFileSystem.writeFileBytes",
                // `BadUrl` and `Transport` carry the only actionable half of a
                // failed request.
                "host.HostNetwork.fetch",
                // Starting a program fails the way the filesystem does and for
                // the same reason: `ENOEXEC` and `E2BIG` have no `IoError`
                // variant either, and the string is the only place a refused
                // `spawn` says which it was.
                "host.HostSpawn.spawnProcess",
                "host.HostTcp.tcpConnect",
                "host.HostTcp.tcpRead",
                "host.HostTcp.tcpWrite",
                // Two doubles with a sentence to give. A `TestFileSystem` whose
                // directory still holds something answers `.Other` for the same
                // reason a real one does, and a `readRange` at a negative
                // offset says so — both in the words the JavaScript double
                // writes, so one conformance block reads the same on both
                // backends.
                "host_testing.fsReadRange",
                "host_testing.fsRemoveDir",
            ]
        );
        for writer in [
            "host.HostStdout.print",
            "host.HostStdout.println",
            "host.HostStdout.writeBytes",
            "host.HostStderr.eprint",
            "host.HostStderr.eprintln",
        ] {
            assert_eq!(
                entry(writer).map(|e| e.ret),
                Some(Ret::Res),
                "`{writer}` gained the message shape, and `cli/runtime/host.rs`'s writers take \
                 no out-pointer for one"
            );
        }
    }

    /// `IoError` is the shape `cli/runtime/lib.rs` §2.1's **message** rule was
    /// written for, and this is the assertion that ties the rule to the type.
    ///
    /// The rule itself is a function of a layout
    /// (`runtime_native::error_message_offset`, with its own rows over
    /// hand-built ones); what a layout cannot say is *which* declaration it came
    /// from. So this reads `platform/effect`: exactly one variant of `IoError`
    /// carries anything, it is the last, and what it carries is one `Str`. An
    /// eighth variant with a payload, or a payload on any of the first six,
    /// takes the filesystem back out of the message shape and every row above
    /// with it.
    #[test]
    fn the_message_shape_is_the_one_io_error_has() {
        let source = module_source("platform/effect");
        let body = source
            .split("export enum IoError {")
            .nth(1)
            .expect("`IoError` is declared in `platform/effect`")
            .split("\n}")
            .next()
            .expect("`IoError` never closes");
        let variants: Vec<&str> = body
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//"))
            .map(|l| l.trim_end_matches(','))
            .collect();
        assert_eq!(variants.len(), 7, "`IoError` has {} variants: {variants:?}", variants.len());
        let (last, rest) = variants.split_last().expect("seven variants");
        for variant in rest {
            assert!(
                !variant.contains('('),
                "`IoError.{variant}` carries a payload, and §2.1's message shape admits one \
                 payload variant and only in last place"
            );
        }
        assert_eq!(
            *last, "Other(Str)",
            "§2.1's message shape is one trailing `Str`, and `IoError`'s last variant is not one"
        );
    }

    /// No two rows may claim the same key: `entry` answers the first, so a
    /// duplicate would be a row that silently never runs.
    #[test]
    fn no_key_appears_twice() {
        let mut keys: Vec<&str> = ENTRIES.iter().map(|e| e.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "a key is in the table twice");
    }
}
