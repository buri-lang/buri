//! Deterministic Buri source generation for the compiler benchmarks.
//!
//! The suite measures throughput, and throughput is only meaningful over
//! *representative* input. So most of the input is generated, from a seed, at
//! whatever scale the caller asks for — and `compiler.rs` runs the real front
//! end over it before any timer starts, so a generator bug shows up as a failed
//! validation rather than as a benchmark that measures the error paths (see
//! `design/PERFORMANCE.md` §3, §4). A little of it is checked in instead, with a
//! manifest naming the profile and the seed it came from; `corpus.rs` is that
//! half, and it produces the same [`Program`] this file does.
//!
//! Two things the generator is deliberately *not*:
//!
//!   * It is not a fuzzer. Every program it emits is meant to compile, and one
//!     that does not is a bug in this file.
//!   * It is not a single-construct microbenchmark generator. The `mixed`
//!     profile is the shape the targets are stated against, and it emits the
//!     mix a real module has — declarations, bodies, generics, matches,
//!     imports, literals, comments — because a file of nothing but
//!     `fn f(): Int { 1 }` measures one path through the parser and nothing
//!     about the checker.
//!
//! The stress profiles beside it exist for the opposite reason: each one is a
//! single construct, pushed until it is the whole cost, so that a phase which
//! is superlinear in *something* says which something.
//!
//! # The parameter space
//!
//! Five named shapes answer five questions. "Large files versus many small",
//! "many libraries versus few", "many enums versus few" are not five more
//! names, they are five *axes*, and axes compose. So the generator's input is
//! [`Params`], a record of about twenty dimensions, and a *profile* is that
//! record with two or three fields moved. [`Params::default()`] is the
//! realistic mix the goals are stated against, and it is byte-for-byte the
//! corpus `design/PERFORMANCE.md` §6 quotes its numbers over — which is a
//! promise, not a nicety, and §5.1 of the design says how it is held.
//!
//! **The most important implementation constraint in this file**: any new
//! dimension that needs a random draw takes it from [`Rng`] *aux*, seeded
//! separately, so that a dimension left at its default consumes nothing from
//! the primary stream and the default bytes cannot move. Where a dimension does
//! read the primary stream — the fan-out, the field counts, the construct mix —
//! its default expression is arranged to make exactly the draws the pre-
//! parameter generator made, in the same order.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "benchmark harness. The lint set in `Cargo.toml` pins a promise \
              about the toolchain — that no input panics it — and a generator \
              that feeds the toolchain is not the toolchain. Every index here \
              is into a slice this file just built, and every arithmetic \
              operation is over a counter bounded by a scale the caller passed \
              in lines of source."
)]

/// Bumped by hand whenever a change here would move the bytes of any profile.
///
/// A saved corpus records the revision it was written at. A manifest naming an
/// *older* revision is legal and expected — byte-stability across a generator
/// change is the entire point of saving one — so `--validate` prints it as a
/// note and never as an error.
pub const GENERATOR_REVISION: u32 = 1;

/// A generated program: its modules, in an order where every module's imports
/// come before it.
///
/// The order is load-bearing. `Loader::load_source_in` returns early on a path
/// it has already seen, so loading the modules leaf-first means an import
/// never reaches the workspace resolver — which is what lets the whole program
/// live in memory with no repository on disk. `corpus.rs` preserves it by
/// filename, which is why the saved files are `m0000.buri` … `main.buri`.
pub struct Program {
    pub modules: Vec<Module>,
}

pub struct Module {
    pub path: String,
    pub text: String,
}

impl Program {
    /// Non-blank lines of generated source, which is what the targets in
    /// `design/PERFORMANCE.md` are stated in.
    pub fn lines(&self) -> usize {
        self.modules.iter().map(|m| nonblank_lines(&m.text)).sum()
    }

    /// Bytes of generated source, UTF-8. Reported beside the line rate because
    /// a line is a unit of authorship and a byte is a unit of work, and the
    /// two ratios drift as a generator's style changes.
    pub fn bytes(&self) -> usize {
        self.modules.iter().map(|m| m.text.len()).sum()
    }
}

/// Lines with something on them. Comments count: the lexer reads them, the
/// parser attaches them, and a measurement that excluded them would flatter
/// every phase in proportion to how well the source is documented.
pub fn nonblank_lines(text: &str) -> usize {
    text.lines().filter(|l| !l.trim().is_empty()).count()
}

// ---------------------------------------------------------------------------
// The parameter space
// ---------------------------------------------------------------------------

/// Which emitter builds the source.
///
/// The enum survives the move to [`Params`] only where a dimension genuinely
/// cannot be expressed as a weight: an expression nested two hundred deep and a
/// single enum with five thousand variants are not *mixes* of anything, they
/// are their own programs. Everything else — struct-heavy, enum-heavy,
/// derive-heavy, comment-free — is [`Shape::Mixed`] with weights moved.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Realistic construct mix across many modules.
    Mixed,
    /// One expression, nested `depth` deep. Stresses recursive descent, the
    /// parser's depth guard, and the checker's recursion over expressions.
    DeepNesting,
    /// One `match` with thousands of arms over one wide enum. Stresses
    /// exhaustiveness checking and decision-tree construction, both of which
    /// have a plausible superlinear term in the arm count.
    WideMatch,
    /// Thousands of two-line functions. Stresses per-item overhead: symbol
    /// table insertion, scope construction, per-function setup in every phase.
    ManySmallFunctions,
    /// A handful of thousand-line functions. Stresses per-body cost: local
    /// scope depth, inference over a long chain of `let`s, and whatever the
    /// backend does per basic block.
    FewLargeFunctions,
}

/// Whether a profile is a program somebody might have written, or a shape built
/// to break one pass.
///
/// A type rather than a convention, because `design/PERFORMANCE.md` §3's rule
/// that stress shapes are never blended into a realistic mix and never quoted
/// against a goal is worth making unrepresentable-to-violate: the report groups
/// by family and prints the goal column only for [`Family::Realistic`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Realistic,
    Stress,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::Realistic => "realistic",
            Family::Stress => "stress",
        }
    }
}

/// The default seed. Fixed, because two runs of the suite have to compile the
/// same bytes or the numbers are not comparable.
pub const SEED: u64 = 0x0B00_1A57_5EED_0001;

/// Every dimension the generator has.
///
/// `default()` is the realistic mix the goals are stated against; a profile is
/// this struct with two or three fields moved, and a `--param` override is the
/// same thing spelled on a command line.
#[derive(Clone)]
pub struct Params {
    /// Which emitter runs. Not settable with `--param`: it is a property of the
    /// profile, and the four stress emitters ignore most of what follows.
    pub shape: Shape,

