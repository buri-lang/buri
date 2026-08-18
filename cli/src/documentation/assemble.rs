//! Regenerating the documents a reader meets on GitHub.
//!
//! `cli/src/docs/SPEC.md` and the top-level `README.md` are not edited. They
//! are assembled from the topics in `doc_topics`, so there is exactly one copy
//! of every sentence: the one `buri docs` serves. `buri docs assemble --check`
//! fails when the checked-in file has drifted from what the topics produce,
//! which is the same shape as `buri format --check` and `buri gen --check`.
//!
//! The assembled specification lives beside the topics it is made of rather
//! than at the repository root: one directory is the documentation, and a
//! generated copy two levels above the sources it came from is how a reader
//! ends up editing the wrong file. `README.md` stays at the root because
//! GitHub renders it there and nowhere else, and it is deliberately small —
//! an introduction, how to install, and where the documentation actually is.
//!
//! **Section numbers are written down, not computed.** Ninety-nine comments in
//! `cli/src/` cite `SPEC 10.5` and friends. If assembly renumbered positionally,
//! inserting a section would silently invalidate all of them. Instead each
//! entry in `DOCUMENTS` pins its number, a topic file carries that number in
//! its own heading, and `every_section_number_is_pinned` checks the two agree.
//!
//! Assembly is concatenation. A topic file is a verbatim slice of the document
//! it came from — same heading level, same numbering, same trailing separator —
//! so the round trip is byte-for-byte and the drift test cannot be fooled by a
//! renderer that happens to normalize.

use crate::diagnostics::Invariant as _;
use crate::documentation::topics::{self, Topic};

pub struct Section {
    /// The section number as written in the topic's own heading, pinned so a
    /// renumber is a deliberate, reviewable edit — or `None` for a document
    /// whose sections are prose and carry no numbers. That used to be an empty
    /// string, which reads as "section number zero-length" and had to be
    /// tested for at every use.
    pub number: Option<&'static str>,
    pub topic: &'static str,
}

impl Section {
    /// The topic this section is. `every_section_names_a_topic` is what makes
    /// the failure unreachable; without it a typo in the table below would fail
    /// somewhere inside `buri docs assemble` instead of in the test suite.
    pub fn topic(&self) -> &'static Topic {
        topics::find(self.topic).or_ice(&format!(
            "`{}` is named by the `DOCUMENTS` table, which `every_section_names_a_topic` \
             holds against the topic list",
            self.topic
        ))
    }
}

const fn sec(number: &'static str, topic: &'static str) -> Section {
    Section { number: Some(number), topic }
}

/// A section of a document that is prose rather than a numbered specification.
const fn prose(topic: &'static str) -> Section {
    Section { number: None, topic }
}

pub struct Document {
    /// Where the assembled file lives, relative to the repository root.
    pub path: &'static str,
    /// Whatever precedes the first section: title, version line, separator.
    pub front: &'static str,
    pub sections: &'static [Section],
}

pub const DOCUMENTS: &[Document] = &[
    Document {
        path: "cli/src/docs/SPEC.md",
        front: topics::LANG_FRONT,
        sections: &[
            sec("1", "lang/introduction"),
            sec("2", "lang/notation"),
            sec("3", "lang/lexical"),
            sec("4", "lang/modules"),
            sec("5", "lang/types"),
            sec("6", "lang/expressions"),
            sec("7", "lang/patterns"),
            sec("8", "lang/evaluation"),
            sec("9", "lang/functions"),
            sec("10", "lang/effects"),
            sec("11", "lang/programs"),
            sec("12", "lang/grammar-rationale"),
            sec("13", "lang/invariants"),
            sec("14", "lang/static-rules"),
            sec("15", "lang/open-questions"),
        ],
    },
    Document {
        path: "README.md",
        front: topics::GUIDE_FRONT,
        // The guide's sections are prose, not a numbered specification, so
        // there is nothing to pin.
        //
        // Three sections and no more. The whole guide used to be dumped here,
        // which made the front page of the repository a forty-minute read and
        // put a second copy of the language tour a click away from the first.
        // What a reader wants from a README is what this is, how to get it,
        // and where the documentation lives; the rest is `buri docs` and the
        // files under `cli/src/docs/`, which is where it was being edited
        // anyway.
        sections: &[
            prose("guide/readme-intro"),
            prose("guide/installing"),
            prose("guide/readme-links"),
        ],
    },
];

