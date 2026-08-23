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

#[derive(Clone, Debug)]
pub struct Stencil {
    pub name: String,
    pub code: Vec<u8>,
    pub holes: Vec<Hole>,
    /// Index in `holes` of the continuation whose `b` is the body's very last
    /// instruction. Copy-and-patch elides that branch when the continuation is
    /// laid out immediately after (the paper's "control is passed directly to
    /// the next operation").
    pub tail: Option<usize>,
}

impl Stencil {
    pub fn find(&self, name: &str) -> Option<usize> {
        self.holes.iter().position(|h| h.name == name)
    }
}

#[derive(Default)]
pub struct Library {
    pub stencils: Vec<Stencil>,
    pub index: HashMap<String, u32>,
    /// Wall time clang spent, milliseconds, when this library was built.
    pub build_ms: f64,
    pub config: String,
}

impl Library {
    pub fn get(&self, key: &str) -> Option<&Stencil> {
        self.index.get(key).and_then(|i| self.stencils.get(*i as usize))
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
const MAGIC: [u8; 8] = *b"CPJITLB1";

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
            lengths.push(len);
            stencils.push(Stencil {
                name,
                code: Vec::new(),
                holes,
                tail: (tail != u32::MAX).then_some(tail as usize),
            });
        }
        for (s, len) in stencils.iter_mut().zip(lengths) {
            s.code = c.take(len)?.to_vec();
        }
        Ok(Library { stencils, index, build_ms: 0.0, config })
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
            tail: Some(0),
        };
        let mut index = HashMap::new();
        index.insert(s.name.clone(), 0);
        Library { stencils: vec![s], index, build_ms: 1.0, config: String::from("r3") }
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