    // -- size and distribution ------------------------------------------
    /// Target non-blank lines. A floor: the generator emits whole
    /// declarations, so it overshoots by at most one declaration per module.
    pub lines: usize,
    /// Non-blank lines per module before the generator starts the next one.
    /// The axis behind "large files vs many small files" — and, because
    /// `middle::lower` assigns one codegen unit per source module, the axis
    /// behind per-unit cost in the native backends too.
    pub lines_per_module: usize,
    /// How many clusters the modules are partitioned into (`index % clusters`).
    /// The axis behind "many libraries vs few": imports stay inside a cluster
    /// except as `cross_cluster` allows.
    pub clusters: usize,
    /// One import in N leaves the module's own cluster. 0 means never, which
    /// makes the clusters independent programs sharing one `main`.
    pub cross_cluster: u32,
    /// Imports per module, inclusive range. Import-graph fan-out.
    pub fanout: (u32, u32),
    /// How far back a dependency may be drawn from, as a percentage of the
    /// modules emitted so far: 100 is uniform (a shallow, wide graph), 5 is
    /// near-neighbour only (a deep chain). Sema's cross-module resolution and
    /// monomorphization's walk both care which.
    pub dep_span_pct: u32,

    // -- construct mix (relative weights; a zero removes the kind) ------
    pub w_struct: u32,
    pub w_enum: u32,
    pub w_generic_fn: u32,
    pub w_arith_fn: u32,
    pub w_match_fn: u32,
    pub w_string_fn: u32,
    pub w_list_fn: u32,

    // -- per-construct size --------------------------------------------
    pub fields_per_struct: (u32, u32),
    pub variants_per_enum: (u32, u32),
    /// Methods in the `impl` block a struct gets, including the two every
    /// struct has. Trait/impl density.
    pub methods_per_struct: u32,
    /// How many traits each type derives, from `Eq, Show, Ord, Hash, ToJson,
    /// FromJson` in that order. Load-bearing for `middle::derives`, which only
    /// the native branch runs.
    pub derives: u32,
    /// `let` bindings in an arithmetic body. The continuous version of "few
    /// large functions vs many small".
    pub body_lets: (u32, u32),
    /// Arms in an extra generated integer `match`, beyond the fixed
    /// nested/guarded one. `(0, 0)` emits none.
    pub match_arms: (u32, u32),
    /// Distinct type arguments each generic is instantiated at. The dial that
    /// makes monomorphization expensive without making the source bigger.
    pub generic_args: u32,
    /// Parenthesised nesting depth inside generated arithmetic expressions.
    pub nesting: u32,

    // -- surface ---------------------------------------------------------
    /// Percentage of doc-comment groups kept. Comments count toward the
    /// denominator (`design/PERFORMANCE.md` §2), so this is a real dial on
    /// every reported rate, and the reason a `comment-heavy`/`comment-free`
    /// pair is worth having.
    pub doc_comment_pct: u32,
    /// Extra `//` lines prepended to every top-level declaration. The other
    /// half of the comment pair: `doc_comment_pct` removes, this adds.
    pub comment_block_lines: u32,
    /// Blank lines as a percentage of emitted lines. Generated *and* excluded
    /// from the denominator — the conservative combination §2 already argues
    /// for; today they are only incidental.
    pub blank_pct: u32,
    /// Identifier padding, in characters. Per-declaration names gain a suffix
    /// of this length, which moves bytes/line and bytes/token without moving
    /// lines/token — the only way to tell a byte-rate regression from a
    /// line-rate one.
    pub ident_len: u32,

    // -- invariants ------------------------------------------------------
    /// Whether `main` reaches every declaration. **Must stay true** for any
    /// corpus a lowering row is taken over; a `false` here is how the suite's
    /// first run reported lowering as faster than lexing.
    pub reach: bool,
    pub seed: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            shape: Shape::Mixed,
            lines: 10_000,
            // 250 because that is about where the repository's own `.buri`
            // files sit, and because the module count is the axis semantic
            // analysis is most likely to be superlinear in.
            lines_per_module: 250,
            clusters: 1,
            cross_cluster: 3,
            fanout: (1, 3),
            dep_span_pct: 100,
            w_struct: 1,
            w_enum: 1,
            w_generic_fn: 1,
            w_arith_fn: 1,
            w_match_fn: 1,
            w_string_fn: 1,
            w_list_fn: 1,
            fields_per_struct: (2, 5),
            variants_per_enum: (3, 7),
            methods_per_struct: 2,
            derives: 2,
            body_lets: (4, 11),
            match_arms: (0, 0),
            generic_args: 2,
            nesting: 1,
            doc_comment_pct: 100,
            comment_block_lines: 0,
            blank_pct: 0,
            ident_len: 0,
            reach: true,
            seed: SEED,
        }
    }
}

/// Every `--param` key, for the error message and for `--list`.
pub const KEYS: &[&str] = &[
    "lines",
    "lines_per_module",
    "clusters",
    "cross_cluster",
    "fanout",
    "dep_span_pct",
    "w_struct",
    "w_enum",
    "w_generic_fn",
    "w_arith_fn",
    "w_match_fn",
    "w_string_fn",
    "w_list_fn",
    "fields_per_struct",
    "variants_per_enum",
    "methods_per_struct",
    "derives",
    "body_lets",
    "match_arms",
    "generic_args",
    "nesting",
    "doc_comment_pct",
    "comment_block_lines",
    "blank_pct",
    "ident_len",
    "reach",
    "seed",
];

