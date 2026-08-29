//! Turning what clang emitted for **x86-64** into stencils.
//!
//! `extract.rs`'s sibling, and much the shorter of the two, because this is the
//! paper's home instruction set and almost nothing has to be invented here.
//! Paper §5.3 says the builder "extracts their binary code and the linker
//! relocation records containing information about the holes"; on x86-64 that
//! sentence is the whole of the file, where on AArch64 it was the first quarter
//! and four instruction-rewriting folds were the rest.
//!
//! # Why there are no folds here
//!
//! `extract.rs`'s header table lists four rewrites and says why each exists:
//! AArch64 has no 32-bit immediate field, so clang *cannot* be asked for the
//! operand shapes the emitter wants and the folds recover them by rewriting
//! instructions. Three of the four are answered on x86-64 by the instruction
//! set itself, at zero cost and with no rewriting at all:
//!
//! | AArch64 fold | what x86-64 does instead |
//! |---|---|
//! | `fold_addressing` — a frame offset in a load's own `imm12` | the frame offset is a `disp32` in the load's own `ModRM`, and clang emits `(%rdi,%rax)` only because the offset is an *unknown* symbol; the hole's `lea` is the one instruction the fold would have deleted |
//! | `fold_imm` — a literal in an `add`'s `imm12` | an x86-64 ALU op takes a full `imm32` and clang uses it wherever the value is known; where it is a hole, the `mov` a patched `lea` becomes is already one instruction |
//! | `fold_cond` — a two-way branch in two instructions | `jcc rel32` + `jmp rel32` is what clang emits for the shape, and the third branch AArch64 needed is not there |
//! | `swap_arms` — the folded twin with the arms exchanged | the twin of a fold that does not exist |
//!
//! What is left is a real difference and it is recorded rather than papered
//! over: on this target a stencil's body is exactly the bytes clang emitted,
//! and the *last* of those four — picking whichever arm falls through — is
//! therefore not available. `design/native/CODEGEN-STENCIL.md` §"the x86-64
//! checklist" says what measuring it would take.
//!
//! The deeper reason to stop here, stated plainly because it is a judgement and
//! not a fact: every AArch64 fold was measured on a machine that could *run*
//! the patched code, and this port is being written on one that cannot run a
//! single x86-64 instruction. An instruction-motion pass whose only evidence is
//! a disassembly listing is not something to put under a code generator.
//!
//! Like `extract.rs` and `elfobj.rs`, this file runs once in `cli/build.rs` and
//! is not compiled into the toolchain.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "every quantity here is a byte position inside one stencil body \
              that is already in memory and at most a few hundred bytes long. \
              The three subtractions step back from a `disp32` field to the \
              ModRM, the opcode and the REX prefix that must precede it — \
              three, two and one — and each is guarded by the `off >= 3` test \
              that precedes it, so none can wrap. Every result is offered to \
              `.get` rather than trusted. The additions are over one stencil's \
              spilled constants, whose length is a section already read out of \
              the same object and whose addend was range-checked before any \
              of them"
)]

use super::elfobj as elf;
use super::library::{ConstRef, Hole, HoleKind, Site, SiteKind, Stencil};

/// `jmp rel32`.
const JMP_REL32: u8 = 0xe9;
/// `call rel32`.
const CALL_REL32: u8 = 0xe8;
/// `lea r64, m`.
const LEA: u8 = 0x8d;

/// What a pc-relative x86-64 addend means, and the range it may take.
///
/// The psABI computes these kinds as `S + A - P`, where `P` is the address of
/// the four-byte field. The processor computes a rip-relative displacement from
/// the address of the **next instruction**. So the addend is exactly the
/// negated distance from the start of the field to the end of the instruction,
/// and clang writes it for that reason and no other.
///
/// For a `jmp`, a `call`, a `jcc` and a `lea` the field is the last thing in
/// the instruction and the addend is `-4`. For a memory operand with a trailing
/// immediate it is longer, and the difference is load-bearing rather than
/// cosmetic: `cmpl $0, sym@GOTPCREL(%rip)` is `83 3d <disp32> <imm8>` and
/// carries `-5`, so a patcher that assumed four would aim every such reference
/// one byte past its target. The distance is therefore recorded per site — see
/// [`one`]'s `pairs` — instead of being a constant anywhere.
///
/// `-8` is the widest: a `disp32` followed by an `imm32`.
const PCREL_ADDEND: i64 = -4;
const WIDEST_ADDEND: i64 = -8;

