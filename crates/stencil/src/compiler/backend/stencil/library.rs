//! Stencils, holes, and the library that maps a configuration to a stencil.
//!
//! Paper correspondence (§3, §5.3): a stencil is "a binary code function that
//! implements a computation logic fragment, where literals, jump addresses, and
//! stack offsets are missing". A hole is "a unique external variable, which
//! translates to a unique symbol referenced by the linker relocation record".
//! Both of those are literally what this file holds: `code` is the bytes clang
//! emitted for one C function, and `holes` is the relocation records grouped by
//! the undefined symbol they name.
//!
//! The one place this deviates from the paper is *what a hole is patched with*,
//! and it is an ISA fact rather than a design choice — see [`HoleKind`].

use super::abi::{HoleName, Reg, Slot};
use crate::hash::Map as HashMap;

/// How a hole's value reaches the instruction stream.
///
/// The three kinds are the same on both instruction sets this library is built
/// for, and mean the same thing — a small literal, a full-width datum, a jump
/// target — but *what a patch does* differs, and the difference is the whole
/// substance of the AArch64 port. The x86-64 side is described under each
/// variant after the AArch64 one.
///
/// On x86-64, the paper's target, every hole is one contiguous field: a 32-bit
/// displacement or a 64-bit `movabs` immediate, and "patching" is a store. On
/// AArch64 no instruction has a 32-bit immediate field, so a hole is always a
/// *pair* of instructions and patching is an instruction rewrite:
///
/// * [`HoleKind::Imm32`] — clang emits `adrp Xd, sym@PAGE` + `add Xd, Xd,
///   sym@PAGEOFF` for the address of a hidden symbol. That pair can only
///   produce a PC-relative address, never an arbitrary small integer, so the
///   patcher **rewrites the pair** into `movz Xd, #lo16` + `movk Xd, #hi16,
///   lsl 16`. Same two instructions, no memory reference, any value below 2^32.
///   This is the AArch64 analogue of the paper's `movabs`, and it is exact.
/// * [`HoleKind::Imm64`] — for a value that needs all 64 bits, clang's GOT form
///   `adrp Xd, sym@GOTPAGE` + `ldr Xd, [Xd, sym@GOTPAGEOFF]` is retargeted at a
///   slot in the JIT region's own constant pool. **This costs one L1 load that
///   the paper's x86-64 `movabs` does not**, and it is the single largest
///   fidelity gap in this port. §"deviations" in the report says so.
/// * [`HoleKind::Branch`] — `ARM64_RELOC_BRANCH26` on `b`/`bl`, patched with a
///   signed 26-bit word displacement. This one *is* the paper's case.
///
/// On x86-64/ELF the same three kinds are recovered from what clang emits for
/// the same C, and each is cheaper:
///
/// * [`HoleKind::Imm32`] — a hidden hole compiles to `lea rD, [rip+disp32]`,
///   seven bytes. `mov rD, imm32` (`REX.W C7 /0`) is *also* seven bytes, so the
///   patch is an in-place rewrite with nothing left over and no second
///   instruction involved. Where the value would not survive that form's sign
///   extension the patcher writes the five-byte zero-extending `mov rD32,
///   imm32` and two bytes of `nop`, which is still one instruction and no
///   memory reference.
/// * [`HoleKind::Imm64`] — a default-visibility hole compiles to `mov rD,
///   sym@GOTPCREL(%rip)`, and the patcher retargets its `disp32` at the JIT
///   region's own constant pool. Same single load as the AArch64 form, and the
///   same fidelity gap against the paper's `movabs` — which does not fit,
///   being ten bytes where the GOT load is seven.
/// * [`HoleKind::Branch`] — `R_X86_64_PLT32` on `jmp`/`call`, patched with a
///   signed 32-bit byte displacement. The paper's case exactly, and unlike
///   AArch64's 26-bit word field it can never be out of range for anything
///   this emitter produces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HoleKind {
    Imm32,
    Imm64,
    Branch,
}

