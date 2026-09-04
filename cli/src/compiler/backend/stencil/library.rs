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

use std::collections::HashMap;

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
    pub name: String,
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

// ---------------------------------------------------------------------------
// Names a compiler invented
// ---------------------------------------------------------------------------
//
// A stencil is one contiguous run of bytes copied out of the code section and
// patched, and every hole in it names a symbol the *generated C* declared:
// `_JIT_A`, a `buri_rt_*` entry point, a compiler-rt helper such as `__divti3`.
// A compiler that splits a function in two breaks both halves of that sentence
// at once — the body copied out is no longer the whole function, and the branch
// left behind names a symbol the stencil library will never carry.
//
// Both Linux readers already refuse the *visible* form of the split, because
// ELF gives the outlined half its own section: `extract_elf_arm64` refuses any
// section beside `.text`, and `extract_elf_x86` refuses an executable one (it
// has to allow the others, which are where a spilled SSE constant lives).
// Mach-O has no section prefixes, so on `macos-arm64` the outlined half lands
// *inside* `__text` and there is no section to see — which is how clang's
// hot/cold splitter put a `BRANCH26` hole named `st_decref_drop.cold.1` into
// the shipped host library with no error at all. The rule below is the
// container-independent half of the same guard, and it is what makes the three
// targets refuse the same object.

/// The families of name a compiler synthesises for a piece it carved out of a
/// function, and what to call each one in a refusal.
///
/// The rule [`outlining_artifact`] actually enforces is **structural**, not a
/// lookup in this table: a name the source declared is a C identifier, and
/// every segment below is reached across a `.` for the precise reason that a C
/// identifier cannot contain one — a compiler picks a spelling the source could
/// not have written so that its own names can never collide. An unrecognised
/// synthetic name is therefore refused too, and this table only decides how the
/// refusal reads.
///
/// Enumerated from what the toolchains that build this library emit, rather
/// than guessed at:
///
/// | segment | what emits it |
/// |---|---|
/// | `cold` | LLVM's `HotColdSplitting`, which names the outlined half `<fn>.cold.<n>`. Apple's clang runs it at `-O2` and upstream's does not, which is the G2 case; GCC's `-freorder-blocks-and-partition` spells its cold half the same way |
/// | `unlikely` | the cold-section spelling — LLVM's machine-function splitter and GCC both name the section `.text.unlikely.`, and a renamed symbol carries the same word |
/// | `part` | GCC's partial inlining, `<fn>.part.<n>` |
/// | `isra` | GCC's interprocedural scalar replacement of aggregates |
/// | `constprop` | GCC's interprocedural constant-propagation clone |
/// | `specialized` | LLVM's `FunctionSpecialization` |
/// | `resume`, `destroy`, `cleanup` | the three funclets `CoroSplit` makes of a coroutine |
/// | `llvm` | LLVM's rename of an internalised or promoted symbol, `<fn>.llvm.<hash>` |
/// | `lto_priv` | GCC's LTO privatisation |
///
/// `OUTLINED_FUNCTION_<n>` — the MachineOutliner's, on by default at `-Oz` for
/// AArch64 — is the one that *is* a valid C identifier, so it is named
/// separately in [`outlining_artifact`] rather than here.
const ARTIFACT_SEGMENTS: &[(&str, &str)] = &[
    ("cold", "a hot/cold split of it"),
    ("unlikely", "a cold-section split of it"),
    ("part", "a partial inline of it"),
    ("isra", "an argument-shape clone of it"),
    ("constprop", "a constant-propagation clone of it"),
    ("specialized", "a specialisation of it"),
    ("resume", "a coroutine funclet of it"),
    ("destroy", "a coroutine funclet of it"),
    ("cleanup", "a coroutine funclet of it"),
    ("llvm", "an internalising rename of it"),
    ("lto_priv", "an LTO privatisation of it"),
];