/// Whether `b` is a `REX` prefix with `W` set — a 64-bit operand size.
fn is_rex_w(b: u8) -> bool {
    b & 0xf8 == 0x48
}

/// Whether a `ModRM` byte is the rip-relative form `mod = 00, r/m = 101`, which
/// is the only addressing mode any of these relocations can name.
fn is_rip_modrm(b: u8) -> bool {
    b & 0xc7 == 0x05
}

/// One stencil this target cannot have, and the reason.
///
/// A stencil is dropped rather than the whole shard refused, because the two
/// things that can go wrong are per-key and the library is still worth having
/// without them: `sources.rs` reports the list and the emitter refuses the IR
/// shapes that needed the missing keys, which is the same "refuse with a
/// sentence" path an unimplemented intrinsic already takes.
pub struct Dropped {
    pub name: String,
    pub why: String,
}

/// Turns one clang-compiled ELF/x86-64 object into stencils.
///
/// The dropped list is returned beside them rather than logged, so that the
/// build script can name every missing key once instead of the extractor
/// naming each one where it cannot say what it was for.
pub fn extract_elf_x86(obj: &elf::Obj) -> Result<(Vec<Stencil>, Vec<Dropped>), String> {
    if obj.machine != elf::EM_X86_64 {
        return Err(format!("expected an x86-64 ELF object, got machine {}", obj.machine));
    }
    let mut out = Vec::new();
    let mut dropped = Vec::new();
    for (sname, start, end) in elf::functions(obj) {
        if !sname.starts_with("st_") {
            continue;
        }
        let Some(body) = obj.text.get(start..end) else {
            return Err(format!("{sname}: {start:#x}..{end:#x} is outside .text"));
        };
        let code = body.to_vec();
        match one(&sname, code, obj, start, end) {
            Ok(st) => out.push(st),
            Err(Refusal::Drop(why)) => dropped.push(Dropped { name: sname, why }),
            Err(Refusal::Fatal(e)) => return Err(e),
        }
    }
    Ok((out, dropped))
}

/// The read-only bytes one stencil reads, as they are collected.
///
/// A section is copied **whole** rather than sliced at the symbol: the size of
/// a datum in a constant pool is not something its symbol records, and the
/// sections clang produces here are a few dozen bytes.
#[derive(Default)]
struct Spilled {
    bytes: Vec<u8>,
    align: u32,
    /// Where each section already copied begins in `bytes`, by section index.
    at: Vec<(u16, u32)>,
}

/// Copies the section a spilled reference names, and answers the datum's offset
/// within [`Spilled::bytes`].
///
/// The psABI computes a pc-relative kind as `S + A - P` and the processor adds
/// that to the address of the **next** instruction, so an addend that is the
/// negated distance from the field to that address makes the reference resolve
/// to `S` exactly. `S` is the section's base plus `st_value`, which is why the
/// offset answered here is `st_value` and the bytes are the whole section.
fn spilled(
    consts: &mut Spilled,
    sname: &str,
    obj: &elf::Obj,
    sym: &elf::Sym,
    addend: i64,
) -> Result<u32, Refusal> {
    let named = if sym.name.is_empty() { "an unnamed section constant" } else { &sym.name };
    if !(WIDEST_ADDEND..=PCREL_ADDEND).contains(&addend) {
        return Err(Refusal::Drop(format!(
            "it reaches {named} with addend {addend}, which is not a distance to an \
             instruction's end"
        )));
    }
    let Some(section) = obj.other_sections.iter().find(|o| o.index == sym.sect) else {
        return Err(Refusal::Drop(format!(
            "it reaches {named} in section {}, for which this reader carries no bytes",
            sym.sect
        )));
    };
    if section.data.is_empty() {
        return Err(Refusal::Drop(format!("it reaches {named}, in a section with no bytes")));
    }
    if let Some((_, at)) = consts.at.iter().find(|(i, _)| *i == sym.sect) {
        return Ok(at + u32::try_from(sym.value).unwrap_or(0));
    }
    // An SSE constant asks for sixteen, and reading one at less is a fault
    // rather than a slowdown, so the alignment is carried and not assumed.
    let align = u32::try_from(section.align.max(1)).unwrap_or(16);
    consts.align = consts.align.max(align);
    while !consts.bytes.len().is_multiple_of(align as usize) {
        consts.bytes.push(0);
    }
    let base = u32::try_from(consts.bytes.len())
        .map_err(|_| Refusal::Fatal(format!("{sname}: spilled constants past 4 GiB")))?;
    consts.bytes.extend_from_slice(&section.data);
    consts.at.push((sym.sect, base));
    Ok(base + u32::try_from(sym.value).unwrap_or(0))
}