impl Params {
    /// One dimension, from a `key=value` pair.
    ///
    /// Values are decimal integers, `true`/`false`, `a..b` inclusive ranges, or
    /// `0x…` for the seed. No parser and no dependency: forty lines of `match`,
    /// with an unknown key listing the valid ones.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        fn num(v: &str) -> Result<u64, String> {
            let t = v.trim();
            let parsed = if let Some(hex) = t.strip_prefix("0x") {
                u64::from_str_radix(&hex.replace('_', ""), 16)
            } else {
                t.replace('_', "").parse::<u64>()
            };
            parsed.map_err(|_| format!("`{v}` is not a number"))
        }
        fn range(v: &str) -> Result<(u32, u32), String> {
            let (lo, hi) = v.split_once("..").ok_or_else(|| {
                format!("`{v}` is not a range; write it as `lo..hi`, inclusive")
            })?;
            let lo = num(lo)? as u32;
            let hi = num(hi)? as u32;
            if hi < lo {
                return Err(format!("`{v}` is empty: the high end is below the low one"));
            }
            Ok((lo, hi))
        }
        fn flag(v: &str) -> Result<bool, String> {
            match v.trim() {
                "true" | "1" => Ok(true),
                "false" | "0" => Ok(false),
                _ => Err(format!("`{v}` is not `true` or `false`")),
            }
        }
        match key {
            "lines" => self.lines = num(value)? as usize,
            "lines_per_module" => self.lines_per_module = num(value)? as usize,
            "clusters" => self.clusters = (num(value)? as usize).max(1),
            "cross_cluster" => self.cross_cluster = num(value)? as u32,
            "fanout" => self.fanout = range(value)?,
            "dep_span_pct" => self.dep_span_pct = num(value)? as u32,
            "w_struct" => self.w_struct = num(value)? as u32,
            "w_enum" => self.w_enum = num(value)? as u32,
            "w_generic_fn" => self.w_generic_fn = num(value)? as u32,
            "w_arith_fn" => self.w_arith_fn = num(value)? as u32,
            "w_match_fn" => self.w_match_fn = num(value)? as u32,
            "w_string_fn" => self.w_string_fn = num(value)? as u32,
            "w_list_fn" => self.w_list_fn = num(value)? as u32,
            "fields_per_struct" => self.fields_per_struct = range(value)?,
            "variants_per_enum" => self.variants_per_enum = range(value)?,
            "methods_per_struct" => self.methods_per_struct = num(value)? as u32,
            "derives" => self.derives = (num(value)? as u32).min(DERIVABLE.len() as u32),
            "body_lets" => self.body_lets = range(value)?,
            "match_arms" => self.match_arms = range(value)?,
            "generic_args" => self.generic_args = num(value)? as u32,
            "nesting" => self.nesting = (num(value)? as u32).max(1),
            "doc_comment_pct" => self.doc_comment_pct = num(value)? as u32,
            "comment_block_lines" => self.comment_block_lines = num(value)? as u32,
            "blank_pct" => self.blank_pct = (num(value)? as u32).min(90),
            "ident_len" => self.ident_len = num(value)? as u32,
            "reach" => self.reach = flag(value)?,
            "seed" => self.seed = num(value)?,
            other => {
                return Err(format!(
                    "unknown parameter `{other}`; the dimensions are: {}",
                    KEYS.join(", ")
                ))
            }
        }
        if self.weights().iter().sum::<u32>() == 0 {
            return Err(String::from(
                "every construct weight is zero, so there is nothing to emit; leave one above zero",
            ));
        }
        Ok(())
    }

    fn weights(&self) -> [u32; 7] {
        [
            self.w_struct,
            self.w_enum,
            self.w_generic_fn,
            self.w_arith_fn,
            self.w_match_fn,
            self.w_string_fn,
            self.w_list_fn,
        ]
    }

    /// Every field that is not at its default, sorted, as `k=v` text.
    ///
    /// This is what a saved corpus's manifest records, and what `--list`
    /// prints: a profile is only ever a small delta from the default, so the
    /// delta *is* the definition.
    pub fn delta(&self) -> String {
        let d = Params::default();
        let mut out: Vec<String> = Vec::new();
        macro_rules! scalar {
            ($($f:ident),* $(,)?) => {$(
                if self.$f != d.$f { out.push(format!("{}={}", stringify!($f), self.$f)); }
            )*};
        }
        macro_rules! pair {
            ($($f:ident),* $(,)?) => {$(
                if self.$f != d.$f {
                    out.push(format!("{}={}..{}", stringify!($f), self.$f.0, self.$f.1));
                }
            )*};
        }
        scalar!(
            lines,
            lines_per_module,
            clusters,
            cross_cluster,
            dep_span_pct,
            w_struct,
            w_enum,
            w_generic_fn,
            w_arith_fn,
            w_match_fn,
            w_string_fn,
            w_list_fn,
            methods_per_struct,
            derives,
            generic_args,
            nesting,
            doc_comment_pct,
            comment_block_lines,
            blank_pct,
            ident_len,
            reach,
        );
        pair!(fanout, fields_per_struct, variants_per_enum, body_lets, match_arms);
        if self.seed != d.seed {
            out.push(format!("seed={:#018x}", self.seed));
        }
        out.sort();
        out.join(" ")
    }
}

// ---------------------------------------------------------------------------
// Named profiles
// ---------------------------------------------------------------------------

/// Every profile, in report order: the realistic family first, then the stress
/// family.
///
/// `--param` is for an investigation; a profile is what gets a row in the doc,
/// because a row nobody can re-run by name is not a measurement anybody can
/// repeat.
pub const PROFILES: &[&str] = &[
    // Realistic
    "mixed",
    "mixed-many-files",
    "mixed-few-files",
    "mixed-libs",
    "mixed-deep-graph",
    "mixed-wide-graph",
    // Stress
    "deep-nesting",
    "wide-match",
    "many-small-fns",
    "few-large-fns",
    "struct-heavy",
    "struct-light",
    "enum-heavy",
    "generic-blowup",
    "derive-heavy",
    "impl-heavy",
    "match-heavy",
    "comment-heavy",
    "comment-free",
    "long-idents",
];

/// The family and parameters of a named profile.
pub fn profile(name: &str) -> Option<(Family, Params)> {
    let mut p = Params::default();
    let family = match name {
        // -- realistic ------------------------------------------------------
        //
        // The headline. Byte-identical to the corpus `design/PERFORMANCE.md`
        // §6 quotes, which is why nothing here moves.
        "mixed" => Family::Realistic,
        // Many small files: per-module overhead, loader and symbol-table
        // setup, and — natively — per-codegen-unit cost.
        "mixed-many-files" => {
            p.lines_per_module = 40;
            p.fanout = (2, 5);
            Family::Realistic
        }
        // Large files: whether anything is superlinear in module *size*.
        "mixed-few-files" => {
            p.lines_per_module = 5_000;
            p.fanout = (1, 2);
            Family::Realistic
        }
        // Many libraries: a clustered import graph with thin edges between.
        "mixed-libs" => {
            p.clusters = 12;
            p.cross_cluster = 8;
            Family::Realistic
        }
        // A deep dependency chain rather than a wide fan.
        "mixed-deep-graph" => {
            p.dep_span_pct = 5;
            Family::Realistic
        }
        // Import-graph fan-out at the same line count.
        "mixed-wide-graph" => {
            p.fanout = (6, 12);
            Family::Realistic
        }

        // -- stress ---------------------------------------------------------
        "deep-nesting" => {
            p.shape = Shape::DeepNesting;
            Family::Stress
        }
        "wide-match" => {
            p.shape = Shape::WideMatch;
            Family::Stress
        }
        "many-small-fns" => {
            p.shape = Shape::ManySmallFunctions;
            Family::Stress
        }
        "few-large-fns" => {
            p.shape = Shape::FewLargeFunctions;
            Family::Stress
        }
        // Lots of structs: layout, derives, field resolution.
        "struct-heavy" => {
            p.w_struct = 8;
            p.w_enum = 0;
            p.w_generic_fn = 0;
            p.w_match_fn = 0;
            p.w_string_fn = 0;
            p.w_list_fn = 0;
            p.fields_per_struct = (6, 12);
            Family::Stress
        }
        // The control for the row above. Only meaningful as a pair.
        "struct-light" => {
            p.w_struct = 0;
            Family::Stress
        }
        // Lots of enums, matched exhaustively — the *realistic* neighbourhood
        // of `wide-match`.
        "enum-heavy" => {
            p.w_enum = 8;
            p.w_struct = 0;
            p.w_generic_fn = 0;
            p.w_match_fn = 0;
            p.w_string_fn = 0;
            p.w_list_fn = 0;
            p.variants_per_enum = (12, 24);
            Family::Stress
        }
        // Monomorphization: eight copies of every generic, at one source size.
        "generic-blowup" => {
            p.w_generic_fn = 8;
            p.generic_args = 8;
            Family::Stress
        }
        // `middle::derives`, which only the native branch runs — invisible in
        // every JS row the suite has ever printed.
        "derive-heavy" => {
            p.derives = 6;
            p.w_struct = 4;
            p.w_enum = 4;
            Family::Stress
        }
        // Method resolution and per-impl setup.
        "impl-heavy" => {
            p.methods_per_struct = 12;
            p.w_struct = 6;
            Family::Stress
        }
        // Decision-tree construction on realistic arm counts.
        "match-heavy" => {
            p.w_match_fn = 8;
            p.match_arms = (8, 20);
            Family::Stress
        }
        // The lexer's comment path, and the honesty of §2's "comments count".
        "comment-heavy" => {
            p.comment_block_lines = 6;
            Family::Stress
        }
        // The control. The *ratio* of the two is the number worth recording.
        "comment-free" => {
            p.doc_comment_pct = 0;
            Family::Stress
        }
        // Bytes/token and the lexer's identifier path, at a fixed token count.
        "long-idents" => {
            p.ident_len = 32;
            Family::Stress
        }
        _ => return None,
    };
    Some((family, p))
}

