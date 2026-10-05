//! Generated programs that grow a value, run on every backend, with the work
//! they do counted.
//!
//! Each file in `cli/tests/growth/` grows a list, a string, or a record holding
//! a list, through one combination of the shapes ownership bugs have come from
//! (`generator.rs` lists them). Its header pins the line it prints, which the
//! generator computed, and an upper bound on the blocks it allocates, which a
//! value grown in place stays under and a value copied on every push does not.
//!
//! The files are checked in and regenerate from [`SEED`]:
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test native growth::the_checked_in_cases_are_up_to_date
//! ```
//!
//! Cases run in batches of [`PER_BATCH`], one program each, on JavaScript for
//! the output and on every native backend built in for the output and the
//! bound, under the heap check. A batch over its bound builds each of its cases
//! alone and names the ones over theirs.

use crate::shared::{probed, ran_checked};
use std::path::PathBuf;

mod generator;

/// The seed the checked-in cases were drawn from.
const SEED: u64 = 0x6772_6f77_7468;

/// How many cases there are.
const COUNT: usize = 320;

/// Cases per program.
const PER_BATCH: usize = 20;

/// How many batch tests there are; `COUNT / PER_BATCH`, rounded up.
const BATCHES: usize = 16;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/growth")
}

fn file_name(id: usize) -> String {
    format!("case_{id:03}.buri")
}

/// Every case's file, as the generator writes it.
fn generated() -> Vec<(String, String)> {
    generator::shapes(SEED, COUNT)
        .iter()
        .enumerate()
        .map(|(id, shape)| (file_name(id), generator::source(id, shape, generator::blocks(shape))))
        .collect()
}

#[test]
fn the_checked_in_cases_are_up_to_date() {
    assert_eq!(BATCHES, COUNT.div_ceil(PER_BATCH), "`BATCHES` must cover `COUNT`");
    let want = generated();
    let dir = corpus();
    if std::env::var("BURI_BLESS").is_ok_and(|v| v == "1") {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in &want {
            std::fs::write(dir.join(name), text).unwrap();
        }
        return;
    }
    let mut have: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}; regenerate with BURI_BLESS=1", dir.display()))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    have.sort();
    let names: Vec<String> = want.iter().map(|(n, _)| n.clone()).collect();
    assert_eq!(have, names, "the files in {} are not the generator's; run it with BURI_BLESS=1", dir.display());
    for (name, text) in &want {
        let on_disk = std::fs::read_to_string(dir.join(name)).unwrap();
        assert!(
            on_disk == *text,
            "{name} differs from what the generator writes for seed {SEED:#x}; run it with BURI_BLESS=1"
        );
    }
}

/// Every pair of dimension values the generator can draw, such as a `match`
/// on a record accumulator with a helper too large to inline, is in some case.
#[test]
fn the_cases_cover_every_pair_of_shapes() {
    let (missing, total) = generator::uncovered(&generator::shapes(SEED, COUNT));
    assert!(missing.is_empty(), "{} of {total} pairs are in no case: {missing:?}", missing.len());
}

/// One checked-in case: its declarations, the line it prints, and its bound.
struct Case {
    name: String,
    id: usize,
    decls: String,
    expect: String,
    blocks: u64,
}

fn read_case(id: usize) -> Case {
    let name = file_name(id);
    let text = std::fs::read_to_string(corpus().join(&name))
        .unwrap_or_else(|e| panic!("{name}: {e}; regenerate with BURI_BLESS=1"));
    parse_case(id, name, &text)
}

fn parse_case(id: usize, name: String, text: &str) -> Case {
    let header = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .unwrap_or_else(|| panic!("{name} has no `{key}` line"))
            .to_string()
    };
    let expect = header("// expect: ");
    let blocks = header("// blocks: ").parse().unwrap();
    // Everything between the imports and `main`.
    let imports = text
        .rfind("\nfrom ")
        .and_then(|at| text[at + 1..].find('\n').map(|end| at + 1 + end))
        .unwrap_or_else(|| panic!("{name} has no imports"));
    let main = text.find(generator::MAIN_START).unwrap_or_else(|| panic!("{name} has no `main`"));
    let decls = text[imports..main].to_string();
    Case { name, id, decls, expect, blocks }
}