/// Whether `name` is something the generated C could have written down.
///
/// A C identifier, in the only sense that matters here: ASCII letters, digits
/// and underscores, not starting with a digit. Every name this library binds a
/// hole to is one — the `_JIT_*` holes, the `buri_rt_*` entry points, the
/// handful of compiler-rt helpers — and no name a compiler synthesises for
/// itself is.
fn is_c_identifier(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What `name` is, when it is a name a compiler invented rather than one the
/// source declared — and `None` when the source could have declared it.
///
/// Two names are deliberately *not* artifacts:
///
///  * the **empty** name, which is an ELF section symbol. A relocation against
///    one is how a spilled constant is reached on x86-64, and `x86.rs` handles
///    it as a constant rather than as a hole.
///  * a name beginning `.L`, an assembler-local label. Those are the labels on
///    the spilled constants themselves — `elfobj.rs` keeps them in the table
///    for exactly that reason — and they are data, never code.
pub fn outlining_artifact(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.starts_with(".L") {
        return None;
    }
    if name.starts_with("OUTLINED_FUNCTION_") {
        return Some("a machine-outliner fragment");
    }
    if is_c_identifier(name) {
        return None;
    }
    // The first segment is the function the compiler carved this out of, and
    // the *suffix* segments are what say what the carving was. An unrecognised
    // one is still refused: the `.` alone is proof the name was not written in
    // the source.
    let known = |seg: &str| ARTIFACT_SEGMENTS.iter().find(|(s, _)| *s == seg);
    Some(
        name.split('.')
            .skip(1)
            .find_map(|seg| known(seg).map(|(_, w)| *w))
            .unwrap_or("a name the compiler invented"),
    )
}

/// The refusal a hole whose target the compiler invented earns, or `None` when
/// the name is one the source declared.
///
/// Shared by both finishers — `extract.rs`'s `finish_arm64` for the two AArch64
/// containers and `x86.rs`'s `one` — so that the three targets refuse the same
/// object in the same words. One sentence in one place is the point: the wound
/// this guards against was one target erroring on the section a split produced
/// while another accepted the very same split in silence.
pub fn refuse_hole_name(stencil: &str, hole: &str) -> Option<String> {
    outlining_artifact(hole).map(|what| {
        format!(
            "{stencil}: a stencil reaches {hole}, which is {what}; every hole must be a \
             symbol the source declared"
        )
    })
}

/// The same refusal for a stencil's *own* name.
///
/// The outlined half is a function too, and on Mach-O it is a function in
/// `__text` whose name begins `_st_`, so it walks straight into the extractor's
/// `st_*` filter and becomes a stencil of its own. That is the branch of the
/// wound that survives a compiler which resolved the branch without leaving a
/// relocation behind: no hole to refuse, and a key in the library that is half
/// of a function.
pub fn refuse_stencil_name(stencil: &str) -> Option<String> {
    outlining_artifact(stencil).map(|what| {
        format!(
            "{stencil} is not a stencil, it is {what}; every stencil must be a function \
             the source declared"
        )
    })
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

    pub fn find(&self, name: &str) -> Option<usize> {
        self.holes.iter().position(|h| h.name == name)
    }
}

/// The folded twins of a stencil, most specific first.
///
/// A twin is the same operation with an offset or a literal folded into an
/// `imm12` field, so it exists only where the fold is representable; the two
/// folds are independent, which is why there are three names and not two.
/// [`Jit::emit`](crate::compiler::backend::stencil::jit::Jit::emit) tries them
/// in this order and takes the first whose fields the operands fit.
pub const FOLD_SUFFIXES: [&str; FOLD_SUFFIXES_LEN] = ["+ifold+fold", "+fold", "+ifold"];
pub const FOLD_SUFFIXES_LEN: usize = 3;

/// The slot of [`FOLD_SUFFIXES`] holding the plain `+fold` twin.
pub const FOLD_PLAIN: usize = 1;

/// The slot of [`Library::fold_twins`] that names no stencil.
const NO_TWIN: u32 = u32::MAX;

#[derive(Default)]
pub struct Library {
    pub stencils: Vec<Stencil>,
    pub index: HashMap<String, u32>,
    /// Wall time clang spent, milliseconds, when this library was built.
    pub build_ms: f64,
    pub config: String,
    /// Each stencil's [`FOLD_SUFFIXES`] twins, by index into `stencils`,
    /// resolved on first use and never again.
    ///
    /// The emitter asks for all three on **every stencil it copies**, and it
    /// used to ask by building three `String`s with `format!` and hashing each
    /// one — three allocations and three hash lookups per machine instruction
    /// this backend emits. The names are a function of the library alone, so
    /// the answer is too, and computing it once turns the question into three
    /// array reads.
    ///
    /// A `OnceLock` rather than a field every constructor fills, because the
    /// library is decoded once per process behind a `OnceLock` of its own and
    /// is then shared by every codegen thread: a value that is derived rather
    /// than stored cannot be forgotten by a caller that builds a `Library` some
    /// other way.
    pub twins: std::sync::OnceLock<Vec<[u32; FOLD_SUFFIXES_LEN]>>,
}

impl Library {
    pub fn get(&self, key: &str) -> Option<&Stencil> {
        self.index.get(key).and_then(|i| self.stencils.get(*i as usize))
    }

    /// [`Library::get`], with the index the fold twins are asked by.
    pub fn at(&self, key: &str) -> Option<(usize, &Stencil)> {
        let i = *self.index.get(key)? as usize;
        Some((i, self.stencils.get(i)?))
    }

    /// The `k`th [`FOLD_SUFFIXES`] twin of the stencil at `i`, if the library
    /// has one.
    pub fn fold_twin(&self, i: usize, k: usize) -> Option<&Stencil> {
        let j = *self.fold_twins().get(i)?.get(k)?;
        if j == NO_TWIN {
            return None;
        }
        self.stencils.get(j as usize)
    }

    fn fold_twins(&self) -> &[[u32; FOLD_SUFFIXES_LEN]] {
        self.twins.get_or_init(|| {
            let mut out = vec![[NO_TWIN; FOLD_SUFFIXES_LEN]; self.stencils.len()];
            let mut name = String::new();
            for (i, s) in self.stencils.iter().enumerate() {
                for (k, suffix) in FOLD_SUFFIXES.iter().enumerate() {
                    name.clear();
                    name.push_str(&s.name);
                    name.push_str(suffix);
                    if let Some(j) = self.index.get(name.as_str()) {
                        if let Some(slot) = out.get_mut(i).and_then(|row| row.get_mut(k)) {
                            *slot = *j;
                        }
                    }
                }
            }
            out
        })
    }

    pub fn bytes(&self) -> usize {
        self.stencils.iter().map(|s| s.code.len()).sum()
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
// meets a library it did not build. What it does carry is [`Library::config`],
// which enters `Backend::identity` so that two toolchains whose *stencils*
// differ do not share cached objects.
//
// Everything is little-endian and length-prefixed. The one shape decision is
// that the code bytes of every stencil are concatenated into a single run at
// the end: the decoder copies that run once instead of half a million times.

/// The magic and the layout revision, checked on decode so that a stale
/// `OUT_DIR` is a build error rather than a wrong instruction stream.
const MAGIC: [u8; 8] = *b"STENCIL2";

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(n).ok_or("stencil library: length overflow")?;
        let out = self.bytes.get(self.at..end).ok_or("stencil library: truncated")?;
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
    fn str(&mut self) -> Result<String, String> {
        let n = self.usize()?;
        let b = self.take(n)?;
        String::from_utf8(b.to_vec()).map_err(|e| format!("stencil library: {e}"))
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

impl Library {
    /// The bytes `cli/build.rs` writes.
    ///
    /// The index is not written: it is `(key, id)` for every stencil in order,
    /// so rebuilding it on decode is a walk of a list that has just been read.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        put_str(&mut out, &self.config);
        put_u32(&mut out, self.stencils.len() as u32);
        // Keys in the order the stencils are in, which is the order the
        // generators produced them: deterministic, and no map is iterated.
        let mut keys: Vec<(&str, u32)> = self.index.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        keys.sort_unstable_by_key(|(_, v)| *v);
        for (key, _) in &keys {
            put_str(&mut out, key);
        }
        for s in &self.stencils {
            put_str(&mut out, &s.name);
            put_u32(&mut out, s.code.len() as u32);
            put_u32(&mut out, s.tail.map_or(u32::MAX, |t| t as u32));
            put_u32(&mut out, s.holes.len() as u32);
            for h in &s.holes {
                put_str(&mut out, &h.name);
                put_u32(
                    &mut out,
                    match h.kind {
                        HoleKind::Imm32 => 0,
                        HoleKind::Imm64 => 1,
                        HoleKind::Branch => 2,
                    },
                );
                put_pairs(&mut out, &h.pairs);
                put_u32s(&mut out, &h.branches);
                put_u32s(&mut out, &h.conds);
                put_pairs(&mut out, &h.lo12);
            }
            put_u32(&mut out, s.consts.len() as u32);
            out.extend_from_slice(&s.consts);
            put_u32(&mut out, s.consts_align);
            put_u32(&mut out, s.const_refs.len() as u32);
            for c in &s.const_refs {
                put_u32(&mut out, c.field);
                put_u32(&mut out, c.insn_end);
                put_u32(&mut out, c.at);
            }
        }
        for s in &self.stencils {
            out.extend_from_slice(&s.code);
        }
        out
    }

    /// The inverse, over `include_bytes!`'s slice.
    pub fn decode(bytes: &[u8]) -> Result<Library, String> {
        let mut c = Cursor { bytes, at: 0 };
        if c.take(MAGIC.len())? != MAGIC {
            return Err(String::from(
                "the embedded stencil library is not one this toolchain wrote",
            ));
        }
        let config = c.str()?;
        let n = c.usize()?;
        let mut index: HashMap<String, u32> = HashMap::with_capacity(n);
        for i in 0..n {
            index.insert(c.str()?, i as u32);
        }
        let mut stencils = Vec::with_capacity(n);
        let mut lengths = Vec::with_capacity(n);
        for _ in 0..n {
            let name = c.str()?;
            let len = c.usize()?;
            let tail = c.u32()?;
            let nh = c.usize()?;
            let mut holes = Vec::with_capacity(nh);
            for _ in 0..nh {
                let hname = c.str()?;
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
                const_refs.push(ConstRef { field: c.u32()?, insn_end: c.u32()?, at: c.u32()? });
            }
            lengths.push(len);
            stencils.push(Stencil {
                name,
                code: Vec::new(),
                holes,
                consts,
                consts_align,
                const_refs,
                tail: (tail != u32::MAX).then_some(tail as usize),
            });
        }
        for (s, len) in stencils.iter_mut().zip(lengths) {
            s.code = c.take(len)?.to_vec();
        }
        Ok(Library { stencils, index, build_ms: 0.0, config, ..Library::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Library {
        let s = Stencil {
            name: String::from("bin/add/i64/ff/f"),
            code: vec![1, 2, 3, 4, 5, 6, 7, 8],
            holes: vec![Hole {
                name: String::from("_JIT_A"),
                kind: HoleKind::Imm32,
                sites: vec![Site { off: 0, kind: SiteKind::Adrp }],
                pairs: vec![(0, 4)],
                branches: vec![8],
                conds: vec![],
                lo12: vec![(4, 8)],
            }],
            consts: vec![9, 9, 9, 9, 9, 9, 9, 9],
            consts_align: 16,
            const_refs: vec![ConstRef { field: 2, insn_end: 6, at: 0 }],
            tail: Some(0),
        };
        let mut index = HashMap::new();
        index.insert(s.name.clone(), 0);
        Library {
            stencils: vec![s],
            index,
            build_ms: 1.0,
            config: String::from("r3"),
            ..Library::default()
        }
    }

    /// [`FOLD_PLAIN`] names the plain `+fold` slot, which is the one
    /// `Jit::elidable_arm` asks for. An index into a constant array is the kind
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
        let lib = sample();
        assert_eq!(lib.at("bin/add/i64/ff/f").map(|(i, _)| i), Some(0));
        for k in 0..FOLD_SUFFIXES_LEN {
            assert!(lib.fold_twin(0, k).is_none(), "slot {k}");
        }

        let mut with_twins = sample();
        for (k, suffix) in FOLD_SUFFIXES.iter().enumerate() {
            let mut twin = with_twins.stencils.first().cloned().expect("the sample stencil");
            twin.name = format!("bin/add/i64/ff/f{suffix}");
            let at = with_twins.stencils.len() as u32;
            with_twins.index.insert(twin.name.clone(), at);
            with_twins.stencils.push(twin);
            let _ = k;
        }
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
        for (i, st) in with_twins.stencils.iter().enumerate().skip(1) {
            for (k, suffix) in FOLD_SUFFIXES.iter().enumerate() {
                let want = with_twins.index.contains_key(&format!("{}{suffix}", st.name));
                assert_eq!(with_twins.fold_twin(i, k).is_some(), want, "{i} {k}");
            }
        }
        // Past the end is `None` rather than a panic.
        assert!(with_twins.fold_twin(999, 0).is_none());
        assert!(with_twins.fold_twin(0, FOLD_SUFFIXES_LEN).is_none());
    }

    /// The two halves are compiled from this file, so the property that has to
    /// hold is that they are inverses — not that the bytes have any particular
    /// shape.
    #[test]
    fn a_library_survives_a_round_trip() {
        let lib = sample();
        let back = Library::decode(&lib.encode()).unwrap();
        assert_eq!(back.config, "r3");
        assert_eq!(back.stencils.len(), 1);
        let s = back.get("bin/add/i64/ff/f").unwrap();
        assert_eq!(s.code, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(s.tail, Some(0));
        let h = s.holes.first().unwrap();
        assert_eq!(h.name, "_JIT_A");
        assert_eq!(h.kind, HoleKind::Imm32);
        assert_eq!(h.pairs, vec![(0, 4)]);
        assert_eq!(h.branches, vec![8]);
        assert_eq!(h.lo12, vec![(4, 8)]);
        // The spilled constants travel with the stencil, because there is
        // nothing else that could supply them at emission time.
        assert_eq!(s.consts, vec![9, 9, 9, 9, 9, 9, 9, 9]);
        assert_eq!(s.consts_align, 16);
        assert_eq!(s.const_refs.len(), 1);
        assert_eq!(s.const_refs.first().map(|c| (c.field, c.insn_end, c.at)), Some((2, 6, 0)));
        // The relocation records are the build script's working notes and are
        // deliberately not carried.
        assert!(h.sites.is_empty());
    }

    /// A stale `OUT_DIR` must be a build error and never a wrong instruction
    /// stream, which is the whole reason the magic is there.
    #[test]
    fn foreign_bytes_are_refused() {
        assert!(Library::decode(b"not a library at all").is_err());
        assert!(Library::decode(&[]).is_err());
    }

    /// The name that was actually in the shipped `macos-arm64` library, and the
    /// one the ELF targets errored on. A synthetic hole rather than a compiled
    /// fixture: inducing the split needs Apple's clang specifically — upstream
    /// clang 21 does not run `HotColdSplitting` at `-O2` at all — so a fixture
    /// would compile to nothing on the machines that do not reproduce it and
    /// would assert nothing there. What the extractor sees of that split is
    /// exactly this string, and this is the check it now fails.
    #[test]
    fn the_hot_cold_split_that_shipped_is_refused() {
        let e = refuse_hole_name("st_decref_drop", "st_decref_drop.cold.1")
            .expect("a hole named after a hot/cold split must be refused");
        assert!(e.contains("st_decref_drop.cold.1"), "{e}");
        assert!(e.contains("a hot/cold split of it"), "{e}");
        let rule = "every hole must be a symbol the source declared";
        assert!(e.contains(rule), "{e}");
        // And the same function reached as a stencil of its own, which is the
        // shape a compiler that left no relocation behind would produce.
        let e = refuse_stencil_name("st_decref_drop.cold.1")
            .expect("a stencil that is half of one must be refused");
        let rule = "every stencil must be a function the source declared";
        assert!(e.contains(rule), "{e}");
    }

    /// One assertion per family the table names, plus the two spellings that
    /// carry no `.` — the outliner's, and an ordinary name, which must pass.
    #[test]
    fn every_outlining_family_is_named_rather_than_lumped_together() {
        let cases = [
            ("st_x.cold", "a hot/cold split of it"),
            ("st_x.cold.3", "a hot/cold split of it"),
            ("st_x.unlikely.0", "a cold-section split of it"),
            ("st_x.part.0", "a partial inline of it"),
            ("st_x.isra.0", "an argument-shape clone of it"),
            ("st_x.constprop.0", "a constant-propagation clone of it"),
            ("st_x.specialized.1", "a specialisation of it"),
            ("st_x.resume", "a coroutine funclet of it"),
            ("st_x.destroy", "a coroutine funclet of it"),
            ("st_x.cleanup", "a coroutine funclet of it"),
            ("st_x.llvm.98765", "an internalising rename of it"),
            ("st_x.lto_priv.0", "an LTO privatisation of it"),
            ("OUTLINED_FUNCTION_0", "a machine-outliner fragment"),
            // Not in the table, and still refused: the `.` is the proof.
            ("st_x.whatever_comes_next.2", "a name the compiler invented"),
        ];
        for (name, what) in cases {
            assert_eq!(outlining_artifact(name), Some(what), "{name}");
        }
    }

    /// The other half of the rule, which is the half that would break the
    /// build if it were wrong: every name a hole is legitimately bound to must
    /// pass. The four kinds are the emitter's holes, the runtime entry points,
    /// compiler-rt's helpers, and the two ELF spellings `x86.rs` relocates
    /// against for a spilled constant.
    #[test]
    fn the_names_holes_are_really_bound_to_all_pass() {
        for name in [
            "JIT_A",
            "JIT_CONT",
            "JIT_CONT0",
            "_JIT_A",
            "buri_rt_alloc",
            "buri_rt_i128_divmod",
            "buri_rt_test_fail_compared",
            "memcpy",
            "__divti3",
            "__udivti3",
            // An ELF section symbol, and an assembler-local label on a spilled
            // constant pool: both reach `x86.rs`'s spilled path and neither is
            // an outlined anything.
            "",
            ".LCPI0_0",
            ".Lswitch.table.st_x",
        ] {
            assert_eq!(outlining_artifact(name), None, "{name}");
            assert!(refuse_hole_name("st_x", name).is_none(), "{name}");
        }
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
