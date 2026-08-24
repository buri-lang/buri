//! The two facts the stencil *builder* and the stencil *user* must agree on.
//!
//! Everything else about a stencil travels inside the library the build script
//! writes, so a disagreement is a missing key and a loud failure. These two do
//! not travel: they are baked into the generated C on one side and into the
//! emitter's key strings on the other, and a mismatch between them is a stencil
//! that copies cleanly and computes the wrong thing. So they live in one file
//! that both sides compile — `cli/build.rs` reaches it with `#[path]`, and the
//! `super::` paths in this directory resolve the same way in both module trees.

/// Where an operand of an instruction lives.
///
/// The paper's §5.1 "operand kind", which is the axis a stencil is specialised
/// along: "the same operation is compiled several times, for the case where it
/// operates on constants, registers, or stack locations".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Loc {
    /// A frame slot, addressed by a byte-offset hole.
    Frame,
    /// A literal, patched into the instruction stream.
    Imm,
    /// CPS register `k`.
    Reg(u8),
}

impl Loc {
    /// The letter this location contributes to a stencil key. The key is the
    /// whole of the lookup, so this spelling is part of the library's format.
    pub fn tag(self) -> String {
        match self {
            Loc::Frame => String::from("f"),
            Loc::Imm => String::from("i"),
            Loc::Reg(k) => format!("r{k}"),
        }
    }
}

/// One target a stencil library is built for.
///
/// A stencil is the bytes clang emitted for a C function, so it is specific to
/// an instruction set *and* to a container: the ISA decides the instructions,
/// and the container decides how the holes were spelled as relocations and how
/// the emitted object has to spell them back. Three combinations are built, and
/// the fourth — x86-64 macOS — is deliberately not, because nothing runs it.
///
/// This lives beside [`CPS_REGISTER_COUNT`] for the same reason [`CPS_REGISTER_COUNT`] does: the build
/// script picks a target to compile *for* and the emitter picks a target to
/// look a library *up* by, and the two must agree on the spelling or the
/// toolchain would silently emit arm64 bytes into an x86-64 object.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tgt {
    MacosArm64,
    LinuxArm64,
    LinuxX86_64,
}

impl Tgt {
    /// Every target a toolchain bakes a library for, in the order `build.rs`
    /// writes them and `mod.rs` reads them. Order is part of the format: the
    /// baked digests are concatenated into `Backend::identity` in it.
    pub const ALL: [Tgt; 3] = [Tgt::MacosArm64, Tgt::LinuxArm64, Tgt::LinuxX86_64];

    /// The name the blob is written under, and the one the emitter asks for.
    /// It is `Output::dir()`'s spelling (`build/buildfile.rs`), so a benchmark
    /// row, an artifact directory and a stencil library all read the same.
    pub fn slug(self) -> &'static str {
        match self {
            Tgt::MacosArm64 => "macos-arm64",
            Tgt::LinuxArm64 => "linux-arm64",
            Tgt::LinuxX86_64 => "linux-x86_64",
        }
    }

    /// What `clang -target` is given. The same two spellings
    /// `cranelift/mod.rs::triple_of` and `llvm/target.rs::triple` produce, so
    /// three backends name a target one way.
    pub fn triple(self) -> &'static str {
        match self {
            Tgt::MacosArm64 => "arm64-apple-darwin",
            Tgt::LinuxArm64 => "aarch64-unknown-linux-gnu",
            Tgt::LinuxX86_64 => "x86_64-unknown-linux-gnu",
        }
    }

    /// Whether the stencils are A64. The instruction-level rewrites — the four
    /// folds and the patcher — are chosen on this and never on the container.
    pub fn is_arm64(self) -> bool {
        !matches!(self, Tgt::LinuxX86_64)
    }

    /// Whether the emitted object is an ELF. The container decides the object
    /// writer and the relocation vocabulary, and nothing else.
    pub fn is_elf(self) -> bool {
        !matches!(self, Tgt::MacosArm64)
    }
}