// ---------------------------------------------------------------------------
// The PRNG
// ---------------------------------------------------------------------------

/// SplitMix64. Four lines, no dependency, and the same sequence on every
/// machine — which is the whole requirement: two runs of the suite must
/// compile the same bytes, or the numbers are not comparable.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number below `n`. `n` is a small literal at every call site here, so
    /// the modulo bias is below one part in 2^60 and irrelevant to a shape.
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len() as u64) as usize]
    }

    /// A draw from an inclusive range. `(1, 3)` is `1 + below(3)`, which is
    /// exactly the expression the pre-parameter generator used for the
    /// fan-out, the field count and the `let` count — so the default bytes do
    /// not move.
    fn span(&mut self, r: (u32, u32)) -> u32 {
        r.0 + self.below(u64::from(r.1 - r.0 + 1)) as u32
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Generate a program of at least `params.lines` non-blank lines.
///
/// The count is a floor rather than an exact figure: the generator emits whole
/// declarations, so it overshoots by at most one declaration per module. The
/// measured rate divides by the *actual* line count, so the overshoot costs
/// nothing but a slightly ragged label.
pub fn program(params: &Params) -> Program {
    match params.shape {
        Shape::Mixed => mixed(params),
        Shape::DeepNesting => deep_nesting(params.lines),
        Shape::WideMatch => wide_match(params.lines),
        Shape::ManySmallFunctions => many_small(params.lines),
        Shape::FewLargeFunctions => few_large(params.lines),
    }
}

// ---------------------------------------------------------------------------
// The realistic mix
// ---------------------------------------------------------------------------

/// The traits a generated type can derive, in the order `derives` takes them.
///
/// The first two are `Eq, Show`, which is what every generated type derived
/// before `derives` was a dimension — so `derives = 2` emits the same bytes.
const DERIVABLE: &[&str] = &["Eq", "Show", "Ord", "Hash", "ToJson", "FromJson"];

/// How many of [`DERIVABLE`] need no import. Past this the module has to pull
/// in `core/json`, which is a line of source and so a thing the default corpus
/// must not pay for.
const DERIVABLE_IN_SCOPE: usize = 4;

fn derive_list(n: u32) -> String {
    DERIVABLE[..(n as usize).min(DERIVABLE.len())].join(", ")
}

fn mixed(p: &Params) -> Program {
    let mut rng = Rng(p.seed ^ 0x4275_7269_0000_0001);
    // The second stream. Every dimension added after the parameter space
    // existed draws from here, so that leaving it at its default consumes
    // nothing from `rng` and the default corpus stays byte-identical.
    let mut aux = Rng(p.seed ^ 0x4175_7869_6C69_6172);
    let mut modules: Vec<Module> = Vec::new();
    let mut emitted = 0usize;
    let mut index = 0usize;

    while emitted < p.lines {
        let path = format!("//bench/m{index:04}");
        // Import from the modules already emitted. The first module has
        // nothing to import, which is the leaf every other module's imports
        // bottom out in.
        let mut deps: Vec<usize> = Vec::new();
        if index > 0 {
            let want = rng.span(p.fanout) as usize;
            for _ in 0..want {
                let d = draw_dep(&mut rng, index, p);
                if !deps.contains(&d) {
                    deps.push(d);
                }
            }
        }
        let want_here = p.lines_per_module.min(p.lines.saturating_sub(emitted).max(40));
        let text = surface(mixed_module(index, &deps, want_here, p, &mut rng, &mut aux), p);
        emitted += nonblank_lines(&text);
        modules.push(Module { path, text });
        index += 1;
    }

    // The entry point. Monomorphization needs a root, and `Roots::Main` needs
    // a `main`, so the last module is one — and it calls into the program so
    // that the lowering benchmark has something reachable to lower rather than
    // an empty program and a large pile of dead code.
    let last = modules.len();
    modules.push(Module {
        path: "//bench/main".to_string(),
        text: surface(main_module(last, p), p),
    });

    Program { modules }
}

/// One earlier module for this one to import.
///
/// The default — one cluster, a full-width span — is `rng.below(index)`, the
/// expression the pre-parameter generator used, and it is taken early so that
/// the default corpus cannot move. The clustered and near-neighbour paths below
/// only run when a profile asks for them.
fn draw_dep(rng: &mut Rng, index: usize, p: &Params) -> usize {
    if p.clusters <= 1 && p.dep_span_pct >= 100 {
        return rng.below(index as u64) as usize;
    }
    let span = ((index as u64 * u64::from(p.dep_span_pct)) / 100).clamp(1, index as u64);
    let r = rng.below(span) as usize;
    let mut d = index - 1 - r;
    if p.clusters > 1 {
        let cross = p.cross_cluster > 0 && rng.below(u64::from(p.cross_cluster)) == 0;
        if !cross {
            // Walk back to the nearest earlier module of this module's own
            // cluster. A cluster whose first member is this module has no
            // earlier member, and then the cross-cluster edge is taken anyway
            // — which is what makes `main` reach every cluster.
            let c = index % p.clusters;
            let mut walk = d;
            while walk % p.clusters != c && walk > 0 {
                walk -= 1;
            }
            if walk % p.clusters == c {
                d = walk;
            }
        }
    }
    d
}

/// One module of the realistic mix.
///
/// The weights are the point of this function. A module is mostly function
/// bodies, with a type declaration every few of them and a comment on most
/// things — which is what the repository's own source looks like, and what a
/// benchmark asserting "10,000,000 lines per second" has to be measured over
/// if the number is to mean anything to somebody compiling their own code.
fn mixed_module(
    index: usize,
    deps: &[usize],
    want_lines: usize,
    p: &Params,
    rng: &mut Rng,
    aux: &mut Rng,
) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "//! Generated module {index} of the compiler benchmark corpus.\n\
         //!\n\
         //! Emitted by `cli/benches/generate.rs` from a fixed seed. Nothing in\n\
         //! here is meant to be read; it is meant to be *compiled*, at a\n\
         //! controlled size and with the construct mix of real source.\n\n"
    ));
    s.push_str("from \"core/str\" import * as str;\n");
    s.push_str("from \"core/list\" import * as list;\n");
    s.push_str("from \"core/cap\" import { Alloc };\n");
    // `Eq`, `Show`, `Ord` and `Hash` are in scope everywhere; the JSON pair is
    // not. The import appears only when `derives` reaches them, so the default
    // corpus is unchanged.
    if p.derives as usize > DERIVABLE_IN_SCOPE {
        s.push_str("from \"core/json\" import { ToJson, FromJson };\n");
    }
    for d in deps {
        s.push_str(&format!(
            "from \"//bench/m{d:04}\" import {{ Node{d}, seed{d}, blend{d}, reach{d} }};\n"
        ));
    }
    s.push('\n');

    // Every module exports the same three names under its own index, which is
    // what lets any module import any earlier one without the generator
    // tracking a symbol table.
    s.push_str(&format!(
        "/// The record every module in this corpus exports, so that an import\n\
         /// of any module resolves to a known shape.\n\
         export struct Node{index} {{\n\
         \x20 export label: Str,\n\
         \x20 export value: Int,\n\
         \x20 export weight: Float,\n\
         }}\n\n\
         derive {} for Node{index};\n\n\
         export fn seed{index}(): Int {{ {} }}\n\n",
        derive_list(p.derives),
        (index as i64 * 7919) % 10_000 + 1
    ));

    // `blend` is what makes the import graph load-bearing rather than
    // decorative: it calls into every dependency, so the checker has to
    // resolve across module boundaries and monomorphization has to walk them.
    s.push_str(&format!(
        "/// Folds this module's seed together with its dependencies'.\n\
         export fn blend{index}(base: Int): Int {{\n"
    ));
    if deps.is_empty() {
        s.push_str(&format!("  base + seed{index}()\n}}\n\n"));
    } else {
        for (i, d) in deps.iter().enumerate() {
            s.push_str(&format!("  let d{i} = blend{d}(base) + seed{d}();\n"));
        }
        let sum: String =
            (0..deps.len()).map(|i| format!("d{i}")).collect::<Vec<_>>().join(" + ");
        s.push_str(&format!("  {sum} + seed{index}()\n}}\n\n"));
    }

    // A dependency's *type* is used too, not only its functions, so that
    // cross-module type resolution and cross-module monomorphization are both
    // on the measured path.
    if let Some(d) = deps.first() {
        s.push_str(&format!(
            "/// Reads a dependency's record, so the import carries a type as\n\
             /// well as a function.\n\
             export fn lift{index}(n: Node{d}): Node{index} {{\n\
             \x20 Node{index} {{ label: n.label, value: n.value + seed{index}(), weight: n.weight }}\n\
             }}\n\n"
        ));
    }

    let weights = p.weights();
    let total: u32 = weights.iter().sum();
    let mut counter = 0usize;
    let mut probes: Vec<String> = Vec::new();
    while nonblank_lines(&s) < want_lines {
        // With every weight at 1 this is `rng.below(7)` and the arm it selects
        // is the draw itself, which is what the pre-parameter generator did.
        let mut r = rng.below(u64::from(total)) as u32;
        let mut kind = weights.len() - 1;
        for (i, w) in weights.iter().enumerate() {
            if r < *w {
                kind = i;
                break;
            }
            r -= *w;
        }
        let n = tag(counter, p);
        match kind {
            0 => chunk_record_struct(&mut s, index, &n, p, rng),
            1 => chunk_enum(&mut s, index, &n, p, rng),
            2 => chunk_generic_fn(&mut s, index, &n, p),
            3 => chunk_arithmetic_fn(&mut s, index, &n, p, rng),
            4 => chunk_match_fn(&mut s, index, &n, p, aux),
            5 => chunk_string_fn(&mut s, index, &n, rng),
            _ => chunk_list_fn(&mut s, index, &n, rng),
        }
        probes.push(format!("probe{index}_{n}"));
        counter += 1;
    }

    // Everything the module declared, reachable from one exported function.
    //
    // Without this the lowering benchmark measures dead-code elimination: a
    // `main` that calls only `blend` leaves every declaration above
    // unreachable, monomorphization never requests them, and the phase reports
    // a throughput for a program that is 95% discarded. The first run of this
    // suite made exactly that mistake and reported lowering as *faster than
    // lexing*, which is the tell.
    s.push_str(&format!(
        "/// Every declaration in this module, reachable from one call, so that\n\
         /// monomorphization has to visit the whole of it.\n\
         export fn reach{index}<C: Alloc>(ctx: C): Int {{\n\
         \x20 0\n"
    ));
    if p.reach {
        for probe in &probes {
            s.push_str(&format!("    + {probe}(ctx)\n"));
        }
    }
    for d in deps {
        s.push_str(&format!("    + reach{d}(ctx)\n"));
    }
    s.push_str("}\n");
    s
}

