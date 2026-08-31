//! One call into `libburi_rt.a`, from a frame-threaded body.
//!
//! `runtime.rs` is the table; this is the emission rule it drives, and it is
//! the same three steps every backend here takes:
//!
//! ```text
//!   1. the Buri arguments, flattened into scalar leaves
//!   2. the element pair — stride, then retain glue
//!   3. the out-pointer
//! ```
//!
//! The difference is where an argument *is*. A register-machine backend builds
//! a value list and lets its allocator place it; this one has no register
//! allocator at the call boundary, so where an argument is has to be spelled in the
//! stencil. There are two families for that and [`Jit::c_call_to`] picks
//! between them per call site:
//!
//! * **`crts`, the slots family** — one frame-offset hole per argument, which
//!   `extract::fold_addressing` puts in the `imm12` of the load that uses it.
//!   An argument that is already a whole frame word costs one instruction and
//!   no store at all. This is the one taken whenever every offset the call
//!   names fits that field;
//! * **`crt`, the array family** — the arguments copied into a **contiguous
//!   scratch area** with the ordinary `mov` and `imm` stencils, and one stencil
//!   reading them off consecutively into `x0`–`x7` and `d0`–`d1`. A folded
//!   `imm12` reaches 32 KiB into a frame and this is what a wider frame still
//!   uses.
//!
//! Both are `(integers, doubles, result)` and nothing else — the cross product
//! `sources.rs`'s header rejects is over operand *kinds*, and every argument of
//! either family is a slot. An operand that is not already a frame word — a
//! literal, a narrow field, an address, a glue symbol — is materialised into
//! the scratch area first in **both**, and read from there.
//!
//! Writing such an argument out to memory before a call is not a cost this
//! convention pays extra: a Buri call already writes its arguments into the
//! callee's frame, and this is the same store to a different address.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "every operation here is a byte offset within one frame this \
              emitter laid out, or an index into an argument list bounded by \
              `MAX_INT_ARGS` before it is used. The frame's size is a `u32` \
              computed by `jit::frame_sigs` from the same program, so no offset \
              can exceed it and no sum of two of them can wrap"
)]

// How many integer and float registers a call may use: the widest `crt` shape
// the stencil library holds, read from `abi.rs` — the one file the library
// builder and this emitter both compile — rather than written down twice.
use super::abi::{MAX_FLOAT_ARGS as MAX_FLOAT, MAX_INT_ARGS as MAX_INT};
use super::jit::{Fn2, Jit, V};
use super::runtime::{Entry, Extra, OptRepr, Ret, BURI_OK};
use crate::compiler::backend::intrinsic_keys::step_call;
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{EnumRepr, Layout, Repr};
use crate::compiler::semantics::types::{self as types, Ty};

/// Where the C argument area starts inside the scratch words `jit::frame_sigs`
/// reserves.
///
/// Past the sixteen the emitter and the open-coded list loops already use for
/// their own temporaries (`lists.rs` §"the scratch words"), so that a runtime
/// call in the middle of a loop does not tread on the loop's index.
pub const CARG_WORD: u32 = 16;

/// The scratch word a fallible entry's discriminant lands in.
///
/// Past the argument area, and **not** the destination: the payload of an
/// `Option` has already been written through the out-pointer by the time the
/// call returns, and a niche `Option`'s payload is at offset zero — so storing
/// the discriminant into the destination would overwrite the answer with the
/// question.
const DISC_WORD: u32 = CARG_WORD + MAX_INT as u32 + MAX_FLOAT as u32;

/// One more scratch word, for the sequences that need a second operand alive
/// beside the one they are writing.
pub(crate) const SPARE_WORD: u32 = DISC_WORD + 1;

/// Where a three-way comparison's raw answer waits while the boolean an
/// ordering operator wants is built out of it.
pub(crate) const RAW_WORD: u32 = SPARE_WORD + 1;


fn round8(n: u32) -> u32 {
    (n + 7) & !7
}

/// The element type of a value whose IR type is a `[T]`, and `None` for
/// anything else.
fn array_elem(prog: &ir::Program, t: ir::Type) -> Option<Ty> {
    let ir::Type::Agg(id) = t else { return None };
    match prog.type_info(id).ty.clone() {
        Ty::Array(e) => Some(*e),
        _ => None,
    }
}

/// One scalar of a flattened Buri value: where it is in the frame and how wide.
#[derive(Clone, Copy)]
pub(crate) struct Leaf {
    offset: u32,
    width: u32,
    float: bool,
}

/// What goes into one C argument register.
#[derive(Clone)]
pub(crate) enum Src {
    /// The word at a frame offset, already whole.
    Word(u32),
    /// A narrow field of an aggregate, zero-extended into the register.
    Narrow(u32, u32),
    /// A literal.
    Imm(u64),
    /// The address of a frame offset.
    Addr(u32),
    /// The address of a function this unit generated (`glue.rs`).
    Sym(String),
}

/// `Str` is `{ base, ptr, len }` (VALUE-MODEL.md §3), and bit 63 of `len` is
/// the ASCII flag rather than part of the count.
const STR_PTR: u32 = 8;
const STR_LEN: u32 = 16;

