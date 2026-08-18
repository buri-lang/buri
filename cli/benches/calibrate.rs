//! Speed of light: what the machine costs before a compiler is written.
//!
//! `design/PERFORMANCE.md` §3.2 lists this as a deliberate omission and says
//! the row "should get one before anybody works on the lexer — at the current
//! gap the ceiling is not the binding constraint, but it will be." At the
//! 10 M lines/s goal the front end's budget is 100 ns/line and ~14 ns/token,
//! which is around fifty cycles, and no design argument about how to spend
//! fifty cycles means anything until somebody has measured what a bare pass
//! over the same bytes costs on the same machine.
//!
//! Five loops, over the same corpus text the timed rows use, each doing one of
//! the things a front end has to do and nothing else:
//!
//! ```text
//! memcpy       copy_from_slice over the whole corpus     pure bandwidth
//! byte-scan    a `match` classification producing a sum  dispatch, no writes
//! token-write  append a 12-byte record per real token    the lexer's write side
//! node-write   append a 24-byte record per tree node     the parser's write side
//! alloc-pair   Box::new / drop, once per token           the allocator coefficient
//! ```
//!
//! # The interpretation rule, written down before the numbers arrived
//!
//! Quoted verbatim from the wave-3 design (`flat-ast-design.md` §4, stage 0),
//! so that the reading of the result is not chosen after seeing it:
//!
//! > **The interpretation rule, written down before the numbers arrive:** if
//! > `byte-scan + token-write + node-write` already consumes more than
//! > ~60 ns/line on this machine, 10 M lines/s is not reachable by any
//! > front-end design and `design/PERFORMANCE.md` §6's number should move
//! > rather than the code. If it is under ~30 ns/line, the goal is sound and
//! > the remaining gap is engineering.
//!
//! The binary prints that verdict itself, from the constants below, so the
//! rule is applied by the harness rather than by whoever reads the table.
//!
//! These are *ceilings*, not predictions. A real lexer reads the bytes it
//! classifies and writes the tokens it produces, so it pays the sum of the
//! three at best; nothing here says it can reach the sum.

use std::time::Duration;

/// The two thresholds of the interpretation rule above, in nanoseconds per
/// line of the corpus. Named rather than inlined so the verdict the binary
/// prints and the rule quoted in the doc comment cannot drift apart.
const CEILING_HOPELESS_NS_PER_LINE: f64 = 60.0;
const CEILING_SOUND_NS_PER_LINE: f64 = 30.0;

/// A byte-classification table of the shape a lexer's first dispatch has: one
/// class per byte, so the scan loop is a load and a jump rather than a chain
/// of comparisons. This is the *cheapest* thing a lexer's inner loop can be,
/// which is what makes it a ceiling.
const CLASS: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = match i as u8 {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => 1,
            b'0'..=b'9' => 2,
            b' ' | b'\t' | b'\r' => 3,
            b'\n' => 4,
            b'"' | b'\'' => 5,
            _ => 6,
        };
        i += 1;
    }
    t
};

/// The record a structure-of-arrays token array would append per token: a kind
/// byte and a span. Twelve bytes, which is what `design`'s stage 7 proposes
/// and what a write-side ceiling should therefore be measured against.
#[derive(Clone, Copy)]
#[repr(C)]
struct TokenRec {
    kind: u8,
    start: u32,
    end: u32,
}

/// The 24-byte `Node` of the flattened parse tree: kind, subtree size, and a
/// four-word payload. Pinned here as well as in `parsing::tree` because the
/// ceiling is only a ceiling for *this* width.
#[derive(Clone, Copy)]
#[allow(
    dead_code,
    reason = "the record exists for its width and its alignment. The loop \
              appends it and never reads it back, which is the point: what is \
              being measured is the cost of writing twenty-four bytes per node."
)]
struct NodeRec {
    kind: u8,
    subtree: u32,
    payload: [u32; 4],
}

const _: () = assert!(std::mem::size_of::<NodeRec>() == 24);

