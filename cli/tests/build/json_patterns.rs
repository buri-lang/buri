//! The regular expressions a JSON schema's `pattern` holds, through `buri lint`:
//! what each one matches, and why each refused one is refused.
//!
//! One schema gives each pattern a property of its own, and one data file
//! gives each property its text, a line apiece, so a violation's line names
//! its row of [`MATCHES`]. A second schema holds every pattern of [`REFUSED`].

use crate::harness::*;

/// A pattern, a text, and whether the pattern matches somewhere in it.
const MATCHES: &[(&str, &str, bool)] = &[
    ("b", "abc", true),
    ("^b", "abc", false),
    ("c$", "abc", true),
    ("b$", "abc", false),
    ("^a.c$", "abc", true),
    ("^a.c$", "a\nc", false),
    ("^a|b$", "xb", true),
    ("^(?:ab|cd)+$", "abcdab", true),
    ("^(?:ab|cd)+$", "abc", false),
    ("^(ab){2}$", "abab", true),
    ("^(ab){2}$", "ab", false),
    ("^(ab){1,2}$", "ababab", false),
    ("^(ab){2,}$", "ababab", true),
    ("^(ab)*?c$", "ababc", true),
    ("^(ab)+?$", "abab", true),
    ("^(ab){0,1}?b$", "b", true),
    ("^(?:a*)*$", "aaaa", true),
    ("^(?:a*)+b$", "aac", false),
    ("^a*?b$", "aaab", true),
    ("^a+?$", "", false),
    ("^a{2}$", "aa", true),
    ("^a{2,}$", "a", false),
    ("^a{1,2}$", "aaa", false),
    ("^a{,2}$", "a{,2}", true),
    ("^a{x}$", "a{x}", true),
    ("^a{1,x}$", "a{1,x}", true),
    ("^a{1$", "a{1", true),
    ("{", "{", true),
    ("^(?=ab)a", "ab", true),
    ("^(?=ab)a", "ac", false),
    ("^(?!foo).*$", "bar", true),
    ("^(?!foo).*$", "food", false),
    ("^(?<word>\\w+)$", "abc", true),
    ("\\bfoo\\b", "a foo b", true),
    ("\\bfoo\\b", "afoob", false),
    ("\\Boo\\B", "foob", true),
    ("\\Bfoo", "foo", false),
    ("^\\d\\D\\w\\W\\s\\S$", "1a_ \tx", true),
    ("^\\d\\D\\w\\W\\s\\S$", "1a_-x ", false),
    ("^\\s$", "\u{a0}", true),
    ("^\\s$", "\u{2003}", true),
    ("^[^0-9]+$", "abc", true),
    ("^[^0-9]+$", "a1", false),
    ("^[\\d\\s]+$", "1 2", true),
    ("^[\\b]$", "\u{8}", true),
    ("^[a-]+$", "a-a", true),
    ("^[\\x41-\\x43]+$", "ABC", true),
    ("^[a-\\x7a]+$", "az", true),
    ("^[\\]]$", "]", true),
    ("^\\t\\n\\r\\v\\f$", "\t\n\r\u{b}\u{c}", true),
    ("^\\0$", "\0", true),
    ("^\\x41\\u0042\\u{43}$", "ABC", true),
    ("^\\u{1F600}$", "\u{1F600}", true),
    ("^\\cJ$", "\n", true),
    ("^\\.\\-\\/$", ".-/", true),
    ("^.$", "\u{1F600}", true),
    ("^(?:ab){0,1}?c$", "abd", false),
];

/// A pattern and a text the matcher abandons rather than answers: one that
/// takes too many steps, and one that recurses too deeply.
fn too_costly() -> Vec<(String, String)> {
    vec![
        ("^(a+)+$".to_string(), format!("{}b", "a".repeat(40))),
        ("^(?:ab)*$".to_string(), "ab".repeat(1500)),
    ]
}

/// A pattern this toolchain refuses, and why.
const REFUSED: &[(&str, &str)] = &[
    ("a)", "`)` is unbalanced"),
    ("^*", "an assertion cannot be repeated"),
    ("\\b+", "an assertion cannot be repeated"),
    ("(?=a)*", "an assertion cannot be repeated"),
    ("a{3,2}", "`{3,2}` is out of order"),
    ("a**", "a quantifier cannot follow a quantifier"),
    ("a+?+", "a quantifier cannot follow a quantifier"),
    ("(?<=a)b", "lookbehind is not supported"),
    ("(?<!a)b", "lookbehind is not supported"),
    ("(?<name", "a group name is not closed"),
    ("(?x)", "`(?` starts no group this reads"),
    ("(?", "`(?` starts no group this reads"),
    ("(a", "a group is not closed"),
    ("*a", "`*` has nothing to repeat"),
    ("+", "`+` has nothing to repeat"),
    ("?", "`?` has nothing to repeat"),
    ("{2}", "`{` has nothing to repeat"),
    ("[a", "a `[` class is not closed"),
    ("[a-\\d]", "a range cannot end in a class escape"),
    ("[z-a]", "`z-a` is out of order"),
    ("a\\", "the pattern ends in `\\`"),
    ("[\\", "the pattern ends in `\\`"),
    ("\\u{}", "`\\u{` needs hexadecimal digits and a `}`"),
    ("\\u{41", "`\\u{` needs hexadecimal digits and a `}`"),
    ("\\u{110000}", "`\\u{…}` names no character"),
    ("\\c1", "`\\c` needs a letter"),
    ("\\c", "`\\c` needs a letter"),
    ("\\p{L}", "`\\p{…}` Unicode properties are not supported"),
    ("\\P{L}", "`\\p{…}` Unicode properties are not supported"),
    ("(?<a>x)\\k<a>", "back-references are not supported"),
    ("(a)\\1", "back-references are not supported"),
    ("\\q", "`\\q` is not an escape"),
    ("\\01", "`\\0` is not an escape"),
    ("\\xZZ", "an escape needs 2 hexadecimal digits"),
    ("\\x4", "an escape needs 2 hexadecimal digits"),
    ("\\uD800", "a surrogate escape is not supported"),
    ("[\\B]", "`\\B` is not an escape"),
];