impl HoleName {
    /// The relocation a hole by this name is written with: the class its slot
    /// is declared in, and a branch for a symbol outside the program, which a
    /// stencil only ever calls.
    pub fn kind(&self) -> HoleKind {
        match self {
            HoleName::Slot(Slot::Lit(_)) => HoleKind::Imm32,
            HoleName::Slot(Slot::Word(_)) => HoleKind::Imm64,
            HoleName::Slot(Slot::Jump(_)) | HoleName::Symbol(_) => HoleKind::Branch,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SiteKind {
    /// `adrp` of a `PAGE21` pair; rewritten to `movz`.
    Adrp,
    /// `add` of a `PAGEOFF12` pair; rewritten to `movk`.
    AddLo12,
    /// `adrp` of a GOT pair; retargeted at the constant pool page.
    GotAdrp,
    /// `ldr` of a GOT pair; retargeted at the constant pool slot.
    GotLdr,
    /// `b` or `bl`.
    Branch26,
    /// A `b.cc`/`cbz`/`cbnz` whose 19-bit word displacement is the hole. Never
    /// produced by a relocation record — clang has no way to spell it — only by
    /// [`fold_cond`], which is this port's second invented fold.
    Cond19,

    // x86-64. A site is recorded at the **`disp32`/`rel32` field**, not at the
    // instruction, because on this ISA the field is what a relocation names and
    // what a patch writes; where the patcher also needs the opcode, the
    // instruction's start travels beside it in `Hole::pairs`.
    /// `lea rD, [rip+disp32]` for a hidden hole; rewritten to a `mov` of the
    /// literal. The x86-64 counterpart of the [`Adrp`](SiteKind::Adrp) /
    /// [`AddLo12`](SiteKind::AddLo12) pair, in one instruction instead of two.
    LeaPc32,
    /// The `disp32` of a rip-relative memory operand that reads the hole out of
    /// the GOT; retargeted at the constant pool. The counterpart of
    /// [`GotAdrp`](SiteKind::GotAdrp) / [`GotLdr`](SiteKind::GotLdr).
    GotPc32,
    /// The `rel32` of a `jmp` or `call`. The counterpart of
    /// [`Branch26`](SiteKind::Branch26).
    Rel32,
    /// The `rel32` of a `jcc`. The counterpart of [`Cond19`](SiteKind::Cond19)
    /// — and, unlike it, **not** something a fold had to invent: clang emits a
    /// conditional branch straight to a `musttail` continuation on this target,
    /// because x86-64 has a 32-bit conditional displacement and AArch64's is 19
    /// bits and could not be trusted to reach. The whole of `fold_cond` is what
    /// this one relocation record gives away for nothing.
    CondRel32,
}

#[derive(Clone, Debug)]
pub struct Site {
    pub off: u32,
    pub kind: SiteKind,
}

#[derive(Clone, Debug)]
pub struct Hole {
    pub name: HoleName,
    pub kind: HoleKind,
    /// The relocation records this hole was built from, grouped. Populated by
    /// the build script's extractor and **empty in a decoded library**: the
    /// three folds that read it are the ones that turn a hole into an `imm12`
    /// or an `imm19` field, and they run once, when the library is built.
    pub sites: Vec<Site>,
    /// `(adrp offset, add-or-ldr offset)`, already matched by destination
    /// register, for the two immediate kinds.
    pub pairs: Vec<(u32, u32)>,
    /// Offsets of `b`/`bl` instructions, for [`HoleKind::Branch`].
    pub branches: Vec<u32>,
    /// Offsets of conditional branches whose `imm19` is the hole. See
    /// [`fold_cond`].
    pub conds: Vec<u32>,
    /// `(offset, scale)` of a load or store whose *unsigned 12-bit offset
    /// field* is the hole. See [`fold_addressing`].
    pub lo12: Vec<(u32, u32)>,
}

/// A place where a stencil reads bytes clang spilled, and where in
/// [`Stencil::consts`] those bytes are.
///
/// Not a [`Hole`]: nothing is patched into the instruction and there is no
/// value to bind. What the emitter does is copy [`Stencil::consts`] into the
/// unit's own constant pool and aim the reference at the copy, which is the
/// same thing the linker would have done with clang's `.rodata`.
#[derive(Clone, Copy, Debug)]
pub struct ConstRef {
    /// Byte offset of the four-byte `disp32` field within the stencil.
    pub field: u32,
    /// Byte offset of the end of the instruction the field belongs to, which
    /// is where the processor measures a rip-relative displacement from.
    pub insn_end: u32,
    /// Byte offset within [`Stencil::consts`] of the datum being read.
    pub at: u32,
}

#[derive(Clone, Debug)]
pub struct Stencil {
    pub name: String,
    pub code: Vec<u8>,
    pub holes: Vec<Hole>,
    /// Read-only bytes this stencil reads and no hole can hold. **Empty for
    /// every AArch64 stencil**, where a constant that would need one is an
    /// error the extractor refuses (`extract.rs`); on x86-64 an SSE sign mask
    /// and the two biases of a `u64`-to-`double` conversion arrive this way.
    pub consts: Vec<u8>,
    /// The alignment [`Stencil::consts`] must be copied at. Sixteen for an SSE
    /// constant, and reading one at less is a fault rather than a slowdown.
    pub consts_align: u32,
    pub const_refs: Vec<ConstRef>,
    /// Index in `holes` of the continuation whose `b` is the body's very last
    /// instruction. Copy-and-patch elides that branch when the continuation is
    /// laid out immediately after (the paper's "control is passed directly to
    /// the next operation").
    pub tail: Option<usize>,
}

impl Stencil {
    /// The three spill fields of a stencil that has none, for the struct-update
    /// syntax. Every AArch64 stencil is one: `extract.rs` refuses an object
    /// that spilled anything at all.
    pub fn unspilled() -> Stencil {
        Stencil {
            name: String::new(),
            code: Vec::new(),
            holes: Vec::new(),
            consts: Vec::new(),
            consts_align: 0,
            const_refs: Vec::new(),
            tail: None,
        }
    }
}

/// The folded twins of a stencil, most specific first.
///
/// A twin is the same operation with an offset or a literal folded into an
/// `imm12` field, so it exists only where the fold is representable; the two
/// folds are independent, which is why there are three names and not two.
/// [`Jit::emit`](super::jit::Jit::emit) tries them
/// in this order and takes the first whose fields the operands fit.
pub const FOLD_SUFFIXES: [&str; FOLD_SUFFIXES_LEN] = ["+ifold+fold", "+fold", "+ifold"];
pub const FOLD_SUFFIXES_LEN: usize = 3;

/// The slot of [`FOLD_SUFFIXES`] holding the plain `+fold` twin.
pub const FOLD_PLAIN: usize = 1;

/// The slot of [`Library::fold_twins`] that names no stencil.
const NO_TWIN: u32 = u32::MAX;

/// The suffix of a two-target stencil's twin whose arms are laid out the other
/// way round (`extract::swap_arms`).
const SWAP_SUFFIX: &str = "+swap";

/// A row of [`Library::twins`]: the [`FOLD_SUFFIXES`] twins, then the
/// [`SWAP_SUFFIX`] one.
const TWINS: usize = FOLD_SUFFIXES_LEN + 1;

/// The two-target stencils the emitter names without asking whether the
/// library has them, by their index in it.
///
/// [`Known::KEYS`] is the list: the build script refuses a library that lacks
/// one ([`Builder::missing_known`]), and [`Library::decode`] finds each once,
/// so a branch the emitter writes is an index rather than a lookup that could
/// come back empty.
#[derive(Clone, Copy, Debug)]
pub struct Known {
    /// `br/f`: a branch on a boolean in the frame.
    pub br_f: usize,
    /// `br/r0`, `br/r1` and `br/r2`: a branch on a boolean in a register.
    pub br_r0: usize,
    pub br_r1: usize,
    pub br_r2: usize,
    /// `brcmp/eq/u64/fi`: a frame word against a literal.
    pub eq_u64_fi: usize,
    /// `brcmp/lt/u64/ff`: two frame words, unsigned.
    pub lt_u64_ff: usize,
    /// `incbr/lt`: a counted loop's increment and test.
    pub inc_lt: usize,
    /// `tagbr/eq8`, `tagbr/eq32` and `tagbr/eq`: an enum's tag of one, four
    /// or eight bytes against a variant.
    pub tag_eq8: usize,
    pub tag_eq32: usize,
    pub tag_eq: usize,
}

impl Known {
    /// Every key a [`Known`] holds, in its fields' order.
    pub const KEYS: [&'static str; 10] = [
        "br/f",
        "br/r0",
        "br/r1",
        "br/r2",
        "brcmp/eq/u64/fi",
        "brcmp/lt/u64/ff",
        "incbr/lt",
        "tagbr/eq8",
        "tagbr/eq32",
        "tagbr/eq",
    ];

    /// Each key's index in `index`, or the first key it lacks.
    fn find(index: &HashMap<&str, u32>) -> Result<Known, String> {
        let [br_f, br_r0, br_r1, br_r2, eq_u64_fi, lt_u64_ff, inc_lt, tag_eq8, tag_eq32, tag_eq] =
            Known::KEYS.map(|key| index.get(key).map(|i| *i as usize).ok_or(key));
        let missing = |key: &str| format!("the library has no `{key}` stencil");
        Ok(Known {
            br_f: br_f.map_err(missing)?,
            br_r0: br_r0.map_err(missing)?,
            br_r1: br_r1.map_err(missing)?,
            br_r2: br_r2.map_err(missing)?,
            eq_u64_fi: eq_u64_fi.map_err(missing)?,
            lt_u64_ff: lt_u64_ff.map_err(missing)?,
            inc_lt: inc_lt.map_err(missing)?,
            tag_eq8: tag_eq8.map_err(missing)?,
            tag_eq32: tag_eq32.map_err(missing)?,
            tag_eq: tag_eq.map_err(missing)?,
        })
    }

    /// The branch on a boolean in this register, or in the frame for `None`.
    pub fn br(&self, at: Option<Reg>) -> usize {
        match at {
            None => self.br_f,
            Some(Reg::R0) => self.br_r0,
            Some(Reg::R1) => self.br_r1,
            Some(Reg::R2) => self.br_r2,
        }
    }
}

/// A library the build script is filling: the stencils in key order, and each
/// key's index. [`Builder::encode`] writes it out; [`Library::decode`] reads it
/// back.
#[derive(Default)]
pub struct Builder {
    stencils: Vec<Stencil>,
    index: HashMap<String, u32>,
    config: String,
}

impl Builder {
    pub fn new(config: String) -> Builder {
        Builder {
            config,
            ..Builder::default()
        }
    }

    pub fn push(&mut self, key: String, s: Stencil) {
        self.index.insert(key, self.stencils.len() as u32);
        self.stencils.push(s);
    }

    /// The first of [`Known::KEYS`] this library lacks, which the build script
    /// refuses: the emitter writes each without asking.
    pub fn missing_known(&self) -> Option<&'static str> {
        Known::KEYS
            .into_iter()
            .find(|key| !self.index.contains_key(*key))
    }
}

/// A library read out of the bytes [`Builder::encode`] wrote, a stencil at a
/// time.
///
/// **Every `buri` process that emits native code decodes one, and most of them
/// use a few hundred of its twenty-five thousand stencils.** Decoding them all
/// up front was half a million allocations, 28% of the processor time of a
/// small `buri test`. So the decode reads the keys and the table of where each
/// stencil starts, and a stencil's holes, code and constants are read the
/// first time the emitter asks for that stencil. So are its folded twins.
pub struct Library {
    pub config: String,
    /// The stencils the emitter names without asking.
    pub known: Known,
    bytes: &'static [u8],
    index: HashMap<&'static str, u32>,
    /// Where each stencil's record and its code start, as the table in the
    /// bytes has them.
    starts: Vec<(u32, u32)>,
    /// Each stencil, read the first time it is asked for. `None` is a record
    /// that did not read back, which reads as a stencil the library lacks.
    stencils: Vec<std::sync::OnceLock<Option<Stencil>>>,
    /// Each stencil's [`FOLD_SUFFIXES`] twins and its [`SWAP_SUFFIX`] one, by
    /// index into `stencils`.
    ///
    /// The emitter asks for all three on **every stencil it copies**, and it
    /// used to ask by building three `String`s with `format!` and hashing each
    /// one — three allocations and three hash lookups per machine instruction
    /// this backend emits. The names are a function of the library alone, so
    /// the answer is too, and it is worked out once per stencil.
    twins: Vec<std::sync::OnceLock<[u32; TWINS]>>,
}

impl Library {
    pub fn get(&self, key: &str) -> Option<&Stencil> {
        self.at(key).map(|(_, s)| s)
    }

    /// [`Library::get`], with the index the fold twins are asked by.
    pub fn at(&self, key: &str) -> Option<(usize, &Stencil)> {
        let i = *self.index.get(key)? as usize;
        Some((i, self.stencil(i)?))
    }

    /// The stencil at `i`, read out of the bytes the first time.
    pub fn stencil(&self, i: usize) -> Option<&Stencil> {
        let (record, code) = *self.starts.get(i)?;
        self.stencils
            .get(i)?
            .get_or_init(|| read_stencil(self.bytes, record, code).ok())
            .as_ref()
    }

    /// How many stencils the library has.
    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every stencil, in key order.
    pub fn iter(&self) -> impl Iterator<Item = &Stencil> {
        (0..self.len()).filter_map(|i| self.stencil(i))
    }

    /// Every key, in no particular order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.index.keys().copied()
    }

    /// The `k`th [`FOLD_SUFFIXES`] twin of the stencil at `i`, if the library
    /// has one.
    pub fn fold_twin(&self, i: usize, k: usize) -> Option<&Stencil> {
        self.twin(i, k).map(|(_, s)| s)
    }

    /// The [`SWAP_SUFFIX`] twin of the stencil at `i`, and its index, if the
    /// library has one.
    pub fn swap_twin(&self, i: usize) -> Option<(usize, &Stencil)> {
        self.twin(i, FOLD_SUFFIXES_LEN)
    }

    fn twin(&self, i: usize, k: usize) -> Option<(usize, &Stencil)> {
        let s = self.stencil(i)?;
        let j = *self
            .twins
            .get(i)?
            .get_or_init(|| self.twins_of(&s.name))
            .get(k)?;
        if j == NO_TWIN {
            return None;
        }
        Some((j as usize, self.stencil(j as usize)?))
    }

    /// The twins of the stencil called `name`, looked up by name.
    fn twins_of(&self, name: &str) -> [u32; TWINS] {
        let mut out = [NO_TWIN; TWINS];
        let mut twin = String::new();
        for (slot, suffix) in out
            .iter_mut()
            .zip(FOLD_SUFFIXES.iter().chain([&SWAP_SUFFIX]))
        {
            twin.clear();
            twin.push_str(name);
            twin.push_str(suffix);
            if let Some(j) = self.index.get(twin.as_str()) {
                *slot = *j;
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// The serialized library
// ---------------------------------------------------------------------------
//
// `cli/build.rs` runs clang, extracts the stencils and writes this; the backend
// reads it back out of `include_bytes!`. It is not a general format and has no
// version negotiation, because both halves are compiled from this file in the
// same build — a change here changes both sides at once, and a toolchain never
// meets a library it did not build: `build.rs` reruns whenever this file
// changes. What it does carry is [`Library::config`], which enters
// `Backend::identity` so that two toolchains whose *stencils* differ do not
// share cached objects.
//
// Everything is little-endian and length-prefixed:
//
//   config, n, n keys,
//   n × (record offset, code offset)     where each stencil starts
//   n records                            name, holes, constants
//   every stencil's code, concatenated
//
// The table is what lets a decode read the keys and stop: a record is read
// when its stencil is first asked for ([`Library::stencil`]).

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .ok_or("stencil library: length overflow")?;
        let out = self
            .bytes
            .get(self.at..end)
            .ok_or("stencil library: truncated")?;
        self.at = end;
        Ok(out)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([
            *b.first().unwrap_or(&0),
            *b.get(1).unwrap_or(&0),
            *b.get(2).unwrap_or(&0),
            *b.get(3).unwrap_or(&0),
        ]))
    }
    fn usize(&mut self) -> Result<usize, String> {
        Ok(self.u32()? as usize)
    }
    fn text(&mut self) -> Result<&'a str, String> {
        let n = self.usize()?;
        std::str::from_utf8(self.take(n)?).map_err(|e| format!("stencil library: {e}"))
    }
    fn str(&mut self) -> Result<String, String> {
        self.text().map(String::from)
    }
    fn pairs(&mut self) -> Result<Vec<(u32, u32)>, String> {
        let n = self.usize()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let a = self.u32()?;
            out.push((a, self.u32()?));
        }
        Ok(out)
    }
    fn u32s(&mut self) -> Result<Vec<u32>, String> {
        let n = self.usize()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.u32()?);
        }
        Ok(out)
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