/// The widest CPS register file AAPCS64 allows: `x0` is the frame pointer and
/// `x1`–`x7` are the argument registers, so seven integers, and `d0`–`d7` are
/// eight doubles. Anything past that goes on the machine stack and the
/// continuation call stops being a single `b`.
///
/// SysV x86-64 is narrower and is what actually binds a three-target
/// toolchain: `rdi` is the frame pointer and `rsi`, `rdx`, `rcx`, `r8`, `r9`
/// are the rest, so **five** integers against AAPCS64's seven, with eight
/// doubles in `xmm0`–`xmm7` either way. [`CPS_REGISTER_COUNT`] is three, which
/// is inside both and is why one width serves all three libraries — see its
/// header for why three is where the library stops growing for nothing.
const AAPCS64_REGISTER_CAPACITY: usize = 7;
const SYSV_REGISTER_CAPACITY: usize = 5;

/// [`CPS_REGISTER_COUNT`] must fit the narrowest convention any target uses, or
/// the stencil prototype would spill on that target and a continuation call
/// would stop being one jump.
const _: () = assert!(
    CPS_REGISTER_COUNT <= SYSV_REGISTER_CAPACITY
        && CPS_REGISTER_COUNT <= AAPCS64_REGISTER_CAPACITY
);

/// How many CPS registers this toolchain's stencil library is built for.
///
/// Three rather than seven, because the width was swept from two to seven and
/// **the number of register assignments came out identical at every width**:
/// demand saturates at two, since a stencil is only ever live-in on an
/// expression temporary or a loop variable. Every kernel cell across the sweep
/// sits within ±4 % of every other with no trend, while seven registers cost
/// 5.5× the library, 11× the install-time clang run and 15× the load. Three is
/// where the library stops growing for nothing.
///
/// A library built at one width is not interchangeable with an emitter at
/// another — the width is spelled into every key — which is why this is one
/// constant both sides compile rather than a knob set in two places.
pub const CPS_REGISTER_COUNT: usize = 3;

/// How wide a runtime call this backend can make.
///
/// Eight integers is AAPCS64's whole register half; the ninth and tenth go on
/// the **machine** stack, which is the one place generated code touches it and
/// is entirely clang's business — the stencil is the zero-register prototype,
/// so nothing of this backend's is live across the call and the `musttail` to
/// the continuation takes one argument whatever the callee took.
///
/// Ten because that is what the widest entry in `cli/runtime` needs:
/// `buri_rt_str_replace(self, from, to, out)` is three `Str`s flattened
/// (`lib.rs` §2 rule 1) and an out-pointer.
///
/// It is here rather than in `sources.rs` because both sides need it: the
/// generator to expand the family, and `rtcall.rs` to refuse a wider shape with
/// a sentence instead of asking for a stencil that does not exist.
pub const MAX_INT_ARGS: usize = 10;
pub const MAX_FLOAT_ARGS: usize = 2;

/// [`rt_slot`] and [`rt_float_slot`] name holes the generated C declares one by
/// one, so a wider limit than the prelude declares would generate C naming an
/// undeclared symbol.
const _: () = assert!(MAX_INT_ARGS <= 10 && MAX_FLOAT_ARGS <= 2);

/// The hole a runtime call's callee is bound to.
///
/// One name per *shape* rather than one name for every call, because the hole
/// is a symbol declared in the generated C and C has one type per name — and
/// the declared type is what decides which registers clang reads the arguments
/// out of. `sources.rs::runtime_calls` declares them; `rtcall.rs` binds them;
/// this is the one spelling both compile.
pub fn rt_callee(ints: usize, floats: usize, ret: &str) -> String {
    format!("JIT_RT_{ints}_{floats}_{ret}")
}

/// The hole a **slots-only** runtime call reads its `i`th integer argument
/// from: a byte offset into the frame, which `extract::fold_addressing` puts in
/// the `imm12` field of the load that uses it.
///
/// The array-passing `crt` family reads every argument out of one contiguous
/// scratch area, so it needs two holes whatever the shape; this family reads
/// each argument from wherever it already is, so it needs one hole per
/// argument. Both exist, and `rtcall.rs` picks between them per call site on
/// whether every offset fits the field.
///
/// Declared in `sources.rs`'s prelude, bound in `rtcall.rs`; this is the one
/// spelling both compile.
pub fn rt_slot(i: usize) -> String {
    format!("JIT_S{i}")
}

/// [`rt_slot`] for a float argument, in the second register bank.
pub fn rt_float_slot(i: usize) -> String {
    format!("JIT_G{i}")
}
