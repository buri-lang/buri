//! The stencil generators, and the library builder.
//!
//! Paper §5.1–5.3: "the stencil library is constructed from programmer-specified
//! stencil generators … templates are compiled by the Clang C++ compiler to
//! object code. The stencil library builder then extracts their binary code and
//! the linker relocation records". This file is the generator half — it writes C
//! rather than C++ because there is no template metaprogramming here, the
//! Cartesian product is expanded in Rust — and `stencil::extract` is the other
//! half.
//!
//! # The calling convention, and what it costs against the paper's
//!
//! The paper compiles stencils with **ghccc**, in which every parameter is in a
//! register and *every* register is caller-saved, so a continuation call is one
//! `jmp` and a pass-through parameter is zero instructions. Clang exposes ghccc
//! only to LLVM IR, not to C, and on this host (arm64-apple-darwin) it is not
//! available at all. `preserve_none`, the modern replacement, is X86-only in
//! clang 19.
//!
//! So this port uses **AAPCS64 + `__attribute__((musttail))`**, which recovers
//! the two properties that matter and loses one:
//!
//! * recovered: a continuation call is a single `b` (verified — a stencil whose
//!   body is only `TAIL` compiles to exactly `b _JIT_CONT`);
//! * recovered: pass-through parameters cost nothing, because AAPCS64 puts
//!   arguments 1–8 in `x0`–`x7` and 1–8 float arguments in `d0`–`d7`, and a
//!   parameter forwarded at the same ordinal is already in the right register;
//! * lost: only **7** integer registers are available for the CPS register file
//!   (`x1`–`x7`, since `x0` is the frame pointer) against ghccc's 10+, and
//!   `x19`–`x28` are callee-saved so a stencil that needed them would emit a
//!   prologue. None does; every stencil here is a leaf or a single call.
//!
//! The measured consequence is in the report: it is small, because the register
//! pressure in a stencil is 2–3 values.

use super::abi::{Loc, NREGS};
use super::extract::{extract, fold_addressing, fold_cond, fold_imm, swap_arms};
use super::machobj as macho;
use super::stencil::{Library, Stencil};
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    /// (a) one stencil per IR operation; every operand and result in the frame.
    Base = 0,
    /// (b.1) immediate-operand variants.
    Imm = 1,
    /// (c) width-specialised arithmetic (the extend/truncate fold).
    Width = 2,
    /// (d) fused compare-and-branch.
    CmpBr = 3,
    /// (b.2) register-operand and register-result variants: the paper's CPS
    /// register allocation.
    Reg = 4,
    /// (e)+(f) multi-operation supernodes.
    Super = 5,
    /// (f) the addressing-mode fold: a frame-slot hole becomes the `imm12`
    /// field of the load that uses it, which is what the paper gets from
    /// x86-64's `disp32` for nothing. See `stencil::fold_addressing`.
    Addr = 6,
    /// (g) branch mechanics: the *false* arm of every two-target stencil is the
    /// one whose `b` is elidable, so the emitter arranges for it to be the
    /// fallthrough — inverting the comparison when that is what it takes. Needs
    /// the negated `br`/`tagbr` families in the library.
    Br = 7,
    /// (h) the immediate fold: a literal hole becomes the `imm12` field of the
    /// `add`/`sub`/`cmp` that consumes it, the exact analogue of `Addr` for
    /// [`Loc::Imm`]. Plus compare-against-zero variants, which is the one
    /// constant this ISA has a register for.
    IFold = 8,
    /// (i) frame-slot coalescing: a value whose only use is an edge copy is
    /// given the destination's slot, so the copy disappears.
    Coal = 9,
    /// (j) the `mem2reg` the paper names as future work: loop-carried variables
    /// held in the CPS register file across block boundaries.
    M2r = 10,
    /// (k) block layout: reverse postorder rather than IR order, so that the
    /// branch a loop takes every iteration is the fallthrough and the one it
    /// takes once is the branch.
    Lay = 11,
    /// (l) enum dispatch, ported from the Cranelift backend's `compare_chain`
    /// (`fp-wave1`). Two halves, both about the chain of equality tests a
    /// `match` lowers to:
    ///
    /// * **the total chain** — `middle::exhaustiveness` has already proved the
    ///   match total, and every `Term::Switch` in this IR arrives with
    ///   `default: None`, so the *last* arm needs no test of its own. On a
    ///   two-arm `Option` match that halves the dispatch;
    /// * **the byte tag folds into the compare** — `tagbr/eq8` asked C for
    ///   `AT(uint8_t, A) == (uint8_t)OFF(N)`, and clang answered with
    ///   `cmp w8, w9, uxtb`, the *extended*-register form, which
    ///   [`fold_imm`] cannot rewrite into an `imm12` field. The
    ///   comparison is done at 64 bits instead — identical for a tag that fits
    ///   its own field — so the pair folds and the arm is four instructions
    ///   rather than six. `tagbr/eq` and `tagbr/eq32` already folded; the byte
    ///   tag is the one the corpus actually uses.
    Tag = 12,
}

impl Level {
    /// The inverse of [`Level::name`]'s short spelling, for a level named on
    /// the sweep's command line. `None` is a name no level answers to.
    pub fn parse(s: &str) -> Option<Level> {
        Some(match s {
            "base" | "L0" => Level::Base,
            "imm" | "L1" => Level::Imm,
            "width" | "L2" => Level::Width,
            "cmpbr" | "L3" => Level::CmpBr,
            "reg" | "L4" => Level::Reg,
            "super" | "L5" => Level::Super,
            "addr" | "L6" => Level::Addr,
            "br" | "L7" => Level::Br,
            "ifold" | "L8" => Level::IFold,
            "coal" | "L9" => Level::Coal,
            "m2r" | "L10" => Level::M2r,
            "lay" | "L11" => Level::Lay,
            "tag" | "L12" => Level::Tag,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Level::Base => "L0-base",
            Level::Imm => "L1-imm",
            Level::Width => "L2-width",
            Level::CmpBr => "L3-cmpbr",
            Level::Reg => "L4-reg",
            Level::Super => "L5-super",
            Level::Addr => "L6-addr",
            Level::Br => "L7-br",
            Level::IFold => "L8-ifold",
            Level::Coal => "L9-coal",
            Level::M2r => "L10-m2r",
            Level::Lay => "L11-lay",
            Level::Tag => "L12-tag",
        }
    }
    pub fn all() -> [Level; 13] {
        [
            Level::Base,
            Level::Imm,
            Level::Width,
            Level::CmpBr,
            Level::Reg,
            Level::Super,
            Level::Addr,
            Level::Br,
            Level::IFold,
            Level::Coal,
            Level::M2r,
            Level::Lay,
            Level::Tag,
        ]
    }
}

// ---------------------------------------------------------------------------
// Scalar types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sc {
    pub tag: &'static str,
    pub cty: &'static str,
    pub bits: u32,
    pub float: bool,
    pub signed: bool,
}

pub const I64: Sc = Sc { tag: "i64", cty: "int64_t", bits: 64, float: false, signed: true };
pub const U64: Sc = Sc { tag: "u64", cty: "uint64_t", bits: 64, float: false, signed: false };
pub const I32: Sc = Sc { tag: "i32", cty: "int32_t", bits: 32, float: false, signed: true };
pub const U32: Sc = Sc { tag: "u32", cty: "uint32_t", bits: 32, float: false, signed: false };
pub const I16: Sc = Sc { tag: "i16", cty: "int16_t", bits: 16, float: false, signed: true };
pub const U16: Sc = Sc { tag: "u16", cty: "uint16_t", bits: 16, float: false, signed: false };
pub const I8: Sc = Sc { tag: "i8", cty: "int8_t", bits: 8, float: false, signed: true };
pub const U8: Sc = Sc { tag: "u8", cty: "uint8_t", bits: 8, float: false, signed: false };
pub const F64: Sc = Sc { tag: "f64", cty: "double", bits: 64, float: true, signed: true };
pub const F32: Sc = Sc { tag: "f32", cty: "float", bits: 32, float: true, signed: true };