impl Jit<'_> {
    /// Emits a call to `entry`, or answers `false` and leaves nothing emitted
    /// when the shape is one this backend does not have.
    ///
    /// Answering rather than aborting is what lets the caller record a reason
    /// and keep going: a refused shape is a diagnostic naming the operation,
    /// and the emission continues so that one build reports every one of them
    /// rather than the first.
    pub(crate) fn rt_call(
        &mut self,
        prog: &ir::Program,
        st: &mut Fn2,
        entry: &Entry,
        dest: Option<(u32, ir::Type)>,
        args: &[(u32, ir::Type)],
    ) -> Result<(), String> {
        let mut ints: Vec<Src> = Vec::new();
        let mut floats: Vec<Src> = Vec::new();

        for (i, (slot, t)) in args.iter().copied().enumerate() {
            // A **context argument is dropped**, whatever it weighs and
            // whatever type it turned out to be. The runtime allocates through
            // `buri_rt_alloc` and has no use for one, so the C signature has no
            // parameter for it.
            //
            // Which argument that is comes from the row ([`Entry::ctx`]) rather
            // than from the value's type, and the difference is not academic.
            // Asking "is this a `Ty::Ctx`?" was the rule here, and it is the
            // right answer only while every `C: Alloc` is instantiated at a
            // `context { … }`. `C` is an ordinary type parameter with an
            // ordinary bound (SPEC 10.1), so a value that *implements* `Alloc`
            // satisfies it without being a context — SPEC 10.8's attenuating
            // `ReadOnly<C>`, and `core/host/testing`'s `alloc()`, which is a
            // `struct TestAlloc(I64)` carrying a handle. One of those slipped
            // past the type test, spread to a leaf, and shifted every argument
            // after it one register down: `push` reached `buri_rt_list_push`
            // with the handle where the pointer belongs and died in `memmove`
            // before the first test block.
            if entry.ctx == Some(i) {
                continue;
            }
            // A context the row does *not* name is this table being wrong, not
            // this call being unusual, and every context in the corpus reaches
            // here — so a missing annotation is a refusal at build time rather
            // than a shifted argument list at run time.
            if matches!(source_ty(prog, t), Some(Ty::Ctx(_))) {
                return Err(format!(
                    "{}: a context at argument {i} that the runtime table does not name",
                    entry.key
                ));
            }
            if entry.by_ref == Some(i) {
                ints.push(Src::Addr(slot));
                continue;
            }
            // The **step** is not flattened: `{ code, env }` is this backend's
            // business and crosses inside the state record below
            // ([`Extra::Step`]). It is the last argument at every key
            // `step_call` names, so skipping it here and appending four words
            // after the loop writes the same C signature the LLVM table
            // describes with an `Arg::Step` in its place.
            if entry.extra == Extra::Step && step_call(entry.key).is_some_and(|c| c.func == i) {
                continue;
            }
            for leaf in self.leaves(prog, t)? {
                let at = slot + leaf.offset;
                if leaf.float {
                    floats.push(Src::Word(at));
                } else if leaf.width == 8 {
                    ints.push(Src::Word(at));
                } else {
                    ints.push(Src::Narrow(at, leaf.width));
                }
            }
        }

        if entry.extra == Extra::Element {
            let elem = self.element_ty(prog, dest.map(|d| d.1), args);
            let Some(elem) = elem else {
                return Err(format!("{}: no element type", entry.key));
            };
            let stride = u64::from(self.layouts_of(elem.clone()).stride.max(1));
            ints.push(Src::Imm(stride));
            // The retain glue of `lib.rs` §2 rule 4: the per-element function
            // that increfs whatever counted pointers one element holds, and a
            // **null** pointer for an element type that holds none — which is
            // the common case and what the runtime tests for.
            match self.element_glue(elem) {
                Some(name) => ints.push(Src::Sym(name)),
                None => ints.push(Src::Imm(0)),
            }
        }

        if entry.extra == Extra::Step {
            self.step_extra(prog, st, entry, dest.map(|d| d.1), args, &mut ints)?;
        }

        let dslot = dest.map(|d| d.0).unwrap_or(0);
        let opt = match entry.ret {
            Ret::Opt => {
                let Some((_, dty)) = dest else {
                    return Err(format!("{}: no destination", entry.key));
                };
                let ir::Type::Agg(id) = dty else {
                    return Err(format!("{}: an `Option` destination that is not one", entry.key));
                };
                let l = self.layout_of(prog, id);
                let Some(o) = OptRepr::of(&l) else {
                    return Err(format!("{}: an `Option` destination that is not one", entry.key));
                };
                ints.push(Src::Addr(dslot + o.payload));
                Some(o)
            }
            Ret::Out => {
                let Some((_, dty)) = dest else {
                    return Err(format!("{}: no destination", entry.key));
                };
                // A zero-sized result has no bytes to write, and a parameter
                // for one is a thing the two sides can disagree about for free.
                if self.width_of(prog, dty) > 0 {
                    ints.push(Src::Addr(dslot));
                }
                None
            }
            Ret::Void | Ret::NoReturn | Ret::Scalar | Ret::Tag | Ret::Res => None,
        };

        // `lib.rs` §2.1's `Result<T, E>`: `.Ok`'s payload through an
        // out-pointer where it has bytes, and then **`E`'s own shape** decides
        // the failure side. An enum error is named by the discriminant's index;
        // anything else — `bytes.fromUtf8`'s `Utf8Error(Int)` — is a value with
        // one place to go, so it goes through a second out-pointer and the
        // discriminant carries nothing but "it failed". Which of the two is not
        // a column in the table and must not be: it is a fact the type already
        // makes, and a column could disagree with it.
        let res = match entry.ret {
            Ret::Res => {
                let Some((_, dty)) = dest else {
                    return Err(format!("{}: no destination", entry.key));
                };
                let ir::Type::Agg(id) = dty else {
                    return Err(format!("{}: a `Result` destination that is not one", entry.key));
                };
                let l = self.layout_of(prog, id);
                let Some((ok, err, err_ty)) = self.result_shape(prog, dty) else {
                    return Err(format!("{}: a `Result` destination that is not one", entry.key));
                };
                let err_l = self.layouts_of(err_ty.clone());
                let ok_bytes = self.ok_payload_bytes(prog, dty, ok);
                if !l.variant(ok).is_empty() && ok_bytes > 0 {
                    ints.push(Src::Addr(dslot + super::lists::payload_at(&l, ok)));
                }
                // `E`'s own shape, as one question: an enum error is named by
                // its tag's width, and anything else has none and goes through
                // a second out-pointer instead.
                let tag_w = match &err_l.repr {
                    Repr::Enum { repr: EnumRepr::Bare { tag }, .. }
                    | Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, .. } => Some(tag.size()),
                    _ => None,
                };
                let err_at = super::lists::payload_at(&l, err);
                if tag_w.is_none() && err_l.size > 0 {
                    ints.push(Src::Addr(dslot + err_at));
                }
                Some((l, ok, err, err_at, tag_w, err_l.size))
            }
            _ => None,
        };

        // The result's register shape, which is the destination's own except
        // where the discriminant of a fallible entry is what comes back.
        let kind = match entry.ret {
            Ret::Void | Ret::NoReturn | Ret::Out => "v",
            // An `i32` from C, and a frame slot holds an integer
            // zero-extended — so a tag that fits its own field is the whole
            // word, and one that is written into an aggregate needs the narrow
            // store below.
            Ret::Opt | Ret::Tag | Ret::Res => "w",
            Ret::Scalar => {
                let Some((_, dty)) = dest else {
                    return Err(format!("{}: no destination", entry.key));
                };
                let mut leaves = self.leaves(prog, dty)?;
                let Some(one) = leaves.pop().filter(|_| leaves.is_empty()) else {
                    return Err(format!("{}: a result that is not one scalar", entry.key));
                };
                scalar_kind(one, dty)
            }
        };

        // A `Tag` whose destination is an aggregate has to land at the tag's
        // own offset and width; one whose destination is a scalar already
        // occupies the whole slot, which is what the `w` shape wrote.
        let narrow = match (entry.ret, dest) {
            (Ret::Tag, Some((_, ir::Type::Agg(id)))) => {
                let l = self.layout_of(prog, id);
                match &l.repr {
                    Repr::Enum { repr: EnumRepr::Bare { tag }, .. }
                    | Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, .. } => {
                        Some(tag.size())
                    }
                    _ => return Err(format!("{}: a tag destination that is not an enum", entry.key)),
                }
            }
            _ => None,
        };
        let into = if opt.is_some() || narrow.is_some() || res.is_some() {
            st.scratch + DISC_WORD * 8
        } else {
            dslot
        };
        self.c_call(entry.symbol, st, &ints, &floats, into, kind)?;
        if let Some(o) = opt {
            self.store_option_tag(st, dslot, &o);
        }
        if let Some(w) = narrow {
            self.store_w(dslot, into, w);
        }
        if let Some((l, ok, err, err_at, tag_w, err_size)) = res {
            self.store_result_tag(st, dslot, &l, ok, err, err_at, tag_w, err_size);
        }
        Ok(())
    }

    /// `.Ok`'s discriminant or `.Err`'s, once a `Ret::Res` entry has answered.
    ///
    /// Both payloads are already where they belong — the runtime wrote them
    /// through the out-pointers — so what is left is the label, and on the
    /// failure side of an *enum* error the variant index the discriminant is.
    ///
    /// The backend cannot enforce `lib.rs` §2.1's "the variant it names carries
    /// no fields": `n` is a register. So the error's payload area is **zeroed**,
    /// which makes an entry that broke the promise produce an empty payload —
    /// wrong, and safely wrong — rather than a reference count on whatever the
    /// frame held. `llvm/emit.rs::call_result` zeroes the same bytes for the
    /// same reason.
    #[allow(
        clippy::too_many_arguments,
        reason = "one `Result` destination's shape, read off two layouts by the \
                  caller that already had both in hand"
    )]
    fn store_result_tag(
        &mut self,
        st: &mut Fn2,
        dslot: u32,
        l: &Layout,
        ok: usize,
        err: usize,
        err_at: u32,
        tag_w: Option<u32>,
        err_size: u32,
    ) {
        let disc = st.scratch + DISC_WORD * 8;
        let bad = st.label();
        let done = st.label();
        // `BURI_OK` is `-1` as an `i32`, and the `w` result shape put it in the
        // slot zero-extended, so the comparison is against its unsigned form.
        let key = self.arm_key("brcmp/eq/u64/fi", "JIT_F");
        self.emit(
            &key,
            &[
                ("JIT_A", V::I(u64::from(disc))),
                ("JIT_K", V::I(u64::from(BURI_OK as u32))),
                ("JIT_T", V::Fall),
                ("JIT_F", V::Blk(bad)),
            ],
        );
        self.store_disc(l, dslot, ok);
        self.emit("jump", &[("JIT_T", V::Blk(done))]);
        let here = self.region.code_addr();
        st.place(bad, here);
        if let Some(w) = tag_w {
            let mut off = 0u32;
            while off + 8 <= err_size {
                self.imm_to(dslot + err_at + off, 0);
                off += 8;
            }
            while off < err_size {
                self.imm_w(dslot + err_at + off, 1, 0);
                off += 1;
            }
            // The index is the discriminant itself, at the error enum's own tag
            // width — one store at offset zero, because `middle/layout.rs`
            // gives a niche only to `Option<T>`.
            self.store_w(dslot + err_at, disc, w);
        }
        self.store_disc(l, dslot, err);
        let here = self.region.code_addr();
        st.place(done, here);
    }

    /// `(the `.Ok` variant, the `.Err` variant, `E`)` for a `Result`
    /// destination.
    fn result_shape(&mut self, prog: &ir::Program, dty: ir::Type) -> Option<(usize, usize, Ty)> {
        let source = source_ty(prog, dty)?;
        types::result_shape(self.tables, &source)
    }

    /// How many bytes `.Ok`'s payload occupies, from `T` rather than from the
    /// enum: a variant records *where* a field is and never how big it is, and
    /// `Result<(), E>` has a field of no size.
    fn ok_payload_bytes(&mut self, prog: &ir::Program, dty: ir::Type, ok: usize) -> u32 {
        let Some(Ty::Con(_, args)) = source_ty(prog, dty) else { return 0 };
        let Some(payload) = args.get(ok).cloned() else { return 0 };
        self.layouts_of(payload).size
    }

    /// One C call: the arguments into the scratch area, then the `crt` stencil
    /// whose shape they make.
    ///
    /// Separate from [`Jit::rt_call`] because the runtime is reached two ways.
    /// Most entries come from `runtime.rs`'s table and have a Buri signature to
    /// flatten; the handful `middle::derives` and `core/testing/assert` leave
    /// behind — a renderer chosen by primitive, a failure report taking three
    /// borrowed `Str`s — have no table row and a shape the emitter knows
    /// outright. Both are the same three instructions once the arguments are
    /// placed, and this is that.
    pub(crate) fn c_call(
        &mut self,
        symbol: &'static str,
        st: &Fn2,
        ints: &[Src],
        floats: &[Src],
        dslot: u32,
        kind: &str,
    ) -> Result<(), String> {
        self.c_call_to(V::Ext(symbol), symbol, st, ints, floats, dslot, kind)
    }

    /// [`Jit::c_call`] to a function **this unit generated** (`glue.rs`), whose
    /// name is chosen at emission time and so cannot be a `&'static str`.
    #[allow(
        clippy::too_many_arguments,
        reason = "one call's shape, as `c_call`'s, plus the symbol by value"
    )]
    pub(crate) fn c_call_sym(
        &mut self,
        symbol: String,
        st: &Fn2,
        ints: &[Src],
        floats: &[Src],
        dslot: u32,
        kind: &str,
    ) -> Result<(), String> {
        self.c_call_to(V::Sym(symbol.clone()), &symbol, st, ints, floats, dslot, kind)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one call's shape, and the callee twice: once as the hole's \
                  value and once as the name a refusal would print"
    )]
    fn c_call_to(
        &mut self,
        callee_v: V,
        symbol: &str,
        st: &Fn2,
        ints: &[Src],
        floats: &[Src],
        dslot: u32,
        kind: &str,
    ) -> Result<(), String> {
        if ints.len() > MAX_INT || floats.len() > MAX_FLOAT {
            return Err(format!(
                "{symbol}: {} integer and {} float arguments, past what a `crt` stencil holds",
                ints.len(),
                floats.len()
            ));
        }
        let base = st.scratch + CARG_WORD * 8;
        let fbase = base + MAX_INT as u32 * 8;
        let callee = super::abi::rt_callee(ints.len(), floats.len(), kind);

        // Where each argument will be **read from**. An operand that is already
        // a whole frame word is read where it lies; everything else — a literal,
        // a narrow field, an address, a glue symbol — is materialised into the
        // scratch area first, exactly as it always was, and read from there.
        let place = |i: usize, at: u32, src: &Src| match src {
            Src::Word(from) => *from,
            _ => at + i as u32 * 8,
        };
        let iat: Vec<u32> =
            ints.iter().enumerate().map(|(i, s)| place(i, base, s)).collect();
        let fat: Vec<u32> =
            floats.iter().enumerate().map(|(i, s)| place(i, fbase, s)).collect();

        // The slots family is the fast one and the array family is the fallback,
        // and the question that decides is whether every offset fits the field
        // the fold puts it in. `Jit::emit` takes a folded twin only when *all*
        // of its `imm12` holes fit, so a single offset past 32 KiB would leave
        // the whole call materialising an offset per argument — worse than the
        // form it replaced. Asking here instead makes that case the array one.
        let fits = |o: &u32| o.is_multiple_of(8) && u64::from(*o) / 8 < 4096;
        let dfits = kind == "v" || fits(&dslot);
        if self.slots_crt_available() && dfits && iat.iter().chain(&fat).all(fits) {
            for (src, at) in ints.iter().zip(&iat).chain(floats.iter().zip(&fat)) {
                if !matches!(src, Src::Word(_)) {
                    self.marshal(*at, src);
                }
            }
            let mut binds: Vec<(String, V)> = Vec::new();
            for (i, at) in iat.iter().enumerate() {
                binds.push((super::abi::rt_slot(i), V::I(u64::from(*at))));
            }
            for (i, at) in fat.iter().enumerate() {
                binds.push((super::abi::rt_float_slot(i), V::I(u64::from(*at))));
            }
            binds.push((String::from("JIT_D"), V::I(u64::from(dslot))));
            binds.push((callee.clone(), callee_v));
            binds.push((String::from("JIT_CONT0"), V::Fall));
            let refs: Vec<(&str, V)> =
                binds.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
            self.emit(&format!("crts/{}/{}/{kind}", ints.len(), floats.len()), &refs);
            return Ok(());
        }

        for (i, src) in ints.iter().enumerate() {
            self.marshal(base + i as u32 * 8, src);
        }
        for (i, src) in floats.iter().enumerate() {
            self.marshal(fbase + i as u32 * 8, src);
        }
        self.emit(
            &format!("crt/{}/{}/{kind}", ints.len(), floats.len()),
            &[
                ("JIT_A", V::I(u64::from(base))),
                ("JIT_B", V::I(u64::from(fbase))),
                ("JIT_D", V::I(u64::from(dslot))),
                (&callee, callee_v),
                ("JIT_CONT0", V::Fall),
            ],
        );
        Ok(())
    }

    /// Whether the slots-only `crt` family may be used at all.
    ///
    /// Always, in the repository. It is a function rather than a constant so
    /// that a measurement copy can ablate it in one place and price the family
    /// against exactly the code it replaced; nothing in the product reads an
    /// environment variable here.
    fn slots_crt_available(&self) -> bool {
        true
    }

    /// A borrowed `Str`'s bytes, as the `(ptr, len)` pair `lib.rs` §2 rule 1
    /// passes one as.
    ///
    /// The tag bits the length carries are masked off, because what a runtime
    /// entry is handed is a byte count — the same masking every backend does
    /// before a runtime call. The mask costs a scratch word, which is why the
    /// caller says which one.
    pub(crate) fn str_arg(&mut self, slot: u32, scratch: u32) -> (Src, Src) {
        self.emit(
            "bin/and/u64/fi/f",
            &[
                ("JIT_D", V::I(u64::from(scratch))),
                ("JIT_A", V::I(u64::from(slot + STR_LEN))),
                ("JIT_K", V::I(crate::compiler::middle::layout::STR_LEN_MASK)),
                ("JIT_CONT", V::Fall),
            ],
        );
        (Src::Word(slot + STR_PTR), Src::Word(scratch))
    }

    /// The per-element retain glue for an element type, or `None` where the
    /// element owns no counted block — which is what `cli/runtime/list.rs`
    /// reads a null pointer as.
    pub(crate) fn element_glue(&mut self, elem: Ty) -> Option<String> {
        self.rc_counted(&elem)
            .then(|| self.helper(super::glue::Helper::Walk { ty: elem, retain: true }))
    }

    /// One argument into its place in the scratch area.
    pub(crate) fn marshal(&mut self, at: u32, src: &Src) {
        match src.clone() {
            Src::Word(from) => self.mv(at, from, 8),
            Src::Narrow(from, w) => self.load_w(at, from, w),
            Src::Imm(v) => self.imm_to(at, v),
            Src::Sym(name) => self.emit(
                "imm/64",
                &[
                    ("JIT_D", V::I(u64::from(at))),
                    ("JIT_M", V::Sym(name)),
                    ("JIT_CONT", V::Fall),
                ],
            ),
            Src::Addr(from) => self.emit(
                "lea",
                &[
                    ("JIT_D", V::I(u64::from(at))),
                    ("JIT_A", V::I(u64::from(from))),
                    ("JIT_CONT", V::Fall),
                ],
            ),
        }
    }

    /// The discriminant of an entry that answered an `Option`.
    ///
    /// The payload is already where it belongs — it was written through the
    /// out-pointer — so all that is left is to label the slot, and to do it
    /// without the emitter learning whether the layout chose a tag or a niche.
    /// A niche needs nothing on the success side, because the pointer the
    /// runtime wrote *is* the discriminant.
    fn store_option_tag(&mut self, st: &mut Fn2, dslot: u32, o: &OptRepr) {
        let disc = st.scratch + DISC_WORD * 8;
        let some = st.label();
        let done = st.label();
        // `BURI_OK` is `-1` as an `i32`, and the `w` result shape put it in the
        // slot zero-extended, so the comparison is against its unsigned form.
        let key = self.arm_key("brcmp/eq/u64/fi", "JIT_F");
        self.emit(
            &key,
            &[
                ("JIT_A", V::I(u64::from(disc))),
                ("JIT_K", V::I(u64::from(BURI_OK as u32))),
                ("JIT_T", V::Blk(some)),
                ("JIT_F", V::Fall),
            ],
        );
        // The failure arm. A niche `Option` says `.None` with a null pointer,
        // which is the same store at a different width and is why the emitter
        // never learns which of the two the layout chose.
        let (at, w, v) = if o.niche {
            (dslot + o.tag.0, 8, 0)
        } else {
            (dslot + o.tag.0, o.tag.1, o.none)
        };
        self.imm_w(at, w, v);
        self.emit("jump", &[("JIT_T", V::Blk(done))]);
        let here = self.region.code_addr();
        st.place(some, here);
        if !o.niche {
            self.imm_w(dslot + o.tag.0, o.tag.1, o.some);
        }
        let here = self.region.code_addr();
        st.place(done, here);
    }

    /// [`Extra::Step`]'s four words: the generated entry thunk, the state
    /// record, and the two element strides.
    ///
    /// The runtime's own loop counter is the fifth thing the boundary carries
    /// and it is **not** one of these: it is an argument of the thunk rather
    /// than of the entry, because it changes per element and these do not.
    ///
    /// The record's shape is `glue.rs`'s [`super::glue::E_FRAME`] — the
    /// closure's two words, the frame the entry thunk is to work in, and the
    /// step's contexts. It is built **past this function's own frame**, which is
    /// where a Buri callee's frame begins and what `lists.rs` writes into
    /// before it calls a step, and the entry thunk's frame is put past the
    /// record in turn. Nothing else is live up there while a call is in
    /// flight, and putting the record in a scratch word instead would have
    /// bounded the contexts it can carry by a constant.
    ///
    /// The runtime never looks inside any of it: it is handed the address and
    /// hands it back to the thunk.
    fn step_extra(
        &mut self,
        prog: &ir::Program,
        st: &mut Fn2,
        entry: &Entry,
        dest: Option<ir::Type>,
        args: &[(u32, ir::Type)],
        ints: &mut Vec<Src>,
    ) -> Result<(), String> {
        let Some(call) = step_call(entry.key) else {
            return Err(format!("{}: a step row with no `step_call` row", entry.key));
        };
        let Some((fslot, fty)) = args.get(call.func).copied() else {
            return Err(format!("{}: no step argument", entry.key));
        };
        let Some(Ty::Fn(params, ret)) = source_ty(prog, fty) else {
            return Err(format!("{}: a step argument that is not a function", entry.key));
        };
        let Some(source) = args.iter().find_map(|(_, t)| array_elem(prog, *t)) else {
            return Err(format!("{}: no source list", entry.key));
        };
        let Some(answer) = dest.and_then(|t| array_elem(prog, t)) else {
            return Err(format!("{}: no result list", entry.key));
        };
        let in_stride = self.layouts_of(source).stride.max(1);
        let out_stride = self.layouts_of(answer).stride.max(1);

        let widths: Vec<u32> =
            params.iter().map(|t| self.layouts_of(t.clone()).size).collect();
        let (ctx_at, bytes) = super::glue::state_shape(&widths, call.index);
        // A context is a *value* here rather than the dropped argument a
        // runtime entry takes, because what reads it is the step and not the
        // runtime. One that owned a count would need a retain per element, and
        // no context does: `core/host`'s are empty structs and
        // `core/host/testing`'s carry a handle.
        let supplied: Vec<(u32, ir::Type)> =
            call.ctx.into_iter().filter_map(|i| args.get(i).copied()).collect();
        if supplied.len() != ctx_at.len() {
            return Err(format!(
                "{}: a step taking {} contexts where the call names {}",
                entry.key,
                ctx_at.len(),
                supplied.len()
            ));
        }
        // `ctx_at` is parallel to the supplied contexts; `widths` to the
        // closure's parameters. An index parameter sits in the second and not
        // in the first, so the walk carries its own cursor rather than reusing
        // the enumeration.
        let ctx_params: Vec<usize> = (0..params.len().saturating_sub(1))
            .filter(|i| call.index != Some(*i))
            .collect();
        for ((off, (from, t)), i) in ctx_at.iter().copied().zip(supplied).zip(ctx_params) {
            let w = widths.get(i).copied().unwrap_or(0);
            if w == 0 {
                continue;
            }
            if let Some(ty) = source_ty(prog, t) {
                if self.rc_counted(&ty) {
                    return Err(format!(
                        "{}: a step whose context owns a reference count",
                        entry.key
                    ));
                }
            }
            self.mv(st.frame.size + off, from, round8(w));
        }
        let state = st.frame.size;
        self.mv(state, fslot, 16);
        self.emit(
            "lea",
            &[
                ("JIT_D", V::I(u64::from(state + super::glue::E_FRAME))),
                ("JIT_A", V::I(u64::from(state + bytes))),
                ("JIT_CONT", V::Fall),
            ],
        );
        let thunk =
            self.helper(super::glue::Helper::Entry { params, ret: *ret, index: call.index });
        ints.push(Src::Sym(thunk));
        ints.push(Src::Addr(state));
        ints.push(Src::Imm(u64::from(in_stride)));
        ints.push(Src::Imm(u64::from(out_stride)));
        Ok(())
    }

    /// The element type of the `[T]` an `Extra::Element` row operates on: the
    /// result's where the result is a list, and the first list argument's
    /// otherwise. Both orders are needed — `list.repeat` mentions `T` only in
    /// its result — and it is the order `llvm/runtime.rs`'s `Arg::Elems` rows
    /// are read in.
    fn element_ty(
        &mut self,
        prog: &ir::Program,
        dest: Option<ir::Type>,
        args: &[(u32, ir::Type)],
    ) -> Option<Ty> {
        let of = |t: ir::Type| -> Option<Ty> {
            let ir::Type::Agg(id) = t else { return None };
            match prog.type_info(id).ty.clone() {
                Ty::Array(e) => Some(*e),
                _ => None,
            }
        };
        dest.and_then(of).or_else(|| args.iter().find_map(|(_, t)| of(*t)))
    }

    /// A value's flattened form: the scalars a C signature would carry it as.
    ///
    /// `llvm/repr.rs`'s slot cover is the same one, and the two must agree leaf
    /// for leaf because they describe the same C function. An aggregate
    /// whose width is not a multiple of eight is refused rather than covered
    /// with narrow chunks: no `buri_rt_*` entry takes one, so a program that
    /// produced one would be a signature this table has got wrong.
    fn leaves(&mut self, prog: &ir::Program, t: ir::Type) -> Result<Vec<Leaf>, String> {
        Ok(match t {
            ir::Type::Unit => Vec::new(),
            ir::Type::I1 | ir::Type::I8 => vec![Leaf { offset: 0, width: 8, float: false }],
            ir::Type::I16 | ir::Type::I32 | ir::Type::I64 | ir::Type::Ptr => {
                vec![Leaf { offset: 0, width: 8, float: false }]
            }
            ir::Type::F32 => vec![Leaf { offset: 0, width: 4, float: true }],
            ir::Type::F64 => vec![Leaf { offset: 0, width: 8, float: true }],
            ir::Type::I128 => vec![
                Leaf { offset: 0, width: 8, float: false },
                Leaf { offset: 8, width: 8, float: false },
            ],
            ir::Type::Agg(id) => {
                let size = self.layout_of(prog, id).size;
                if !size.is_multiple_of(8) {
                    return Err(format!("an aggregate of {size} bytes at a runtime boundary"));
                }
                (0..size / 8)
                    .map(|i| Leaf { offset: i * 8, width: 8, float: false })
                    .collect()
            }
        })
    }
}