/// The per-declaration name suffix.
///
/// `ident_len = 0` gives back the counter itself, so every generated name is
/// what it was before this dimension existed. Above zero it appends a fixed
/// pad, which moves bytes and characters without moving the token count — the
/// only way to separate a byte-rate regression from a line-rate one.
fn tag(counter: usize, p: &Params) -> String {
    if p.ident_len == 0 {
        counter.to_string()
    } else {
        format!("{counter}_{}", "x".repeat(p.ident_len as usize))
    }
}

fn chunk_record_struct(s: &mut String, m: usize, n: &str, p: &Params, rng: &mut Rng) {
    let fields = rng.span(p.fields_per_struct) as usize;
    let types = ["Int", "Str", "Float", "Bool"];
    s.push_str(&format!(
        "/// A record with {fields} fields, derived comparisons and two methods.\n\
         export struct Rec{m}_{n} {{\n"
    ));
    for f in 0..fields {
        s.push_str(&format!("  export f{f}: {},\n", types[f % types.len()]));
    }
    s.push_str("}\n\n");
    s.push_str(&format!("derive {} for Rec{m}_{n};\n\n", derive_list(p.derives)));
    s.push_str(&format!(
        "impl Rec{m}_{n} {{\n\
         \x20 /// A pure accessor, of the kind that is most of a real impl block.\n\
         \x20 export fn scaled(self: Rec{m}_{n}, factor: Int): Int {{\n\
         \x20   self.f0 * factor + {}\n\
         \x20 }}\n\n\
         \x20 /// Allocates, and says so with `C: Alloc`.\n\
         \x20 export fn render<C: Alloc>(self: Rec{m}_{n}, ctx: C): Str {{\n\
         \x20   str.format(ctx, \"rec{m}_{n}: ${{self.f0}}/${{self.f1}}\")\n\
         \x20 }}\n",
        rng.below(1000)
    ));
    // Beyond the two above, which every struct has. `methods_per_struct = 2`
    // adds nothing and the bytes are unchanged.
    let extra = p.methods_per_struct.saturating_sub(2);
    for j in 0..extra {
        s.push_str(&format!(
            "\n\x20 /// Method {j} of the impl-density dial.\n\
             \x20 export fn extra{j}(self: Rec{m}_{n}, k: Int): Int {{\n\
             \x20   self.f0 + k * {} + {j}\n\
             \x20 }}\n",
            j + 1
        ));
    }
    s.push_str("}\n\n");
    let inits: Vec<String> = (0..fields)
        .map(|f| format!("f{f}: {}", literal_for(types[f % types.len()])))
        .collect();
    let calls: String = (0..extra).map(|j| format!(" + r.extra{j}(2)")).collect();
    s.push_str(&format!(
        "/// Reaches everything above from one call. See `reach` below.\n\
         export fn probe{m}_{n}<C: Alloc>(ctx: C): Int {{\n\
         \x20 let r = Rec{m}_{n} {{ {} }};\n\
         \x20 r.scaled(3) + r.render(ctx).len(){calls}\n\
         }}\n\n",
        inits.join(", ")
    ));
}

/// A literal of the given primitive type, for a generated struct initializer.
fn literal_for(ty: &str) -> &'static str {
    match ty {
        "Str" => "\"probe\"",
        "Float" => "1.5",
        "Bool" => "true",
        _ => "7",
    }
}

