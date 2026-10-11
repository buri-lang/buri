//! The official JSON-Schema-Test-Suite, draft 2020-12 (`cli/tests/json-schema/`),
//! checked by `buri lint` the way a repository's JSON files are.
//!
//! Each group's schema becomes a schema file, and each case a data file a
//! generator lists. A data file needs a `"$schema"` key, which would change the
//! data, so it holds the case under `value`, and names a wrapper schema whose
//! `value` property is a `$ref` to the group's schema:
//!
//! ```text
//! lib/suite/items-3.schema.json        the group's schema, verbatim
//! lib/suite/items-3.wrap.schema.json   { "properties": { "value": { "$ref": "items-3.schema.json" } } }
//! lib/suite/items-3-0.json             { "$schema": "items-3.wrap.schema.json", "value": <data> }
//! ```
//!
//! The suite's remotes, which it serves at `http://localhost:1234/`, are
//! checked in under `lib/suite/remotes/`, and every wrapper reaches each of
//! them through `$defs`, so a check reads them. One without an `$id` is given
//! its URL as `$id`, as a checked-in schema declaring that `$id` would be.
//!
//! A valid case draws nothing, and an invalid one a `json-schema-violation` at
//! its data file. A group this toolchain refuses by design is in [`REFUSED`],
//! and draws the finding named there instead.

use crate::harness::*;
use std::collections::BTreeMap;

/// A group whose schema is refused by design, the finding it draws, and why.
const REFUSED: &[(&str, &str, &str, &str)] = &[
    ("vocabulary", "schema that uses custom metaschema with with no validation vocabulary", "json-schema-draft-unsupported", DRAFT),
    ("vocabulary", "ignore unrecognized optional vocabulary", "json-schema-draft-unsupported", DRAFT),
    ("defs", "validate definition against metaschema", "schema-outside-repository", META),
    ("ref", "remote ref, containing refs itself", "schema-outside-repository", META),
    ("refRemote", "remote HTTP ref with different $id", "schema-outside-repository", RETRIEVAL),
    ("refRemote", "remote HTTP ref with different URN $id", "schema-outside-repository", RETRIEVAL),
    ("pattern", "pattern with Unicode property escape requires unicode mode", "json-invalid-schema", UNICODE),
    ("patternProperties", "patternProperties with Unicode property escape", "json-invalid-schema", UNICODE),
];

const DRAFT: &str = "only the 2020-12 meta-schema is known; a custom meta-schema is refused";
const META: &str = "the 2020-12 meta-schema is known by name but not bundled, and nothing is fetched";
const RETRIEVAL: &str = "a schema is reached by its path or its `$id`, so a URL it was served at but does not declare reaches nothing";
const UNICODE: &str = "`\\p{…}` is refused by name, rather than read as something else";

/// A JSON value as written: its text, and its members or elements, each with
/// its key as written, quotes and all.
struct Raw<'a> {
    text: &'a str,
    items: Vec<(&'a str, Raw<'a>)>,
}

impl<'a> Raw<'a> {
    fn parse(text: &'a str) -> Raw<'a> {
        let mut at = 0;
        let raw = Raw::value(text, &mut at);
        assert!(text[at..].trim().is_empty(), "trailing text after JSON");
        raw
    }

    fn value(s: &'a str, at: &mut usize) -> Raw<'a> {
        let b = s.as_bytes();
        let skip = |at: &mut usize| {
            while *at < b.len() && b[*at].is_ascii_whitespace() {
                *at += 1;
            }
        };
        skip(at);
        let start = *at;
        let mut items = Vec::new();
        match b[*at] {
            open @ (b'{' | b'[') => {
                let close = if open == b'{' { b'}' } else { b']' };
                *at += 1;
                loop {
                    skip(at);
                    if b[*at] == close {
                        *at += 1;
                        break;
                    }
                    let key = if open == b'{' {
                        let k = Raw::value(s, at).text;
                        skip(at);
                        assert_eq!(b[*at], b':');
                        *at += 1;
                        k
                    } else {
                        ""
                    };
                    items.push((key, Raw::value(s, at)));
                    skip(at);
                    if b[*at] == b',' {
                        *at += 1;
                    }
                }
            }
            b'"' => {
                *at += 1;
                while b[*at] != b'"' {
                    *at += if b[*at] == b'\\' { 2 } else { 1 };
                }
                *at += 1;
            }
            _ => {
                while *at < b.len() && !b",]} \t\r\n".contains(&b[*at]) {
                    *at += 1;
                }
            }
        }
        Raw { text: &s[start..*at], items }
    }

    fn get(&self, key: &str) -> Option<&Raw<'a>> {
        let quoted = format!("\"{key}\"");
        self.items.iter().find(|(k, _)| *k == quoted).map(|(_, v)| v)
    }
}

fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        match path.is_dir() {
            true => out.extend(files_under(&path)),
            false => out.push(path),
        }
    }
    out.sort();
    out
}

/// One case: its data file, and whether the suite calls it valid.
struct Case {
    file: String,
    valid: bool,
    what: String,
}

/// One group: its files' common prefix, the finding its refusal draws, and
/// its cases.
struct Group {
    prefix: String,
    what: String,
    refused: Option<&'static str>,
    cases: Vec<Case>,
}