/// `text` as a JSON string.
fn quoted(text: &str) -> String {
    buri::json::Value::str(text).to_string()
}

const SCHEMA: &str = "https://json-schema.org/draft/2020-12/schema";

#[test]
fn json_patterns_match_what_they_say() {
    let s = Scratch::repo("json-patterns");
    s.write("tools/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    s.write("tools/gen/tool.buri", crate::json_schema_suite::GENERATOR);
    s.write("lib/p/lib.buri", "export let nothing: Int = 0;\n");
    s.write(
        "lib/p/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"//tools/gen\", inputs: [\"matches.json\", \"refused.json\"] },\n    ]\n}\n",
    );

    // `None` for a text the matcher abandons.
    let mut rows: Vec<(String, String, Option<bool>)> =
        MATCHES.iter().map(|(p, t, m)| (p.to_string(), t.to_string(), Some(*m))).collect();
    rows.extend(too_costly().into_iter().map(|(p, t)| (p, t, None)));
    let mut refusals: Vec<(String, String)> = REFUSED.iter().map(|(p, w)| (p.to_string(), w.to_string())).collect();
    refusals.push((format!("{}a{}", "(".repeat(102), ")".repeat(102)), "groups nest too deeply".to_string()));

    let mut schema = format!("{{\"$schema\": \"{SCHEMA}\", \"properties\": {{\n");
    let mut data = String::from("{\"$schema\": \"matches.schema.json\",\n");
    for (i, (pattern, text, _)) in rows.iter().enumerate() {
        schema.push_str(&format!("\"m{i}\": {{\"pattern\": {}}},\n", quoted(pattern)));
        data.push_str(&format!("\"m{i}\": {},\n", quoted(text)));
    }
    schema.push_str("\"$schema\": true}}\n");
    data.push_str("\"end\": 0}\n");
    s.write("lib/p/matches.schema.json", &schema);
    s.write("lib/p/matches.json", &data);

    let mut refused = format!("{{\"$schema\": \"{SCHEMA}\", \"properties\": {{\n");
    for (i, (pattern, _)) in refusals.iter().enumerate() {
        refused.push_str(&format!("\"r{i}\": {{\"pattern\": {}}},\n", quoted(pattern)));
    }
    refused.push_str("\"$schema\": true}}\n");
    s.write("lib/p/refused.schema.json", &refused);
    s.write("lib/p/refused.json", "{\"$schema\": \"refused.schema.json\"}\n");

    let run = s.run(&["lint", "//lib/p", "--error-format=json"]);
    run.exits(1);
    let mut said: Vec<(String, u32, String)> = Vec::new();
    for line in run.stdout.lines().chain(run.stderr.lines()).filter(|l| l.starts_with('{')) {
        let d = buri::json::parse(line).unwrap();
        let text = |path: &str| d.at(path).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let line = d.at("location.line").and_then(|v| v.as_u32()).unwrap();
        said.push((text("location.file"), line, format!("{} {}", text("code"), text("message"))));
    }
    let at = |file: &str, line: usize| -> Vec<&str> {
        said.iter().filter(|(f, l, _)| f == file && *l as usize == line).map(|(_, _, m)| m.as_str()).collect()
    };

    let mut failures = Vec::new();
    for (i, (pattern, text, matches)) in rows.iter().enumerate() {
        // Line 1 is the `"$schema"` key, so row `i` is on line `i + 2`.
        let found = at("lib/p/matches.json", i + 2);
        let ok = match matches {
            None => found.len() == 1 && found[0].contains("is too costly to match"),
            Some(true) => found.is_empty(),
            Some(false) => found.len() == 1 && found[0].starts_with("json-schema-violation"),
        };
        if !ok {
            let text: String = text.chars().take(60).collect();
            failures.push(format!("`{pattern}` against {text:?}: expected {matches:?}, found {found:?}"));
        }
    }
    for (i, (pattern, why)) in refusals.iter().enumerate() {
        let found = at("lib/p/refused.schema.json", i + 2);
        let want = format!("json-invalid-schema the pattern `{pattern}` does not read: {why}");
        if found != [want.as_str()] {
            failures.push(format!("`{pattern}`: expected {want:?}, found {found:?}"));
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}\n\nall:\n{said:#?}", failures.len(), failures.join("\n"));
}