fn put_pairs(out: &mut Vec<u8>, v: &[(u32, u32)]) {
    put_u32(out, v.len() as u32);
    for (a, b) in v {
        put_u32(out, *a);
        put_u32(out, *b);
    }
}

fn put_u32s(out: &mut Vec<u8>, v: &[u32]) {
    put_u32(out, v.len() as u32);
    for x in v {
        put_u32(out, *x);
    }
}

/// One stencil's record, the inverse of what [`Library::encode`] writes for it.
fn put_record(out: &mut Vec<u8>, s: &Stencil) {
    put_str(out, &s.name);
    put_u32(out, s.code.len() as u32);
    put_u32(out, s.tail.map_or(u32::MAX, |t| t as u32));
    put_u32(out, s.holes.len() as u32);
    for h in &s.holes {
        put_str(out, &h.name.to_string());
        put_u32(
            out,
            match h.kind {
                HoleKind::Imm32 => 0,
                HoleKind::Imm64 => 1,
                HoleKind::Branch => 2,
            },
        );
        put_pairs(out, &h.pairs);
        put_u32s(out, &h.branches);
        put_u32s(out, &h.conds);
        put_pairs(out, &h.lo12);
    }
    put_u32(out, s.consts.len() as u32);
    out.extend_from_slice(&s.consts);
    put_u32(out, s.consts_align);
    put_u32(out, s.const_refs.len() as u32);
    for c in &s.const_refs {
        put_u32(out, c.field);
        put_u32(out, c.insn_end);
        put_u32(out, c.at);
    }
}

