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
    /// A CPS register.
    Reg(Reg),
}

impl Loc {
    /// The letter this location contributes to a stencil key. The key is the
    /// whole of the lookup, so this spelling is part of the library's format.
    pub fn tag(self) -> &'static str {
        match self {
            Loc::Frame => "f",
            Loc::Imm => "i",
            Loc::Reg(r) => r.tag(),
        }
    }
}

/// One of the [`CPS_REGISTER_COUNT`] CPS registers. Integers and floats each
/// number theirs from zero, so a register is its number in either file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reg {
    R0,
    R1,
    R2,
}

impl Reg {
    /// Every register, in order. Its type has [`CPS_REGISTER_COUNT`] in it, so
    /// the two can't disagree.
    pub const ALL: [Reg; CPS_REGISTER_COUNT] = [Reg::R0, Reg::R1, Reg::R2];

    /// How a stencil key spells the register.
    pub fn tag(self) -> &'static str {
        match self {
            Reg::R0 => "r0",
            Reg::R1 => "r1",
            Reg::R2 => "r2",
        }
    }
}

/// The register's number, as the generated C and the stencil keys spell it.
impl std::fmt::Display for Reg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8)
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
/// This lives beside [`CPS_REGISTER_COUNT`] for the same reason that constant
/// does: the build script picks a target to compile *for* and the emitter picks
/// a target to look a library *up* by, and the two must agree on the spelling or
/// the toolchain would silently emit arm64 bytes into an x86-64 object.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StencilTarget {
    MacosArm64,
    LinuxArm64,
    LinuxX86_64,
}

impl StencilTarget {
    /// Every target a toolchain bakes a library for, in the order `build.rs`
    /// writes them and `mod.rs` reads them. Order is part of the format: the
    /// baked digests are concatenated into `Backend::identity` in it.
    pub const ALL: [StencilTarget; 3] = [
        StencilTarget::MacosArm64,
        StencilTarget::LinuxArm64,
        StencilTarget::LinuxX86_64,
    ];