#[test]
fn the_json_schema_test_suite() {
    let s = Scratch::repo("json-schema-suite");
    s.write("tools/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    s.write("tools/gen/tool.buri", GENERATOR);
    s.write("lib/suite/lib.buri", "export let nothing: Int = 0;\n");

    let remotes = tests_dir().join("json-schema/remotes");
    let mut defs = Vec::new();
    for path in files_under(&remotes) {
        let rel = path.strip_prefix(&remotes).unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let text = match Raw::parse(&text).get("$id") {
            Some(_) => text,
            None => text.replacen('{', &format!("{{\"$id\": \"http://localhost:1234/{rel}\", "), 1),
        };
        s.write(&format!("lib/suite/remotes/{rel}"), &text);
        defs.push(format!("\"r{}\": {{\"$ref\": \"remotes/{rel}\"}}", defs.len()));
    }
    let defs = defs.join(", ");

    let dir = tests_dir().join("json-schema/tests");
    let mut paths: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    paths.sort();
    let mut groups = Vec::new();
    for path in &paths {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(path).unwrap();
        for (gi, (_, group)) in Raw::parse(&text).items.iter().enumerate() {
            let what = group.get("description").unwrap().text.trim_matches('"').to_string();
            let prefix = format!("{stem}-{gi}");
            s.write(&format!("lib/suite/{prefix}.schema.json"), group.get("schema").unwrap().text);
            s.write(
                &format!("lib/suite/{prefix}.wrap.schema.json"),
                &format!(
                    "{{\"$schema\": \"https://json-schema.org/draft/2020-12/schema\", \
                     \"properties\": {{\"value\": {{\"$ref\": \"{prefix}.schema.json\"}}}}, \"$defs\": {{{defs}}}}}\n"
                ),
            );
            let refused = REFUSED.iter().find(|r| r.0 == stem && r.1 == what).map(|r| r.2);
            let mut cases = Vec::new();
            for (ci, (_, case)) in group.get("tests").unwrap().items.iter().enumerate() {
                let file = format!("{prefix}-{ci}.json");
                s.write(
                    &format!("lib/suite/{file}"),
                    &format!("{{\"$schema\": \"{prefix}.wrap.schema.json\", \"value\": {}}}\n", case.get("data").unwrap().text),
                );
                let valid = case.get("valid").unwrap().text == "true";
                let what = case.get("description").unwrap().text.trim_matches('"').to_string();
                cases.push(Case { file, valid, what });
            }
            groups.push(Group { prefix, what: format!("{stem} / {what}"), refused, cases });
        }
    }
    for r in REFUSED {
        assert!(
            groups.iter().any(|g| g.what == format!("{} / {}", r.0, r.1)),
            "REFUSED names a group the suite does not have: {} / {}",
            r.0,
            r.1
        );
    }
    let inputs: Vec<String> = groups.iter().flat_map(|g| &g.cases).map(|c| format!("        \"{}\",\n", c.file)).collect();
    s.write(
        "lib/suite/BUILD.buri",
        &format!("library {{\n    generators: [\n        {{ tool: \"//tools/gen\", inputs: [\n{}        ] }},\n    ]\n}}\n", inputs.concat()),
    );

    let run = s.run(&["lint", "//lib/suite", "--error-format=json"]);
    run.exits(1);
    // Each file's findings, by code.
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in run.stdout.lines().chain(run.stderr.lines()).filter(|l| l.starts_with('{')) {
        let d = buri::json::parse(line).unwrap();
        let (Some(code), Some(file)) = (d.get("code").and_then(|c| c.as_str()), d.at("location.file").and_then(|f| f.as_str())) else {
            continue;
        };
        found.entry(file.trim_start_matches("lib/suite/").to_string()).or_default().push(code.to_string());
    }

    let mut failures = Vec::new();
    let mut cases = 0;
    for g in &groups {
        let mine = |f: &str| f.starts_with(&format!("{}.", g.prefix)) || f.starts_with(&format!("{}-", g.prefix));
        let codes: Vec<&String> = found.iter().filter(|(f, _)| mine(f)).flat_map(|(_, c)| c).collect();
        if let Some(code) = g.refused {
            if !codes.iter().any(|c| *c == code) {
                failures.push(format!("{}: expected `{code}`, found {codes:?}", g.what));
            }
            continue;
        }
        for (file, codes) in found.iter().filter(|(f, _)| f.starts_with(&format!("{}.", g.prefix))) {
            failures.push(format!("{}: the schema drew {codes:?} at {file}", g.what));
        }
        for case in &g.cases {
            cases += 1;
            let codes = found.get(&case.file).map(Vec::as_slice).unwrap_or_default();
            let ok = match case.valid {
                true => codes.is_empty(),
                false => !codes.is_empty() && codes.iter().all(|c| c == "json-schema-violation"),
            };
            if !ok {
                let expected = if case.valid { "valid" } else { "invalid" };
                failures.push(format!("{} / {} ({}): expected {expected}, found {codes:?}", g.what, case.what, case.file));
            }
        }
    }
    assert!(cases > 1000, "only {cases} cases ran");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

/// A generator that answers with one module. The check runs before it, and
/// it never runs here, since some cases are always invalid.
pub const GENERATOR: &str = r#"from "core/buri/ast" import * as ast;
from "core/tool" import { Generated, GenerateRequest };
from "platform/effect" import { Allocator };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    let modules = match (ast.parse(ctx, "", "export let inputs: Int = 0;\n")) {
        .Ok(parsed) => [("config", parsed)],
        .Err(_) => [],
    };
    Generated { modules: modules, diagnostics: [], needs: [] }
}
"#;