/// The stencil whose record starts at `record` and whose code starts at `code`.
fn read_stencil(bytes: &[u8], record: u32, code: u32) -> Result<Stencil, String> {
    let mut c = Cursor {
        bytes,
        at: record as usize,
    };
    let name = c.str()?;
    let len = c.usize()?;
    let tail = c.u32()?;
    let nh = c.usize()?;
    let mut holes = Vec::with_capacity(nh);
    for _ in 0..nh {
        let hname = HoleName::of(&c.str()?);
        let kind = match c.u32()? {
            0 => HoleKind::Imm32,
            1 => HoleKind::Imm64,
            _ => HoleKind::Branch,
        };
        holes.push(Hole {
            name: hname,
            kind,
            sites: Vec::new(),
            pairs: c.pairs()?,
            branches: c.u32s()?,
            conds: c.u32s()?,
            lo12: c.pairs()?,
        });
    }
    let nc = c.usize()?;
    let consts = c.take(nc)?.to_vec();
    let consts_align = c.u32()?;
    let nr = c.usize()?;
    let mut const_refs = Vec::with_capacity(nr);
    for _ in 0..nr {
        const_refs.push(ConstRef {
            field: c.u32()?,
            insn_end: c.u32()?,
            at: c.u32()?,
        });
    }
    let code = Cursor {
        bytes,
        at: code as usize,
    }
    .take(len)?
    .to_vec();
    Ok(Stencil {
        name,
        code,
        holes,
        consts,
        consts_align,
        const_refs,
        tail: (tail != u32::MAX).then_some(tail as usize),
    })
}

