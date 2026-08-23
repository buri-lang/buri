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

/// The widest CPS register file AAPCS64 allows: `x0` is the frame pointer and
/// `x1`–`x7` are the argument registers, so seven integers, and `d0`–`d7` are
/// eight doubles. Anything past that goes on the machine stack and the
/// continuation call stops being a single `b`.
pub const CAP_REGS: usize = 7;

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
pub const NREGS: usize = 3;

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
