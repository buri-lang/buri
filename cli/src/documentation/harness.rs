//! Shared preambles for documentation examples.
//!
//! A document often shows three examples that all need the same `Shape` enum.
//! Repeating the declaration in every fence would clutter the prose; hiding it
//! with `# ` in every fence would duplicate it three times. A harness is the
//! third option: real Buri, in a real file, spliced ahead of any block whose
//! fence says `use=shapes`.
//!
//! These files are part of the parse-and-format corpus (`cli/tests/language/corpus.rs`
//! walks this directory), so a harness that stops compiling — or stops being
//! formatted the way `buri format` formats it — fails the build like any other
//! source.
//!
//! Adding one is two steps: write the file, add the line here.

pub const HARNESSES: &[(&str, &str)] = &[
    ("shapes", include_str!("../docs/harness/shapes.buri")),
    ("money", include_str!("../docs/harness/money.buri")),
];

/// The preamble text for a name, if there is one.
pub fn source(name: &str) -> Option<&'static str> {
    HARNESSES.iter().find(|(n, _)| *n == name).map(|(_, text)| *text)
}