impl Builder {
    /// The bytes `cli/build.rs` writes.
    ///
    /// The index is not written: it is `(key, id)` for every stencil in order,
    /// so rebuilding it on decode is a walk of a list that has just been read.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_str(&mut out, &self.config);
        put_u32(&mut out, self.stencils.len() as u32);
        // Keys in the order the stencils are in, which is the order the
        // generators produced them: deterministic, and no map is iterated.
        let mut keys: Vec<(&str, u32)> = self.index.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        keys.sort_unstable_by_key(|(_, v)| *v);
        for (key, _) in &keys {
            put_str(&mut out, key);
        }
        let mut records = Vec::new();
        let mut starts = Vec::with_capacity(self.stencils.len());
        let mut code_at = 0usize;
        for s in &self.stencils {
            starts.push((records.len(), code_at));
            put_record(&mut records, s);
            code_at = code_at.saturating_add(s.code.len());
        }
        let table_end = out
            .len()
            .saturating_add(self.stencils.len().saturating_mul(8));
        let code_base = table_end.saturating_add(records.len());
        for (record, code) in starts {
            put_u32(&mut out, table_end.saturating_add(record) as u32);
            put_u32(&mut out, code_base.saturating_add(code) as u32);
        }
        out.extend_from_slice(&records);
        for s in &self.stencils {
            out.extend_from_slice(&s.code);
        }
        out
    }
}