/// A scalar result's `crt` shape letter.
///
/// A frame slot always holds a whole 64-bit word, so a narrow integer result is
/// zero-extended into it and an `F32` is stored as its own 32 bits in the low
/// half — which is `sources.rs::write`'s convention, stated once and obeyed
/// here.
///
/// **The letter is the C return type's width, and `Leaf` is not where that
/// lives.** [`Jit::leaves`] answers a *slot*, which is eight bytes for every
/// integer; what the callee returns is the destination's own IR type, and a
/// `Bool` comes back from `buri_rt_str_eq` as a `u8` where an `Int` comes back
/// from `buri_rt_str_hash` as a `u64`. Both psABIs leave the upper bits of a
/// narrower integer return **unspecified**, so a stencil that declared
/// `uint64_t` for the first reads whatever was in the register — which AAPCS64
/// hid, because Rust's arm64 codegen zeroes it on the way out, and SysV did
/// not. `sources.rs::RETURN_SHAPES` has a shape per width for exactly that,
/// and `llvm/runtime.rs` builds its call signature from the same fact.
fn scalar_kind(leaf: Leaf, t: ir::Type) -> &'static str {
    if leaf.float {
        return if leaf.width == 4 { "f" } else { "d" };
    }
    c_int_shape(t)
}

/// The `crt` letter for an integer result of IR type `t`, by the width C gives
/// it. `Ptr` and everything at least a word wide is the plain `uint64_t` shape.
fn c_int_shape(t: ir::Type) -> &'static str {
    match t {
        ir::Type::I1 | ir::Type::I8 => "b",
        ir::Type::I16 => "h",
        ir::Type::I32 => "u",
        _ => "i",
    }
}