/// One calibration row.
pub struct Ceiling {
    pub name: &'static str,
    /// What the loop bounds, for the table.
    pub bounds: &'static str,
    pub median: Duration,
    pub dispersion: f64,
    pub fastest: Duration,
    /// How many of the loop's own unit it processed — bytes, tokens, nodes —
    /// so a per-unit figure is derivable without knowing which loop it was.
    pub units: usize,
    pub unit: &'static str,
}

impl Ceiling {
    pub fn ns_per_line(&self, lines: usize) -> f64 {
        if lines == 0 {
            return 0.0;
        }
        self.median.as_secs_f64() * 1e9 / lines as f64
    }

    pub fn ns_per_unit(&self) -> f64 {
        if self.units == 0 {
            return 0.0;
        }
        self.median.as_secs_f64() * 1e9 / self.units as f64
    }
}

/// The corpus a calibration run is taken over, and the counts every row is
/// normalized by. Built once by the caller from the same `Program` the timed
/// rows use, so a ceiling and a measurement are over identical bytes.
pub struct Corpus<'a> {
    pub texts: Vec<&'a str>,
    pub lines: usize,
    pub bytes: usize,
    pub tokens: usize,
}

impl Corpus<'_> {
    /// How many tree nodes the corpus is worth.
    ///
    /// Two sevenths of the token count: `parse-wave2-report.md` §3 counts
    /// ~2 000 interior `Box<Expr>` per 1 000 lines against 7 416 tokens, and
    /// the flattened tree has one node per interior *and* leaf expression, so
    /// this lands within a few percent of what the parser actually appends.
    /// Wrong by a constant factor at worst, and the row reports its own
    /// per-node figure, so a reader can rescale it.
    pub fn nodes(&self) -> usize {
        self.tokens.saturating_mul(2) / 7
    }
}