/// The cases as one program printing a line each, and the lines it prints.
fn program(cases: &[Case]) -> (String, String) {
    let mut source = String::from(generator::IMPORTS);
    for case in cases {
        source.push('\n');
        source.push_str(&case.decls);
    }
    source.push_str(generator::MAIN);
    source.push('\n');
    for case in cases {
        source.push_str(&format!(
            "  let _ = io.println(host.stdout, g{:03}(host.alloc)).ignore();\n",
            case.id
        ));
    }
    source.push_str("  .Ok(())\n}\n");
    let expected = cases.iter().map(|c| format!("{}\n", c.expect)).collect();
    (source, expected)
}

/// The blocks a program of these cases may allocate: each case's own bound,
/// and what printing a line per case costs.
fn bound(cases: &[Case]) -> u64 {
    cases.iter().map(|c| c.blocks).sum::<u64>() + generator::BATCH_BLOCKS
}

fn batch(index: usize) {
    let cases: Vec<Case> =
        (index * PER_BATCH..((index + 1) * PER_BATCH).min(COUNT)).map(read_case).collect();
    let name = format!("growth-{index:02}");
    let (source, expected) = program(&cases);

    if let Some((status, stdout, stderr)) = crate::agreement::javascript(&name, &source) {
        assert_eq!(status, 0, "{name} on JavaScript exited {status}: {stderr}");
        assert_lines(&name, "javascript", &cases, &stdout, &expected);
    }

    for (backend, build) in crate::e2e::probed_backends() {
        let r = ran_checked(&build(&name, &source));
        if let Some(n) = crate::shared::leaked_blocks(r.status, &r.stderr) {
            panic!("{name} on {backend} leaked {n} block(s)");
        }
        assert_eq!(r.status, 0, "{name} on {backend} exited {}: {}", r.status, r.stderr);
        assert_lines(&name, backend, &cases, &r.stdout, &expected);
        let (blocks, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{name} on {backend}: {live} blocks still live at exit");
        if blocks > bound(&cases) {
            let over: Vec<String> = cases
                .iter()
                .filter_map(|case| {
                    let (alone, _) = program(std::slice::from_ref(case));
                    let r = ran_checked(&build(&format!("growth-{}", case.id), &alone));
                    let (blocks, _) = probed(&r.stderr);
                    let allowed = case.blocks + generator::BATCH_BLOCKS;
                    (blocks > allowed).then(|| format!("{} allocated {blocks}, at most {allowed}", case.name))
                })
                .collect();
            panic!(
                "{name} on {backend} allocated {blocks} blocks, over its bound of {}: a value \
                 copied where it should have grown in place.\n{}",
                bound(&cases),
                over.join("\n")
            );
        }
    }
}

/// Each line against its case, so a failure names the file.
fn assert_lines(name: &str, backend: &str, cases: &[Case], stdout: &str, expected: &str) {
    if stdout == expected {
        return;
    }
    let got: Vec<&str> = stdout.lines().collect();
    for (at, case) in cases.iter().enumerate() {
        let line = got.get(at).copied().unwrap_or("<nothing>");
        assert_eq!(line, case.expect, "{name} on {backend}: {} printed something else", case.name);
    }
    panic!("{name} on {backend} printed more than its cases:\n{stdout}");
}

macro_rules! batches {
    ($($test:ident = $index:expr,)*) => {
        $(
            #[test]
            fn $test() {
                batch($index);
            }
        )*
    };
}

batches! {
    batch_00 = 0,
    batch_01 = 1,
    batch_02 = 2,
    batch_03 = 3,
    batch_04 = 4,
    batch_05 = 5,
    batch_06 = 6,
    batch_07 = 7,
    batch_08 = 8,
    batch_09 = 9,
    batch_10 = 10,
    batch_11 = 11,
    batch_12 = 12,
    batch_13 = 13,
    batch_14 = 14,
    batch_15 = 15,
}