/// The width a **signed** integer narrower than a frame word has, and `None`
/// for everything else — an unsigned one, or one that already fills the word.
///
/// A slot holds an integer zero-extended, so this is the set of types whose
/// value has to be widened before it can cross to a C signature that takes a
/// signed sixty-four-bit one.
fn int_bits(prim: crate::compiler::semantics::types::Prim) -> Option<u32> {
    use crate::compiler::semantics::types::Prim;
    Some(match prim {
        Prim::I8 => 8,
        Prim::I16 => 16,
        Prim::I32 => 32,
        _ => return None,
    })
}

pub(crate) fn source_ty(prog: &ir::Program, t: ir::Type) -> Option<Ty> {
    match t {
        ir::Type::Agg(id) => Some(prog.type_info(id).ty.clone()),
        _ => None,
    }
}

/// `core/order`'s three variants, in declaration order, which is what
/// `middle::layout` gives a three-variant enum as a bare tag and what
/// `buri_rt_str_compare` returns.
pub(crate) const LESS: u64 = 0;
pub(crate) const EQUAL: u64 = 1;
pub(crate) const GREATER: u64 = 2;

impl Jit<'_> {
    /// `buri_rt_str_eq` or `buri_rt_str_compare`, with the answer in `dest`.
    ///
    /// **Six** arguments, not four: `lib.rs` §2 rule 1 flattens a `Str` to all
    /// three of its words, and both entries take two of them — the `base` each
    /// ignores is still a parameter, and passing four slid every argument one
    /// register down. The length words go across **unmasked**, because every
    /// entry in `cli/runtime/text.rs` takes the stored word and masks the ASCII
    /// flag off itself.
    pub(crate) fn str_compare(
        &mut self,
        st: &Fn2,
        symbol: &'static str,
        a: u32,
        b: u32,
        dest: u32,
    ) -> Result<(), String> {
        let args = [
            Src::Word(a),
            Src::Word(a + STR_PTR),
            Src::Word(a + STR_LEN),
            Src::Word(b),
            Src::Word(b + STR_PTR),
            Src::Word(b + STR_LEN),
        ];
        // `buri_rt_str_eq` answers a `u8` and `buri_rt_str_compare` a C `int`,
        // and both psABIs leave the rest of the register unspecified in both
        // cases: the result shape has to be the **declared** width, not the
        // register's, and not the wider of the two either. One shape for both
        // was a `Bool` read out of the top three bytes of an `int` on SysV.
        let kind = if symbol == "buri_rt_str_eq" { "b" } else { "w" };
        self.c_call(symbol, st, &args, &[], dest, kind)
    }

    /// Turns `buri_rt_str_compare`'s three-way answer into the boolean an
    /// ordering operator wants.
    ///
    /// `raw` is the answer and `dest` is where the boolean goes, and they are
    /// **two different slots** on purpose: `<=` is two equality tests of the
    /// same three-way answer, and writing the first test's result over the
    /// answer would leave the second comparing a boolean.
    ///
    /// `want` empty means the answer is already the boolean, which is
    /// `buri_rt_str_eq`'s; `BinOp::Ne` is that answer inverted.
    pub(crate) fn order_test(&mut self, st: &Fn2, raw: u32, dest: u32, op: ir::BinOp, want: &[u64]) {
        let scratch = st.scratch + SPARE_WORD * 8;
        match (op, want) {
            (ir::BinOp::Eq, []) => self.mv(dest, raw, 8),
            (ir::BinOp::Ne, []) => self.eq_imm(dest, raw, 0),
            _ => {
                // One equality test per accepted variant, or-ed together. Two
                // is the widest case — `<=` is `Less` or `Equal` — so this is
                // never more than three stencils.
                for (i, v) in want.iter().enumerate() {
                    if i == 0 {
                        self.eq_imm(dest, raw, *v);
                        continue;
                    }
                    self.eq_imm(scratch, raw, *v);
                    self.emit(
                        "bin/or/u64/ff/f",
                        &[
                            ("JIT_D", V::I(u64::from(dest))),
                            ("JIT_A", V::I(u64::from(dest))),
                            ("JIT_B", V::I(u64::from(scratch))),
                            ("JIT_CONT", V::Fall),
                        ],
                    );
                }
            }
        }
    }

    fn eq_imm(&mut self, dest: u32, src: u32, v: u64) {
        self.emit(
            "bin/eq/u64/fi/f",
            &[
                ("JIT_D", V::I(u64::from(dest))),
                ("JIT_A", V::I(u64::from(src))),
                ("JIT_K", V::I(v)),
                ("JIT_CONT", V::Fall),
            ],
        );
    }
}