fn chunk_enum(s: &mut String, m: usize, n: &str, p: &Params, rng: &mut Rng) {
    let variants = rng.span(p.variants_per_enum) as usize;
    s.push_str(&format!(
        "/// An enum of {variants} variants, matched exhaustively below.\n\
         export enum State{m}_{n} {{\n\
         \x20 export Idle,\n\
         \x20 export Running(Int),\n\
         \x20 export Failed {{ code: Int, message: Str }},\n"
    ));
    for v in 3..variants {
        s.push_str(&format!("  export Step{v}(Int, Str),\n"));
    }
    s.push_str("}\n\n");
    s.push_str(&format!("derive {} for State{m}_{n};\n\n", derive_list(p.derives)));
    s.push_str(&format!(
        "/// The match every enum in a real program has somewhere.\n\
         export fn rank{m}_{n}(state: State{m}_{n}): Int {{\n\
         \x20 match (state) {{\n\
         \x20   .Idle => 0,\n\
         \x20   .Running(ticks) => ticks + 1,\n\
         \x20   .Failed {{ code, .. }} => code * -1,\n"
    ));
    for v in 3..variants {
        s.push_str(&format!("    .Step{v}(k, _) => k + {v},\n"));
    }
    s.push_str("  }\n}\n\n");
    s.push_str(&format!(
        "/// Construction, so the variants are reachable from a root.\n\
         export fn start{m}_{n}(ticks: Int): State{m}_{n} {{\n\
         \x20 if (ticks == 0) {{ .Idle }} else if (ticks < {}) {{ .Running(ticks) }}\n\
         \x20 else {{ .Failed {{ code: ticks, message: \"overrun\" }} }}\n\
         }}\n\n",
        10 + rng.below(90)
    ));
    s.push_str(&format!(
        "/// Reaches everything above from one call. See `reach` below.\n\
         export fn probe{m}_{n}<C: Alloc>(ctx: C): Int {{\n\
         \x20 let s = start{m}_{n}(5);\n\
         \x20 rank{m}_{n}(s) + s.show(ctx).len()\n\
         }}\n\n"
    ));
}

/// The type arguments `generic_args` instantiates a generic at, past the two
/// every one of them already had.
const EXTRA_ARGS: &[(&str, &str)] = &[
    ("Float", "[1.5]"),
    ("Bool", "[true]"),
    ("Char", "['a']"),
    ("[Int]", "[[1]]"),
    ("[Str]", "[[\"a\"]]"),
    ("[Float]", "[[1.5]]"),
    ("[Bool]", "[[true]]"),
];

fn chunk_generic_fn(s: &mut String, m: usize, n: &str, p: &Params) {
    s.push_str(&format!(
        "/// A generic over an unconstrained parameter, instantiated below.\n\
         export fn firstOr{m}_{n}<T>(xs: [T], fallback: T): T {{\n\
         \x20 match (xs.first()) {{\n\
         \x20   .Some(v) => v,\n\
         \x20   .None => fallback,\n\
         \x20 }}\n\
         }}\n\n\
         /// A generic with a bound, which is the case the checker does real\n\
         /// work for: the bound has to be discharged at each instantiation.\n\
         export fn describeAll{m}_{n}<T: Show, C: Alloc>(ctx: C, xs: [T]): [Str] {{\n\
         \x20 xs.mapCtx(ctx, fn(c, x) => x.show(c))\n\
         }}\n\n"
    ));
    // Past the two below, each extra type argument is one more copy
    // monomorphization has to make at the same source size. `generic_args = 2`
    // emits nothing here and the bytes are unchanged.
    let extra = (p.generic_args.saturating_sub(2) as usize).min(EXTRA_ARGS.len());
    if extra > 0 {
        s.push_str(&format!(
            "/// One more generic, instantiated at {extra} further types below,\n\
             /// so monomorphization pays without the source growing.\n\
             export fn countOf{m}_{n}<T>(xs: [T]): Int {{\n\
             \x20 xs.len()\n\
             }}\n\n"
        ));
    }
    let more: String = EXTRA_ARGS[..extra]
        .iter()
        .map(|(ty, lit)| format!("\n    + countOf{m}_{n}<{ty}>({lit})"))
        .collect();
    s.push_str(&format!(
        "/// Two instantiations, so monomorphization has copies to make.\n\
         export fn useGeneric{m}_{n}(ns: [Int], ss: [Str]): Int {{\n\
         \x20 firstOr{m}_{n}<Int>(ns, 0) + firstOr{m}_{n}<Str>(ss, \"\").len(){more}\n\
         }}\n\n\
         /// Reaches everything above from one call. See `reach` below.\n\
         export fn probe{m}_{n}<C: Alloc>(ctx: C): Int {{\n\
         \x20 useGeneric{m}_{n}([1, 2], [\"a\"]) + describeAll{m}_{n}(ctx, [1, 2]).len()\n\
         }}\n\n"
    ));
}

fn chunk_arithmetic_fn(s: &mut String, m: usize, n: &str, p: &Params, rng: &mut Rng) {
    let lets = rng.span(p.body_lets) as usize;
    s.push_str(&format!(
        "/// A body of {lets} bindings over integer arithmetic — the shape most\n\
         /// lines in most programs actually have.\n\
         export fn compute{m}_{n}(input: Int): Int {{\n\
         \x20 let a0 = input * {} + {};\n",
        1 + rng.below(9),
        rng.below(1000)
    ));
    // `nesting = 1` is no parentheses at all, which is what the pre-parameter
    // generator emitted.
    let open = "(".repeat(p.nesting.saturating_sub(1) as usize);
    let close = ")".repeat(p.nesting.saturating_sub(1) as usize);
    for i in 1..lets {
        let op = *rng.pick(&["+", "-", "*"]);
        s.push_str(&format!(
            "  let a{i} = {open}a{} {op} {}{close};\n",
            i - 1,
            1 + rng.below(97)
        ));
    }
    s.push_str(&format!(
        "  if (a{} > {}) {{ a{} }} else {{ a{} - {} }}\n}}\n\n",
        lets - 1,
        rng.below(10_000),
        lets - 1,
        lets - 1,
        rng.below(100)
    ));
    s.push_str(&format!(
        "/// Reaches everything above from one call. See `reach` below.\n\
         export fn probe{m}_{n}<C: Alloc>(ctx: C): Int {{\n\
         \x20 let _ = ctx;\n\
         \x20 compute{m}_{n}(7)\n\
         }}\n\n"
    ));
}

