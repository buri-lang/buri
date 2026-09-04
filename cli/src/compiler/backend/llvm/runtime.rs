//! The `buri_rt_*` boundary: which intrinsics this backend has a symbol for,
//! and what that symbol's C signature is.
//!
//! `cli/runtime/lib.rs`'s module comment is the contract; this file is the
//! transcription of it that generated code is emitted against. A disagreement
//! between the two is a miscompile that only shows up as a wrong answer at run
//! time, which is why the shapes are named ([`Arg`], [`Ret`]) rather than
//! spelled out per symbol as parameter counts.
//!
//! # Why a table and not a mangling
//!
//! `cli/runtime/lib.rs` §1 states the rule — "every exported symbol is
//! `buri_rt_` followed by `snake_case`", so `host.HostFs.readFile` is
//! `buri_rt_host_fs_read_file` — and a fifteen-line mangler would produce that
//! string. It would also produce `buri_rt_str_concat` for `str.concat` and
//! `buri_rt_list_len` for `list.len`, **neither of which exists**: the archive
//! is deliberately not the whole of the 203-function intrinsic surface
//! (`cli/runtime/lib.rs` §0), and what is not in it is either generated code —
//! `str.concat` is an allocation and two copies — or a later wave.
//!
//! A mangler would therefore turn "this toolchain cannot compile that program
//! yet" into a link error naming a symbol, or worse into a call to something
//! that happens to exist. A table turns it into
//! [`super::Llvm::missing_intrinsics`], which is asked *before* a second is
//! spent in LLVM — which is the reason that hook is on the trait
//! (`backend/mod.rs`, `design/TODO.md#the-native-backend`).
//!
//! So the mangler lives in `backend/runtime_native.rs` as `symbol_for`, and it
//! is used only to *name* the symbol a table entry claims; the table decides
//! what exists.

/// One entry in a runtime function's C parameter list.
///
/// `cli/runtime/lib.rs` §2 rule 1: every parameter is a scalar leaf, flattened
/// in declaration order. A `Str` is three parameters and a `[T]` is two.
///
/// Most variants consume one Buri argument and emit its leaves. [`Arg::Stride`]
/// and [`Arg::Retain`] consume **no** Buri argument at all: they are §2 rule
/// 4's "a generic parameter is a pointer and a stride", where the two extra
/// words come from `middle::layout` and from the backend's own glue rather than
/// from the call. That is why this is a description of the *C* parameter list
/// walked with a cursor into the Buri one, and not a per-argument table: the
/// two lists have different lengths at every generic entry.
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
    /// Zero: a zero-sized `self` or context, dropped from the signature
    /// (VALUE-MODEL.md §8).
    Dropped,
    /// `ptr`, `len` — two, and the argument's element type is remembered for
    /// the [`Arg::Stride`] and [`Arg::Retain`] that follow it. The same two
    /// words [`Arg::List`] emits, kept a separate variant because "which type
    /// is `T`" is the question the generic entries are built around and a
    /// second reading of the argument list to answer it would be a place for
    /// the two to disagree.
    Elems,
    /// One pointer to a stack copy of a Buri argument whose type the runtime
    /// cannot name (§2 rule 4). Also remembers the element type, which is what
    /// makes `list.repeat` — whose only mention of `T` is the item — work.
    Spilled,
    /// `middle::layout`'s element stride, as an immediate. Consumes no Buri
    /// argument.
    Stride,
    /// The per-element retain glue, or null where the element type holds no
    /// counted pointers (`cli/runtime/list.rs`'s header). Consumes no Buri
    /// argument.
    Retain,
    // -- the closure trampoline ---------------------------------------------
    /// A **runtime-driven step**: four parameters, from one Buri closure
    /// argument (`backend/intrinsic_keys.rs`'s `step_call`).
    ///
    /// ```text
    ///   entry       the generated `ccc` thunk, `void(state, index, in, out)`
    ///   state       this backend's own record, opaque to the runtime
    ///   in_stride   the source element's stride
    ///   out_stride  the result element's stride
    /// ```
    ///
    /// It consumes the closure and emits none of its words: `{ code, env }` is
    /// this backend's business and reaches the runtime inside `state`, which is
    /// an entry-block `alloca` the call site fills in. What the runtime gets is
    /// a C function it can call once per element with three pointers, at every
    /// element type there is — which is [`Arg::Spilled`]'s answer to "the
    /// runtime cannot name `T`" applied to a *call* rather than to a value.
    ///
    /// The strides ride along rather than arriving as [`Arg::Stride`] because a
    /// step reads one element type and writes another, and `Arg::Stride` names
    /// exactly one — the element `generic_element` found. Two of them in one
    /// variant keeps "which stride is which" a property of this table instead
    /// of an ordering convention between two independent rows.
    Step,
}

impl Arg {
    /// How many C parameters this shape emits.
    pub fn leaves(self) -> usize {
        match self {
            Arg::Step => 4,
            Arg::Str => 3,
            Arg::Bytes | Arg::List | Arg::Elems => 2,
            Arg::Scalar | Arg::Spilled | Arg::Stride | Arg::Retain => 1,
            Arg::Dropped => 0,
        }
    }

    /// Whether this shape takes the next Buri argument. The two shapes the
    /// backend supplies for itself do not.
    pub fn consumes(self) -> bool {
        !matches!(self, Arg::Stride | Arg::Retain)
    }
}

/// What comes back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ret {
    /// Nothing, and the Buri result is `()`.
    Void,
    /// One scalar, at the Buri result's own register shape.
    Scalar,
    /// Nothing, and the call does not come back (`SPEC 6.9`).
    NoReturn,
    /// One integer of exactly this many bits, which is **not** the dest's
    /// register shape and is narrowed to it at the call site.
    ///
    /// The C boundary has no `i1` and no `i8` enum tag: `buri_rt_str_eq`
    /// returns `u8` and `buri_rt_str_compare` returns `i32`, while `Bool` is an
    /// `i1` and `Order` is whatever width `middle::layout` gave a three-variant
    /// bare tag. Declaring the import at the dest's width instead would be a
    /// return-register disagreement with the archive that nothing diagnoses —
    /// the values happen to be small enough that it would usually work, which
    /// is the worst kind of ABI bug.
    Int(u32),
    /// An aggregate written through a trailing out-pointer (§2 rule 2).
    ///
    /// The emitter allocates one entry-block `alloca` at the *dest's own*
    /// layout, passes it as the last parameter, and loads the dest's slots back
    /// out of it. `BuriStr` and `BuriList` are `#[repr(C)]` records of exactly
    /// the words `middle::layout` gives `Str` and `[T]`, which is what makes
    /// "the dest's layout" the right description of the buffer and saves the
    /// table from carrying a second spelling of it.
    Out,
    /// An `i32` discriminant plus a payload through a trailing out-pointer
    /// (§2 rule 3).
    ///
    /// [`BURI_OK`] is the success arm and `0` is `.None`. The out-pointer is
    /// the dest enum's own buffer offset to where `middle::layout` put
    /// `.Some`'s payload, so the runtime writes the payload *in place* and the
    /// backend only has to settle the discriminant — a tag store for
    /// `EnumRepr::Tagged`, nothing at all for `EnumRepr::Niche`, whose `.Some`
    /// is exactly "the payload with its niche pointer non-null".
    Sum,
    /// [`Ret::Res`], and the entry **also writes `E`'s message** through one
    /// more trailing out-pointer (`cli/runtime/lib.rs` §2.1's message shape).
    ///
    /// A column rather than a fact read off `E`, for the reason
    /// `backend/runtime_table.rs`'s twin states in full: whether an enum error
    /// is named by an index is the type's business, and whether an entry has
    /// anything to *say* when it names the payload-carrying variant is the
    /// implementation's. `Fs` carries a message because `ENOTEMPTY` and
    /// `EISDIR` have no `IoError` variant at all; the stream writers do not,
    /// because the pointer is an address into the destination and a function
    /// that prints would stop keeping its `Result` in registers.
    ResMsg,
    /// A `Result<T, E>`: an `i32` discriminant, `.Ok`'s payload through a
    /// trailing out-pointer, and an error variant named by its index
    /// (`cli/runtime/lib.rs` §2.1).
    ///
    /// [`Ret::Sum`] with the failure side carrying information. An `Option`'s
    /// `.None` is one value and needs no number; a `Result`'s `.Err` is a value
    /// of `E`, and `0 ..= n` says which — of a variant §2.1 restricts to
    /// carrying no fields, so the tag is the whole of it.
    ///
    /// The out-pointer is **omitted where `T` is zero-sized**
    /// (`TestFs.writeFile`'s `Result<(), IoError>`), for the reason [`Ret::Out`]
    /// omits it: a parameter for a value with no bytes is one the archive and
    /// the backend can disagree about for free.
    Res,
}

/// One runtime entry this backend can emit a call to.
pub struct Entry {
    /// The intrinsic key `monomorphize` built: `host.HostStdout.println`.
    pub key: &'static str,
    /// The exported symbol, per `cli/runtime/lib.rs` §1.
    pub symbol: &'static str,
    /// One per Buri parameter, in declaration order, *including* the zero-sized
    /// `self` — so the list can be checked against the IR signature directly
    /// rather than after a filtering step whose result nothing verifies.
    pub args: &'static [Arg],
    pub ret: Ret,
}