impl Jit<'_> {
    /// `derivePrimShow.<T>` and the template hole `middle::derives` leaves
    /// behind, at a primitive.
    ///
    /// `llvm/emit.rs::show_prim` is the twin and the renderers are the same
    /// symbols, so that two backends cannot render a `Float` differently.
    /// `quoted` is the whole of the difference between the two holes: `$show`
    /// quotes a `Str` and a `Char`, `$str` does not.
    ///
    /// Two arms are open-coded rather than called. A `Str` at `$str` is its own
    /// answer — three words, copied — and a `Bool` is one of two literals, for
    /// which a call would allocate a string the compiler already has.
    pub(crate) fn show_prim(
        &mut self,
        st: &mut Fn2,
        prim: crate::compiler::semantics::types::Prim,
        src: u32,
        dest: u32,
        quoted: bool,
    ) -> Result<(), String> {
        use crate::compiler::semantics::types::Prim;
        let scr = st.scratch;
        match prim {
            Prim::Str | Prim::Template if !quoted => {
                self.mv(dest, src, 24);
                Ok(())
            }
            Prim::Str | Prim::Template => {
                let (p, l) = self.str_arg(src, scr);
                self.c_call("buri_rt_show_str", st, &[p, l, Src::Addr(dest)], &[], dest, "v")
            }
            Prim::Char => {
                let symbol =
                    if quoted { "buri_rt_show_char" } else { "buri_rt_char_to_str" };
                self.c_call(symbol, st, &[Src::Word(src), Src::Addr(dest)], &[], dest, "v")
            }
            Prim::F64 => self.c_call(
                "buri_rt_show_f64",
                st,
                &[Src::Addr(dest)],
                &[Src::Word(src)],
                dest,
                "v",
            ),
            Prim::Bool => {
                self.show_bool(st, src, dest);
                Ok(())
            }
            // Every integer that fits an `i64`. `buri_rt_str_from_int` takes a
            // signed one, so a `U64` above `i64::MAX` would render negative —
            // which is a wrong answer rather than a missing one, so it is
            // refused here rather than rounded off.
            //
            // A **narrow signed** value has to be widened first. A frame slot
            // holds an integer zero-extended, whatever its type
            // (`sources.rs::write`), and the typed stencils reinterpret the low
            // bytes — so an `I8` of `-3` is `0xfd` in its slot, and handing that
            // to an `i64` parameter renders `253`. `sext` is the widening the
            // C signature asks for, and it is needed at exactly the four signed
            // widths below sixty-four.
            Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64 | Prim::U8 | Prim::U16 | Prim::U32 => {
                let at = match int_bits(prim) {
                    Some(bits) => {
                        self.emit(
                            &format!("sext/{bits}"),
                            &[
                                ("JIT_D", V::I(u64::from(scr))),
                                ("JIT_A", V::I(u64::from(src))),
                                ("JIT_CONT", V::Fall),
                            ],
                        );
                        scr
                    }
                    None => src,
                };
                self.c_call(
                    "buri_rt_str_from_int",
                    st,
                    &[Src::Word(at), Src::Addr(dest)],
                    &[],
                    dest,
                    "v",
                )
            }
            // Every integer wider than an `i64`, through the runtime's own
            // 128-bit decimal. A `U64` above `i64::MAX` is one of them: it is
            // not an `i64` and `buri_rt_str_from_int` would render it negative,
            // so it goes across as a `u128` whose high half is zero rather than
            // through a signed parameter it does not fit.
            Prim::U64 => self.c_call(
                "buri_rt_show_u128",
                st,
                &[Src::Word(src), Src::Imm(0), Src::Addr(dest)],
                &[],
                dest,
                "v",
            ),
            Prim::I128 | Prim::U128 => {
                let symbol =
                    if prim == Prim::I128 { "buri_rt_show_i128" } else { "buri_rt_show_u128" };
                self.c_call(
                    symbol,
                    st,
                    &[Src::Word(src), Src::Word(src + 8), Src::Addr(dest)],
                    &[],
                    dest,
                    "v",
                )
            }
            // `F32` is not here, and it is a marshalling question rather than a
            // gap: a `crt` stencil declares every float parameter `double`, and
            // an `F32` sits in its slot as its own thirty-two bits — so the
            // shape `buri_rt_show_f32` wants is one this call boundary does not
            // have. Widening first would render the `F64` and not the `F32`.
            other => Err(format!("rendering a `{}`", other.name())),
        }
    }

    /// `true` and `false` as literals, which is what the two backends that
    /// generate a helper for this produce and one allocation less than a call.
    fn show_bool(&mut self, st: &mut Fn2, src: u32, dest: u32) {
        let yes = st.label();
        let done = st.label();
        let key = self.arm_key("br/f", "JIT_F");
        self.emit(
            &key,
            &[
                ("JIT_A", V::I(u64::from(src))),
                ("JIT_T", V::Blk(yes)),
                ("JIT_F", V::Fall),
            ],
        );
        self.str_literal(dest, b"false");
        self.emit("jump", &[("JIT_T", V::Blk(done))]);
        let here = self.region.code_addr();
        st.place(yes, here);
        self.str_literal(dest, b"true");
        let here = self.region.code_addr();
        st.place(done, here);
    }

    /// A `Str` literal into a frame slot: `{ base: null, ptr, len }` with the
    /// ASCII flag set, and IMMORTAL by construction because a null `base` owns
    /// nothing (VALUE-MODEL.md §3).
    pub(crate) fn str_literal(&mut self, dest: u32, bytes: &[u8]) {
        let at = self.region.pool_bytes(bytes);
        let ascii = bytes.iter().all(|b| *b < 0x80);
        self.imm_to(dest, 0);
        self.imm_to_ptr(dest + 8, at);
        self.imm_to(dest + 16, bytes.len() as u64 | (u64::from(ascii) << 63));
    }
}