/// The five ceilings, in the order the table prints them.
///
/// `bench` is the harness's own repeat-and-median function, passed in rather
/// than duplicated: a ceiling measured with a different warmup or a different
/// summary statistic is not comparable with the row it is a ceiling for.
pub fn run(
    corpus: &Corpus<'_>,
    mut bench: impl FnMut(&mut dyn FnMut()) -> (Duration, f64, Duration, usize),
) -> Vec<Ceiling> {
    let mut out = Vec::new();

    // -- memcpy -------------------------------------------------------------
    //
    // The floor for anything that reads every byte. The destination is
    // allocated once, outside the timer, because a `Vec` allocation per
    // repetition would be measuring the allocator instead.
    let mut dst: Vec<u8> = vec![0; corpus.bytes];
    let (median, dispersion, fastest, _) = bench(&mut || {
        let mut at = 0usize;
        for t in &corpus.texts {
            let n = t.len();
            dst[at..at + n].copy_from_slice(t.as_bytes());
            at += n;
        }
        std::hint::black_box(&dst);
    });
    out.push(Ceiling {
        name: "memcpy",
        bounds: "bandwidth: reading every byte once",
        median,
        dispersion,
        fastest,
        units: corpus.bytes,
        unit: "byte",
    });

    // -- byte-scan ----------------------------------------------------------
    //
    // The lexer's dispatch with no write side: one table lookup per byte,
    // folded into a checksum so the loop cannot be deleted. A real lexer does
    // strictly more than this per byte.
    let (median, dispersion, fastest, _) = bench(&mut || {
        let mut sum = 0u64;
        for t in &corpus.texts {
            for &b in t.as_bytes() {
                sum = sum.wrapping_add(CLASS[b as usize] as u64);
            }
        }
        std::hint::black_box(sum);
    });
    out.push(Ceiling {
        name: "byte-scan",
        bounds: "the lexer's dispatch, without its writes",
        median,
        dispersion,
        fastest,
        units: corpus.bytes,
        unit: "byte",
    });

    // -- token-write --------------------------------------------------------
    //
    // The lexer's write side without its scan: one 12-byte record per token,
    // into a buffer sized once. `clear` rather than a fresh `Vec` for the same
    // reason as above — the allocation has its own row.
    let mut toks: Vec<TokenRec> = Vec::with_capacity(corpus.tokens + 1);
    let ntok = corpus.tokens;
    let (median, dispersion, fastest, _) = bench(&mut || {
        toks.clear();
        for i in 0..ntok {
            toks.push(TokenRec { kind: (i & 0x3f) as u8, start: i as u32, end: i as u32 + 3 });
        }
        std::hint::black_box(&toks);
    });
    out.push(Ceiling {
        name: "token-write",
        bounds: "the lexer's write side, without its scan",
        median,
        dispersion,
        fastest,
        units: corpus.tokens,
        unit: "token",
    });

    // -- node-write ---------------------------------------------------------
    //
    // The parser's write side under the flattened design: one 24-byte node
    // appended per tree node, sequentially, into a pre-sized arena.
    let nnode = corpus.nodes();
    let mut nodes: Vec<NodeRec> = Vec::with_capacity(nnode + 1);
    let (median, dispersion, fastest, _) = bench(&mut || {
        nodes.clear();
        for i in 0..nnode {
            nodes.push(NodeRec {
                kind: (i & 0x3f) as u8,
                subtree: i as u32,
                payload: [i as u32, 0, 0, 0],
            });
        }
        std::hint::black_box(&nodes);
    });
    out.push(Ceiling {
        name: "node-write",
        bounds: "the parser's write side, flattened",
        median,
        dispersion,
        fastest,
        units: nnode,
        unit: "node",
    });

    // -- alloc-pair ---------------------------------------------------------
    //
    // The coefficient every allocation argument in `parse-analysis.md`,
    // `parse-wave2-report.md` §3 and `flat-ast-design.md` §3 rests on, which
    // has until now been quoted from the literature as "25-30 ns" and never
    // measured here. One `malloc`/`free` pair per token of the corpus.
    //
    // The box is `black_box`ed before it is dropped so the pair cannot be
    // elided; that is the whole loop.
    let (median, dispersion, fastest, _) = bench(&mut || {
        for i in 0..ntok {
            let b = Box::new(i as u64);
            std::hint::black_box(&b);
            drop(b);
        }
    });
    out.push(Ceiling {
        name: "alloc-pair",
        bounds: "one malloc/free pair, the coefficient",
        median,
        dispersion,
        fastest,
        units: corpus.tokens,
        unit: "pair",
    });

    out
}

/// The verdict of the interpretation rule, applied to a finished run.
///
/// Returns the combined front-end ceiling in nanoseconds per line and the
/// sentence the rule says about it.
pub fn verdict(rows: &[Ceiling], lines: usize) -> (f64, String) {
    let sum: f64 = rows
        .iter()
        .filter(|r| matches!(r.name, "byte-scan" | "token-write" | "node-write"))
        .map(|r| r.ns_per_line(lines))
        .sum();
    let text = if sum > CEILING_HOPELESS_NS_PER_LINE {
        format!(
            "{sum:.1} ns/line is over the {CEILING_HOPELESS_NS_PER_LINE:.0} ns/line the rule \
             calls hopeless: 10 M lines/s is not reachable by any front-end design on this \
             machine, and design/PERFORMANCE.md §6's number should move rather than the code"
        )
    } else if sum < CEILING_SOUND_NS_PER_LINE {
        format!(
            "{sum:.1} ns/line is under the {CEILING_SOUND_NS_PER_LINE:.0} ns/line the rule calls \
             sound: the 10 M lines/s goal is physical on this machine and the remaining gap is \
             engineering"
        )
    } else {
        format!(
            "{sum:.1} ns/line falls between the rule's two thresholds \
             ({CEILING_SOUND_NS_PER_LINE:.0} and {CEILING_HOPELESS_NS_PER_LINE:.0} ns/line): the \
             goal is reachable only by a front end that runs at a large fraction of the \
             machine's ceiling, which nothing in the literature does"
        )
    };
    (sum, text)
}
