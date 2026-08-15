//! Regenerating the documents a reader meets on GitHub.
//!
//! `SPEC.md` and `README.md` are not edited. They are assembled from the
//! topics in `doc_topics`, so there is exactly one copy of every sentence: the
//! one `buri docs` serves. `buri docs assemble --check` fails when the
//! checked-in file has drifted from what the topics produce, which is the same
//! shape as `buri format --check` and `buri gen --check`.
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

use crate::doc_topics::{self, Topic};

pub struct Section {
    /// The section number as written in the topic's own heading. Pinned so a
    /// renumber is a deliberate, reviewable edit.
    pub number: &'static str,
    pub topic: &'static str,
}

const fn sec(number: &'static str, topic: &'static str) -> Section {
    Section { number, topic }
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
        path: "SPEC.md",
        front: doc_topics::LANG_FRONT,
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
        front: doc_topics::GUIDE_FRONT,
        // The guide's sections are prose, not a numbered specification, so
        // there is nothing to pin; the empty number says so.
        sections: &[
            sec("", "guide/goals"),
            sec("", "guide/three-ideas"),
            sec("", "guide/numbers"),
            sec("", "guide/methods-and-traits"),
            sec("", "guide/restricting-effects"),
            sec("", "guide/errors"),
            sec("", "guide/imports"),
            sec("", "guide/whats-in"),
            sec("", "guide/status"),
            sec("", "guide/naming"),
        ],
    },
];

/// The text a document's sections produce.
pub fn assemble(doc: &Document) -> String {
    let mut out = String::with_capacity(64 * 1024);
    out.push_str(doc.front);
    for s in doc.sections {
        let topic = doc_topics::find(s.topic)
            .unwrap_or_else(|| panic!("`{}` names topic `{}`, which does not exist", doc.path, s.topic));
        out.push('\n');
        out.push_str(topic.text);
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
    use crate::doc_md;

    /// A topic's heading must carry the number `DOCUMENTS` pins for it. This
    /// is what protects the ninety-nine `SPEC N.M` citations in `cli/src/`
    /// from a silent renumber.
    #[test]
    fn every_section_number_is_pinned() {
        for doc in DOCUMENTS {
            for s in doc.sections {
                if s.number.is_empty() {
                    continue;
                }
                let topic = doc_topics::find(s.topic).unwrap();
                let headings = doc_md::headings(topic.text);
                let first = headings.first().expect("a topic has a heading");
                assert!(
                    first.title.starts_with(&format!("{}. ", s.number)),
                    "{} pins `{}` at section {}, but its heading reads `{}`",
                    doc.path,
                    s.topic,
                    s.number,
                    first.title
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

    /// Every `lang/` and `guide/` topic is reachable from an assembled
    /// document — a topic that is in neither would be served by `buri docs`
    /// and invisible on GitHub.
    #[test]
    fn no_topic_is_orphaned() {
        for t in doc_topics::TOPICS {
            let assembled = DOCUMENTS.iter().any(|d| d.sections.iter().any(|s| s.topic == t.id));
            let expected = matches!(t.kind, doc_topics::Kind::Lang | doc_topics::Kind::Guide);
            assert_eq!(
                assembled, expected,
                "`{}` is {} an assembled document but should be the other way",
                t.id,
                if assembled { "in" } else { "not in" }
            );
        }
    }
}
