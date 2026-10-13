//! What the documents' tests scan for and the toolchain doesn't: headings and
//! their anchors, inline links, and unclosed fences. Each skips fenced blocks
//! the way `markdown::fences` finds them, so they can't disagree about where a
//! block is.
use buri::documentation::markdown;

/// Where a link points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    /// `[text](#anchor)`: a heading in this document.
    SameDocument { anchor: String },
    /// `[text](path)` or `[text](path#anchor)`.
    File {
        path: String,
        anchor: Option<String>,
    },
    /// `http://` or `https://`. Outside the corpus, so nothing resolves it.
    External { url: String },
    /// `[text]()`, reported so the link checker can say so.
    Nowhere,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub text: String,
    pub destination: Destination,
    pub line: usize,
}

/// The 1-based line of the fence that's never closed, if there is one. Only
/// the last fence can be, and its body then runs to the end of the text.
pub fn unterminated_fence(text: &str) -> Option<usize> {
    let last = markdown::fences(text).pop()?;
    let closer = last.body_line - 1 + last.body.lines().count();
    text.lines().nth(closer).is_none().then_some(last.line)
}

/// Every line outside a fenced block, with its 1-based number.
fn prose(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let fenced: Vec<(usize, usize)> = markdown::fences(text)
        .iter()
        .map(|f| (f.line, f.body_line + f.body.lines().count()))
        .collect();
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line))
        .filter(move |(line_no, _)| {
            !fenced
                .iter()
                .any(|(first, last)| (*first..=*last).contains(line_no))
        })
}

/// An ATX heading outside a fenced block.
#[derive(Clone, Debug)]
pub struct Heading<'a> {
    pub title: &'a str,
    pub line: usize,
}

/// Every ATX heading outside a fenced block. `SPEC.md` has shell transcripts
/// whose lines begin with `#`, and those aren't headings.
pub fn headings(text: &str) -> Vec<Heading<'_>> {
    prose(text)
        .filter_map(|(line, written)| {
            let trimmed = written.trim_start();
            let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
            // `#hashtag` isn't a heading; ATX wants a space.
            let title = trimmed.get(hashes..)?.strip_prefix(' ')?.trim();
            (1..=6).contains(&hashes).then_some(Heading { title, line })
        })
        .collect()
}

/// GitHub's anchor for a heading: lowercase, drop everything but
/// alphanumerics, spaces, hyphens and underscores, then spaces become hyphens.
///
/// Exact rather than approximate, because `every_link_resolves` holds our
/// `#anchor`s to it, and a near-miss would pass links that break on GitHub.
pub fn slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            out.extend(c.to_lowercase());
        } else if c == ' ' {
            out.push('-');
        }
    }
    out
}

/// Every inline `[text](target)` outside a fenced block or a code span.
pub fn links(text: &str) -> Vec<Link> {
    let mut out = Vec::new();
    for (line_no, line) in prose(text) {
        let b = line.as_bytes();
        let mut i = 0;
        let mut in_code = false;
        while let Some(&c) = b.get(i) {
            match c {
                b'`' => {
                    in_code = !in_code;
                    i += 1;
                }
                b'\\' => i += 2,
                b'[' if !in_code => {
                    let Some(close) = find_balanced(b, i, b'[', b']') else {
                        i += 1;
                        continue;
                    };
                    if b.get(close + 1) != Some(&b'(') {
                        i = close + 1;
                        continue;
                    }
                    let Some(paren) = find_balanced(b, close + 1, b'(', b')') else {
                        i = close + 1;
                        continue;
                    };
                    // A title after the destination, as in `(url "title")`.
                    let destination = line[close + 2..paren].split_whitespace().next();
                    out.push(Link {
                        text: line[i + 1..close].to_string(),
                        destination: destination_of(destination.unwrap_or("")),
                        line: line_no,
                    });
                    i = paren + 1;
                }
                _ => i += 1,
            }
        }
    }
    out
}

fn destination_of(destination: &str) -> Destination {
    if destination.starts_with("http://") || destination.starts_with("https://") {
        return Destination::External {
            url: destination.to_string(),
        };
    }
    if let Some(anchor) = destination.strip_prefix('#') {
        return Destination::SameDocument {
            anchor: anchor.to_string(),
        };
    }
    if destination.is_empty() {
        return Destination::Nowhere;
    }
    match destination.split_once('#') {
        Some((path, anchor)) => Destination::File {
            path: path.to_string(),
            anchor: Some(anchor.to_string()),
        },
        None => Destination::File {
            path: destination.to_string(),
            anchor: None,
        },
    }
}