impl Jit<'_> {
    /// Which argument of `str.concat` [`Jit::str_concat`] never sees, at the
    /// arity this call arrived with.
    ///
    /// `str.concat` has no table row, so the rule [`Entry::ctx`] states for
    /// every other key is stated here instead, and it is the same rule read off
    /// the same place — the **declaration**. `Str.concat<C: Alloc>(self, ctx: C,
    /// other: Str)` is three arguments and the middle one is the context;
    /// `lower::template`'s `str.concat(a, b)` is two and never had one. Either
    /// way `buri_rt_str_concat` sees two `Str`s and nothing else.
    ///
    /// By position rather than by type, for [`Entry::ctx`]'s reason: a `C:
    /// Alloc` instantiated at a value that merely *implements* `Alloc` is not a
    /// `Ty::Ctx`, and `s.concat(alloc(), t)` used to reach `str_concat` as three
    /// arguments — which this backend refuses by arity, so it was a "report it"
    /// diagnostic on a program the front end was right to accept.
    pub(crate) const fn concat_ctx(argc: usize) -> Option<usize> {
        if argc == 3 { Some(1) } else { None }
    }

    /// `str.concat(a, b)`, and the `str.concat(self, ctx, other)` a method call
    /// spells: one call to `buri_rt_str_concat`.
    ///
    /// The context is dropped whatever it weighs, which is the same rule
    /// [`Jit::rt_call`] applies to a `buri_rt_*` entry and for the same reason.
    /// `emit.rs`'s arm has already dropped it, at the index
    /// [`Jit::concat_ctx`] names, so what arrives here is two `Str`s — and an
    /// argument list of any other length is refused below rather than
    /// truncated, because a third `Str` here would be a shape this call cannot
    /// express.
    ///
    /// **A call, where the other backend open-codes.** `llvm/emit.rs::concat`
    /// emits MEMORY.md §5.3's three paths as instructions;
    /// this backend emitted the *exact* path only and always allocated, which
    /// showed up as `core/alloc`'s `count` and `total` — observable numbers —
    /// disagreeing with the release build's. The three paths are now
    /// `cli/runtime/text.rs`'s `buri_rt_str_concat` and this is the call to
    /// them, which is the shape MEMORY.md §5.3 already gives `[T]` append: a
    /// header load, two compares, three arms and a `memmove` are a dozen
    /// stencils and a block layout here, against one `crt` stencil for a call.
    ///
    /// The two length words go **unmasked**, unlike every other `Str` argument
    /// this file passes ([`Jit::str_arg`] masks). The ASCII flag in bit 63 is an
    /// input to a concatenation rather than a tag to be stripped — the answer
    /// is ASCII exactly when both halves are — and the callee masks what it
    /// needs as a count.
    pub(crate) fn str_concat(&mut self, st: &Fn2, args: &[u32], dest: u32) -> Result<(), String> {
        let (Some(a), Some(b)) = (args.first().copied(), args.get(1).copied()) else {
            return Err(String::from("str.concat of fewer than two strings"));
        };
        if args.len() > 2 {
            return Err(String::from("str.concat of more than two strings"));
        }
        self.c_call(
            "buri_rt_str_concat",
            st,
            &[
                Src::Word(a),
                Src::Word(a + STR_PTR),
                Src::Word(a + STR_LEN),
                Src::Word(b),
                Src::Word(b + STR_PTR),
                Src::Word(b + STR_LEN),
                Src::Addr(dest),
            ],
            &[],
            0,
            "v",
        )
    }
}