/// The text a document's sections produce.
pub fn assemble(doc: &Document) -> String {
    let mut out = String::with_capacity(64 * 1024);
    out.push_str(doc.front);
    for s in doc.sections {
        out.push('\n');
        out.push_str(s.topic().text);
    }
    out
}

/// Each document whose checked-in file differs from its assembly, with what it
/// should contain.
pub fn drifted(root: &std::path::Path) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for doc in DOCUMENTS {
        let want = assemble(doc);
        let have = std::fs::read_to_string(root.join(doc.path)).unwrap_or_default();
        if have != want {
            out.push((doc.path, want));
        }
    }
    out
}

/// Writes every assembled document. Returns the paths that changed.
pub fn write(root: &std::path::Path) -> std::io::Result<Vec<&'static str>> {
    let mut changed = Vec::new();
    for (path, text) in drifted(root) {
        std::fs::write(root.join(path), text)?;
        changed.push(path);
    }
    Ok(changed)
}

/// The topics an assembled document is built from, for `buri docs` to say so.
pub fn document_of(topic: &Topic) -> Option<&'static Document> {
    DOCUMENTS.iter().find(|d| d.sections.iter().any(|s| s.topic == topic.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documentation::markdown;

    /// A topic's heading must carry the number `DOCUMENTS` pins for it. This
    /// is what protects the ninety-nine `SPEC N.M` citations in `cli/src/`
    /// from a silent renumber.
    #[test]
    fn every_section_number_is_pinned() {
        for doc in DOCUMENTS {
            for s in doc.sections {
                let Some(number) = s.number else { continue };
                let headings = markdown::headings(s.topic().text);
                let first = headings.first().expect("a topic has a heading");
                assert!(
                    first.title.starts_with(&format!("{number}. ")),
                    "{} pins `{}` at section {number}, but its heading reads `{}`",
                    doc.path,
                    s.topic,
                    first.title
                );
            }
        }
    }

    /// What makes `Section::topic` total in practice. Two other tests catch a
    /// typo indirectly — the real topic goes orphaned — but only this one says
    /// which entry is wrong.
    #[test]
    fn every_section_names_a_topic() {
        for doc in DOCUMENTS {
            for s in doc.sections {
                assert!(
                    topics::find(s.topic).is_some(),
                    "{} names `{}`, which is not a topic",
                    doc.path,
                    s.topic
                );
            }
        }
    }

    #[test]
    fn every_topic_belongs_to_at_most_one_document() {
        let mut seen = std::collections::HashSet::new();
        for doc in DOCUMENTS {
            for s in doc.sections {
                assert!(
                    seen.insert(s.topic),
                    "`{}` is assembled into two documents",
                    s.topic
                );
            }
        }
    }

    /// Every `lang/` topic is a section of the specification, and nothing else
    /// is. A `lang/` topic left out would be served by `buri docs` and missing
    /// from the document that is supposed to be the whole language.
    ///
    /// `guide/` is deliberately not held to this. Guide topics are pages first;
    /// three of them happen to also be the README, and the rest are read
    /// through `buri docs` or in `cli/src/docs/guide/`. A `build/` topic is
    /// never assembled — the build system's pages are the files themselves.
    #[test]
    fn only_lang_topics_are_specification_sections() {
        for t in topics::TOPICS {
            let assembled = DOCUMENTS.iter().any(|d| d.sections.iter().any(|s| s.topic == t.id));
            match t.kind {
                topics::Kind::Lang => {
                    assert!(assembled, "`{}` is a language topic but is in no document", t.id);
                }
                topics::Kind::Build => {
                    assert!(!assembled, "`{}` is a build topic and must not be assembled", t.id);
                }
                topics::Kind::Guide => {}
            }
        }
    }
}
