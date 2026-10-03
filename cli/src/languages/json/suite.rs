//! The official JSON-Schema-Test-Suite, draft 2020-12, run against the
//! validator. The vendored data and where it came from are in
//! `cli/tests/json-schema/README.md`.
//!
//! Each group's schema is read as a schema file and each case's `data` is
//! checked against it, as a data file naming that schema would be. The suite's
//! remotes, which it serves at `http://localhost:1234/`, are further schema
//! files in the same registry. One without an `$id` is given its URL as `$id`,
//! which is what a checked-in schema declaring that `$id` would be; nothing is
//! fetched, and a remote whose own `$id` differs from its URL is reached only
//! by that `$id`.
//!
//! A group this toolchain refuses by design is in [`REFUSED`], and must be
//! refused with the finding named there. Nothing else is left out.

use super::schema::{self, is_2020_12, Registry, SchemaFile};
use super::syntax::{self, Dialect, Node};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A group whose schema this toolchain refuses by design.
struct Refused {
    file: &'static str,
    group: &'static str,
    /// The finding's code, and text its problem contains.
    code: &'static str,
    problem: &'static str,
    why: &'static str,
}

const DRAFT: &str = "only the 2020-12 meta-schema is known; a custom meta-schema and its `$vocabulary` are refused";
const META: &str = "the 2020-12 meta-schema is known by name but not bundled, and nothing is fetched";
const UNICODE: &str = "`\\p{…}` is refused by name, rather than read as something else";
const RETRIEVAL: &str = "a schema is reached by its path or its `$id`, and nothing is fetched, so a URL it was served at but does not declare reaches nothing";

/// Every group left out, each with the finding it must draw instead.
///
/// `optional/` is not vendored: `format` asserts nothing in 2020-12 unless a
/// vocabulary asks, and the rest are behaviours 2020-12 leaves open.
/// `format.json` and `content.json` run, since they hold `format` and the
/// content keywords to being annotations.
const REFUSED: &[Refused] = &[
    Refused { file: "vocabulary.json", group: "schema that uses custom metaschema with with no validation vocabulary", code: "json-schema-draft-unsupported", problem: "", why: DRAFT },
    Refused { file: "vocabulary.json", group: "ignore unrecognized optional vocabulary", code: "json-schema-draft-unsupported", problem: "", why: DRAFT },
    Refused { file: "defs.json", group: "validate definition against metaschema", code: "schema-outside-repository", problem: "", why: META },
    Refused { file: "ref.json", group: "remote ref, containing refs itself", code: "schema-outside-repository", problem: "", why: META },
    Refused { file: "pattern.json", group: "pattern with Unicode property escape requires unicode mode", code: "json-schema-invalid", problem: "`\\p{…}` Unicode properties are not supported", why: UNICODE },
    Refused { file: "patternProperties.json", group: "patternProperties with Unicode property escape", code: "json-schema-invalid", problem: "`\\p{…}` Unicode properties are not supported", why: UNICODE },
    Refused { file: "refRemote.json", group: "remote HTTP ref with different $id", code: "schema-outside-repository", problem: "", why: RETRIEVAL },
    Refused { file: "refRemote.json", group: "remote HTTP ref with different URN $id", code: "schema-outside-repository", problem: "", why: RETRIEVAL },
];

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/json-schema")
}

