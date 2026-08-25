//! Replays the Lean formalisation's exhaustiveness verdicts against the real
//! checker.
//!
//! `formal/` mechanises `semantics/exhaustiveness.rs` — the usefulness
//! algorithm, the array-length bound, the `expand`/`expand_lengths` pre-passes
//! — and proves that an accepted `match` really does cover its scrutinee's
//! type. A proof about a *model* is only worth what the model's fidelity is
//! worth, and nothing but a test keeps the two from drifting apart.
//!
//! So the Lean side enumerates `match` statements, runs its own algorithm on
//! each, and writes `(program, verdict)` pairs to
//! `formal/vectors/exhaustiveness.txt`. This test compiles every one of them
//! through the ordinary driver and asserts the two agree — on exhaustiveness
//! *and* on which arms are unreachable.
//!
//! The vectors are checked in, so running the Rust suite never needs Lean.
//! Regenerate them with:
//!
//! ```sh
//! cd formal && lake env lean --run Vectors.lean
//! ```
//!
//! ## What this does and does not catch
//!
//! It compares verdicts, not diagnostics: a disagreement means the Lean model
//! and the Rust checker compute different answers, which invalidates the
//! proofs. It says nothing about *which* case a diagnostic names — the reject
//! corpus's exact-output goldens are the right tool for that, and they exist.
//!
//! It goes through `driver::analyze_snippet`, the same entry point the
//! documentation harness uses, so every vector also exercises the parser, name
//! resolution and the type checker on its way to the exhaustiveness pass. A
//! vector that fails to compile for any *other* reason is a failure here too:
//! it means the Lean pattern pool and the Buri surface syntax have drifted.
use std::path::{Path, PathBuf};

use buri::compiler::driver;
use buri::compiler::modules::Role;
use buri::diagnostics::SourceMap;

/// One enumerated `match`, with what the Lean model said about it.
struct Vector {
    id: u32,
    exhaustive: bool,
    unreachable: Vec<usize>,
    scrutinee: String,
    arms: Vec<String>,
}

/// How many vectors share one compiled module. The standard library is loaded
/// once per `analyze_snippet` call and dominates the cost, so batching is what
/// keeps this test under a couple of seconds.
const BATCH: usize = 64;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <root>/cli.
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn load() -> (Vec<String>, Vec<Vector>) {
    let path = repo_root().join("formal/vectors/exhaustiveness.txt");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    let mut prelude = Vec::new();
    let mut vectors = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields[0] == "P" {
            prelude.push(fields[1].to_string());
            continue;
        }
        assert!(
            fields.len() >= 5,
            "{}:{}: a vector needs id, verdict, unreachable, type and at least one arm",
            path.display(),
            lineno + 1
        );
        let id: u32 = fields[0].parse().expect("vector id");
        let exhaustive = match fields[1] {
            "E" => true,
            "N" => false,
            other => panic!("{}:{}: verdict must be E or N, found {other:?}", path.display(), lineno + 1),
        };
        let unreachable = if fields[2] == "-" {
            Vec::new()
        } else {
            fields[2].split(',').map(|n| n.parse().expect("arm index")).collect()
        };
        vectors.push(Vector {
            id,
            exhaustive,
            unreachable,
            scrutinee: fields[3].to_string(),
            arms: fields[4..].iter().map(|s| s.to_string()).collect(),
        });
    }
    (prelude, vectors)
}

/// Where each vector, and each of its arms, lives in the assembled module.
struct Layout {
    id: u32,
    /// Byte range of the whole function.
    body: std::ops::Range<usize>,
    /// Byte range of each arm's line, in arm order.
    arms: Vec<std::ops::Range<usize>>,
}

/// One module holding `BATCH` vectors, each as its own function. Every arm sits
/// on a line of its own, which is what makes a diagnostic's span attributable
/// to an arm.
fn assemble(prelude: &[String], batch: &[Vector]) -> (String, Vec<Layout>) {
    let mut src = String::new();
    for line in prelude {
        src.push_str(line);
        src.push('\n');
    }
    let mut layouts = Vec::new();
    for v in batch {
        src.push('\n');
        let start = src.len();
        src.push_str(&format!("fn v{}(x: {}): Int {{\n  match (x) {{\n", v.id, v.scrutinee));
        let mut arms = Vec::new();
        for arm in &v.arms {
            let arm_start = src.len();
            src.push_str(&format!("    {arm} => 0,\n"));
            arms.push(arm_start..src.len());
        }
        src.push_str("  }\n}\n");
        layouts.push(Layout { id: v.id, body: start..src.len(), arms });
    }
    (src, layouts)
}

/// What the Rust checker said about one vector.
#[derive(Default)]
struct Observed {
    exhaustive: bool,
    unreachable: Vec<usize>,
}