impl Library {
    /// The inverse, over `include_bytes!`'s slice: the keys and the table, and
    /// each stencil when it is first asked for ([`Library::stencil`]).
    pub fn decode(bytes: &'static [u8]) -> Result<Library, String> {
        let mut c = Cursor { bytes, at: 0 };
        let config = c.str()?;
        let n = c.usize()?;
        let mut index: HashMap<&'static str, u32> =
            HashMap::with_capacity_and_hasher(n, Default::default());
        for i in 0..n {
            index.insert(c.text()?, i as u32);
        }
        let mut starts = Vec::with_capacity(n);
        for _ in 0..n {
            let record = c.u32()?;
            starts.push((record, c.u32()?));
        }
        Ok(Library {
            config,
            known: Known::find(&index)?,
            bytes,
            index,
            starts,
            stencils: (0..n).map(|_| std::sync::OnceLock::new()).collect(),
            twins: (0..n).map(|_| std::sync::OnceLock::new()).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Builder {
        let s = Stencil {
            name: String::from("bin/add/i64/ff/f"),
            code: vec![1, 2, 3, 4, 5, 6, 7, 8],
            holes: vec![Hole {
                name: HoleName::of("JIT_A"),
                kind: HoleKind::Imm32,
                sites: vec![Site {
                    off: 0,
                    kind: SiteKind::Adrp,
                }],
                pairs: vec![(0, 4)],
                branches: vec![8],
                conds: vec![],
                lo12: vec![(4, 8)],
            }],
            consts: vec![9, 9, 9, 9, 9, 9, 9, 9],
            consts_align: 16,
            const_refs: vec![ConstRef {
                field: 2,
                insn_end: 6,
                at: 0,
            }],
            tail: Some(0),
        };
        let mut lib = Builder::new(String::from("r3"));
        lib.push(s.name.clone(), s.clone());
        // The stencils every library has, after the one the tests read.
        for key in Known::KEYS {
            let mut known = s.clone();
            known.name = key.to_string();
            lib.push(known.name.clone(), known);
        }
        lib
    }

    fn read(lib: &Builder) -> Library {
        Library::decode(lib.encode().leak()).unwrap()
    }

    /// [`FOLD_PLAIN`] names the plain `+fold` slot, which is the one
    /// `Jit::elidable_at` asks for. An index into a constant array is the kind
    /// of thing that silently means something else after the array is reordered.
    #[test]
    fn the_plain_fold_slot_is_the_plain_fold_suffix() {
        assert_eq!(FOLD_SUFFIXES.get(FOLD_PLAIN), Some(&"+fold"));
        assert_eq!(FOLD_SUFFIXES.len(), FOLD_SUFFIXES_LEN);
    }

    /// A library whose stencil has no twin answers `None` in all three slots,
    /// and one that has them answers each in its own slot.
    #[test]
    fn fold_twins_are_resolved_into_their_own_slots() {
        let lib = read(&sample());
        assert_eq!(lib.at("bin/add/i64/ff/f").map(|(i, _)| i), Some(0));
        for k in 0..FOLD_SUFFIXES_LEN {
            assert!(lib.fold_twin(0, k).is_none(), "slot {k}");
        }

        let mut with_twins = sample();
        for suffix in FOLD_SUFFIXES {
            let mut twin = with_twins
                .stencils
                .first()
                .cloned()
                .expect("the sample stencil");
            twin.name = format!("bin/add/i64/ff/f{suffix}");
            with_twins.push(twin.name.clone(), twin);
        }
        let with_twins = read(&with_twins);
        for (k, suffix) in FOLD_SUFFIXES.iter().enumerate() {
            assert_eq!(
                with_twins.fold_twin(0, k).map(|s| s.name.as_str()),
                Some(format!("bin/add/i64/ff/f{suffix}").as_str())
            );
        }
        // A suffix is part of a name, so `+ifold`'s own `+fold` twin is
        // `+ifold+fold` — which the library has, because it was just added.
        // That is the one relation between two twins, and every other slot of
        // every other twin is empty.
        for (i, st) in with_twins.iter().enumerate().skip(1) {
            for (k, suffix) in FOLD_SUFFIXES.iter().enumerate() {
                let want = with_twins.get(&format!("{}{suffix}", st.name)).is_some();
                assert_eq!(with_twins.fold_twin(i, k).is_some(), want, "{i} {k}");
            }
        }
        // Past the end is `None` rather than a panic.
        assert!(with_twins.fold_twin(999, 0).is_none());
    }

    /// The two halves are compiled from this file, so the property that has to
    /// hold is that they are inverses — not that the bytes have any particular
    /// shape.
    #[test]
    fn a_library_survives_a_round_trip() {
        let back = read(&sample());
        assert_eq!(back.config, "r3");
        assert_eq!(back.len(), 1 + Known::KEYS.len());
        let s = back.get("bin/add/i64/ff/f").unwrap();
        assert_eq!(s.code, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(s.tail, Some(0));
        let h = s.holes.first().unwrap();
        assert_eq!(h.name, HoleName::of("JIT_A"));
        assert_eq!(h.kind, HoleKind::Imm32);
        assert_eq!(h.pairs, vec![(0, 4)]);
        assert_eq!(h.branches, vec![8]);
        assert_eq!(h.lo12, vec![(4, 8)]);
        // The spilled constants travel with the stencil, because there is
        // nothing else that could supply them at emission time.
        assert_eq!(s.consts, vec![9, 9, 9, 9, 9, 9, 9, 9]);
        assert_eq!(s.consts_align, 16);
        assert_eq!(s.const_refs.len(), 1);
        assert_eq!(
            s.const_refs.first().map(|c| (c.field, c.insn_end, c.at)),
            Some((2, 6, 0))
        );
        // The relocation records are the build script's working notes and are
        // deliberately not carried.
        assert!(h.sites.is_empty());
    }

    /// A library without a stencil the emitter writes unasked is refused, by
    /// the build and by the decode, and one with them all answers each key's
    /// own index.
    #[test]
    fn a_library_has_every_known_stencil() {
        let lib = sample();
        assert_eq!(lib.missing_known(), None);
        let back = read(&lib);
        let k = back.known;
        let found: [usize; Known::KEYS.len()] = [
            k.br_f,
            k.br_r0,
            k.br_r1,
            k.br_r2,
            k.eq_u64_fi,
            k.lt_u64_ff,
            k.inc_lt,
            k.tag_eq8,
            k.tag_eq32,
            k.tag_eq,
        ];
        for (key, at) in Known::KEYS.iter().zip(found) {
            assert_eq!(back.at(key).map(|(i, _)| i), Some(at), "{key}");
        }

        let mut short = Builder::new(String::from("r3"));
        for key in Known::KEYS.iter().filter(|k| **k != "incbr/lt") {
            let mut s = Stencil::unspilled();
            s.name = key.to_string();
            short.push(s.name.clone(), s);
        }
        assert_eq!(short.missing_known(), Some("incbr/lt"));
        let refused = Library::decode(short.encode().leak()).err();
        assert_eq!(
            refused.as_deref(),
            Some("the library has no `incbr/lt` stencil")
        );
    }

    /// Bytes that end before the table does are refused, never read past.
    #[test]
    fn truncated_bytes_are_refused() {
        assert!(Library::decode(b"not a library at all").is_err());
        assert!(Library::decode(&[]).is_err());
    }

    /// Cache keys are computed from these bytes, so two encodes of one library
    /// must not differ — and the index is a `HashMap`, whose iteration order
    /// does not repeat between runs.
    #[test]
    fn encoding_is_deterministic() {
        let lib = sample();
        assert_eq!(lib.encode(), lib.encode());
        assert_eq!(sample().encode(), sample().encode());
    }
}