/// Element widths the indexed load/store family is specialised at. Anything
/// else goes through `eload/n`, whose size is a hole and whose body is a call.
pub const ELEM_WIDTHS: [u32; 12] = [1, 2, 4, 8, 12, 16, 20, 24, 32, 40, 48, 64];

/// The types every level generates arithmetic for.
const CORE_TYPES: [Sc; 4] = [I64, U64, F64, F32];
/// The extra widths [`Level::Width`] adds.
const NARROW_TYPES: [Sc; 6] = [I32, U32, I16, U16, I8, U8];

pub const BIN_OPS: [(&str, &str, bool); 16] = [
    ("add", "+", false),
    ("sub", "-", false),
    ("mul", "*", false),
    ("div", "/", false),
    ("rem", "%", false),
    ("and", "&", false),
    ("or", "|", false),
    ("xor", "^", false),
    ("shl", "<<", false),
    ("shr", ">>", false),
    ("eq", "==", true),
    ("ne", "!=", true),
    ("lt", "<", true),
    ("le", "<=", true),
    ("gt", ">", true),
    ("ge", ">=", true),
];

fn op_applies(op: &str, t: Sc) -> bool {
    match op {
        "and" | "or" | "xor" | "shl" | "shr" | "rem" => !t.float,
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// C emission
// ---------------------------------------------------------------------------

fn prelude(n: usize) -> String {
    let ar: Vec<String> = (0..n).map(|k| format!("uint64_t r{k}")).collect();
    let ag: Vec<String> = (0..n).map(|k| format!("double g{k}")).collect();
    let pr: Vec<String> = (0..n).map(|k| format!("r{k}")).collect();
    let pg: Vec<String> = (0..n).map(|k| format!("g{k}")).collect();
    format!(
        "#define ARGS_R {}\n#define ARGS_G {}\n#define PASS_R {}\n#define PASS_G {}\n{}",
        ar.join(", "),
        ag.join(", "),
        pr.join(", "),
        pg.join(", "),
        PRELUDE
    )
}

const PRELUDE: &str = r#"
// GENERATED — the stencil generators of cpjit. See src/gen.rs.
#include <stdint.h>
#include <string.h>

#define HID __attribute__((visibility("hidden")))

// The uniform stencil prototype. x0 is the frame pointer; x1-x3 and d0-d2 are
// the CPS register file (the paper's Figure 8 pass-through parameters).
#define ARGS  uint64_t *fp, ARGS_R, ARGS_G
#define PASS  fp, PASS_R, PASS_G
// The zero-register prototype, for stencils that make a call: nothing may be
// live in the CPS registers across one, so nothing is passed through and clang
// spills nothing.
#define ARGS0 uint64_t *fp
#define PASS0 fp

extern uint64_t *_JIT_CONT(ARGS) HID;
extern uint64_t *_JIT_CONT0(ARGS0) HID;
#define TAIL  __attribute__((musttail)) return _JIT_CONT(PASS)
#define TAIL0 __attribute__((musttail)) return _JIT_CONT0(PASS0)

// A hole. Hidden visibility gives adrp+add, which the patcher rewrites to
// movz+movk: any value below 2^32, no memory reference.
#define H32(n) extern char n[] HID;
// Default visibility gives adrp+ldr through the GOT, which the patcher
// retargets at the JIT region's constant pool: any 64-bit value, one load.
#define H64(n) extern char n[];

#define OFF(n) ((uintptr_t)(n))
#define AT(ty, n) (*(ty *)((char *)fp + OFF(n)))

// The pieces of the runtime a stencil may reach on its own. Each becomes an
// undefined symbol in the object, so each becomes a BRANCH26 relocation the
// system linker resolves out of `libburi_rt.a` — the same archive and the same
// `buri_rt_*` contract both other native backends link against
// (`cli/runtime/lib.rs`).
//
// `noreturn` is load-bearing and not decoration: without it clang must assume
// the call comes back, so every stencil that can reach one saves and restores
// the whole CPS register file around it. That turned `bin/rem/i64/ff/f` — the
// inner loop of the prime kernel — into 41 instructions, twelve of them
// `stp`/`ldp` of callee-saved registers on the path that never traps.
#define NORET __attribute__((noreturn))
NORET void buri_rt_abort_div_zero(void);
NORET void buri_rt_abort_unreachable(void);
NORET void buri_rt_abort(const char *, uint64_t);
void buri_rt_decref(uint64_t, void *);
uint64_t buri_rt_alloc(uint64_t);

H32(_JIT_A) H32(_JIT_B) H32(_JIT_C) H32(_JIT_D)
H32(_JIT_E) H32(_JIT_N) H32(_JIT_P) H32(_JIT_Q)
H64(_JIT_R) H64(_JIT_M)
extern uint64_t *_JIT_T(ARGS) HID;
extern uint64_t *_JIT_F(ARGS) HID;
extern uint64_t *_JIT_CALLEE(uint64_t *) HID;
"#;

struct Out {
    head: String,
    bodies: Vec<String>,
    keys: Vec<String>,
    names: Vec<String>,
}

impl Out {
    fn new() -> Out {
        Out {
            head: format!("{}{HELPERS}", prelude(NREGS)),
            bodies: Vec::new(),
            keys: Vec::new(),
            names: Vec::new(),
        }
    }
    fn push(&mut self, key: &str, body: String) {
        let name = format!("st_{}", key.replace(['/', '.', '-'], "_"));
        // (l) Every stencil hands the frame pointer back in `x0`, where it
        // already is. Nothing costs anything for it — a `musttail` chain
        // forwards the return value for free — and it is what lets a call
        // stencil recover `fp` from the callee instead of preserving it.
        let body = body.replace("void $NAME(", "uint64_t *$NAME(");
        self.bodies.push(format!("\n// {key}\n{}\n", body.replace("$NAME", &name)));
        self.keys.push(key.to_string());
        self.names.push(name);
    }
}

/// Reads operand `slot` (hole letter `A`/`B`) of scalar type `t` at location `l`.
fn read(t: Sc, l: Loc, hole: &str) -> String {
    match l {
        Loc::Frame => format!("AT({}, _JIT_{hole})", t.cty),
        Loc::Imm => {
            if t.float {
                format!("imm_{}()", t.tag)
            } else {
                format!("({})(uintptr_t)_JIT_K", t.cty)
            }
        }
        Loc::Reg(k) => {
            if t.float {
                format!("({})g{k}", t.cty)
            } else {
                format!("({})r{k}", t.cty)
            }
        }
    }
}

/// Writes `expr` of scalar type `t` into destination `l`.
///
/// A frame slot always holds a whole 64-bit word: an integer narrower than 64
/// bits is **zero-extended** into it and a `float` is stored as its 32 bits in
/// the low half, so that one `mov` stencil per byte-width serves every type and
/// a frame slot is never partially defined.
///
/// `Loc` spells operands as well as destinations, so it has an `Imm` case that
/// nothing can be written to. [`dsts_for`] never yields one; the error says so
/// rather than assuming it, because the alternative is a stencil that stores
/// somewhere plausible and computes the wrong thing.
fn write(t: Sc, l: Loc, expr: &str) -> Result<String, String> {
    let widened = if t.float && t.bits == 32 {
        format!("f32_bits({expr})")
    } else if t.float {
        format!("f64_bits({expr})")
    } else if t.bits == 64 {
        format!("(uint64_t)({expr})")
    } else {
        format!("(uint64_t)(uint{}_t)({expr})", t.bits)
    };
    Ok(match l {
        Loc::Frame => format!("AT(uint64_t, _JIT_D) = {widened};"),
        Loc::Reg(k) => {
            if t.float {
                format!("g{k} = (double)({expr});")
            } else {
                format!("r{k} = {widened};")
            }
        }
        Loc::Imm => return Err(String::from("an immediate is not a destination")),
    })
}

const HELPERS: &str = r#"
static inline uint64_t f64_bits(double d) { uint64_t b; memcpy(&b, &d, 8); return b; }
static inline uint64_t f32_bits(float f) { uint32_t b; memcpy(&b, &f, 4); return (uint64_t)b; }
static inline double bits_f64(uint64_t b) { double d; memcpy(&d, &b, 8); return d; }
static inline float bits_f32(uint64_t b) { uint32_t x = (uint32_t)b; float f; memcpy(&f, &x, 4); return f; }
H64(_JIT_K)
static inline double imm_f64(void) { return bits_f64((uint64_t)(uintptr_t)_JIT_K); }
static inline float imm_f32(void) { return bits_f32((uint64_t)(uintptr_t)_JIT_K); }
"#;

/// Every stencil the level's library contains, as C source shards.
pub fn sources(level: Level) -> Result<Vec<Out2>, String> {
    let mut o = Out::new();
    moves(&mut o);
    arithmetic(&mut o, level)?;
    control(&mut o, level);
    calls(&mut o);
    runtime_calls(&mut o);
    regmoves(&mut o, level);
    memory(&mut o, level);
    if level >= Level::Super {
        supernodes(&mut o, level);
    }
    Ok(shard(o))
}

pub struct Out2 {
    pub src: String,
    pub keys: Vec<String>,
    pub names: Vec<String>,
}

/// Splits one generated translation unit into shards clang can compile in
/// parallel. Each shard repeats the prelude.
fn shard(o: Out) -> Vec<Out2> {
    // `Out::push` appends to the three vectors together, so they are the same
    // length and the three chunk walks stay in step; `per` is at least 200, so
    // it is never the zero `chunks` refuses.
    let per = 200usize.max(o.bodies.len().div_ceil(10));
    let mut out = Vec::new();
    let shards = o.bodies.chunks(per).zip(o.keys.chunks(per)).zip(o.names.chunks(per));
    for ((bodies, keys), names) in shards {
        let mut s = o.head.clone();
        for b in bodies {
            s.push_str(b);
        }
        out.push(Out2 { src: s, keys: keys.to_vec(), names: names.to_vec() });
    }
    out
}

// ---------------------------------------------------------------------------
// Families
// ---------------------------------------------------------------------------

/// Frame-to-frame copies of a fixed width, and immediate stores. Every
/// aggregate operation in the IR — `MakeStruct`, `GetField`, `GetPayload`,
/// `GetTag` on a tagged enum — is one of these, because an aggregate lives flat
/// in the frame at its real `middle::layout` offsets.
fn moves(o: &mut Out) {
    for n in [1u32, 2, 4, 8, 16, 24, 32, 48, 64] {
        o.push(
            &format!("mov/{n}"),
            format!(
                "void $NAME(ARGS) {{ memcpy((char*)fp + OFF(_JIT_D), (char*)fp + OFF(_JIT_A), {n}); TAIL; }}"
            ),
        );
    }
    // A copy whose size is itself a hole, for an aggregate wider than the
    // widest fixed stencil.
    o.push(
        "mov/n",
        "void $NAME(ARGS) { memcpy((char*)fp + OFF(_JIT_D), (char*)fp + OFF(_JIT_A), OFF(_JIT_N)); TAIL; }"
            .into(),
    );
    // Store an immediate: 64-bit through the constant pool, and a 32-bit form
    // that needs no pool slot at all.
    o.push("imm/64", "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = (uint64_t)(uintptr_t)_JIT_M; TAIL; }".into());
    o.push("imm/32", "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = (uint64_t)OFF(_JIT_N); TAIL; }".into());
    o.push("imm/z", "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = 0; TAIL; }".into());
    // Store an immediate at a byte width, for a narrow field of an aggregate.
    for (n, ty) in [(1u32, "uint8_t"), (2, "uint16_t"), (4, "uint32_t")] {
        o.push(
            &format!("immw/{n}"),
            format!("void $NAME(ARGS) {{ AT({ty}, _JIT_D) = ({ty})OFF(_JIT_N); TAIL; }}"),
        );
    }
    // Load a narrow field of an aggregate into a whole frame word, and store
    // the low bytes of a frame word into a narrow field.
    for (n, uty, ity) in [(1u32, "uint8_t", "int8_t"), (2, "uint16_t", "int16_t"), (4, "uint32_t", "int32_t")] {
        o.push(
            &format!("loadu/{n}"),
            format!("void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = (uint64_t)AT({uty}, _JIT_A); TAIL; }}"),
        );
        o.push(
            &format!("loads/{n}"),
            format!("void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = (uint64_t)(int64_t)AT({ity}, _JIT_A); TAIL; }}"),
        );
        o.push(
            &format!("store/{n}"),
            format!("void $NAME(ARGS) {{ AT({uty}, _JIT_D) = ({uty})AT(uint64_t, _JIT_A); TAIL; }}"),
        );
    }
    // Sign- and zero-extension of a frame word from a width, which is what the
    // Base level uses instead of a width-specialised arithmetic stencil.
    for bits in [8u32, 16, 32] {
        o.push(
            &format!("sext/{bits}"),
            format!(
                "void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = (uint64_t)(int64_t)(int{bits}_t)AT(uint64_t, _JIT_A); TAIL; }}"
            ),
        );
        o.push(
            &format!("zext/{bits}"),
            format!(
                "void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = (uint64_t)(uint{bits}_t)AT(uint64_t, _JIT_A); TAIL; }}"
            ),
        );
    }
    // A store through a pointer held in a frame slot, at a constant offset:
    // what `MakeArray` needs once the payload has been allocated.
    for (n, ty) in [(1u32, "uint8_t"), (2, "uint16_t"), (4, "uint32_t"), (8, "uint64_t")] {
        o.push(
            &format!("pstore/{n}"),
            format!(
                "void $NAME(ARGS) {{ *({ty} *)(AT(uint64_t, _JIT_A) + OFF(_JIT_N)) = \
                 ({ty})AT(uint64_t, _JIT_B); TAIL; }}"
            ),
        );
    }
    // The indexed element access an open-coded `list.*` loop is made of:
    // a base pointer in one frame slot, an index in another, a constant stride,
    // and a fixed-width copy in either direction.
    //
    // `cranelift/emit.rs::elem_at` is the same address in the same three
    // pieces, and drawing it as *one* stencil rather than a multiply, an add
    // and a load is the paper's §4.3 supernode argument applied to the shape a
    // list loop is entirely made of — the shape §6 says supernodes help most.
    // Unlike the L5 supernodes (which measured 1.00× because the branch between
    // two adjacent stencils was already elided), these remove real
    // instructions: an address computation that would otherwise be three
    // stencils' worth of frame traffic per element.
    // The **zero-extending** indexed load, for an element narrower than a
    // frame word. `gen.rs::write`'s convention is that a frame slot always
    // holds a whole 64-bit word — "an integer narrower than 64 bits is
    // zero-extended into it … so that a frame slot is never partially
    // defined" — and a plain `memcpy` of one byte leaves the other seven
    // holding whatever was there. That is not a slow program, it is a wrong
    // one: `Prim::Bool` compares at 64 bits (`emit.rs::prim_tag`), so a
    // `[Bool]` element compared against `true` answered on garbage.
    for (n, ty) in [(1u32, "uint8_t"), (2, "uint16_t"), (4, "uint32_t")] {
        o.push(
            &format!("eloadz/{n}"),
            format!(
                "void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = (uint64_t)*(const {ty} *)\
                 (AT(uint64_t, _JIT_A) + AT(uint64_t, _JIT_B) * OFF(_JIT_P)); TAIL; }}"
            ),
        );
        o.push(
            &format!("eloadz/{n}/s"),
            format!(
                "void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = (uint64_t)*(const {ty} *)\
                 (AT(uint64_t, _JIT_A) + AT(uint64_t, _JIT_B) * {n}); TAIL; }}"
            ),
        );
    }
    for n in ELEM_WIDTHS {
        // The stride-equals-width twin. A `[T]` whose element needs no
        // alignment padding — which is almost every one — has `stride == size`,
        // and baking that in turns `movz`/`movk`/`mul` into the `ldr`'s own
        // scaled-register form. It is a stencil *variant* in exactly the
        // paper's sense: an operand kind, "an index scaled by a known stride"
        // against "an index scaled by a patched one".
        o.push(
            &format!("eload/{n}/s"),
            format!(
                "void $NAME(ARGS) {{ memcpy((char *)fp + OFF(_JIT_D), \
                 (const char *)(AT(uint64_t, _JIT_A) + AT(uint64_t, _JIT_B) * {n}), \
                 {n}); TAIL; }}"
            ),
        );
        o.push(
            &format!("estore/{n}/s"),
            format!(
                "void $NAME(ARGS) {{ memcpy((char *)(AT(uint64_t, _JIT_A) + \
                 AT(uint64_t, _JIT_B) * {n}), (const char *)fp + OFF(_JIT_D), \
                 {n}); TAIL; }}"
            ),
        );
        o.push(
            &format!("eload/{n}"),
            format!(
                "void $NAME(ARGS) {{ memcpy((char *)fp + OFF(_JIT_D), \
                 (const char *)(AT(uint64_t, _JIT_A) + AT(uint64_t, _JIT_B) * OFF(_JIT_P)), \
                 {n}); TAIL; }}"
            ),
        );
        o.push(
            &format!("estore/{n}"),
            format!(
                "void $NAME(ARGS) {{ memcpy((char *)(AT(uint64_t, _JIT_A) + \
                 AT(uint64_t, _JIT_B) * OFF(_JIT_P)), (const char *)fp + OFF(_JIT_D), \
                 {n}); TAIL; }}"
            ),
        );
    }
    // An element wider than the widest fixed stencil: the size is a hole, so
    // this one is a real `memcpy` call. Rare enough that the call is right.
    o.push(
        "eload/n",
        "void $NAME(ARGS) { memcpy((char *)fp + OFF(_JIT_D), \
         (const char *)(AT(uint64_t, _JIT_A) + AT(uint64_t, _JIT_B) * OFF(_JIT_P)), \
         OFF(_JIT_N)); TAIL; }"
            .into(),
    );
    o.push(
        "estore/n",
        "void $NAME(ARGS) { memcpy((char *)(AT(uint64_t, _JIT_A) + \
         AT(uint64_t, _JIT_B) * OFF(_JIT_P)), (const char *)fp + OFF(_JIT_D), \
         OFF(_JIT_N)); TAIL; }"
            .into(),
    );
    // The one allocation an open-coded loop needs: `n * stride` bytes with
    // VALUE-MODEL.md §2's header, or a null block for an empty list. Once per
    // list operation, not once per element — so it stays a call.
    // A null block for an empty list is what `buri_rt_list_new` answers and what
    // `cranelift/emit.rs::list_filter` tests for before its `memcpy`, so it is
    // the same convention on both sides.
    o.push(
        "elemalloc",
        "void $NAME(ARGS0) { uint64_t n = AT(uint64_t, _JIT_A); \
         AT(uint64_t, _JIT_D) = n ? (uint64_t)(uintptr_t)buri_rt_alloc(n * (uint64_t)OFF(_JIT_P)) \
         : (uint64_t)0; TAIL0; }"
            .into(),
    );
    // The discriminant of a niche-encoded enum: VALUE-MODEL.md §6's second
    // niche, where `.None` is a null pointer and the tag is a comparison.
    o.push(
        "niche_tag",
        "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = AT(uint64_t, _JIT_A) == 0 \
         ? (uint64_t)OFF(_JIT_N) : (uint64_t)OFF(_JIT_P); TAIL; }"
            .into(),
    );
    // Float/integer conversions, for `num.*.toF64` and friends.
    o.push("cvt/i2f", "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = f64_bits((double)(int64_t)AT(uint64_t, _JIT_A)); TAIL; }".into());
    o.push("cvt/u2f", "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = f64_bits((double)AT(uint64_t, _JIT_A)); TAIL; }".into());
    o.push("cvt/f2i", "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = (uint64_t)(int64_t)bits_f64(AT(uint64_t, _JIT_A)); TAIL; }".into());
    o.push("cvt/f2f32", "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = f32_bits((float)bits_f64(AT(uint64_t, _JIT_A))); TAIL; }".into());
    o.push("cvt/f322f", "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = f64_bits((double)bits_f32(AT(uint64_t, _JIT_A))); TAIL; }".into());
}

/// The types a level generates direct arithmetic for.
fn types_for(level: Level) -> Vec<Sc> {
    let mut v: Vec<Sc> = CORE_TYPES.to_vec();
    if level >= Level::Width {
        v.extend_from_slice(&NARROW_TYPES);
    }
    v
}

/// Operand-location combinations a level generates.
fn locs_for(level: Level) -> Vec<Loc> {
    let mut v = vec![Loc::Frame];
    if level >= Level::Imm {
        v.push(Loc::Imm);
    }
    if level >= Level::Reg {
        for k in 0..NREGS {
            v.push(Loc::Reg(k as u8));
        }
    }
    v
}

fn dsts_for(level: Level) -> Vec<Loc> {
    let mut v = vec![Loc::Frame];
    if level >= Level::Reg {
        for k in 0..NREGS {
            v.push(Loc::Reg(k as u8));
        }
    }
    v
}

fn arithmetic(o: &mut Out, level: Level) -> Result<(), String> {
    let locs = locs_for(level);
    let dsts = dsts_for(level);
    for t in types_for(level) {
        for (name, cop, is_cmp) in BIN_OPS {
            if !op_applies(name, t) {
                continue;
            }
            for a in &locs {
                for b in &locs {
                    if matches!(a, Loc::Imm) {
                        // A constant left operand is folded by the emitter for
                        // the commutative operations and is rare otherwise;
                        // generating it would double the library for nothing.
                        continue;
                    }
                    for d in &dsts {
                        let rt = if is_cmp { U64 } else { t };
                        let ra = read(t, *a, "A");
                        let rb = read(t, *b, "B");
                        // SPEC 7.2 rules that `NaN == NaN` is **true** in this
                        // language, so float equality is not C's. Both native
                        // backends spell it in one place — `float_equality` —
                        // as `a == b || (a != a && b != b)`, and this is the
                        // same three comparisons. Getting it from C's `==` is
                        // how `codegen`'s two NaN conformance tests failed.
                        let expr = if t.float && (name == "eq" || name == "ne") {
                            let eq = format!(
                                "(({ra}) == ({rb}) || (({ra}) != ({ra}) && ({rb}) != ({rb})))"
                            );
                            if name == "eq" { eq } else { format!("!{eq}") }
                        } else {
                            format!("({ra}) {cop} ({rb})")
                        };
                        let guard = if (name == "div" || name == "rem") && !t.float {
                            format!("if (({}) == 0) buri_rt_abort_div_zero();\n  ", rb)
                        } else {
                            String::new()
                        };
                        let w = write(rt, *d, &expr)?;
                        let key =
                            format!("bin/{name}/{}/{}{}/{}", t.tag, a.tag(), b.tag(), d.tag());
                        o.push(&key, format!("void $NAME(ARGS) {{ {guard}{w} TAIL; }}"));
                    }
                }
            }
        }
        // Unary.
        for (name, cop) in [("neg", "-"), ("bnot", "~")] {
            if name == "bnot" && t.float {
                continue;
            }
            for a in &locs {
                if matches!(a, Loc::Imm) {
                    continue;
                }
                for d in &dsts {
                    let expr = format!("{cop}({})", read(t, *a, "A"));
                    let w = write(t, *d, &expr)?;
                    o.push(
                        &format!("un/{name}/{}/{}/{}", t.tag, a.tag(), d.tag()),
                        format!("void $NAME(ARGS) {{ {w} TAIL; }}"),
                    );
                }
            }
        }
    }
    // Boolean not, which the IR spells `UnOp::Not` on an `I1`.
    for a in &locs {
        if matches!(a, Loc::Imm) {
            continue;
        }
        for d in &dsts {
            let expr = format!("(({}) ^ 1)", read(U64, *a, "A"));
            let w = write(U64, *d, &expr)?;
            o.push(&format!("un/lnot/b/{}/{}", a.tag(), d.tag()), format!("void $NAME(ARGS) {{ {w} TAIL; }}"));
        }
    }
    Ok(())
}

fn control(o: &mut Out, level: Level) {
    o.push("jump", "void $NAME(ARGS) { __attribute__((musttail)) return _JIT_T(PASS); }".into());
    let locs = locs_for(level);
    for a in &locs {
        if matches!(a, Loc::Imm) {
            continue;
        }
        let c = read(U64, *a, "A");
        o.push(
            &format!("br/{}", a.tag()),
            format!(
                "void $NAME(ARGS) {{ if ({c}) {{ __attribute__((musttail)) return _JIT_T(PASS); }} \
                 else {{ __attribute__((musttail)) return _JIT_F(PASS); }} }}"
            ),
        );
    }
    if level >= Level::Br {
        // (g) The negated branch. A two-target stencil's body is
        // `cmp ; b.cc L ; b _JIT_T ; L: b _JIT_F`, so it is the **false** arm
        // whose branch is the body's last instruction and therefore the only
        // one copy-and-patch can elide. When the IR's `then` side is the block
        // that comes next, the emitter negates the test and swaps the arms
        // rather than paying a branch for the fallthrough.
        for a in &locs {
            if matches!(a, Loc::Imm) {
                continue;
            }
            let c = read(U64, *a, "A");
            o.push(
                &format!("brn/{}", a.tag()),
                format!(
                    "void $NAME(ARGS) {{ if (!({c})) {{ __attribute__((musttail)) return _JIT_T(PASS); }} \
                     else {{ __attribute__((musttail)) return _JIT_F(PASS); }} }}"
                ),
            );
        }
        for (k, ty) in [("", "uint64_t"), ("8", "uint8_t"), ("32", "uint32_t")] {
            // (l) See `tagbr/eq8`: at [`Level::Tag`] the byte comparison is
            // widened so that the tag constant folds into the compare.
            let (lhs, rhs) = if level >= Level::Tag && k == "8" {
                (format!("(uint64_t)AT({ty}, _JIT_A)"), "(uint64_t)OFF(_JIT_N)".to_string())
            } else {
                (format!("AT({ty}, _JIT_A)"), format!("({ty})OFF(_JIT_N)"))
            };
            o.push(
                &format!("tagbr/ne{k}"),
                format!(
                    "void $NAME(ARGS) {{ if ({lhs} != {rhs}) \
                     {{ __attribute__((musttail)) return _JIT_T(PASS); }} \
                     else {{ __attribute__((musttail)) return _JIT_F(PASS); }} }}"
                ),
            );
        }
    }
    if level >= Level::CmpBr {
        // (d) fused compare-and-branch. The paper's Figure 11b supernode.
        for t in types_for(level) {
            for (name, cop, is_cmp) in BIN_OPS {
                if !is_cmp {
                    continue;
                }
                for a in &locs {
                    if matches!(a, Loc::Imm) {
                        continue;
                    }
                    for b in &locs {
                        let ra = read(t, *a, "A");
                        let rb = read(t, *b, "B");
                        o.push(
                            &format!("brcmp/{name}/{}/{}{}", t.tag, a.tag(), b.tag()),
                            format!(
                                "void $NAME(ARGS) {{ if (({ra}) {cop} ({rb})) \
                                 {{ __attribute__((musttail)) return _JIT_T(PASS); }} \
                                 else {{ __attribute__((musttail)) return _JIT_F(PASS); }} }}"
                            ),
                        );
                    }
                }
            }
        }
    }
    // Return: the chain stops and control goes back through the link register
    // the caller's `bl` set. Every `Term::Return` is preceded by moves into the
    // frame's return area, so this stencil moves nothing.
    o.push("ret", "void $NAME(ARGS0) { return fp; }".into());
    o.push(
        "abort",
        "void $NAME(ARGS0) { (void)fp; buri_rt_abort((const char *)(uintptr_t)_JIT_M, \
         (uint64_t)OFF(_JIT_N)); }"
            .into(),
    );
    o.push("unreachable", "void $NAME(ARGS0) { (void)fp; buri_rt_abort_unreachable(); }".into());
    o.push(
        "unsupported",
        "void $NAME(ARGS0) { (void)fp; buri_rt_abort_unreachable(); }".into(),
    );
}

/// (j) The four families cross-block register allocation needs on top of the
/// operand variants: a frame word into a register, a register back into its
/// frame word, and a register into another register.
///
/// The paper's CPS registers only ever hold an expression temporary between the
/// stencil that makes it and the one that eats it, so it never needs any of
/// these. Holding a *loop variable* across a block boundary does: something has
/// to fill the register on the way in and write it back where anything that
/// cannot read a register will look.
fn regmoves(o: &mut Out, level: Level) {
    if level < Level::M2r {
        return;
    }
    for k in 0..NREGS {
        o.push(
            &format!("ld/r{k}"),
            format!("void $NAME(ARGS) {{ r{k} = AT(uint64_t, _JIT_A); TAIL; }}"),
        );
        o.push(
            &format!("ldg/r{k}"),
            format!("void $NAME(ARGS) {{ g{k} = AT(double, _JIT_A); TAIL; }}"),
        );
        o.push(
            &format!("stw/r{k}"),
            format!("void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = r{k}; TAIL; }}"),
        );
        o.push(
            &format!("stwg/r{k}"),
            format!("void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = f64_bits(g{k}); TAIL; }}"),
        );
        o.push(
            &format!("immr/r{k}"),
            format!("void $NAME(ARGS) {{ r{k} = (uint64_t)OFF(_JIT_N); TAIL; }}"),
        );
        for j in 0..NREGS {
            if j == k {
                continue;
            }
            o.push(
                &format!("mvr/r{k}/r{j}"),
                format!("void $NAME(ARGS) {{ r{k} = r{j}; TAIL; }}"),
            );
            o.push(
                &format!("mvg/r{k}/r{j}"),
                format!("void $NAME(ARGS) {{ g{k} = g{j}; TAIL; }}"),
            );
        }
    }
}

/// The boundary onto `libburi_rt.a`: one stencil per *shape* of C call.
///
/// A stencil cannot name a symbol the emitter chooses, so the callee is a hole
/// — and a hole that is *called* rather than materialised, so that it becomes
/// one `bl` and one `ARM64_RELOC_BRANCH26` instead of a pooled pointer and an
/// indirect call. That is why there is a `_JIT_RT_*` declared per shape rather
/// than one `void *` hole: C has one type per name, and the type is what
/// decides which registers clang reads the arguments out of.
///
/// The arguments do not come from holes at all. AAPCS64 puts the first eight
/// integers in `x0`–`x7` and the first eight doubles in `d0`–`d7`, and reading
/// each from its own frame offset would make the shape the *cross product* of
/// eight offsets — thousands of stencils. Instead the emitter writes the
/// arguments into a **contiguous scratch area** with the ordinary `mov` and
/// `imm` stencils it already has (which is what the frame-threaded convention
/// does for a Buri call anyway), and this stencil reads them off consecutively
/// from one hole. The shape is then just `(integers, doubles, result)`.
///
/// Two areas rather than one because the two register banks are assigned
/// independently: a double in argument position three still goes in `d0` if it
/// is the first float.
fn runtime_calls(o: &mut Out) {
    for ni in 0..=MAX_INT_ARGS {
        for nf in 0..=MAX_FLOAT_ARGS {
            for ret in ["v", "i", "w", "d"] {
                let sym = format!("_{}", super::abi::rt_callee(ni, nf, ret));
                let ptypes: Vec<&str> = std::iter::repeat_n("uint64_t", ni)
                    .chain(std::iter::repeat_n("double", nf))
                    .collect();
                let args: Vec<String> = (0..ni)
                    .map(|i| format!("ia[{i}]"))
                    .chain((0..nf).map(|i| format!("fa[{i}]")))
                    .collect();
                let cret = match ret {
                    "i" => "uint64_t",
                    "w" => "int32_t",
                    "d" => "double",
                    _ => "void",
                };
                let decl = format!(
                    "extern {cret} {sym}({}) HID;",
                    if ptypes.is_empty() { String::from("void") } else { ptypes.join(", ") }
                );
                let call = format!("{sym}({})", args.join(", "));
                let store = match ret {
                    "i" => format!("AT(uint64_t, _JIT_D) = {call};"),
                    "w" => format!("AT(uint64_t, _JIT_D) = (uint64_t)(uint32_t){call};"),
                    "d" => format!("AT(uint64_t, _JIT_D) = f64_bits({call});"),
                    _ => format!("(void){call};"),
                };
                let bind = format!(
                    "{}{}",
                    if ni > 0 {
                        "const uint64_t *ia = (const uint64_t *)((char *)fp + OFF(_JIT_A)); "
                    } else {
                        ""
                    },
                    if nf > 0 {
                        "const double *fa = (const double *)((char *)fp + OFF(_JIT_B)); "
                    } else {
                        ""
                    }
                );
                o.push(
                    &format!("crt/{ni}/{nf}/{ret}"),
                    format!("{decl}\nvoid $NAME(ARGS0) {{ {bind}{store} TAIL0; }}"),
                );
            }
        }
    }
    // `core/bits`, at the three widths `bits.buri` declares. Every one is a
    // single machine instruction; the range check in front of the shifts is the
    // emitter's, because `$shiftCount`'s abort is a call and a stencil that
    // called would cost the whole CPS register file.
    //
    // A frame slot holds an integer zero-extended at its own width, so a narrow
    // shift truncates on the way back in: `(uint8_t)(x << n)` is `U8`'s answer
    // and `x << n` at sixty-four bits is `Int`'s.
    for w in [8u32, 32, 64] {
        let uty = format!("uint{w}_t");
        o.push(
            &format!("bits/shl/{w}"),
            format!(
                "void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = (uint64_t)({uty})\
                 (AT({uty}, _JIT_A) << AT(uint64_t, _JIT_B)); TAIL; }}"
            ),
        );
        o.push(
            &format!("bits/shr/{w}"),
            format!(
                "void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = (uint64_t)({uty})\
                 (AT({uty}, _JIT_A) >> AT(uint64_t, _JIT_B)); TAIL; }}"
            ),
        );
        // `x << n | x >> (w - n)` is undefined at `n == 0`, where the second
        // shift is by the whole width, so the count is masked instead — which
        // is what the AArch64 rotate does natively and what clang recognises.
        o.push(
            &format!("bits/rotateLeft/{w}"),
            format!(
                "void $NAME(ARGS) {{ {uty} x = AT({uty}, _JIT_A); \
                 unsigned n = (unsigned)AT(uint64_t, _JIT_B) & ({w} - 1); \
                 AT(uint64_t, _JIT_D) = (uint64_t)({uty})((x << n) | (x >> (({w} - n) & ({w} - 1)))); TAIL; }}"
            ),
        );
        o.push(
            &format!("bits/rotateRight/{w}"),
            format!(
                "void $NAME(ARGS) {{ {uty} x = AT({uty}, _JIT_A); \
                 unsigned n = (unsigned)AT(uint64_t, _JIT_B) & ({w} - 1); \
                 AT(uint64_t, _JIT_D) = (uint64_t)({uty})((x >> n) | (x << (({w} - n) & ({w} - 1)))); TAIL; }}"
            ),
        );
        o.push(
            &format!("bits/popCount/{w}"),
            format!(
                "void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = \
                 (uint64_t)__builtin_popcountll((uint64_t)AT({uty}, _JIT_A)); TAIL; }}"
            ),
        );
        // `__builtin_clzll` is undefined at zero, and `core/bits` answers the
        // width there.
        o.push(
            &format!("bits/leadingZeros/{w}"),
            format!(
                "void $NAME(ARGS) {{ uint64_t x = (uint64_t)AT({uty}, _JIT_A); \
                 AT(uint64_t, _JIT_D) = x ? (uint64_t)(__builtin_clzll(x) - (64 - {w})) : {w}; TAIL; }}"
            ),
        );
        o.push(
            &format!("bits/trailingZeros/{w}"),
            format!(
                "void $NAME(ARGS) {{ uint64_t x = (uint64_t)AT({uty}, _JIT_A); \
                 AT(uint64_t, _JIT_D) = x ? (uint64_t)__builtin_ctzll(x) : {w}; TAIL; }}"
            ),
        );
    }
    // `sar` is `Int`'s alone: it is the arithmetic shift, and every other width
    // in `core/bits` is unsigned.
    o.push(
        "bits/sar/64",
        "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = (uint64_t)\
         ((int64_t)AT(uint64_t, _JIT_A) >> AT(uint64_t, _JIT_B)); TAIL; }"
            .into(),
    );

    // A copy between two addresses held in frame slots, with the count in a
    // third. `mov/n` copies frame to frame with a constant size; this is the
    // one a `str.concat` needs, where all three are values.
    o.push(
        "memcpy/p",
        "void $NAME(ARGS0) { memcpy((void *)AT(uint64_t, _JIT_D), \
         (const void *)AT(uint64_t, _JIT_A), AT(uint64_t, _JIT_B)); TAIL0; }"
            .into(),
    );
    // The address of a frame slot, for an argument the C side takes by
    // reference: an out-pointer, or a `T` whose width the runtime learns from a
    // stride rather than from a type.
    o.push(
        "lea",
        "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = (uint64_t)(uintptr_t)((char *)fp + OFF(_JIT_A)); TAIL; }"
            .into(),
    );
}

/// How wide a runtime call this backend can make. Eight integers is AAPCS64's
/// whole register half — an entry needing a ninth would pass it on the machine
/// stack, which no stencil in this library touches — and two doubles is the
/// widest float half `cli/runtime` has.
pub const MAX_INT_ARGS: usize = 8;
pub const MAX_FLOAT_ARGS: usize = 2;

fn calls(o: &mut Out) {
    // A direct call. The callee's frame begins at `fp + _JIT_N`, where `_JIT_N`
    // is this function's own frame size, so the emitter has already written the
    // arguments into their places with ordinary `mov` stencils and reads the
    // results back with ordinary `mov` stencils. Nothing is copied here.
    // (l) `_JIT_N` and `_JIT_P` are bound to the *same* frame size. Two holes
    // rather than one because each is then materialised separately and each has
    // a single consumer, which is what the immediate fold needs to turn the
    // pair into `add x0, x0, #N` and `sub x0, x0, #N`.
    o.push(
        "call",
        "void $NAME(ARGS0) { fp = (uint64_t *)((char *)_JIT_CALLEE(\
         (uint64_t *)((char *)fp + OFF(_JIT_N))) - OFF(_JIT_P)); TAIL0; }"
            .into(),
    );
    // An indirect call through a closure's code pointer.
    o.push(
        "calli",
        "void $NAME(ARGS0) { fp = (uint64_t *)((char *)\
         ((uint64_t *(*)(uint64_t *))AT(uint64_t, _JIT_A))\
         ((uint64_t *)((char *)fp + OFF(_JIT_N))) - OFF(_JIT_P)); TAIL0; }"
            .into(),
    );
    // A tail call: reuse this frame rather than pushing one. `middle::tail_calls`
    // has already turned self- and mutual recursion into loops, so this exists
    // for the cross-function tail calls it did not.
    for n in 0..=4usize {
        // `n` is at most four, which is how many hole letters there are.
        let args: Vec<String> = ["A", "B", "C", "E"]
            .iter()
            .take(n)
            .map(|h| format!("AT(uint64_t, _JIT_{h})"))
            .collect();
        let call = format!(
            "((uint64_t (*)({}))(uintptr_t)_JIT_R)({})",
            if n == 0 { "void".to_string() } else { vec!["uint64_t"; n].join(", ") },
            args.join(", ")
        );
        o.push(
            &format!("rt/{n}"),
            format!("void $NAME(ARGS0) {{ AT(uint64_t, _JIT_D) = {call}; TAIL0; }}"),
        );
        o.push(
            &format!("rtv/{n}"),
            format!("void $NAME(ARGS0) {{ (void){call}; TAIL0; }}"),
        );
    }
}

fn memory(o: &mut Out, level: Level) {
    // The one stencil every runtime-supplied operation goes through. See
    // `intrin.rs` for why it is one and not one per signature.

    // Reference counting, open-coded against VALUE-MODEL.md §2's one header:
    // `rc` at `ptr - 16`, `IMMORTAL` is `u64::MAX`, and the increment saturates
    // so that it is branchless.
    o.push(
        "incref",
        "void $NAME(ARGS) { uint64_t p = AT(uint64_t, _JIT_A); if (p) { \
         uint64_t *rc = (uint64_t *)(p - 16); uint64_t v = *rc; \
         *rc = v + (v != (uint64_t)-1); } TAIL; }"
            .into(),
    );
    // The decrement, mirroring `cranelift/emit.rs::decref` instruction for
    // instruction: the inline code is the fast path and nothing else, and a
    // count that is one or `IMMORTAL` goes to `buri_rt_decref`, which owns the
    // free and the drop-glue dispatch. Two backends deciding separately when a
    // block dies is the one divergence MEMORY.md §5 cannot tolerate.
    //
    // `fp` is not recovered from the callee here, because `buri_rt_decref`
    // answers nothing: the stencil keeps it across the call, which is the one
    // place in this library where clang emits a prologue.
    o.push(
        "decref/drop",
        "void $NAME(ARGS0) { uint64_t p = AT(uint64_t, _JIT_A); if (p) { \
         uint64_t *rc = (uint64_t *)(p - 16); uint64_t v = *rc; \
         if (v > 1 && v != (uint64_t)-1) { *rc = v - 1; } \
         else { buri_rt_decref(p, (void *)(uintptr_t)_JIT_M); } } TAIL0; }"
            .into(),
    );
    o.push(
        "decref/free",
        "void $NAME(ARGS0) { uint64_t p = AT(uint64_t, _JIT_A); if (p) { \
         uint64_t *rc = (uint64_t *)(p - 16); uint64_t v = *rc; \
         if (v > 1 && v != (uint64_t)-1) { *rc = v - 1; } \
         else { buri_rt_decref(p, (void *)0); } } TAIL0; }"
            .into(),
    );
    let _ = level;
}

/// (e) and (f): supernodes over two and three IR operations.
///
/// Chosen from what the paper says pays — §4.3's `if (a[i] <op> b[j])` and
/// `c = a[i] <op> b[<literal>]` shapes, and §6's observation that the
/// memory-bound benchmarks are the ones supernodes help most — intersected with
/// what this IR actually contains (the census in the report).
fn supernodes(o: &mut Out, level: Level) {
    // Two and three parallel frame copies in one stencil: every control-flow
    // edge in this IR carries block arguments, and at Base each one is its own
    // `mov` stencil with its own continuation branch.
    for n in 2..=4usize {
        let holes = ["A", "B", "C", "E"];
        let dsts = ["D", "N", "P", "Q"];
        let mut body = String::new();
        // `n` runs to four, which is the length of both letter tables.
        for (i, h) in holes.iter().take(n).enumerate() {
            body.push_str(&format!("uint64_t t{i} = AT(uint64_t, _JIT_{h}); "));
        }
        for (i, d) in dsts.iter().take(n).enumerate() {
            body.push_str(&format!("AT(uint64_t, _JIT_{d}) = t{i}; "));
        }
        o.push(&format!("movn/{n}"), format!("void $NAME(ARGS) {{ {body}TAIL; }}"));
    }
    // Fused multiply-add and add-add over frame slots: the two shapes an
    // index computation and a dot product are made of.
    for t in [I64, F64] {
        let c = t.cty;
        o.push(
            &format!("fma/{}", t.tag),
            format!(
                "void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = {}; TAIL; }}",
                if t.float {
                    "f64_bits(AT(double, _JIT_A) * AT(double, _JIT_B) + AT(double, _JIT_C))"
                        .to_string()
                } else {
                    format!("(uint64_t)(AT({c}, _JIT_A) * AT({c}, _JIT_B) + AT({c}, _JIT_C))")
                }
            ),
        );
        o.push(
            &format!("mulimm_add/{}", t.tag),
            format!(
                "void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = {}; TAIL; }}",
                if t.float {
                    "f64_bits(AT(double, _JIT_A) * imm_f64() + AT(double, _JIT_C))".to_string()
                } else {
                    format!(
                        "(uint64_t)(AT({c}, _JIT_A) * ({c})(uintptr_t)_JIT_K + AT({c}, _JIT_C))"
                    )
                }
            ),
        );
        o.push(
            &format!("addimm_add/{}", t.tag),
            format!(
                "void $NAME(ARGS) {{ AT(uint64_t, _JIT_D) = {}; TAIL; }}",
                if t.float {
                    "f64_bits(AT(double, _JIT_A) + imm_f64() + AT(double, _JIT_C))".to_string()
                } else {
                    format!(
                        "(uint64_t)(AT({c}, _JIT_A) + ({c})(uintptr_t)_JIT_K + AT({c}, _JIT_C))"
                    )
                }
            ),
        );
    }
    // Field load fused into a comparison-and-branch: the enum-tag test that
    // every `match` in this language lowers to.
    o.push(
        "tagbr/eq",
        "void $NAME(ARGS) { if (AT(uint64_t, _JIT_A) == (uint64_t)OFF(_JIT_N)) \
         { __attribute__((musttail)) return _JIT_T(PASS); } \
         else { __attribute__((musttail)) return _JIT_F(PASS); } }"
            .into(),
    );
    // (l) The byte tag. At and below [`Level::Lay`] the comparison is written
    // at the field's own width, and clang answers `cmp w8, w9, uxtb` — the
    // *extended*-register form, which `stencil::fold_imm` refuses, so the tag
    // constant costs a `movz`/`movk` pair in every arm of every match. Widening
    // the comparison to 64 bits removes the extension and the pair folds into
    // the compare's `imm12`. It is the same test: the loaded byte is
    // zero-extended either way, and a tag that did not fit its own field could
    // not have been stored there.
    o.push(
        "tagbr/eq8",
        if level >= Level::Tag {
            "void $NAME(ARGS) { if ((uint64_t)AT(uint8_t, _JIT_A) == (uint64_t)OFF(_JIT_N)) \
             { __attribute__((musttail)) return _JIT_T(PASS); } \
             else { __attribute__((musttail)) return _JIT_F(PASS); } }"
                .to_string()
        } else {
            "void $NAME(ARGS) { if (AT(uint8_t, _JIT_A) == (uint8_t)OFF(_JIT_N)) \
             { __attribute__((musttail)) return _JIT_T(PASS); } \
             else { __attribute__((musttail)) return _JIT_F(PASS); } }"
                .to_string()
        },
    );
    o.push(
        "tagbr/eq32",
        "void $NAME(ARGS) { if (AT(uint32_t, _JIT_A) == (uint32_t)OFF(_JIT_N)) \
         { __attribute__((musttail)) return _JIT_T(PASS); } \
         else { __attribute__((musttail)) return _JIT_F(PASS); } }"
            .into(),
    );
    // Increment, store, and branch on the comparison: the back edge of every
    // open-coded `list.*` loop (`lists.rs`). Without it the increment stores
    // the counter and the comparison loads it straight back, which is the
    // paper's §4.3 "common subtree" with a data dependence rather than only a
    // control one.
    for (name, cop) in [("lt", "<"), ("le", "<=")] {
        o.push(
            &format!("incbr/{name}"),
            format!(
                "void $NAME(ARGS) {{ uint64_t t = AT(uint64_t, _JIT_A) + (uint64_t)OFF(_JIT_N); \
                 AT(uint64_t, _JIT_D) = t; if (t {cop} AT(uint64_t, _JIT_B)) \
                 {{ __attribute__((musttail)) return _JIT_T(PASS); }} \
                 else {{ __attribute__((musttail)) return _JIT_F(PASS); }} }}"
            ),
        );
    }
    // A move and a jump in one stencil: the single commonest pair in the IR,
    // because every loop back edge is a run of moves followed by a jump.
    o.push(
        "movjump/1",
        "void $NAME(ARGS) { AT(uint64_t, _JIT_D) = AT(uint64_t, _JIT_A); \
         __attribute__((musttail)) return _JIT_T(PASS); }"
            .into(),
    );
    for n in 2..=4usize {
        let holes = ["A", "B", "C", "E"];
        let dsts = ["D", "N", "P", "Q"];
        let mut body = String::new();
        // `n` runs to four, which is the length of both letter tables.
        for (i, h) in holes.iter().take(n).enumerate() {
            body.push_str(&format!("uint64_t t{i} = AT(uint64_t, _JIT_{h}); "));
        }
        for (i, d) in dsts.iter().take(n).enumerate() {
            body.push_str(&format!("AT(uint64_t, _JIT_{d}) = t{i}; "));
        }
        o.push(
            &format!("movjump/{n}"),
            format!(
                "void $NAME(ARGS) {{ {body} __attribute__((musttail)) return _JIT_T(PASS); }}"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

/// Generates the C, compiles it with `cc`, and extracts the library.
///
/// `dir` is a scratch directory (`OUT_DIR`) that both the sources and the
/// objects are written into, so that a rebuild whose generated C is
/// byte-identical does not pay for clang again. `jobs` shards are compiled in
/// parallel; the sharding exists because one translation unit of twenty-three
/// thousand functions is a minute of clang and twelve are a second.
///
/// The compiler is `cc` rather than `clang` by name, taken from `CC` when it is
/// set, because that is the variable a cross-build or a Nix shell already sets
/// and the toolchain has no business having its own.
/// One slot per shard, filled by whichever worker took it.
type Shards = Arc<Mutex<Vec<Result<Vec<Stencil>, String>>>>;

pub fn build(cc: &str, dir: &std::path::Path, jobs: usize) -> Result<Library, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let shards = Arc::new(sources(Level::Tag)?);
    let results: Shards = Arc::new(Mutex::new((0..shards.len()).map(|_| Ok(Vec::new())).collect()));
    let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..jobs.max(1) {
        let shards = Arc::clone(&shards);
        let results = Arc::clone(&results);
        let next = Arc::clone(&next);
        let dir = dir.to_path_buf();
        let cc = cc.to_string();
        handles.push(std::thread::spawn(move || {
            loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let Some(shard) = shards.get(i) else { return };
                let r = shard_library(&cc, &dir, i, &shard.src);
                if let Ok(mut slot) = results.lock() {
                    if let Some(cell) = slot.get_mut(i) {
                        *cell = r;
                    }
                }
            }
        }));
    }
    for h in handles {
        h.join().map_err(|_| String::from("a stencil shard's thread panicked"))?;
    }

    let mut byname: HashMap<String, Stencil> = HashMap::new();
    let done = results.lock().map_err(|_| String::from("stencil results were poisoned"))?;
    for r in done.iter() {
        for st in r.as_ref().map_err(String::clone)? {
            byname.insert(st.name.clone(), st.clone());
        }
    }
    let mut lib = Library { config: config(), ..Library::default() };
    for sh in shards.iter() {
        for (key, name) in sh.keys.iter().zip(sh.names.iter()) {
            let Some(base) = byname.get(name) else {
                return Err(format!("{cc} emitted no symbol for {name} ({key})"));
            };
            let mut base = base.clone();
            base.name = key.clone();
            // The conditional-branch fold, which is what makes a two-way branch
            // two instructions instead of three.
            if let Some(c) = fold_cond(&base) {
                base = c;
            }
            // Up to four twins per stencil: the arm swap, the immediate fold,
            // the addressing fold, and the last two together. `Jit::emit` picks
            // the most specific one whose fields the operands fit.
            let mut variants: Vec<(String, Stencil)> = vec![(key.clone(), base.clone())];
            if let Some(sw) = swap_arms(&base) {
                variants.push((format!("{key}+swap"), sw));
            }
            let mut extra = Vec::new();
            for (k, v) in &variants {
                if let Some(t) = fold_imm(v) {
                    extra.push((format!("{k}+ifold"), t));
                }
            }
            variants.append(&mut extra);
            let mut extra = Vec::new();
            for (k, v) in &variants {
                if let Some(t) = fold_addressing(v) {
                    extra.push((format!("{k}+fold"), t));
                }
            }
            variants.append(&mut extra);
            for (k, mut v) in variants {
                v.name.clone_from(&k);
                let id = lib.stencils.len() as u32;
                lib.stencils.push(v);
                lib.index.insert(k, id);
            }
        }
    }
    Ok(lib)
}

/// One shard: write the C, compile it if the C moved, and extract.
fn shard_library(
    cc: &str,
    dir: &std::path::Path,
    i: usize,
    src: &str,
) -> Result<Vec<Stencil>, String> {
    let c = dir.join(format!("s{i}.c"));
    let obj = dir.join(format!("s{i}.o"));
    let unchanged =
        std::fs::read(&c).map(|old| old == src.as_bytes()).unwrap_or(false) && obj.exists();
    if !unchanged {
        std::fs::write(&c, src).map_err(|e| format!("{}: {e}", c.display()))?;
        let out = Command::new(cc)
            .args([
                // The one level that was measured: `-O3` moves the four
                // kernels by 1 % and `-Oz` produces a library that does not
                // run, because it lets clang share a tail between two stencils.
                "-O2",
                "-c",
                "-fno-stack-protector",
                "-fomit-frame-pointer",
                "-fno-unwind-tables",
                // Linker-optimization hints are notes *about* an instruction
                // pair, and a stencil is copied out of the object without
                // them; a hint that survived would describe a pair that has
                // been rewritten.
                "-mllvm",
                "-aarch64-enable-collect-loh=false",
            ])
            .arg(&c)
            .arg("-o")
            .arg(&obj)
            .output()
            .map_err(|e| format!("could not run {cc}: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "{cc} failed on stencil shard {i}:\n{}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    let bytes = std::fs::read(&obj).map_err(|e| format!("{}: {e}", obj.display()))?;
    let o = macho::read(&bytes)?;
    extract(&o).map_err(|e| format!("stencil shard {i}: {e}"))
}

/// The library's identity, which enters `Backend::identity` and therefore every
/// `codegen` cache key this backend produces.
///
/// It names what the *bytes of a stencil* depend on and nothing else: the width
/// of the CPS register file, and the level the generators were run at. The
/// compiler's own version is not here — it is hashed in, along with everything
/// else, by `cli/build.rs`, which has the library in front of it.
pub fn config() -> String {
    format!("{} r{NREGS}", Level::Tag.name())
}