fn check_batch(prelude: &[String], batch: &[Vector]) -> Vec<(u32, Observed)> {
    let (src, layouts) = assemble(prelude, batch);
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "lean_vectors.buri", &src, Role::Source);

    let mut observed: Vec<(u32, Observed)> = layouts
        .iter()
        .map(|l| (l.id, Observed { exhaustive: true, unreachable: Vec::new() }))
        .collect();

    for d in &analysis.diagnostics.items {
        let at = d.span.start as usize;
        let which = layouts.iter().position(|l| l.body.contains(&at));
        match d.code.as_deref() {
            Some("match-not-exhaustive") => {
                let i = which.unwrap_or_else(|| {
                    panic!("a `match-not-exhaustive` at byte {at} belongs to no vector")
                });
                observed[i].1.exhaustive = false;
            }
            Some("unreachable-arm") => {
                let i = which.unwrap_or_else(|| {
                    panic!("an `unreachable-arm` at byte {at} belongs to no vector")
                });
                let arm = layouts[i]
                    .arms
                    .iter()
                    .position(|r| r.contains(&at))
                    .unwrap_or_else(|| panic!("an `unreachable-arm` at byte {at} belongs to no arm"));
                observed[i].1.unreachable.push(arm);
            }
            other => panic!(
                "unexpected diagnostic {:?} while compiling the vector corpus: {}\n\
                 this means the Lean pattern pool and the Buri surface syntax have drifted.\n\
                 source:\n{src}",
                other, d.message
            ),
        }
    }
    for (_, o) in observed.iter_mut() {
        o.unreachable.sort_unstable();
    }
    observed
}

#[test]
fn lean_and_rust_agree_on_exhaustiveness() {
    let (prelude, vectors) = load();
    assert!(!prelude.is_empty(), "the vector file carries no prelude");
    assert!(
        vectors.len() > 500,
        "expected the enumerated corpus, found {} vectors",
        vectors.len()
    );

    let mut failures = Vec::new();
    for batch in vectors.chunks(BATCH) {
        for (id, observed) in check_batch(&prelude, batch) {
            let v = batch.iter().find(|v| v.id == id).unwrap();
            let arms = v.arms.join(" | ");
            if observed.exhaustive != v.exhaustive {
                failures.push(format!(
                    "v{id} ({}: {arms}): Lean says {}, the checker says {}",
                    v.scrutinee,
                    if v.exhaustive { "exhaustive" } else { "not exhaustive" },
                    if observed.exhaustive { "exhaustive" } else { "not exhaustive" },
                ));
            }
            if observed.unreachable != v.unreachable {
                failures.push(format!(
                    "v{id} ({}: {arms}): Lean says arms {:?} are unreachable, the checker says {:?}",
                    v.scrutinee, v.unreachable, observed.unreachable
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "the Lean model and the checker disagree on {} of {} vectors:\n{}",
        failures.len(),
        vectors.len(),
        failures.join("\n")
    );
}

/// The formal analogue of `language/conformance.rs`'s canary: agreement is evidence only
/// if disagreement would be *visible*. This drives two `match` statements whose
/// answers are not in doubt through the same observation path the comparison
/// uses — compile, read the diagnostics, attribute them by byte range — and
/// asserts it sees them.
#[test]
fn the_bridge_can_detect_a_disagreement() {
    let (prelude, _) = load();

    let missing = Vector {
        id: 999_001,
        exhaustive: true, // the claim, deliberately wrong
        unreachable: Vec::new(),
        scrutinee: "Color".to_string(),
        arms: vec![".Red".to_string()],
    };
    let seen = check_batch(&prelude, std::slice::from_ref(&missing));
    assert!(
        !seen[0].1.exhaustive,
        "a `match` on `Color` covering only `.Red` was not reported non-exhaustive; \
         the bridge would accept any claim about exhaustiveness"
    );

    let shadowed = Vector {
        id: 999_002,
        exhaustive: true,
        unreachable: Vec::new(), // the claim, deliberately wrong
        scrutinee: "Color".to_string(),
        arms: vec!["_".to_string(), ".Red".to_string()],
    };
    let seen = check_batch(&prelude, std::slice::from_ref(&shadowed));
    assert_eq!(
        seen[0].1.unreachable,
        vec![1],
        "an arm shadowed by a preceding `_` was not attributed to arm 1; \
         the bridge would accept any claim about reachability"
    );
}

/// The corpus is only evidence if it contains the cases that used to be wrong.
/// `formal/findings/README.md` 6 is a nested alternation the checker rejected;
/// this asserts the corpus still covers that shape, so a future edit to the
/// pattern pool cannot quietly drop it.
#[test]
fn the_corpus_covers_the_nested_alternation() {
    let (_, vectors) = load();
    let nested: Vec<&Vector> =
        vectors.iter().filter(|v| v.arms.iter().any(|a| a.contains("(true | false)"))).collect();
    assert!(
        nested.len() > 20,
        "expected the nested-alternation vectors, found {}",
        nested.len()
    );
    assert!(
        nested.iter().any(|v| v.exhaustive && v.arms.len() > 1),
        "expected at least one exhaustive match built from a nested alternation"
    );
}