    /// The name the blob is written under, and the one the emitter asks for.
    /// It is `Output::dir()`'s spelling (`build/buildfile.rs`), so a benchmark
    /// row, an artifact directory and a stencil library all read the same.
    pub fn slug(self) -> &'static str {
        match self {
            StencilTarget::MacosArm64 => "macos-arm64",
            StencilTarget::LinuxArm64 => "linux-arm64",
            StencilTarget::LinuxX86_64 => "linux-x86_64",
        }
    }

    /// What `clang -target` is given. The same spelling
    /// `llvm/target.rs::triple` produces, so both native backends name a target
    /// one way.
    ///
    /// Linux is `-musl` for the reason `backend::triple_text` argues: the
    /// executables are static PIEs linked against the musl libc this toolchain
    /// ships, and a stencil compiled for one C library and copied into an
    /// object linked against another is a disagreement nothing downstream
    /// checks. Here it is free of consequence rather than merely safe — the
    /// generated C reaches no libc header at all (`sources::compile_flags`
    /// passes `-nostdinc` and no sysroot, and `sources::memcpy_decl` declares
    /// the one function it uses), and `-gnu` and `-musl` share a psABI on both
    /// of these architectures — so clang emits the bytes it emitted before,
    /// and only the name it was asked under changed.
    ///
    /// Measured rather than assumed, when the spelling moved: all three
    /// libraries were rebuilt from an empty scratch directory under `-musl`
    /// and every shard object, and all three encoded blobs, came out identical
    /// to the `-gnu` ones byte for byte — digests included. `Stencil::identity`
    /// is those digests, so nothing that was cached against it was invalidated
    /// by the rename.
    pub fn triple(self) -> &'static str {
        match self {
            StencilTarget::MacosArm64 => "arm64-apple-darwin",
            StencilTarget::LinuxArm64 => "aarch64-unknown-linux-musl",
            StencilTarget::LinuxX86_64 => "x86_64-unknown-linux-musl",
        }
    }

    /// Whether the stencils are A64. The instruction-level rewrites — the four
    /// folds and the patcher — are chosen on this and never on the container.
    pub fn is_arm64(self) -> bool {
        !matches!(self, StencilTarget::LinuxX86_64)
    }

    /// Whether the emitted object is an ELF. The container decides the object
    /// writer and the relocation vocabulary, and nothing else.
    pub fn is_elf(self) -> bool {
        !matches!(self, StencilTarget::MacosArm64)
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
    CPS_REGISTER_COUNT <= SYSV_REGISTER_CAPACITY && CPS_REGISTER_COUNT <= AAPCS64_REGISTER_CAPACITY
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

/// The result shapes a `crt` stencil is generated for, by the letter its keys
/// and its callee hole spell (`sources.rs`'s `return_ctype`).
///
/// **A shape per C return type, and that is the whole point of the list.** A
/// stencil declares the entry it calls, and a declaration that is wider than
/// what the entry returns is not a rounding error: both psABIs leave the upper
/// bits of a narrower integer return **unspecified**, so `uint64_t` against a
/// `u8` reads whatever was in the register. AAPCS64 happened to hide it — Rust's
/// arm64 codegen zeroes the register on the way out — and SysV does not, which
/// is where it was found. `rtcall.rs` picks the letter from the C type the
/// runtime row names (`runtime_table::CScalar`), which is the same fact the
/// LLVM backend builds its call signature from.
///
/// `w` is the odd one and stays: it is the **signed** 32-bit shape a fallible
/// entry's `BURI_*` discriminant comes back in (`cli/runtime/lib.rs` §2), not a
/// width a Buri value has.
pub const RETURN_SHAPES: [&str; 7] = ["v", "i", "w", "d", "b", "h", "u"];

/// The hole a runtime call's callee is bound to.
///
/// One name per *shape* rather than one name for every call, because the hole
/// is a symbol declared in the generated C and C has one type per name — and
/// the declared type is what decides which registers clang reads the arguments
/// out of. `sources.rs::runtime_calls` declares them; `rtcall.rs` binds them;
/// this is the one spelling both compile.
pub fn rt_callee(ints: usize, floats: usize, ret: &'static str) -> Jump {
    Jump::Rt { ints, floats, ret }
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
pub fn rt_slot(i: usize) -> Lit {
    Lit::S(i)
}

/// [`rt_slot`] for a float argument, in the second register bank.
pub fn rt_float_slot(i: usize) -> Lit {
    Lit::G(i)
}

/// A hole the emitter binds: one of the `_JIT_*` symbols `sources.rs`'s
/// prelude declares.
///
/// The prelude declares each one as a small literal (`H32`), a full-width
/// datum (`H64`) or a function, and that declaration decides the relocation
/// clang writes for it. So the name says what a hole takes, and the emitter
/// binds each class with its own kind of value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Lit(Lit),
    Word(Word),
    Jump(Jump),
}

/// A small literal hole, `H32`: a frame offset, a size, a tag.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lit {
    A,
    B,
    C,
    D,
    E,
    N,
    P,
    Q,
    /// [`rt_slot`]'s.
    S(usize),
    /// [`rt_float_slot`]'s.
    G(usize),
}

/// A full-width datum hole, `H64`: a constant, an address, a mask.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Word {
    K,
    M,
    R,
}

/// A hole that is a jump or a call target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Jump {
    Cont,
    Cont0,
    T,
    F,
    Callee,
    /// [`rt_callee`]'s, one per call shape.
    Rt {
        ints: usize,
        floats: usize,
        ret: &'static str,
    },
}

/// What a stencil's hole is named: a [`Slot`] the emitter binds, or a symbol
/// outside the program that the stencil body reached on its own — an abort,
/// `buri_rt_free`, `memcpy`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HoleName {
    Slot(Slot),
    Symbol(String),
}

impl HoleName {
    /// The name a relocation spells, read.
    pub fn of(name: &str) -> HoleName {
        match name.strip_prefix("JIT_").and_then(Slot::of) {
            Some(slot) => HoleName::Slot(slot),
            None => HoleName::Symbol(name.to_string()),
        }
    }
}