/// Which of a branch hole's two lists a site belongs in.
enum Branch {
    /// `jmp`/`call`: `Hole::branches`.
    Uncond(u32),
    /// `jcc`: `Hole::conds`.
    Cond(u32),
    /// Not a branch at all.
    None,
}

/// Why one function did not become a stencil.
enum Refusal {
    /// This key is unavailable on this target; the library goes on without it.
    Drop(String),
    /// The object is not the shape this reader understands, which is a
    /// toolchain problem and not a per-key one.
    Fatal(String),
}

fn one(
    sname: &str,
    code: Vec<u8>,
    obj: &elf::Obj,
    start: usize,
    end: usize,
) -> Result<Stencil, Refusal> {
    let mut holes: Vec<Hole> = Vec::new();
    let mut consts = Spilled::default();
    let mut const_refs: Vec<ConstRef> = Vec::new();
    for r in obj.text_relocs.iter().filter(|r| (r.addr as usize) >= start && (r.addr as usize) < end)
    {
        let off = r.addr - start as u32;
        let sym = obj.syms.get(r.symbolnum as usize).ok_or_else(|| {
            Refusal::Fatal(format!("{sname}: relocation names symbol {}", r.symbolnum))
        })?;

        // A relocation against a symbol this object *defines* is not a hole: it
        // is a reference to a constant clang spilled, and there is nothing to
        // patch into it. Three families do it, and all three are cases where
        // AArch64 has an instruction and x86-64 has a constant — `fneg` against
        // an `xorps` with a sign mask, `ucvtf` against the two-bias `unsigned
        // long long` to `double` sequence, and the 128-bit divide's zero check.
        //
        // The bytes travel with the stencil and the emitter copies them into
        // the unit's own constant pool (`jit.rs::spilled_pool`), which is what
        // the linker would have done with clang's `.rodata`. Nothing is dropped
        // and no instruction is rewritten, so these keys are as faithful as
        // every other one.
        if sym.defined {
            let at = spilled(&mut consts, sname, obj, sym, r.addend)?;
            const_refs.push(ConstRef { field: off, insn_end: off + (-r.addend) as u32, at });
            continue;
        }

        if r.addend > PCREL_ADDEND || r.addend < WIDEST_ADDEND {
            return Err(Refusal::Fatal(format!(
                "{sname}+{off:#x}: relocation against {} has addend {}, which is outside \
                 {WIDEST_ADDEND}..={PCREL_ADDEND} and so is not a distance to an \
                 instruction's end",
                sym.name, r.addend
            )));
        }
        // Where the instruction ends, which is what a rip-relative
        // displacement is measured from.
        let insn_end = off + (-r.addend) as u32;
        // Every kind below names a four-byte field that ends an instruction, so
        // the field must be inside the body, and the bytes that decide how to
        // patch it must be before it. How many depends on the kind, and the
        // minimum rather than the usual case is what is required here:
        //
        //  * a `jmp`/`call`/`jcc`: one opcode byte, and a stencil whose whole
        //    body is the continuation's `jmp` — `bin/and/i64` of a register
        //    with itself — really does put its `rel32` at offset 1;
        //  * a GOTPCREL memory operand: one `ModRM` byte, because the operand
        //    size need not be 64 and `mov eax, sym@GOTPCREL(%rip)` has no
        //    `REX` at all. The *relaxation* wants three and checks for itself;
        //  * a `lea`: three, and always three — the address of a hole is a
        //    pointer, so the `lea` is `REX.W` and there is nothing shorter.
        let back = if r.kind == elf::R_X86_64_PC32 { 3 } else { 1 };
        if (off as usize) + 4 > code.len() || off < back {
            return Err(Refusal::Fatal(format!(
                "{sname}+{off:#x}: relocation field is outside the body"
            )));
        }
        let at = off as usize;
        let byte = |i: usize| code.get(i).copied().unwrap_or(0);

        let hname = sym
            .name
            .strip_prefix("_JIT_")
            .map_or_else(|| sym.name.clone(), |rest| format!("JIT_{rest}"));

        let (hk, sk, pair, branch) = match r.kind {
            // A hidden hole: `lea rD, [rip+disp32]`, seven bytes, of which the
            // last four are the field. The patcher rewrites the whole
            // instruction, so it needs its start, which is three bytes back —
            // REX, opcode, ModRM, with no SIB because `mod = 00, r/m = 101` has
            // none.
            elf::R_X86_64_PC32 => {
                if !(is_rex_w(byte(at - 3)) && byte(at - 2) == LEA && is_rip_modrm(byte(at - 1))) {
                    return Err(Refusal::Fatal(format!(
                        "{sname}+{off:#x}: PC32 site against {} is not `lea rD, [rip+disp32]` \
                         ({:02x} {:02x} {:02x})",
                        sym.name,
                        byte(at - 3),
                        byte(at - 2),
                        byte(at - 1)
                    )));
                }
                if r.addend != PCREL_ADDEND {
                    return Err(Refusal::Fatal(format!(
                        "{sname}+{off:#x}: a lea's disp32 is its last field, so its addend \
                         must be {PCREL_ADDEND} and is {}",
                        r.addend
                    )));
                }
                (HoleKind::Imm32, SiteKind::LeaPc32, Some((insn_end, off)), Branch::None)
            }
            // A continuation, or a call into the runtime archive. Three
            // shapes, and the third is the interesting one:
            //
            //  * `jmp rel32` — the ordinary tail;
            //  * `call rel32` — a runtime entry that returns;
            //  * `jcc rel32` — a **conditional** tail. Clang emits one whenever
            //    an arm of the C is nothing but a `musttail`, because on this
            //    target a conditional displacement is 32 bits and always
            //    reaches. AArch64's is 19, clang will not risk it, and
            //    `extract::fold_cond` exists to recover by hand what arrives
            //    here in the relocation record.
            elf::R_X86_64_PLT32 => {
                let op = byte(at - 1);
                if op == JMP_REL32 || op == CALL_REL32 {
                    (HoleKind::Branch, SiteKind::Rel32, None, Branch::Uncond(off))
                } else if at >= 2 && byte(at - 2) == 0x0f && (0x80..=0x8f).contains(&op) {
                    (HoleKind::Branch, SiteKind::CondRel32, None, Branch::Cond(off))
                } else {
                    return Err(Refusal::Fatal(format!(
                        "{sname}+{off:#x}: PLT32 site against {} is not jmp/call/jcc rel32 \
                         ({:02x} {op:02x})",
                        sym.name,
                        byte(at - 2)
                    )));
                }
            }
            // A default-visibility hole, read out of the GOT. The patch is to
            // retarget the `disp32` at the constant pool, which needs the
            // instruction's *end* and nothing else — and the addend already
            // says where that is. The one thing that needs more, relaxing a
            // value that fits 32 bits into an immediate, is only sound for a
            // plain `mov rD, [rip+disp32]`; the patcher recognises that shape
            // from the bytes in front of the field, exactly as the check below
            // recognises the addressing mode, so nothing extra is recorded for
            // it here. A float immediate reaches here as `movsd xmm,
            // [rip+disp32]` and a small comparison as `cmpl $0, [rip+disp32]`,
            // and neither is relaxable.
            elf::R_X86_64_GOTPCREL | elf::R_X86_64_GOTPCRELX | elf::R_X86_64_REX_GOTPCRELX => {
                if !is_rip_modrm(byte(at - 1)) {
                    return Err(Refusal::Fatal(format!(
                        "{sname}+{off:#x}: GOTPCREL site against {} is not rip-relative \
                         (ModRM {:02x})",
                        sym.name,
                        byte(at - 1)
                    )));
                }
                (HoleKind::Imm64, SiteKind::GotPc32, Some((insn_end, off)), Branch::None)
            }
            other => {
                return Err(Refusal::Fatal(format!(
                    "{sname}+{off:#x}: unhandled x86-64 relocation type {other}"
                )))
            }
        };

        let slot = match holes.iter_mut().find(|h| h.name == hname) {
            Some(h) => {
                if h.kind != hk {
                    return Err(Refusal::Fatal(format!("{sname}: hole {hname} has two kinds")));
                }
                h
            }
            None => {
                holes.push(Hole {
                    name: hname,
                    kind: hk,
                    sites: Vec::new(),
                    pairs: Vec::new(),
                    branches: Vec::new(),
                    conds: Vec::new(),
                    lo12: Vec::new(),
                });
                // Just pushed, so there is a last.
                holes.last_mut().ok_or_else(|| {
                    Refusal::Fatal(format!("{sname}: a hole vanished after being pushed"))
                })?
            }
        };
        slot.sites.push(Site { off, kind: sk });
        // `(instruction end, field)`, which is the pair a rip-relative patch
        // needs: the displacement is measured from the first and written into
        // the second. AArch64's `pairs` is `(adrp, add)` and means something
        // else entirely; the two ISAs never share a patcher, and each one's
        // meaning is stated where it is produced and where it is consumed.
        if let Some(p) = pair {
            slot.pairs.push(p);
        }
        match branch {
            Branch::Uncond(b) => slot.branches.push(b),
            Branch::Cond(b) => slot.conds.push(b),
            Branch::None => {}
        }
    }

    // Sites arrive in the object's relocation order, which is clang's; the
    // patcher does not care, but two builds of the same C must produce the same
    // library bytes, so the order is pinned here rather than trusted.
    for h in &mut holes {
        h.sites.sort_by_key(|s| s.off);
        h.pairs.sort_unstable();
        h.branches.sort_unstable();
        h.conds.sort_unstable();
    }
    holes.sort_by(|a, b| a.name.cmp(&b.name));

    // The continuation branch, when it is the last instruction: `jmp rel32` is
    // five bytes, so the opcode is at `len - 5` and its field at `len - 4`.
    let tail = if code.len() >= 5 {
        let field = (code.len() - 4) as u32;
        let opcode = code.get(code.len() - 5).copied().unwrap_or(0);
        holes.iter().position(|h| {
            h.kind == HoleKind::Branch
                && h.sites.len() == 1
                && h.sites.first().is_some_and(|s| s.off == field)
                && opcode == JMP_REL32
        })
    } else {
        None
    };

    // Sorted for the same reason the sites are: two builds of the same C must
    // produce the same library bytes.
    const_refs.sort_by_key(|c| c.field);
    Ok(Stencil {
        name: String::from(sname),
        code,
        holes,
        consts: consts.bytes,
        consts_align: consts.align,
        const_refs,
        tail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_shape_predicates_are_the_encodings_they_name() {
        // `48 8d 05` is `lea rax, [rip+disp32]`; `4c 8d 05` is the same into
        // `r8`, with `REX.R`. Both are `REX.W`, both are rip-relative.
        assert!(is_rex_w(0x48) && is_rex_w(0x4c) && is_rex_w(0x4f));
        assert!(!is_rex_w(0x40) && !is_rex_w(0x44) && !is_rex_w(0x66));
        assert!(is_rip_modrm(0x05) && is_rip_modrm(0x0d) && is_rip_modrm(0x3d));
        // `mod = 01` and `r/m = 100` (SIB) are the two neighbours it must not
        // accept: both would put a byte between the ModRM and the field.
        assert!(!is_rip_modrm(0x45) && !is_rip_modrm(0x04));
    }
}