/// The offset of the `close` matching the `open` at `from`. A `\` escapes the
/// byte after it.
fn find_balanced(b: &[u8], from: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (i, c) in b.iter().enumerate().skip(from) {
        if escaped {
            escaped = false;
            continue;
        }
        match *c {
            b'\\' => escaped = true,
            c if c == open => depth += 1,
            c if c == close => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

#[test]
fn links_are_extracted_with_their_anchors() {
    let text =
        "see [tags](crates/docs/src/docs/reference/build/tags.md#why-forbids) and `[not a link](x)`\n";
    let l = links(text);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].text, "tags");
    assert_eq!(
        l[0].destination,
        Destination::File {
            path: "crates/docs/src/docs/reference/build/tags.md".into(),
            anchor: Some("why-forbids".into()),
        }
    );

    let l = links("[a](#here) [b](other.md) [c](https://x.test) [d]()\n");
    let destinations: Vec<&Destination> = l.iter().map(|l| &l.destination).collect();
    assert_eq!(
        destinations,
        vec![
            &Destination::SameDocument {
                anchor: "here".into()
            },
            &Destination::File {
                path: "other.md".into(),
                anchor: None
            },
            &Destination::External {
                url: "https://x.test".into()
            },
            &Destination::Nowhere,
        ]
    );
}

#[test]
fn a_link_inside_a_fence_is_not_a_link() {
    let text = "[a](a.md)\n```text\n[b](b.md)\n```\n  ```\n  [c](c.md)\n  ```\n[d](d.md)\n";
    let lines: Vec<usize> = links(text).iter().map(|l| l.line).collect();
    assert_eq!(lines, vec![1, 8]);
    assert!(
        links("```\n[a](a.md)").is_empty(),
        "an unclosed fence runs to the end"
    );
}

#[test]
fn an_unclosed_fence_is_found() {
    assert_eq!(unterminated_fence("```buri"), Some(1));
    assert_eq!(
        unterminated_fence("a\n```\nb\n```\n\n```text\nc\n"),
        Some(6)
    );
    assert_eq!(unterminated_fence("```\n```"), None);
    assert_eq!(unterminated_fence("```\n\n\n```\n"), None);
    assert_eq!(unterminated_fence("no fences\n"), None);
    let spec = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crates/docs/src/docs/SPEC.md"
    );
    let spec = std::fs::read_to_string(spec).unwrap();
    assert_eq!(unterminated_fence(&spec), None);
}

/// Text somebody else wrote: multi-byte characters wherever a byte walk might
/// stop between them.
#[test]
fn a_malformed_document_is_read_rather_than_indexed_past() {
    for text in [
        "# 🎈\n\n[é](ü.md#ñ)\n\n| á | é |\n|---|---|\n| ü | ñ |\n",
        "`é",
        "**é",
        "[é",
    ] {
        let _ = headings(text);
        let _ = links(text);
        let _ = unterminated_fence(text);
    }
}

#[test]
fn slugs_match_github() {
    // Each of these is a live anchor in the checked-in documents.
    assert_eq!(
        slug("12. Why the grammar is context-free and unambiguous"),
        "12-why-the-grammar-is-context-free-and-unambiguous"
    );
    assert_eq!(
        slug("Why `forbids` has no platforms"),
        "why-forbids-has-no-platforms"
    );
    assert_eq!(slug("`Result` is must-use"), "result-is-must-use");
    assert_eq!(
        slug("6.8 `?` — error propagation"),
        "68---error-propagation"
    );
}

#[test]
fn headings_skip_fenced_blocks() {
    let text = "# Title\n\n```sh\n# not a heading\n```\n\n## Real\n#hashtag\n####### seven\n";
    let h = headings(text);
    let titles: Vec<(&str, usize)> = h.iter().map(|h| (h.title, h.line)).collect();
    assert_eq!(titles, vec![("Title", 1), ("Real", 7)]);
    let spec = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crates/docs/src/docs/SPEC.md"
    );
    // A guard on the scanner: if this collapses to zero, every link test
    // passes vacuously.
    assert!(headings(&std::fs::read_to_string(spec).unwrap()).len() > 50);
}