fn chunk_match_fn(s: &mut String, m: usize, n: &str, p: &Params, aux: &mut Rng) {
    s.push_str(&format!(
        "/// Nested matching over `Option` and a tuple, with a guard — the\n\
         /// three things a decision tree has to merge.\n\
         export fn choose{m}_{n}(a: Option<Int>, b: Option<Int>): Int {{\n\
         \x20 match ((a, b)) {{\n\
         \x20   (.Some(x), .Some(y)) if x > y => x - y,\n\
         \x20   (.Some(x), .Some(y)) => y - x,\n\
         \x20   (.Some(x), .None) => x,\n\
         \x20   (.None, .Some(y)) => y,\n\
         \x20   (.None, .None) => 0,\n\
         \x20 }}\n\
         }}\n\n\
         /// A `?` chain, which is the other half of how errors move.\n\
         export fn parseBoth{m}_{n}(left: Str, right: Str): Option<Int> {{\n\
         \x20 let a = left.toInt()?;\n\
         \x20 let b = right.toInt()?;\n\
         \x20 .Some(a + b)\n\
         }}\n\n"
    ));
    // `match_arms = 0..0` emits nothing and takes no draw — not even from the
    // auxiliary stream, so that the corpus is identical under any future
    // change to the ones that do.
    let arms = if p.match_arms.1 == 0 { 0 } else { aux.span(p.match_arms) as usize };
    let bucket = if arms == 0 {
        String::new()
    } else {
        let mut b = format!(
            "/// A flat integer `match` of {arms} arms, which is what a decision\n\
             /// tree is actually built over in most programs.\n\
             export fn bucket{m}_{n}(v: Int): Int {{\n\
             \x20 match (v) {{\n"
        );
        for a in 0..arms {
            b.push_str(&format!("    {a} => {},\n", a * 3 + 1));
        }
        b.push_str("    _ => 0,\n  }\n}\n\n");
        b
    };
    s.push_str(&bucket);
    let extra = if arms == 0 { String::new() } else { format!(" + bucket{m}_{n}(3)") };
    s.push_str(&format!(
        "/// Reaches everything above from one call. See `reach` below.\n\
         export fn probe{m}_{n}<C: Alloc>(ctx: C): Int {{\n\
         \x20 let _ = ctx;\n\
         \x20 let extra = match (parseBoth{m}_{n}(\"1\", \"2\")) {{\n\
         \x20   .Some(v) => v,\n\
         \x20   .None => 0,\n\
         \x20 }};\n\
         \x20 choose{m}_{n}(.Some(3), .None) + extra{extra}\n\
         }}\n\n"
    ));
}

fn chunk_string_fn(s: &mut String, m: usize, n: &str, rng: &mut Rng) {
    let words = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel"];
    let w0 = rng.pick(&words);
    let w1 = rng.pick(&words);
    s.push_str(&format!(
        "/// String building through interpolation, which is a template\n\
         /// literal in the lexer and a `format` call in the backend.\n\
         export fn label{m}_{n}<C: Alloc>(ctx: C, id: Int, name: Str): Str {{\n\
         \x20 let prefix = if (id < 0) {{ \"{w0}\" }} else {{ \"{w1}\" }};\n\
         \x20 let body = str.format(ctx, \"${{prefix}}-${{name}}-${{id}}\");\n\
         \x20 body.toUpper(ctx)\n\
         }}\n\n\
         /// Literals of the kinds the lexer has separate paths for: decimal,\n\
         /// hexadecimal, float, char, escaped string.\n\
         export fn constants{m}_{n}(): (Int, Int, Float, Char, Str) {{\n\
         \x20 ({}, 0x{:04X}, {}.{:03}, 'q', \"a\\ttab and a \\\"quote\\\"\")\n\
         }}\n\n\
         /// Reaches everything above from one call. See `reach` below.\n\
         export fn probe{m}_{n}<C: Alloc>(ctx: C): Int {{\n\
         \x20 let (a, b, _, _, e) = constants{m}_{n}();\n\
         \x20 label{m}_{n}(ctx, a, e).len() + b\n\
         }}\n\n",
        rng.below(1_000_000),
        rng.below(0xFFFF),
        rng.below(1000),
        rng.below(1000)
    ));
}

fn chunk_list_fn(s: &mut String, m: usize, n: &str, rng: &mut Rng) {
    let len = 4 + rng.below(8) as usize;
    let items: Vec<String> = (0..len).map(|_| rng.below(10_000).to_string()).collect();
    s.push_str(&format!(
        "/// A fold and a filter over a literal list, with lambdas — closures\n\
         /// are their own path in both the checker and the backend.\n\
         export fn digest{m}_{n}<C: Alloc>(ctx: C): Int {{\n\
         \x20 let xs: [Int] = [{}];\n\
         \x20 let kept = xs.filter(ctx, fn(x) => x % 2 == 0);\n\
         \x20 let total = kept.fold(fn(acc, x) => acc + x, 0);\n\
         \x20 total + xs.len()\n\
         }}\n\n\
         /// An empty list of an explicit type, which is the other side of\n\
         /// inference: nothing constrains the element type but the annotation.\n\
         export fn drain{m}_{n}(): Int {{\n\
         \x20 let empty: [Int] = list.empty<Int>();\n\
         \x20 empty.len()\n\
         }}\n\n\
         /// Reaches everything above from one call. See `reach` below.\n\
         export fn probe{m}_{n}<C: Alloc>(ctx: C): Int {{\n\
         \x20 digest{m}_{n}(ctx) + drain{m}_{n}()\n\
         }}\n\n",
        items.join(", ")
    ));
}

/// The entry point, calling into every module so that nothing the generator
/// emitted is dead from monomorphization's point of view.
fn main_module(count: usize, p: &Params) -> String {
    let mut s = String::from(
        "//! The corpus entry point.\n\
         //!\n\
         //! It calls one exported function from every generated module, so that\n\
         //! monomorphization reaches the whole program from `main` and the\n\
         //! lowering benchmark is not measuring dead-code elimination.\n\n",
    );
    s.push_str("from \"core/cap\" import { Alloc };\n");
    s.push_str("from \"core/host\" import * as host;\n");
    for i in 0..count {
        s.push_str(&format!("from \"//bench/m{i:04}\" import {{ blend{i}, reach{i} }};\n"));
    }
    // `reach` is what makes the lowering benchmark honest: it pulls every
    // declaration of every module into the reachable set, so monomorphization
    // has to visit the whole corpus rather than the handful of functions a
    // `blend`-only entry point would leave alive.
    s.push_str(
        "\n/// The entry point. `main` is the only module that may import\n\
         /// `core/host`, so it is the only place the context can be built.\n\
         export fn main(): Result<(), Str> {\n\
         \x20 let ctx = context { Alloc: host.alloc };\n\
         \x20 let total = 0\n",
    );
    for i in 0..count {
        if p.reach {
            s.push_str(&format!("    + blend{i}({}) + reach{i}(ctx)\n", i + 1));
        } else {
            s.push_str(&format!("    + blend{i}({})\n", i + 1));
        }
    }
    s.push_str("    ;\n  if (total == 0) { .Err(\"empty\") } else { .Ok(()) }\n}\n");
    s
}

// ---------------------------------------------------------------------------
// Surface dimensions
// ---------------------------------------------------------------------------