impl Slot {
    /// The slot spelled `JIT_{name}`.
    fn of(name: &str) -> Option<Slot> {
        Some(match name {
            "A" => Slot::Lit(Lit::A),
            "B" => Slot::Lit(Lit::B),
            "C" => Slot::Lit(Lit::C),
            "D" => Slot::Lit(Lit::D),
            "E" => Slot::Lit(Lit::E),
            "N" => Slot::Lit(Lit::N),
            "P" => Slot::Lit(Lit::P),
            "Q" => Slot::Lit(Lit::Q),
            "K" => Slot::Word(Word::K),
            "M" => Slot::Word(Word::M),
            "R" => Slot::Word(Word::R),
            "CONT" => Slot::Jump(Jump::Cont),
            "CONT0" => Slot::Jump(Jump::Cont0),
            "T" => Slot::Jump(Jump::T),
            "F" => Slot::Jump(Jump::F),
            "CALLEE" => Slot::Jump(Jump::Callee),
            _ => return Slot::numbered(name),
        })
    }

    /// `S{i}`, `G{i}` and `RT_{ints}_{floats}_{ret}`, the families
    /// [`rt_slot`], [`rt_float_slot`] and [`rt_callee`] spell.
    fn numbered(name: &str) -> Option<Slot> {
        if let Some(rest) = name.strip_prefix("RT_") {
            let mut parts = rest.split('_');
            let ints = parts.next()?.parse().ok()?;
            let floats = parts.next()?.parse().ok()?;
            let ret = parts.next()?;
            let ret = *RETURN_SHAPES.iter().find(|r| **r == ret)?;
            let slot = Slot::Jump(Jump::Rt { ints, floats, ret });
            return parts.next().is_none().then_some(slot);
        }
        let (lit, i): (fn(usize) -> Lit, &str) = match name.split_at_checked(1)? {
            ("S", i) => (Lit::S, i),
            ("G", i) => (Lit::G, i),
            _ => return None,
        };
        let slot = Slot::Lit(lit(i.parse().ok()?));
        // Only the spelling `Display` writes back, so `S01` is no slot.
        (slot.to_string().strip_prefix("JIT_") == Some(name)).then_some(slot)
    }
}

impl std::fmt::Display for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Slot::Lit(Lit::S(i)) => return write!(f, "JIT_S{i}"),
            Slot::Lit(Lit::G(i)) => return write!(f, "JIT_G{i}"),
            Slot::Jump(Jump::Rt { ints, floats, ret }) => {
                return write!(f, "JIT_RT_{ints}_{floats}_{ret}");
            }
            Slot::Lit(Lit::A) => "A",
            Slot::Lit(Lit::B) => "B",
            Slot::Lit(Lit::C) => "C",
            Slot::Lit(Lit::D) => "D",
            Slot::Lit(Lit::E) => "E",
            Slot::Lit(Lit::N) => "N",
            Slot::Lit(Lit::P) => "P",
            Slot::Lit(Lit::Q) => "Q",
            Slot::Word(Word::K) => "K",
            Slot::Word(Word::M) => "M",
            Slot::Word(Word::R) => "R",
            Slot::Jump(Jump::Cont) => "CONT",
            Slot::Jump(Jump::Cont0) => "CONT0",
            Slot::Jump(Jump::T) => "T",
            Slot::Jump(Jump::F) => "F",
            Slot::Jump(Jump::Callee) => "CALLEE",
        };
        write!(f, "JIT_{name}")
    }
}

impl std::fmt::Display for Lit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Slot::Lit(*self).fmt(f)
    }
}

impl std::fmt::Display for Word {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Slot::Word(*self).fmt(f)
    }
}

impl std::fmt::Display for Jump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Slot::Jump(*self).fmt(f)
    }
}

impl std::fmt::Display for HoleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HoleName::Slot(s) => s.fmt(f),
            HoleName::Symbol(s) => f.write_str(s),
        }
    }
}