/// The discriminant a fallible runtime entry returns for its success arm.
///
/// `cli/runtime/lib.rs`'s `BURI_OK`, transcribed rather than imported: the
/// runtime is a separate crate compiled for the *host* and linked as an
/// archive, so there is no path from here to its constants. The value is `-1`
/// rather than `0` so that an error variant's index is its index, and a backend
/// that gets the sign wrong fails immediately instead of silently reporting the
/// first error arm.
pub const BURI_OK: i64 = -1;

/// The entries this backend implements.
///
/// Deliberately still a subset. What is *in* it is now the whole of the shapes
/// `cli/runtime/lib.rs` §2 describes — a scalar result, an out-pointer (§2 rule
/// 2), a discriminant plus an out-pointer (§2 rule 3), and a generic parameter
/// carrying its stride and its retain glue (§2 rule 4) — because each of those
/// is one mechanism in [`super::emit`] rather than one special case per symbol.
///
/// What is deliberately still out, and why, so that a reader looking for one of
/// these finds the reason rather than an absence:
///
///  * **`str.concat`, `str.format`, `str.len`, `list.len`.** Open-coded in
///    [`super::emit::Unit::open_coded`]: each is an allocation and two copies,
///    a no-op, a masked load, or a word this backend already has the address
///    of, and a call would cost more than the sequence it stands for.
///  * **`list.empty`.** `cli/runtime/list.rs`'s `block` answers a **null**
///    `ptr` for an empty list, and there is no exported symbol for it anyway;
///    `[]` is `Inst::MakeArray` with no elements, which this backend already
///    emits as a real one-block allocation.
///  * **`json.*`, and every `list.*` entry taking a closure** — `map`,
///    `filter`, `fold`, `any`, `all`, `find`, `findIndex`, `count`, `sortBy`,
///    `zip`, `flatten`. `cli/runtime/list.rs`'s header states why they are not
///    in the archive: a Buri closure's `code` is a thunk at the *flattened*
///    signature of its own element type, so calling one from C would mean
///    synthesizing a parameter list that depends on `T`. Ten of the eleven are
///    emitted by [`super::emit::Unit::list_closure`] instead, which is a
///    different statement from being here.
pub const ENTRIES: &[Entry] = &[
    // -- the text streams ---------------------------------------------------
    //
    // `self` is `HostStdout`, an empty struct, so it is dropped; the argument
    // is a `Template`, which is a `Str` (VALUE-MODEL.md §3.3).
    Entry {
        key: "host.HostStdout.print",
        symbol: "buri_rt_host_stdout_print",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host.HostStdout.println",
        symbol: "buri_rt_host_stdout_println",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host.HostStderr.eprint",
        symbol: "buri_rt_host_stderr_eprint",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host.HostStderr.eprintln",
        symbol: "buri_rt_host_stderr_eprintln",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Res,
    },
    // Bytes, not text: no `base`, because the runtime writes them and keeps
    // nothing.
    Entry {
        key: "host.HostStdout.writeBytes",
        symbol: "buri_rt_host_stdout_write_bytes",
        args: &[Arg::Dropped, Arg::List],
        ret: Ret::Res,
    },
    // -- Fs, the whole effect ----------------------------------------------
    //
    // Twelve operations, and until they landed no native backend had one of
    // them: a binary that bound `Fs: host.fs` was refused before code
    // generation while `cli/runtime/host.rs` had a body for every one
    // (buri-lang/buri#36). What was missing was never the body and never the
    // shape of the *arguments* — it was the shape of the **error**. All eleven
    // fallible ones answer `Result<T, IoError>`, and `IoError.Other(Str)` is
    // what a real filesystem answers for every kind the six classified variants
    // do not name, so a row before §2.1's message shape would have made every
    // unclassified failure `.Other("")`.
    //
    // *Where* the message goes is not a column: `error_message_offset` reads it
    // off `IoError`'s layout. *Whether an entry has one* is [`Ret::ResMsg`],
    // which these eleven carry and the five stream writers above do not.
    //
    // `self` is `HostFs`, an empty struct, so `Arg::Dropped` leads every row.
    // A `Str` body is three parameters and a `[U8]` body is two, which is the
    // difference between `writeFile` and `writeFileBytes`.
    Entry {
        key: "host.HostFs.readFile",
        symbol: "buri_rt_host_fs_read_file",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::ResMsg,
    },
    Entry {
        key: "host.HostFs.writeFile",
        symbol: "buri_rt_host_fs_write_file",
        args: &[Arg::Dropped, Arg::Str, Arg::Str],
        ret: Ret::ResMsg,
    },
    // The one operation of `Fs` that cannot fail, so the one that is not a
    // `Result`: a `Bool` comes back as the `u8` a C boundary has for it.
    Entry {
        key: "host.HostFs.fileExists",
        symbol: "buri_rt_host_fs_file_exists",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Int(8),
    },
    Entry {
        key: "host.HostFs.readDir",
        symbol: "buri_rt_host_fs_read_dir",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::ResMsg,
    },
    Entry {
        key: "host.HostFs.readFileBytes",
        symbol: "buri_rt_host_fs_read_file_bytes",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::ResMsg,
    },
    Entry {
        key: "host.HostFs.writeFileBytes",
        symbol: "buri_rt_host_fs_write_file_bytes",
        args: &[Arg::Dropped, Arg::Str, Arg::List],
        ret: Ret::ResMsg,
    },
    Entry {
        key: "host.HostFs.appendFile",
        symbol: "buri_rt_host_fs_append_file",
        args: &[Arg::Dropped, Arg::Str, Arg::List],
        ret: Ret::ResMsg,
    },
    Entry {
        key: "host.HostFs.renameFile",
        symbol: "buri_rt_host_fs_rename_file",
        args: &[Arg::Dropped, Arg::Str, Arg::Str],
        ret: Ret::ResMsg,
    },
    Entry {
        key: "host.HostFs.removeFile",
        symbol: "buri_rt_host_fs_remove_file",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::ResMsg,
    },
    Entry {
        key: "host.HostFs.removeDir",
        symbol: "buri_rt_host_fs_remove_dir",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::ResMsg,
    },
    Entry {
        key: "host.HostFs.makeDir",
        symbol: "buri_rt_host_fs_make_dir",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::ResMsg,
    },
    Entry {
        key: "host.HostFs.syncFile",
        symbol: "buri_rt_host_fs_sync_file",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::ResMsg,
    },
    // -- Env, and Stdin beside it -------------------------------------------
    //
    // Four rows and no new shape between them, which is what made them the
    // other half of the same gap: an `Option<Str>`, a `[Str]`, an `Option<Str>`
    // and an `Option<[U8]>` are `Ret::Sum` and `Ret::Out`, both of which this
    // table has had since it was written. They were absent because no slice had
    // wired the host surface up, and a program cannot read its own arguments
    // without them.
    Entry {
        key: "host.HostEnv.variable",
        symbol: "buri_rt_host_env_variable",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Sum,
    },
    Entry {
        key: "host.HostEnv.args",
        symbol: "buri_rt_host_env_args",
        args: &[Arg::Dropped],
        ret: Ret::Out,
    },
    Entry {
        key: "host.HostStdin.readLine",
        symbol: "buri_rt_host_stdin_read_line",
        args: &[Arg::Dropped],
        ret: Ret::Sum,
    },
    Entry {
        key: "host.HostStdin.readBytes",
        symbol: "buri_rt_host_stdin_read_bytes",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Sum,
    },
    // -- the scalar capabilities -------------------------------------------
    Entry {
        key: "host.HostClock.nowMillis",
        symbol: "buri_rt_host_clock_now_millis",
        args: &[Arg::Dropped],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host.HostClock.sleepMillis",
        symbol: "buri_rt_host_clock_sleep_millis",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Void,
    },
    Entry {
        key: "host.HostRand.nextInt",
        symbol: "buri_rt_host_rand_next_int",
        args: &[Arg::Dropped, Arg::Scalar, Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host.HostRand.nextFloat",
        symbol: "buri_rt_host_rand_next_float",
        args: &[Arg::Dropped],
        ret: Ret::Scalar,
    },
    // The `Alloc` capability's charge, which is the identity function plus a
    // budget check in the runtime (`memory.rs:330`).
    Entry {
        key: "host.HostAlloc.allocate",
        symbol: "buri_rt_host_alloc_allocate",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host.HostProc.exitWith",
        symbol: "buri_rt_host_proc_exit_with",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::NoReturn,
    },
    // -- Tasks --------------------------------------------------------------
    //
    // `parallel(self, ctx, items, f)`, the second key of the closure trampoline
    // and the one it was built for. `self` is `HostTasks`, an empty struct, so
    // it is `Arg::Dropped` like every other host receiver; `ctx` is the caller's
    // whole context and is `Arg::Dropped` too — the runtime allocates through
    // `buri_rt_alloc` and reads no capability, so no context crosses the archive
    // boundary — but it is not *unused*: the step is handed it, out of the state
    // record `Arg::Step` builds, which is why the shared table names its index.
    // `items` is `Arg::Elems`, because the source's element type is what the
    // entry thunk is generated at; `f` is `Arg::Step`, the four words.
    //
    // The body is in `cli/runtime/rt.rs` behind feature `net`, which is why
    // `runtime_native::net_intrinsic` names the `host.HostTasks.*` family: a
    // toolchain built without the reactor refuses this key with a sentence
    // before code generation rather than with a missing symbol from `cc`.
    Entry {
        key: "host.HostTasks.parallel",
        symbol: "buri_rt_host_tasks_parallel",
        args: &[Arg::Dropped, Arg::Dropped, Arg::Elems, Arg::Step],
        ret: Ret::Out,
    },
    // -- Listen, and Sockets beside it --------------------------------------
    //
    // Seven operations and no closure among them: the accept loop lives in
    // `core/net/server`, in Buri, so none of these is an [`Arg::Step`] and the
    // trampoline above is untouched by an entire server landing — F3's worker
    // per handler included, because that fan-out is `Tasks.parallel`'s row and
    // not one of these.
    //
    // Six of them answer `Result<_, ServeError>` and `ServeError` is a
    // **struct**, so those take `lib.rs` §2.1's second shape: the error crosses
    // whole through an out-pointer and the discriminant says only that it
    // failed. `bytes.fromUtf8` is the other row shaped that way.
    //
    // `listenAccept` and `listenRequest` are two rows because only the first of
    // them waits; `effect Listen` is where that is argued.
    //
    // Each knob is its own argument rather than an options struct, because
    // this table describes a C parameter list one *Buri argument* at a time:
    // an aggregate argument has no `Arg` to be, and inventing one would be a
    // shape only this backend could spell. `effect Listen` says the same thing
    // from the declaration's side.
    Entry {
        key: "host.HostListen.listenBind",
        symbol: "buri_rt_host_listen_bind",
        args: &[Arg::Dropped, Arg::Str, Arg::Scalar, Arg::List, Arg::Scalar, Arg::Scalar],
        ret: Ret::Res,
    },
    Entry {
        key: "host.HostListen.listenAccept",
        symbol: "buri_rt_host_listen_accept",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Res,
    },
    Entry {
        key: "host.HostListen.listenRequest",
        symbol: "buri_rt_host_listen_request",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Res,
    },
    Entry {
        key: "host.HostListen.listenRespond",
        symbol: "buri_rt_host_listen_respond",
        args: &[Arg::Dropped, Arg::Scalar, Arg::Scalar, Arg::List, Arg::List],
        ret: Ret::Res,
    },
    Entry {
        key: "host.HostListen.listenClose",
        symbol: "buri_rt_host_listen_close",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Void,
    },
    // The upgrade and the read after it. Both take a bare handle and answer a
    // `Result<_, ServeError>`; the first's `.Ok` is a scalar, the second's is a
    // `Received` that comes back whole through an out-pointer, exactly as
    // `listenRequest`'s `Request` does. Neither is an [`Arg::Step`] either — a
    // socket's loop and a socket's state are both Buri's.
    Entry {
        key: "host.HostListen.listenUpgrade",
        symbol: "buri_rt_host_listen_upgrade",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Res,
    },
    Entry {
        key: "host.HostListen.listenReceive",
        symbol: "buri_rt_host_listen_receive",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Res,
    },
    // The socket half: `()` on all three, because a frame is enqueued rather
    // than delivered. The text is `Arg::Str` and the payload `Arg::List` for
    // `writeBytes`'s reason — a runtime that keeps nothing needs no owning
    // block — and the close reason is a `Str` for the same reason.
    Entry {
        key: "host.HostSockets.socketSendText",
        symbol: "buri_rt_host_sockets_socket_send_text",
        args: &[Arg::Dropped, Arg::Scalar, Arg::Str],
        ret: Ret::Void,
    },
    Entry {
        key: "host.HostSockets.socketSendBytes",
        symbol: "buri_rt_host_sockets_socket_send_bytes",
        args: &[Arg::Dropped, Arg::Scalar, Arg::List],
        ret: Ret::Void,
    },
    Entry {
        key: "host.HostSockets.socketClose",
        symbol: "buri_rt_host_sockets_socket_close",
        args: &[Arg::Dropped, Arg::Scalar, Arg::Scalar, Arg::Str],
        ret: Ret::Void,
    },
    // -- core/alloc's counters ----------------------------------------------
    //
    // `GeneralPurpose`, `Arena` and `FixedBuffer` carry a handle into a table
    // in `cli/runtime/memory.rs`, so every one of these is scalars in and a
    // scalar out with no `self` to drop: the handle is passed, not the
    // allocator. `charge` can end the process and still returns `Ret::Scalar`,
    // because it returns on every request that fits its budget.
    Entry {
        key: "alloc.newCounter",
        symbol: "buri_rt_alloc_new_counter",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "alloc.charge",
        symbol: "buri_rt_alloc_charge",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "alloc.count",
        symbol: "buri_rt_alloc_count",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "alloc.total",
        symbol: "buri_rt_alloc_total",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    // -- `core/alloc`'s scope (G4) ------------------------------------------
    //
    // `Scoped<C>` carries the wrapped context and an arena handle, and only the
    // handle reaches these: the arena is named by its `I64`, so every one of
    // them is scalars in and a scalar out with no `self` to drop, exactly like
    // the counters above. `arenaCreate` is the one row in this table with no
    // arguments at all.
    Entry { key: "alloc.arenaCreate", symbol: "buri_rt_alloc_arena_create", args: &[], ret: Ret::Scalar },
    Entry {
        key: "alloc.arenaAllocate",
        symbol: "buri_rt_alloc_arena_allocate",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "alloc.arenaRelease",
        symbol: "buri_rt_alloc_arena_release",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "alloc.arenaCount",
        symbol: "buri_rt_alloc_arena_count",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "alloc.arenaTotal",
        symbol: "buri_rt_alloc_arena_total",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    // G5's pair: which arena the platform allocator serves out of, for this
    // carrier and for the dynamic extent of `scoped`'s body.
    Entry {
        key: "alloc.arenaEnter",
        symbol: "buri_rt_alloc_arena_enter",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "alloc.arenaLeave",
        symbol: "buri_rt_alloc_arena_leave",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    // -- core/actor (F6) -----------------------------------------------------
    //
    // `Arg::Dropped` first on every row, and it is the *context*: `core/actor`
    // declares these as `fn <name><C: Tasks, …>(ctx: C, …)`, a module function
    // with the authority in its bound, so argument 0 crosses nothing.
    // `runtime_table.rs` says the same thing with a `ctx: Some(0)` column, and
    // `an_entry_names_the_context_the_other_table_drops` holds the two
    // together.
    //
    // `Arg::List` and not `Arg::Elems` at every carrier, which is the shape
    // decision this whole module rests on: a message crosses as a one-element
    // `[Carried<M>]` and the runtime moves the **block**, so there is no
    // element type to describe and no stride or retain glue to pass. What
    // would need `Arg::Elems` is a runtime that looked *inside* one, and none
    // of the nine bodies does.
    //
    // `Ret::Sum` on the six that answer an `Option`. The out-pointer is the
    // dest's own `.Some` payload — an `i64` for the two depths, a `BuriList`
    // for the four blocks — so a niche `Option<[T]>` is settled by the
    // non-null pointer the runtime wrote, which is what `core/actor`'s
    // `Carried<T>` guarantees.
    Entry {
        key: "actor.mailboxOpen",
        symbol: "buri_rt_actor_mailbox_open",
        args: &[Arg::Dropped, Arg::List, Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "actor.mailboxPush",
        symbol: "buri_rt_actor_mailbox_push",
        args: &[Arg::Dropped, Arg::Scalar, Arg::List],
        ret: Ret::Sum,
    },
    Entry {
        key: "actor.mailboxPop",
        symbol: "buri_rt_actor_mailbox_pop",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Sum,
    },
    Entry {
        key: "actor.mailboxClose",
        symbol: "buri_rt_actor_mailbox_close",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Sum,
    },
    Entry {
        key: "actor.stateTake",
        symbol: "buri_rt_actor_state_take",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Sum,
    },
    Entry {
        key: "actor.statePut",
        symbol: "buri_rt_actor_state_put",
        args: &[Arg::Dropped, Arg::Scalar, Arg::List],
        ret: Ret::Sum,
    },
    Entry {
        key: "actor.replyOpen",
        symbol: "buri_rt_actor_reply_open",
        args: &[Arg::Dropped],
        ret: Ret::Scalar,
    },
    Entry {
        key: "actor.replyPut",
        symbol: "buri_rt_actor_reply_put",
        args: &[Arg::Dropped, Arg::Scalar, Arg::List],
        ret: Ret::Sum,
    },
    Entry {
        key: "actor.replyTake",
        symbol: "buri_rt_actor_reply_take",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Sum,
    },
    // -- core/str, the pure half (`cli/runtime/text.rs`) ---------------------
    //
    // Every one of these takes `self` as a full `Str`: the `base` is passed
    // even where the body ignores it, because §2 rule 1 flattens the *value*
    // and a runtime that later wants to retain a view has to have been handed
    // the block it is a view into.
    Entry {
        key: "str.charAt",
        symbol: "buri_rt_str_char_at",
        args: &[Arg::Str, Arg::Scalar],
        ret: Ret::Sum,
    },
    Entry {
        key: "str.slice",
        symbol: "buri_rt_str_slice",
        args: &[Arg::Str, Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry { key: "str.trim", symbol: "buri_rt_str_trim", args: &[Arg::Str], ret: Ret::Out },
    Entry {
        key: "str.trimStart",
        symbol: "buri_rt_str_trim_start",
        args: &[Arg::Str],
        ret: Ret::Out,
    },
    Entry { key: "str.trimEnd", symbol: "buri_rt_str_trim_end", args: &[Arg::Str], ret: Ret::Out },
    // `u8` out of C and `i1` into the IR — see [`Ret::Int`].
    Entry {
        key: "str.startsWith",
        symbol: "buri_rt_str_starts_with",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Int(8),
    },
    Entry {
        key: "str.endsWith",
        symbol: "buri_rt_str_ends_with",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Int(8),
    },
    Entry {
        key: "str.contains",
        symbol: "buri_rt_str_contains",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Int(8),
    },
    Entry {
        key: "str.indexOf",
        symbol: "buri_rt_str_index_of",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Sum,
    },
    Entry {
        key: "str.splitOnce",
        symbol: "buri_rt_str_split_once",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Sum,
    },
    // `Order` is `{ Less, Equal, Greater }` with no payload, so it is
    // `EnumRepr::Bare` and the result *is* the tag — `0`, `1`, `2` in
    // declaration order, which is the same numbering `text.rs` answers. No
    // `Ret::Sum` translation, because there is no success arm to distinguish.
    Entry {
        key: "str.compare",
        symbol: "buri_rt_str_compare",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Int(32),
    },
    // Not declared in `core/str`: this is the `Str` arm of an `Inst::Binary`
    // at `Prim::Str`, which is what `middle::derives` emits for a derived `Eq`
    // over a type with a `Str` in it (`derives.rs`'s `fn eq`).
    Entry {
        key: "str.eq",
        symbol: "buri_rt_str_eq",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Int(8),
    },
    Entry { key: "str.toInt", symbol: "buri_rt_str_to_int", args: &[Arg::Str], ret: Ret::Sum },
    Entry { key: "str.toFloat", symbol: "buri_rt_str_to_float", args: &[Arg::Str], ret: Ret::Sum },
    // -- core/str, the `<C: Alloc>` half ------------------------------------
    //
    // The context is an empty record in every implementation `core/host`
    // supplies, so it is zero-sized and dropped (VALUE-MODEL.md §8) — it is
    // still listed, because the list is checked against the IR signature and a
    // silently shorter one would misalign every argument after it.
    Entry {
        key: "str.split",
        symbol: "buri_rt_str_split",
        args: &[Arg::Str, Arg::Dropped, Arg::Str],
        ret: Ret::Out,
    },
    Entry {
        key: "str.splitAny",
        symbol: "buri_rt_str_split_any",
        args: &[Arg::Str, Arg::Dropped, Arg::Str],
        ret: Ret::Out,
    },
    Entry {
        key: "str.lines",
        symbol: "buri_rt_str_lines",
        args: &[Arg::Str, Arg::Dropped],
        ret: Ret::Out,
    },
    Entry {
        key: "str.replace",
        symbol: "buri_rt_str_replace",
        args: &[Arg::Str, Arg::Dropped, Arg::Str, Arg::Str],
        ret: Ret::Out,
    },
    Entry {
        key: "str.repeat",
        symbol: "buri_rt_str_repeat",
        args: &[Arg::Str, Arg::Dropped, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "str.toUpper",
        symbol: "buri_rt_str_to_upper",
        args: &[Arg::Str, Arg::Dropped],
        ret: Ret::Out,
    },
    Entry {
        key: "str.toLower",
        symbol: "buri_rt_str_to_lower",
        args: &[Arg::Str, Arg::Dropped],
        ret: Ret::Out,
    },
    Entry {
        key: "str.chars",
        symbol: "buri_rt_str_chars",
        args: &[Arg::Str, Arg::Dropped],
        ret: Ret::Out,
    },
    // `[Char]` in, and the runtime reads it as a block of `u32`s: `Arg::List`
    // rather than `Arg::Elems`, because `Char`'s stride is not a parameter
    // here — `buri_rt_str_from_chars` knows the element type it was written
    // for, which is the one `list.*` entry shape that is *not* generic.
    Entry {
        key: "str.fromChars",
        symbol: "buri_rt_str_from_chars",
        args: &[Arg::Dropped, Arg::List],
        ret: Ret::Out,
    },
    Entry {
        key: "str.fromInt",
        symbol: "buri_rt_str_from_int",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "str.fromFloat",
        symbol: "buri_rt_str_from_float",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Out,
    },
    // `fill` is a `Char`, which is a `U32` scalar — one leaf, not a `Str`'s
    // three. `$str_padStart` repeats the *whole* fill for `width - len(self)`
    // iterations, so a multi-scalar fill overshoots the width; that is a
    // peculiarity of the JavaScript answer rather than a design, and the
    // runtime reproduces it exactly (`cli/runtime/text.rs`).
    Entry {
        key: "str.padStart",
        symbol: "buri_rt_str_pad_start",
        args: &[Arg::Str, Arg::Dropped, Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "str.padEnd",
        symbol: "buri_rt_str_pad_end",
        args: &[Arg::Str, Arg::Dropped, Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    // `Hash`'s method, which starts from the FNV-1a seed and takes no
    // accumulator — unlike `derivePrimHash.Str`, which continues one and is
    // `buri_rt_hash_str`. Two symbols because they are two operations, and the
    // mangler's answer for this key happens to be the one the archive exports.
    Entry { key: "str.hash", symbol: "buri_rt_str_hash", args: &[Arg::Str], ret: Ret::Scalar },
    // -- core/list, the block-copying half (`cli/runtime/list.rs`) ----------
    //
    // `Arg::Stride` and `Arg::Retain` consume no Buri argument: they are the
    // backend's two words, and `cli/runtime/list.rs` puts them last, directly
    // before `out`, at every entry that takes them. So the Buri order and the C
    // order agree up to the point where the C one grows two more parameters —
    // which is what the cursor in `Unit::entry_args` walks.
    Entry {
        key: "list.get",
        symbol: "buri_rt_list_get",
        args: &[Arg::Elems, Arg::Scalar, Arg::Stride, Arg::Retain],
        ret: Ret::Sum,
    },
    Entry {
        key: "list.concat",
        symbol: "buri_rt_list_concat",
        args: &[Arg::Elems, Arg::Dropped, Arg::Elems, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    Entry {
        key: "list.push",
        symbol: "buri_rt_list_push",
        args: &[Arg::Elems, Arg::Dropped, Arg::Spilled, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    Entry {
        key: "list.reverse",
        symbol: "buri_rt_list_reverse",
        args: &[Arg::Elems, Arg::Dropped, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    Entry {
        key: "list.slice",
        symbol: "buri_rt_list_slice",
        args: &[Arg::Elems, Arg::Dropped, Arg::Scalar, Arg::Scalar, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    // `take(self, ctx, n)` and `drop(self, ctx, n)` are one `slice` each in the
    // archive, and the same three-word shape as `slice` here. They were absent
    // from this table and present in the debug backend's, which is a
    // disagreement between two transcriptions of one contract rather than a
    // difference between the backends — `data/lists.buri` under
    // `buri test --release` is what found it.
    Entry {
        key: "list.take",
        symbol: "buri_rt_list_take",
        args: &[Arg::Elems, Arg::Dropped, Arg::Scalar, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    Entry {
        key: "list.drop",
        symbol: "buri_rt_list_drop",
        args: &[Arg::Elems, Arg::Dropped, Arg::Scalar, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    Entry {
        key: "list.repeat",
        symbol: "buri_rt_list_repeat",
        args: &[Arg::Dropped, Arg::Spilled, Arg::Scalar, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    // `[Int]` and nothing generic: no stride, no retain, and an `Int` holds no
    // count to take.
    Entry {
        key: "list.range",
        symbol: "buri_rt_list_range",
        args: &[Arg::Dropped, Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    // -- core/math (`cli/runtime/math.rs`) ----------------------------------
    //
    // Nine of the twenty-two. The other thirteen are the transcendentals —
    // `cbrt`, `pow`, `exp`, `ln`, `log10`, `log2`, the six trigonometric ones
    // and `atan2` — and they are **deliberately** absent rather than a call to
    // libm. IEEE 754 does not fix their answers, so V8's fdlibm port and a
    // platform libm differ in the last bit, and a rendered `Float` shows all
    // seventeen digits of that difference. A named gap is a diagnostic; a libm
    // call is a conformance failure nobody can attribute.
    Entry { key: "math.sqrt", symbol: "buri_rt_math_sqrt", args: &[Arg::Scalar], ret: Ret::Scalar },
    Entry {
        key: "math.absFloat",
        symbol: "buri_rt_math_abs_float",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "math.floor",
        symbol: "buri_rt_math_floor",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry { key: "math.ceil", symbol: "buri_rt_math_ceil", args: &[Arg::Scalar], ret: Ret::Scalar },
    Entry {
        key: "math.trunc",
        symbol: "buri_rt_math_trunc",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "math.round",
        symbol: "buri_rt_math_round",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    // `Bool` out of C is a `u8`, and an `i1` into the IR — see [`Ret::Int`].
    Entry {
        key: "math.isNan",
        symbol: "buri_rt_math_is_nan",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    Entry {
        key: "math.isInfinite",
        symbol: "buri_rt_math_is_infinite",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    Entry {
        key: "math.isFinite",
        symbol: "buri_rt_math_is_finite",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    // Declared in `core/list` and implemented in `text.rs`, because joining is
    // a `Str` producer that happens to read a `[Str]`. The elements are read as
    // `BuriStr`s at a stride the runtime knows, so this is `Arg::List`.
    Entry {
        key: "list.join",
        symbol: "buri_rt_list_join",
        args: &[Arg::List, Arg::Dropped, Arg::Str],
        ret: Ret::Out,
    },
    // -- the closure trampoline, and its one pilot key ----------------------
    //
    // `list.mapCtxStep` is `list.mapCtx` with its step reached through the
    // generated `ccc` entry thunk of [`Arg::Step`] instead of through the loop
    // [`super::emit::Unit::list_closure`] emits. It is the *pilot* for that
    // mechanism and nothing in `core/list` uses it: those combinators keep
    // their loops, which are faster than a call per element can be. The
    // operation the trampoline exists for is `host.HostTasks.parallel`, whose
    // row is in the `core/host` block above.
    //
    // `Arg::Elems` and not `Arg::List`, because the source's element type is
    // what `generic_element` answers and what the entry thunk is generated at.
    // `Arg::Dropped` for the `Alloc` context, as every other `core/list` row
    // has it: the runtime allocates through `buri_rt_alloc`.
    Entry {
        key: "list.mapCtxStep",
        symbol: "buri_rt_list_map_ctx_step",
        args: &[Arg::Elems, Arg::Dropped, Arg::Step],
        ret: Ret::Out,
    },
    // -- core/bytes ---------------------------------------------------------
    //
    // `Arg::List` at every `[U8]` argument, and the two alternatives are both
    // wrong in ways nothing would diagnose. `Arg::Elems` would carry a stride
    // and a retain glue for a `T` that is fixed at `U8` and needs neither.
    // `Arg::Bytes` is the shape a **`Str`** takes when its base is dropped —
    // it emits `pieces.skip(1)`, so on a `[U8]`, whose two pieces are `ptr` and
    // `len`, it passes the length alone and the callee reads a pointer out of
    // it. That is what the first draft of these rows said, and it linked, ran,
    // and crashed `text/bytes.buri` and `proto/binary.buri` under `--release`
    // while the debug backend compiled both — it spreads from the IR and has no
    // per-argument table to get wrong.
    Entry {
        key: "bytes.toUtf8",
        symbol: "buri_rt_bytes_to_utf8",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Out,
    },
    Entry {
        key: "bytes.fromUtf8",
        symbol: "buri_rt_bytes_from_utf8",
        args: &[Arg::Dropped, Arg::List],
        ret: Ret::Res,
    },
    Entry {
        key: "bytes.f64ToBytes",
        symbol: "buri_rt_bytes_f64_to_bytes",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "bytes.f64FromBytes",
        symbol: "buri_rt_bytes_f64_from_bytes",
        args: &[Arg::List, Arg::Scalar],
        ret: Ret::Sum,
    },
    Entry {
        key: "bytes.f32ToBytes",
        symbol: "buri_rt_bytes_f32_to_bytes",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "bytes.f32FromBytes",
        symbol: "buri_rt_bytes_f32_from_bytes",
        args: &[Arg::List, Arg::Scalar],
        ret: Ret::Sum,
    },
    // -- core/char ----------------------------------------------------------
    //
    // Eight rows, and the five predicates are `Ret::Int(8)` rather than
    // `Ret::Scalar`: the C boundary has no `i1`, so the archive returns `u8`
    // and the call site narrows. `toUpper` and `toLower` *are* `Ret::Scalar`,
    // because a `Char` is an `i32` on both sides of the boundary.
    Entry {
        key: "char.isDigit",
        symbol: "buri_rt_char_is_digit",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    Entry {
        key: "char.isAlpha",
        symbol: "buri_rt_char_is_alpha",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    Entry {
        key: "char.isSpace",
        symbol: "buri_rt_char_is_space",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    Entry {
        key: "char.isUpper",
        symbol: "buri_rt_char_is_upper",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    Entry {
        key: "char.isLower",
        symbol: "buri_rt_char_is_lower",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    Entry {
        key: "char.toUpper",
        symbol: "buri_rt_char_to_upper",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "char.toLower",
        symbol: "buri_rt_char_to_lower",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "char.toDigit",
        symbol: "buri_rt_char_to_digit",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Sum,
    },
    // -- core/host/testing's stateful half ------------------------------------
    //
    // `cli/runtime/testing.rs`'s header is the argument for these being in the
    // archive rather than open-coded: each names a slot in one mutable table,
    // which is `runtime.js`'s `$t.h` written for a language that has statics.
    // The rows are `stencil/runtime.rs`'s, one for one.
    //
    // Two things are not obvious and are the same in both tables:
    //
    //  * **Every constructor is `Ret::Out`.** `struct TestStdout(I64)` is a
    //    struct, and `middle/layout.rs` gives every struct `Repr::Aggregate`
    //    however few fields it has, so the result is an aggregate and §2 rule 2
    //    puts it through an out-pointer. Declaring it as returning one word
    //    would agree with the archive by accident on both supported targets.
    //  * **`self` is `Arg::Scalar` and not `Arg::Dropped`.** These receivers
    //    carry the handle; `core/host`'s are empty structs and these are not,
    //    which is the distinction `native/llvm.rs`'s
    //    `a_stateful_context_is_dropped_at_the_runtime_boundary` exists for.
    //
    // `Arg::List` at every `[U8]` argument, for the reason `core/bytes`' group
    // above gives.
    //
    // A **builder** — `at`, `seed`, `variables`, `arguments` — takes its receiver
    // and answers a fresh handle through the out-pointer, so it is
    // `Arg::Scalar` in and `Ret::Out` out. The receiver is passed even where
    // the body ignores it (`at`, `seed`): the C signature is the Buri argument
    // list flattened, and dropping a parameter because one implementation has
    // no use for it is exactly the kind of disagreement nothing diagnoses.
    //
    // `proc` and `TestProc.exitWith` have no rows, for `TestNet.fetch`'s reason
    // rather than the allocator's: both are Buri bodies, so no key reaches this
    // table. `TestProc` records nothing because nothing can read it back.
    //
    // `alloc` and `TestAlloc.allocate` are open-coded (`emit.rs`).
    Entry {
        key: "host_testing.stdout",
        symbol: "buri_rt_host_testing_stdout",
        args: &[],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.stderr",
        symbol: "buri_rt_host_testing_stderr",
        args: &[],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestStdout.print",
        symbol: "buri_rt_host_testing_test_stdout_print",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.TestStdout.println",
        symbol: "buri_rt_host_testing_test_stdout_println",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.TestStdout.writeBytes",
        symbol: "buri_rt_host_testing_test_stdout_write_bytes",
        args: &[Arg::Scalar, Arg::List],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.TestStdout.captured",
        symbol: "buri_rt_host_testing_test_stdout_captured",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestStderr.eprint",
        symbol: "buri_rt_host_testing_test_stderr_eprint",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.TestStderr.eprintln",
        symbol: "buri_rt_host_testing_test_stderr_eprintln",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.TestStderr.captured",
        symbol: "buri_rt_host_testing_test_stderr_captured",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.stdin",
        symbol: "buri_rt_host_testing_stdin",
        args: &[],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestStdin.lines",
        symbol: "buri_rt_host_testing_test_stdin_lines",
        args: &[Arg::Scalar, Arg::List],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestStdin.bytes",
        symbol: "buri_rt_host_testing_test_stdin_bytes",
        args: &[Arg::Scalar, Arg::List],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestStdin.readLine",
        symbol: "buri_rt_host_testing_test_stdin_read_line",
        args: &[Arg::Scalar],
        ret: Ret::Sum,
    },
    Entry {
        key: "host_testing.TestStdin.readBytes",
        symbol: "buri_rt_host_testing_test_stdin_read_bytes",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Sum,
    },
    // The stream's log, read back. `calls` answers a `[Call]` the way `snapshot`
    // answers a `[(Str, Str)]`: `Ret::Out` writes the descriptor and the element
    // width is the record's own layout, which this table does not name.
    Entry {
        key: "host_testing.TestStdin.calls",
        symbol: "buri_rt_host_testing_test_stdin_calls",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    // `TestFs`'s twenty-two, every one over a **handle**: a `TestFs` is a handle
    // and a fault plan since the plan landed, and an argument crosses as its
    // leaves, so a row taking `self` would be handed three values where it
    // expects one. The eleven methods of `Fs` are Buri bodies over these rows,
    // and `host_testing.buri` says why the plan stays in the program — an
    // `IoError` carries a `Str` on `.Other`, which §2.1 cannot name.
    //
    // `snapshot` answers a `[(Str, Str)]`, which is a list like any other at
    // this boundary: `Ret::Out` writes the descriptor, and the element width is
    // the tuple's layout rather than anything this table names. The three
    // builders and `newFs` answer the handle itself and so are `Ret::Scalar`,
    // `newNet`'s shape.
    Entry {
        key: "host_testing.newFs",
        symbol: "buri_rt_host_testing_new_fs",
        args: &[],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.fsFiles",
        symbol: "buri_rt_host_testing_fs_files",
        args: &[Arg::Scalar, Arg::List],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.fsFilesBytes",
        symbol: "buri_rt_host_testing_fs_files_bytes",
        args: &[Arg::Scalar, Arg::List],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.fsReadOnly",
        symbol: "buri_rt_host_testing_fs_read_only",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.fsWithPlan",
        symbol: "buri_rt_host_testing_fs_with_plan",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.fsRead",
        symbol: "buri_rt_host_testing_fs_read",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.fsSnapshot",
        symbol: "buri_rt_host_testing_fs_snapshot",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.fsCalls",
        symbol: "buri_rt_host_testing_fs_calls",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.fsReadFile",
        symbol: "buri_rt_host_testing_fs_read_file",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.fsWriteFile",
        symbol: "buri_rt_host_testing_fs_write_file",
        args: &[Arg::Scalar, Arg::Str, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.fsFileExists",
        symbol: "buri_rt_host_testing_fs_file_exists",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Int(8),
    },
    Entry {
        key: "host_testing.fsReadDir",
        symbol: "buri_rt_host_testing_fs_read_dir",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.fsReadFileBytes",
        symbol: "buri_rt_host_testing_fs_read_file_bytes",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.fsWriteFileBytes",
        symbol: "buri_rt_host_testing_fs_write_file_bytes",
        args: &[Arg::Scalar, Arg::Str, Arg::List],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.fsAppendFile",
        symbol: "buri_rt_host_testing_fs_append_file",
        args: &[Arg::Scalar, Arg::Str, Arg::List],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.fsRenameFile",
        symbol: "buri_rt_host_testing_fs_rename_file",
        args: &[Arg::Scalar, Arg::Str, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.fsRemoveFile",
        symbol: "buri_rt_host_testing_fs_remove_file",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.fsRemoveDir",
        symbol: "buri_rt_host_testing_fs_remove_dir",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::ResMsg,
    },
    Entry {
        key: "host_testing.fsMakeDir",
        symbol: "buri_rt_host_testing_fs_make_dir",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    Entry {
        key: "host_testing.fsSyncFile",
        symbol: "buri_rt_host_testing_fs_sync_file",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Res,
    },
    // The fault plan's promise. The plan never crosses — it is a list of Buri
    // values holding an `IoError`, and §2.1 cannot name an error variant that
    // carries a field, so matching is the `Eq` the `Call` records derive and
    // happens in `host_testing.buri`. These carry the half a program cannot
    // keep: `addFsFault` and `addNetFault` say what an entry would read like in
    // a failure message, `noteFault` records that one fired, and `test.leave`
    // reports the rest. `noteFsCall` is the twelfth way into a log — a call the
    // plan failed never reaches the row that would have recorded it, and it is
    // still a call.
    Entry {
        key: "host_testing.addFsFault",
        symbol: "buri_rt_host_testing_add_fs_fault",
        args: &[Arg::Scalar, Arg::Str, Arg::Str, Arg::Str],
        ret: Ret::Void,
    },
    Entry {
        key: "host_testing.addNetFault",
        symbol: "buri_rt_host_testing_add_net_fault",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Void,
    },
    Entry {
        key: "host_testing.faultFails",
        symbol: "buri_rt_host_testing_fault_fails",
        args: &[Arg::Scalar, Arg::Scalar, Arg::Scalar, Arg::Str],
        ret: Ret::Void,
    },
    Entry {
        key: "host_testing.noteFault",
        symbol: "buri_rt_host_testing_note_fault",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Void,
    },
    Entry {
        key: "host_testing.noteFsCall",
        symbol: "buri_rt_host_testing_note_fs_call",
        args: &[Arg::Scalar, Arg::Str, Arg::Str, Arg::Str],
        ret: Ret::Void,
    },
    Entry {
        key: "host_testing.netRebind",
        symbol: "buri_rt_host_testing_net_rebind",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.netWithPlan",
        symbol: "buri_rt_host_testing_net_with_plan",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    // The call log's remaining four. `spelled` is an `FsCall` constructor's
    // decode and not a filesystem operation: a test writing a call down performs
    // no effect and so has no context to reach `bytes.fromUtf8` with.
    //
    // The other three are `TestNet`'s. `net()` and `TestNet.fetch` are Buri
    // bodies and have no row — the absent-key list below says why — but a log is
    // state, so the handle naming it is minted here (`alloc.newCounter`'s
    // shape), written by `recordFetch` once the responder has answered, and read
    // back by `netCalls`. `recordFetch` is handed `Request` flattened by §2 rule
    // 1: the method's variant index as an `Int`, the URL's three leaves, and two
    // `(ptr, len)` pairs — `buri_rt_host_net_fetch`'s argument list without its
    // answer. `netCalls` takes the handle rather than the `TestNet`, because
    // that value carries the responder too and an argument crosses as its
    // leaves.
    Entry {
        key: "host_testing.spelled",
        symbol: "buri_rt_host_testing_spelled",
        args: &[Arg::List],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.newNet",
        symbol: "buri_rt_host_testing_new_net",
        args: &[],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.recordFetch",
        symbol: "buri_rt_host_testing_record_fetch",
        args: &[Arg::Scalar, Arg::Scalar, Arg::Str, Arg::List, Arg::List],
        ret: Ret::Void,
    },
    Entry {
        key: "host_testing.netCalls",
        symbol: "buri_rt_host_testing_net_calls",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    // -- tasks(): the order the work happens in ------------------------------
    //
    // `parallel` is the closure trampoline's third key and the reason the double
    // is worth having: the test's scheduler reaches its steps through the same
    // entry thunk the real one reaches them through. `self` is `Arg::Scalar`
    // where `host.HostTasks.parallel`'s is `Arg::Dropped`, because a `TestTasks`
    // is a handle and the runtime has to ask it which order this run schedules
    // in; `items` is `Arg::Elems` and `f` is `Arg::Step`, exactly as they are
    // there, and `ctx` — the caller's whole context, which the step is handed —
    // is `Arg::Dropped` between them.
    Entry {
        key: "host_testing.TestTasks.parallel",
        symbol: "buri_rt_host_testing_test_tasks_parallel",
        args: &[Arg::Scalar, Arg::Dropped, Arg::Elems, Arg::Step],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.tasks",
        symbol: "buri_rt_host_testing_tasks",
        args: &[],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestTasks.anyOrder",
        symbol: "buri_rt_host_testing_test_tasks_any_order",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestTasks.everyOrder",
        symbol: "buri_rt_host_testing_test_tasks_every_order",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestTasks.seed",
        symbol: "buri_rt_host_testing_test_tasks_seed",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestTasks.calls",
        symbol: "buri_rt_host_testing_test_tasks_calls",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestTasks.runs",
        symbol: "buri_rt_host_testing_test_tasks_runs",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.TestTasks.orders",
        symbol: "buri_rt_host_testing_test_tasks_orders",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.TestTasks.replan",
        symbol: "buri_rt_host_testing_test_tasks_replan",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestTasks.addFault",
        symbol: "buri_rt_host_testing_test_tasks_add_fault",
        args: &[Arg::Scalar, Arg::Scalar, Arg::Scalar, Arg::Str],
        ret: Ret::Void,
    },
    Entry {
        key: "host_testing.clock",
        symbol: "buri_rt_host_testing_clock",
        args: &[],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestClock.at",
        symbol: "buri_rt_host_testing_test_clock_at",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestClock.nowMillis",
        symbol: "buri_rt_host_testing_test_clock_now_millis",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.TestClock.sleepMillis",
        symbol: "buri_rt_host_testing_test_clock_sleep_millis",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Void,
    },
    Entry {
        key: "host_testing.rand",
        symbol: "buri_rt_host_testing_rand",
        args: &[],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestRand.seed",
        symbol: "buri_rt_host_testing_test_rand_seed",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestRand.nextInt",
        symbol: "buri_rt_host_testing_test_rand_next_int",
        args: &[Arg::Scalar, Arg::Scalar, Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.TestRand.nextFloat",
        symbol: "buri_rt_host_testing_test_rand_next_float",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry { key: "host_testing.env", symbol: "buri_rt_host_testing_env", args: &[], ret: Ret::Out },
    Entry {
        key: "host_testing.TestEnv.variables",
        symbol: "buri_rt_host_testing_test_env_variables",
        args: &[Arg::Scalar, Arg::List],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestEnv.arguments",
        symbol: "buri_rt_host_testing_test_env_arguments",
        args: &[Arg::Scalar, Arg::List],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.TestEnv.variable",
        symbol: "buri_rt_host_testing_test_env_variable",
        args: &[Arg::Scalar, Arg::Str],
        ret: Ret::Sum,
    },
    Entry {
        key: "host_testing.TestEnv.args",
        symbol: "buri_rt_host_testing_test_env_args",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    // `sockets()` — a socket with no network behind it. Seven rows, because the
    // double is small: the double itself, the mint, the three methods of
    // `Sockets`, and the two readers.
    //
    // `socketsSent` and `socketsIsOpen` take the **handle** in `TestFs`'s
    // shape, but for a different reason: a `TestSockets` *is* a bare handle, so
    // `self` would have crossed fine — these two are Buri bodies because one
    // builds a `Message` out of the flat record the archive writes
    // (`cli/runtime/lib.rs` §2.1, and `Received` for the same reason) and the
    // other unwraps a `Socket`.
    Entry {
        key: "host_testing.sockets",
        symbol: "buri_rt_host_testing_sockets",
        args: &[],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.socketsOpen",
        symbol: "buri_rt_host_testing_sockets_open",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.socketsSent",
        symbol: "buri_rt_host_testing_sockets_sent",
        args: &[Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "host_testing.socketsIsOpen",
        symbol: "buri_rt_host_testing_sockets_is_open",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host_testing.TestSockets.socketSendText",
        symbol: "buri_rt_host_testing_test_sockets_socket_send_text",
        args: &[Arg::Scalar, Arg::Scalar, Arg::Str],
        ret: Ret::Void,
    },
    Entry {
        key: "host_testing.TestSockets.socketSendBytes",
        symbol: "buri_rt_host_testing_test_sockets_socket_send_bytes",
        args: &[Arg::Scalar, Arg::Scalar, Arg::List],
        ret: Ret::Void,
    },
    Entry {
        key: "host_testing.TestSockets.socketClose",
        symbol: "buri_rt_host_testing_test_sockets_socket_close",
        args: &[Arg::Scalar, Arg::Scalar, Arg::Scalar, Arg::Str],
        ret: Ret::Void,
    },
    // The one key here that no Buri declaration produces: `middle::monomorphize`
    // emits it after every `test` body, so that "a fault whose call never
    // happens fails the test" is checked once for all three backends rather than
    // three times in three entry points. [`TEST_ENTER`] is the other half and is
    // called from `emit.rs`'s `test_entry_point` instead, because it is the
    // *runner's* protocol — which block to run — and this is the *program's*
    // rule.
    Entry { key: "test.leave", symbol: TEST_LEAVE, args: &[Arg::Scalar], ret: Ret::Void },
    // And the other half of it, emitted after: whether to run this body again,
    // which is how `TestTasks.everyOrder` reruns it once per completion order.
    Entry { key: "test.replay", symbol: TEST_REPLAY, args: &[Arg::Scalar], ret: Ret::Scalar },
];

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
/// `buri_rt_frames_are_per_carrier()` — the artifact's one statement about
/// itself, made once at startup (`cli/runtime/lib.rs` §6).
///
/// **This backend makes it and the frame-threaded one does not**, and that is
/// a fact about where a Buri frame lives rather than a difference of opinion
/// about scheduling. Here a Buri function is an ordinary LLVM function and its
/// locals are `alloca`s, so a carrier's 512 KiB thread stack is its own and the
/// runtime may run two `Tasks.parallel` steps at once. There a program has one
/// Buri stack — the `buri$stencil$stack` block its `main` guards — and a step
/// runs in a frame the *call site* set aside, so two of them would share it.
/// Saying nothing is the safe answer, which is why the call is here and not a
/// parameter of one over there.
pub const FRAMES_PER_CARRIER: &str = "buri_rt_frames_are_per_carrier";
/// `buri_rt_values_may_cross_tasks()` — the artifact's other statement about
/// itself, made once at startup and **before it allocates anything**
/// (`cli/runtime/lib.rs` §6).
///
/// **Both native backends make it**, and only for a program
/// `middle::rc::crosses_tasks` says can reach a task boundary. It is not the
/// same fact as [`FRAMES_PER_CARRIER`] and the two are deliberately not one
/// call: that one is about *where a frame lives*, which is a property of the
/// backend, and this one is about *whether a block can be reached from two
/// carriers*, which is a property of the program. A backend that cannot fan
/// out still makes this call, because the day it learns to is not a day
/// anybody should have to remember a second edit.
///
/// Its effect is that every block the program allocates carries
/// `middle::layout::CAP_SHARED_FLAG`, so every reference operation takes G2's
/// atomic arm and no uniquely-owned in-place write fires. Silence is the safe
/// answer: the runtime's fan-out is gated on the same latch, so an entry point
/// that lost this call runs its tasks one at a time.
pub const VALUES_MAY_CROSS_TASKS: &str = "buri_rt_values_may_cross_tasks";

/// `void *buri_rt_copy_block(void *p, void (*glue)(void *))` — a fresh block
/// holding the same bytes, with `rc == 1` and nothing shared with the original.
///
/// G5's half of the copy out of a scope. The *type*-dependent half is the
/// per-type walk each backend generates (`Unit::copy_rc`,
/// `stencil/glue.rs`'s `Helper::Copy`); this is the block-dependent half, and
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
/// not an [`ENTRIES`] row: this operation has no intrinsic key of its own, it is
/// the 128-bit arm of `num.I128.checkedAdd`.
const I128_CHECKED: &str = "buri_rt_i128_checked";
pub const I128_SATURATING: &str = "buri_rt_i128_saturating";

/// The shape [`I128_CHECKED`] answers in, as an [`Entry`] so the sum-returning
/// call path can be reused. `args` is empty because the argument list is built
/// at the call site from a 128-bit pair rather than from a Buri signature.
pub const I128_CHECKED_ENTRY: Entry = Entry {
    key: "num.<128>.checked",
    symbol: I128_CHECKED,
    args: &[],
    ret: Ret::Sum,
};

/// `buri_rt_str_scalar_len(ptr, byte_len) -> u64` — the slow half of
/// `str.len`, called only where the ASCII flag is clear.
pub const STR_SCALAR_LEN: &str = "buri_rt_str_scalar_len";

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
/// `buri_rt_show_list(xs, count, out)` — `[` + already-rendered elements joined
/// by `, ` + `]`, for `deriveArrayShow`. No element descriptor: the backend has
/// already turned each element into a `Str`, so the block this reads is a
/// `[Str]` at every instantiation.
pub const SHOW_LIST: &str = "buri_rt_show_list";

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
    use crate::compiler::backend::runtime_native::symbol_for;

    /// **The two carrier-stack entries are symbols with no row, and that is
    /// the answer rather than an omission.**
    ///
    /// `buri_rt_stack_acquire` and `buri_rt_stack_release` are `buri_rt_*`
    /// entries this backend never calls, and the reason is the whole shape of
    /// the B7/B8 pair: a Buri frame here *is* a machine frame, so a carrier
    /// entering through `carrier.rs`'s door needs no second stack and no guard
    /// of its own — its thread's is the OS's. The frame-threaded backend calls
    /// them from `stencil/asm.rs`, by name, out of a hand-written shim rather
    /// than through any table.
    ///
    /// A row here would be worse than useless: [`ENTRIES`] is keyed by
    /// *intrinsic key*, and these two have none — no Buri expression names
    /// them and none should. This is the same two-directional test
    /// `host_net_fetch_has_a_symbol_and_no_row` makes, for the same reason: an
    /// absence with a reason, asserted, so that adding one is a deliberate act.
    #[test]
    fn the_carrier_stack_entries_have_symbols_and_no_row() {
        use crate::compiler::backend::carrier;
        for symbol in [carrier::STACK_ACQUIRE, carrier::STACK_RELEASE] {
            assert!(symbol.starts_with("buri_rt_"), "{symbol} is not a runtime symbol");
            assert!(
                !ENTRIES.iter().any(|e| e.symbol == symbol),
                "{symbol} gained a row: this backend's door takes no Buri stack"
            );
        }
    }

    /// Every entry's symbol is the one the contract's rule produces, so the
    /// table is a *subset* of the contract rather than a second opinion about
    /// it. The three examples `cli/runtime/lib.rs` §1 and VALUE-MODEL.md §10
    /// spell out are checked directly.
    #[test]
    fn every_symbol_is_the_rule_applied_to_the_key() {
        for e in ENTRIES {
            assert_eq!(symbol_for(e.key), e.symbol, "{}", e.key);
        }
        assert_eq!(symbol_for("host.HostFs.readFile"), "buri_rt_host_fs_read_file");
        assert_eq!(symbol_for("host.HostProc.exitWith"), "buri_rt_host_proc_exit_with");
        assert_eq!(symbol_for("host.HostStdout.println"), "buri_rt_host_stdout_println");
        // The one that does not collapse, because the repetition is real:
        // `memory.rs:330` exports it under this name.
        assert_eq!(symbol_for("host.HostAlloc.allocate"), "buri_rt_host_alloc_allocate");
        assert_eq!(symbol_for("list.map"), "buri_rt_list_map");
    }

    /// **Every operation of `Fs`, `Env` and `Stdin` has a row here too.**
    ///
    /// buri-lang/buri#36, on this side of `cli/tests/README.md`'s bar. The two
    /// runtime tables are deliberately not copies of each other — this one
    /// reconstructs the C argument list and the other does not — so "the host
    /// surface is complete" has to be asserted twice or it is asserted for one
    /// backend and hoped for the other. The list is read off `core/effect`, so
    /// the operation declared next is covered by the commit that declares it.
    #[test]
    fn every_operation_of_the_host_file_and_environment_effects_has_a_row() {
        let source = crate::compiler::standard_library::MODULES
            .iter()
            .find(|m| m.path == "core/effect")
            .map(|m| m.source)
            .expect("`core/effect` is a module");
        let mut checked = 0usize;
        for (effect, host) in [("Fs", "HostFs"), ("Env", "HostEnv"), ("Stdin", "HostStdin")] {
            let body = source
                .split(&format!("export effect {effect} {{"))
                .nth(1)
                .unwrap_or_else(|| panic!("no `effect {effect}` in `core/effect`"))
                .split("\n}")
                .next()
                .unwrap_or_else(|| panic!("`effect {effect}` never closes"));
            for line in body.lines() {
                let Some(rest) = line.trim().strip_prefix("fn ") else { continue };
                let Some(method) = rest.split('(').next() else { continue };
                let key = format!("host.{host}.{method}");
                assert!(
                    entry(&key).is_some(),
                    "`{key}` is declared on `effect {effect}` and this table has no row"
                );
                checked += 1;
            }
        }
        assert!(checked >= 16, "only {checked} operations were checked");
    }

    /// A key the archive has no body for must not be in the table. If one of
    /// these ever gains a symbol, this test is the reminder to add its shape
    /// rather than to let the mangler invent it.
    ///
    /// `str.concat`, `str.format`, `str.len`, `list.len` and `list.empty` are
    /// open-coded; `derivePrimShow` and `derivePrimHash` arrive qualified by
    /// their primitive (`derivePrimShow.U8`) and so have no key of their own;
    /// `json.decode` and every `list.*` entry taking a closure are outside the
    /// archive for the reasons [`ENTRIES`]'s comment gives.
    #[test]
    fn the_generated_surface_is_not_claimed() {
        for absent in [
            "str.concat",
            "str.format",
            "str.len",
            "list.len",
            "json.decode",
            "derivePrimShow",
            "derivePrimHash",
            "list.empty",
            "list.map",
            "list.sortBy",
            "json.encode",
            // Open-coded, and named here so that "it has no symbol" and "the
            // backend cannot compile it" stay two different statements: the
            // allocator is two instructions on both native backends.
            "host_testing.alloc",
            "host_testing.TestAlloc.allocate",
            // Buri bodies, for the reason two paragraphs up.
            "host_testing.fs",
            "host_testing.TestFs.readFile",
            "host_testing.TestFs.faults",
            // `host.HostFs`'s twelve, `host.HostEnv`'s two and
            // `host.HostStdin`'s two used to be here — sixteen keys with a body
            // in `cli/runtime/host.rs`, no row in either runtime table, and a
            // native binary that could not touch a file or read its own
            // arguments (buri-lang/buri#36). They are rows now, and the two
            // halves of that gap were different: `Env` and `Stdin` were waiting
            // on nothing but the row, and `Fs` was waiting on §2.1's message
            // shape.
            //
            // `host.HostNet.fetch` has a body in the archive and no row, and
            // not because nobody got to it: `Ret::Res` names the error variant
            // by index and `lib.rs` §2.1 restricts that variant to carrying no
            // fields, while `NetError` carries a `Str` on `BadUrl` and on
            // `Transport`. The row waits on §2.1, not on a table edit — the
            // same statement `runtime_table.rs`'s list makes.
            "host.HostNet.fetch",
            // `core/host/testing`'s `net()` answers the same shape and needs
            // no row at all: `TestNet` carries its responder as a value and
            // `TestNet.fetch` is a Buri body that calls it, so no key is
            // produced for it here. That is the same wall read from the other
            // side — a responder is a `{ code, env }` pair the archive has no
            // way to invoke, and its answer is the `Result<Response, NetError>`
            // §2.1 cannot name — and it is why widening §2.1 later changes
            // nothing about the double. Its *log* is a different question and
            // has five rows above: `newNet`, `netRebind`, `netWithPlan`,
            // `recordFetch` and `netCalls` cross nothing §2.1 restricts. So does
            // its **fault plan**, which is the same wall a third time: the plan
            // is a list of Buri values holding a `NetError`, so it stays in the
            // program and only its rendering and its fired flags cross.
            //
            // `host_testing.fs` is absent for a different reason and is not a
            // gap: `fs()` is a Buri body too now, because a `TestFs` is a handle
            // and a plan. `newFs` is the row that mints the handle.
        ] {
            assert!(entry(absent).is_none(), "{absent}");
        }
    }

    /// `host.HostNet.fetch`, in both directions.
    ///
    /// The archive exports exactly the symbol the mangling rule produces — so
    /// a row added later needs no invention — and this table has no row for
    /// it, because `NetError`'s two payload-carrying variants put it outside
    /// `cli/runtime/lib.rs` §2.1's `Result` shape. The pair keeps "the body
    /// exists" and "this backend can call it" two separate claims.
    #[test]
    fn host_net_fetch_has_a_symbol_and_no_row() {
        assert_eq!(symbol_for("host.HostNet.fetch"), "buri_rt_host_net_fetch");
        assert!(entry("host.HostNet.fetch").is_none());
    }

    /// The two shapes the backend supplies for itself emit a parameter and
    /// consume no argument, and everything else consumes exactly one. The
    /// emitter walks the C list with a cursor into the Buri list on precisely
    /// this invariant.
    #[test]
    fn only_the_generic_extras_consume_no_argument() {
        for shape in [
            Arg::Str,
            Arg::Bytes,
            Arg::List,
            Arg::Scalar,
            Arg::Dropped,
            Arg::Elems,
            Arg::Spilled,
            Arg::Step,
        ] {
            assert!(shape.consumes(), "{shape:?}");
        }
        for shape in [Arg::Stride, Arg::Retain] {
            assert!(!shape.consumes(), "{shape:?}");
            assert_eq!(shape.leaves(), 1);
        }
    }

    /// `stride` and `retain` travel together, and only where the entry has a
    /// generic parameter to describe (`cli/runtime/lib.rs` §2 rule 4). An entry
    /// with one and not the other would be a call with a parameter missing,
    /// which the C boundary does not diagnose.
    #[test]
    fn stride_and_retain_come_in_pairs_behind_a_generic() {
        for e in ENTRIES {
            let strides = e.args.iter().filter(|a| **a == Arg::Stride).count();
            let retains = e.args.iter().filter(|a| **a == Arg::Retain).count();
            assert_eq!(strides, retains, "{}", e.key);
            let generic =
                e.args.iter().any(|a| matches!(a, Arg::Elems | Arg::Spilled));
            // [`Arg::Step`] carries **both** of its strides itself, because a
            // step reads one element type and writes another and `Arg::Stride`
            // names exactly one. So a row with a step is generic and has no
            // separate stride, and the equivalence above holds of the rest.
            let stepped = e.args.contains(&Arg::Step);
            assert_eq!(strides > 0, generic && !stepped, "{}", e.key);
        }
    }

    /// Every row with a step is one `backend/intrinsic_keys.rs` names, its
    /// `Arg::Step` sits where that table says the closure is, and it is the
    /// last argument — which is the invariant that lets `runtime_table.rs`,
    /// which has no per-argument column, describe the same C signature.
    #[test]
    fn a_step_is_the_last_argument_of_a_key_the_shared_table_names() {
        use crate::compiler::backend::intrinsic_keys::step_call;
        for e in ENTRIES {
            let at = e.args.iter().position(|a| *a == Arg::Step);
            match (at, step_call(e.key)) {
                (None, None) => {}
                (Some(at), Some(call)) => {
                    assert_eq!(at, call.func, "{}", e.key);
                    assert_eq!(at + 1, e.args.len(), "{}", e.key);
                }
                (Some(_), None) => panic!("{} has a step and no `step_call` row", e.key),
                (None, Some(_)) => panic!("{} is runtime-driven and has no `Arg::Step`", e.key),
            }
        }
    }

    /// The two tables agree about where each key's **context** is: this one
    /// spells it `Arg::Dropped` at that index, the other spells it
    /// `Entry::ctx`, and they describe one C signature.
    ///
    /// One direction only, on purpose. `Arg::Dropped` also covers a zero-sized
    /// receiver — `HostStdout` is an empty struct — which `runtime_table.rs`
    /// needs no column for, because a value of no bytes spreads to no leaves
    /// there anyway. A **context** is the case where that reasoning fails: it
    /// is dropped whatever it weighs, and `C: Alloc` is an ordinary bound that
    /// an ordinary implementing value satisfies (SPEC 10.1), so the argument
    /// can arrive carrying a handle. Hence: every context the other table names
    /// must be dropped here.
    #[test]
    fn an_entry_names_the_context_the_other_table_drops() {
        use crate::compiler::backend::runtime_table;
        let mut checked = 0usize;
        for shared in runtime_table::ENTRIES {
            let Some(at) = shared.ctx else { continue };
            let Some(here) = ENTRIES.iter().find(|e| e.key == shared.key) else { continue };
            assert_eq!(
                here.args.get(at),
                Some(&Arg::Dropped),
                "{}: argument {at} is the context and this table does not drop it",
                shared.key
            );
            checked += 1;
        }
        assert!(checked > 20, "only {checked} contexts were checked against both tables");
    }
}