/// Comment density and blank lines, applied to a finished module.
///
/// Deliberately a post-pass rather than a set of conditionals threaded through
/// every `format!` above: at the defaults it is the identity function, which is
/// checkable by inspection, and the alternative is thirty edits to the strings
/// that `design/PERFORMANCE.md` §6's numbers were taken over.
///
/// Blank lines are generated *and* excluded from the denominator, which is the
/// conservative combination: they cost the lexer real work and they never
/// flatter a rate.
fn surface(text: String, p: &Params) -> String {
    if p.doc_comment_pct >= 100 && p.comment_block_lines == 0 && p.blank_pct == 0 {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut since_blank = 0usize;
    // A blank every `stride` lines. `blank_pct` is capped at 90 by `set`, so
    // the stride is at least 1.
    let stride = if p.blank_pct == 0 { 0 } else { (100 / p.blank_pct.max(1)).max(1) as usize };
    for line in text.lines() {
        let trimmed = line.trim_start();
        let is_doc = trimmed.starts_with("///") || trimmed.starts_with("//!");
        if is_doc && p.doc_comment_pct < 100 {
            // All or nothing per line: the only two values any profile uses
            // are 0 and 100, and a fractional one drops a proportion of the
            // lines rather than pretending to model a documentation style.
            if p.doc_comment_pct == 0 {
                continue;
            }
            let keep = (line.len() as u32).wrapping_mul(2_654_435_761) % 100;
            if keep >= p.doc_comment_pct {
                continue;
            }
        }
        if p.comment_block_lines > 0 && line.starts_with("export ") {
            for k in 0..p.comment_block_lines {
                out.push_str(&format!(
                    "// Comment block line {k} of {}, for the lexer's comment path.\n",
                    p.comment_block_lines
                ));
            }
        }
        out.push_str(line);
        out.push('\n');
        if stride > 0 {
            since_blank += 1;
            if since_blank >= stride {
                out.push('\n');
                since_blank = 0;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Stress shapes
// ---------------------------------------------------------------------------

/// Expressions nested as deeply as the parser will take them.
///
/// `parser.rs` refuses an expression past `MAX_DEPTH`, which is 256, so a
/// single expression cannot be made arbitrarily large this way — and that
/// bound is itself worth knowing, because it means no input can push the
/// parser's stack past a megabyte and a half. The shape reaches the budget by
/// repeating a near-limit expression instead: [`NESTING`] deep, as many times
/// as the line target asks for.
///
/// 100 rather than 255 because one parenthesised operand costs *two* levels of
/// the guard — `expr` and `unary_expr` each call `enter` — so a hundred
/// parentheses is two hundred deep, and the margin above it is what keeps this
/// shape measuring the parser rather than its refusal.
///
/// The nesting is parenthesised addition rather than calls, so no name
/// resolution is involved and the shape isolates the grammar and the
/// checker's recursion over expressions.
const NESTING: usize = 100;

fn deep_nesting(target_lines: usize) -> Program {
    let per_fn = NESTING + 5;
    let count = (target_lines / per_fn).max(1);
    let mut s = String::from("//! Stress shape: expressions nested to the parser's limit.\n\n");
    for f in 0..count {
        s.push_str(&format!("export fn deep{f}(x: Int): Int {{\n  let v =\n"));
        for i in 0..NESTING {
            s.push_str(&format!("    ({} + \n", i % 97));
        }
        s.push_str("    x");
        for _ in 0..NESTING {
            s.push(')');
        }
        s.push_str(";\n  v\n}\n\n");
    }
    s.push_str("export fn main(): Result<(), Str> {\n  let total = 0\n");
    for f in 0..count {
        s.push_str(&format!("    + deep{f}(1)\n"));
    }
    s.push_str("    ;\n  if (total == 0) { .Err(\"zero\") } else { .Ok(()) }\n}\n");
    Program { modules: vec![Module { path: "//bench/main".to_string(), text: s }] }
}

/// One enum with thousands of variants and one match covering all of them.
///
/// Exhaustiveness checking and decision-tree construction are the two passes
/// with a plausible quadratic term in the arm count, and this is the shape
/// that would expose it.
fn wide_match(target_lines: usize) -> Program {
    let arms = target_lines.saturating_sub(10) / 2;
    let mut s = String::from("//! Stress shape: one very wide enum and one very wide match.\n\n");
    s.push_str("export enum Wide {\n");
    for i in 0..arms {
        s.push_str(&format!("  export V{i}(Int),\n"));
    }
    s.push_str("}\n\nexport fn classify(w: Wide): Int {\n  match (w) {\n");
    for i in 0..arms {
        s.push_str(&format!("    .V{i}(n) => n + {i},\n"));
    }
    s.push_str("  }\n}\n\nexport fn main(): Result<(), Str> {\n");
    s.push_str("  if (classify(.V0(1)) == 0) { .Err(\"zero\") } else { .Ok(()) }\n}\n");
    Program { modules: vec![Module { path: "//bench/main".to_string(), text: s }] }
}

/// Thousands of two-line functions, spread over modules the way real code
/// would be. Per-item overhead is the whole cost.
fn many_small(target_lines: usize) -> Program {
    // One declaration line each, plus the one line in `all` that calls it.
    let per_fn = 2;
    let total = target_lines / per_fn;
    let per_module = 400usize;
    let module_count = total.div_ceil(per_module).max(1);
    let mut modules = Vec::new();
    let mut made = 0usize;
    for m in 0..module_count {
        let mut s = format!("//! Stress shape: many small functions, module {m}.\n\n");
        let here = per_module.min(total - made);
        for i in 0..here {
            let g = made + i;
            s.push_str(&format!("export fn tiny{g}(x: Int): Int {{ x + {} }}\n\n", g % 251));
        }
        // One call per function, so none of them is dead: a `main` naming one
        // function per module would leave the other 399 out of the reachable
        // set and the lowering row would be measuring nothing.
        s.push_str(&format!("export fn all{m}(x: Int): Int {{\n  0\n"));
        for i in 0..here {
            s.push_str(&format!("    + tiny{}(x)\n", made + i));
        }
        s.push_str("}\n");
        made += here;
        modules.push(Module { path: format!("//bench/m{m:04}"), text: s });
    }
    let mut main = String::from("//! Stress shape entry point.\n\n");
    for m in 0..module_count {
        main.push_str(&format!("from \"//bench/m{m:04}\" import {{ all{m} }};\n"));
    }
    main.push_str("\nexport fn main(): Result<(), Str> {\n  let total = 0\n");
    for m in 0..module_count {
        main.push_str(&format!("    + all{m}(1)\n"));
    }
    main.push_str("    ;\n  if (total == 0) { .Err(\"zero\") } else { .Ok(()) }\n}\n");
    modules.push(Module { path: "//bench/main".to_string(), text: main });
    Program { modules }
}

/// A few functions of a thousand lines each: one long chain of `let` bindings
/// per function. Per-body cost is the whole cost.
fn few_large(target_lines: usize) -> Program {
    let body = 1_000usize;
    let count = (target_lines / body).max(1);
    let mut s = String::from("//! Stress shape: few large functions.\n\n");
    for f in 0..count {
        s.push_str(&format!("export fn large{f}(x: Int): Int {{\n  let v0 = x + 1;\n"));
        for i in 1..body {
            s.push_str(&format!("  let v{i} = v{} + {};\n", i - 1, i % 89));
        }
        s.push_str(&format!("  v{}\n}}\n\n", body - 1));
    }
    s.push_str("export fn main(): Result<(), Str> {\n  let total = 0\n");
    for f in 0..count {
        s.push_str(&format!("    + large{f}(1)\n"));
    }
    s.push_str("    ;\n  if (total == 0) { .Err(\"zero\") } else { .Ok(()) }\n}\n");
    Program { modules: vec![Module { path: "//bench/main".to_string(), text: s }] }
}