fn parse(path: &Path) -> Node {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    match syntax::parse(&text, Dialect::Json) {
        Ok(doc) => doc.root,
        Err(e) => panic!("{}: {}", path.display(), e.problem),
    }
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files_under(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

/// The remotes, as schema files, each with an `$id`.
fn remotes() -> Vec<SchemaFile> {
    let dir = data_dir().join("remotes");
    let mut paths = Vec::new();
    files_under(&dir, &mut paths);
    paths
        .iter()
        .map(|path| {
            let rel = path.strip_prefix(&dir).unwrap_or(path).to_string_lossy().replace('\\', "/");
            let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let root = parse(path);
            let text = if root.get("$id").is_some() {
                text
            } else {
                let url = format!("http://localhost:1234/{rel}");
                text.replacen('{', &format!("{{\"$id\": \"{url}\", "), 1)
            };
            let root = syntax::parse(&text, Dialect::Json).map(|d| d.root).unwrap_or(root);
            SchemaFile { path: format!("remotes/{rel}"), root }
        })
        .collect()
}

fn items(node: &Node) -> &[Node] {
    match &node.value {
        syntax::Value::Array(items, _) => items,
        _ => &[],
    }
}

fn text(node: &Node, key: &str) -> String {
    node.get(key).and_then(Node::as_str).unwrap_or_default().to_string()
}

#[derive(Default)]
struct Tally {
    run: usize,
    passed: usize,
    excluded: usize,
}

#[test]
fn json_schema_test_suite_draft_2020_12() {
    let mut paths = Vec::new();
    files_under(&data_dir().join("tests"), &mut paths);
    assert!(paths.len() > 40, "the vendored suite is missing");
    let remotes = remotes();
    let mut tally: BTreeMap<String, Tally> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut refused_seen = BTreeSet::new();
    for path in &paths {
        let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let t = tally.entry(file.clone()).or_default();
        for group in items(&parse(path)) {
            let name = text(group, "description");
            let cases = group.get("tests").map(items).unwrap_or_default();
            let Some(root) = group.get("schema").cloned() else { continue };
            let mut files = vec![SchemaFile { path: "schema.json".to_string(), root }];
            files.extend(remotes.iter().map(|r| SchemaFile { path: r.path.clone(), root: r.root.clone() }));
            let mut problems: Vec<(String, String)> = Vec::new();
            if let Some(own) = files[0].root.get("$schema") {
                if !own.as_str().is_some_and(is_2020_12) {
                    problems.push(("json-schema-draft-unsupported".to_string(), String::new()));
                }
            }
            let registry = Registry::new(&files);
            problems.extend(registry.problems().iter().map(|f| {
                let problem = f.binds.iter().find(|(k, _)| k == "problem").map(|(_, v)| v.clone()).unwrap_or_default();
                (f.code.clone(), format!("{}: {problem}", f.file))
            }));
            if let Some(refused) = REFUSED.iter().find(|r| r.file == file && r.group == name) {
                refused_seen.insert((refused.file, refused.group));
                t.excluded += cases.len();
                if !problems.iter().any(|(code, p)| code == refused.code && p.contains(refused.problem)) {
                    failures.push(format!("{file} / {name}: expected `{}` ({}), because {}; got {problems:?}", refused.code, refused.problem, refused.why));
                }
                continue;
            }
            if !problems.is_empty() {
                t.run += cases.len();
                failures.push(format!("{file} / {name}: the schema is refused: {problems:?}"));
                continue;
            }
            for case in cases {
                t.run += 1;
                let expected = matches!(case.get("valid").map(|v| &v.value), Some(syntax::Value::Bool(true)));
                let Some(data) = case.get("data") else { continue };
                let violations = schema::validate(&registry, 0, data);
                if violations.is_empty() == expected {
                    t.passed += 1;
                } else {
                    let what = if expected { format!("invalid: {:?}", violations.first().map(|v| &v.problem)) } else { "valid".to_string() };
                    failures.push(format!("{file} / {name} / {}: expected {}, found {what}", text(case, "description"), if expected { "valid" } else { "invalid" }));
                }
            }
        }
    }
    for r in REFUSED {
        assert!(refused_seen.contains(&(r.file, r.group)), "REFUSED names a group the suite does not have: {} / {}", r.file, r.group);
    }
    let mut total = Tally::default();
    for (file, t) in &tally {
        println!("{file:<36} run {:>4}  passed {:>4}  excluded {:>3}", t.run, t.passed, t.excluded);
        total.run += t.run;
        total.passed += t.passed;
        total.excluded += t.excluded;
    }
    println!("{:<36} run {:>4}  passed {:>4}  excluded {:>3}", "total", total.run, total.passed, total.excluded);
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}
