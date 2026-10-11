//! What the JSON, JSONC and JSON5 readers accept and refuse, through `buri lint`.
//!
//! Each row of [`ROWS`] is one file a generator lists. A file that reads names
//! a schema that allows anything and draws nothing; one that does not draws a
//! `json-syntax` finding saying why.

use crate::harness::*;

/// What every file that reads names.
const S: &str = "\"$schema\": \"any.schema.json\"";

/// A file's extension, its text with `S` standing for the `"$schema"` member,
/// and the problem it draws, if any.
const ROWS: &[(&str, &str, Option<&str>)] = &[
    ("json", "", Some("the file holds no value")),
    ("json", "  // c", Some("JSON has no comments")),
    ("json", "{S} {}", Some("a JSON file holds one value, and this is a second")),
    ("json", "{S, \"a\": \"\u{1}\"}", Some("the control character U+0001 must be escaped in a string")),
    ("json", "\u{7}", Some("the control character U+0007 cannot start a JSON value")),
    ("json", "@", Some("`@` cannot start a JSON value")),
    ("json", "{S, \"a\": \"x\ny\"}", Some("this string is never closed")),
    ("json", "{S, \"a\": \"abc", Some("this string is never closed")),
    ("json", "{S, \"a\": \"\\", Some("this string is never closed")),
    ("json", "{S, \"a\": \"\\'\"}", Some("`\\'` is not an escape")),
    ("json", "{S, \"a\": \"\\u12\"}", Some("an escape needs 4 hexadecimal digits here")),
    ("json", "{S, \"a\": \"\\uD83D\\u12\"}", Some("an escape needs 4 hexadecimal digits here")),
    ("json", "{S, \"a\": \"\\\"\\\\\\/\\b\\f\\n\\r\\t\\u00e9\\uD83D\\uDE00\"}", None),
    ("json", "{S, \"a\": \"\\uD83Dx\", \"b\": \"\\uD83D\\n\", \"c\": \"\\uD83D\\u0041\", \"d\": \"\\uDE00\"}", None),
    ("json", "{S, \"a\": +1}", Some("a leading `+` is JSON5, not JSON")),
    ("json", "{S, \"a\": -Infinity}", Some("`Infinity` is JSON5, not JSON")),
    ("json", "{S, \"a\": Infinity}", Some("`Infinity` is JSON5, not JSON")),
    ("json", "{S, \"a\": NaN}", Some("`NaN` is JSON5, not JSON")),
    ("json", "{S, \"a\": -abc}", Some("this is not a number")),
    ("json", "{S, \"a\": 0x1F}", Some("a hexadecimal number is JSON5, not JSON")),
    ("json", "{S, \"a\": 01}", Some("a number does not start with `0`")),
    ("json", "{S, \"a\": 1.}", Some("a number ending in `.` is JSON5, not JSON")),
    ("json", "{S, \"a\": .5}", Some("a number starting with `.` is JSON5, not JSON")),
    ("json", "{S, \"a\": -}", Some("this is not a number")),
    ("json", "{S, \"a\": 1e}", Some("an exponent needs digits")),
    ("json", "{S, \"a\": [1e5, 1E+5, 1e-5, -0.5, 0, 10]}", None),
    ("json", "{S, \"a\": foo}", Some("`foo` is not a JSON value")),
    ("json", "]", Some("`]` closes nothing here")),
    ("json", ":", Some("expected a value, found `:`")),
    ("json", "{S, \"a\":", Some("the file ends where a value should be")),
    ("json", "{S, a: 1}", Some("an unquoted key is JSON5, not JSON")),
    ("json", "{", Some("this object is never closed")),
    ("json", "{S, 1: 2}", Some("expected a key, found `1`")),
    ("json", "{S, \"a\": 1, \"a\": 2}", Some("the key \"a\" appears twice in this object")),
    ("json", "{S, \"a\" 1}", Some("expected `:` after the key")),
    ("json", "[", Some("this array is never closed")),
    ("json", "{S, \"a\": [1,]}", Some("JSON has no trailing commas")),
    ("json", "{S, \"a\": [1,,2]}", Some("two commas with no value between them")),
    ("json", "{S, \"a\": [1 2]}", Some("expected `,` or `]`, found `2`")),
    ("json", "{S, \"a\": [1}", Some("expected `,` or `]`, found `}`")),
    ("json", "{S, \"a\": [1", Some("this array is never closed")),
    ("json", "{S, \"a\": 1", Some("this object is never closed")),
    ("json", "{S, \"a\": {}, \"b\": [], \"c\": [true, false, null]}", None),
    ("jsonc", "/* a * b */ {S}", None),
    ("jsonc", "{S} /* never closed", Some("this comment is never closed")),
    ("jsonc", "{S} // the end, with no line break", None),
    ("jsonc", "{S, // a comment ending in a carriage return\r\"a\": 1,}", None),
    ("jsonc", "{S, 'a': 1}", Some("a single-quoted string is JSON5, not JSON")),
    ("json5", "{S, 'a': 'it\\'s', b: +1, c: -Infinity, d: NaN, e: Infinity, f: -NaN, g: .5, h: 5., i: -0xff}", None),
    ("json5", "\u{a0}{\u{2003}S\u{2028},\u{2029}a1\u{200c}\u{200d}: 1, $b_: 2} // after\u{2028}", None),
    ("json5", "{S, a: \"\\v\\0\\x41\\q\\\r\n\\\r\\\n\\\u{2028}\\\u{2029}\"}", None),
    ("json5", "{S, a: \"\\01\"}", Some("`\\0` is not an escape")),
    ("json5", "{S, a: \"\\1\"}", Some("`\\1` is not an escape")),
    ("json5", "{S, a: \"\\x4\"}", Some("an escape needs 2 hexadecimal digits here")),
    ("json5", "{S, \\u0061: 1}", Some("an escape in a key is not supported")),
    ("json5", "{S, a: 0x}", Some("`0x` needs digits after it")),
    ("json5", "{S, a: .}", Some("this is not a number")),
    ("json5", "{S, a: -abc}", Some("this is not a number")),
    ("json5", "{S, a: [1,],}", None),
];

#[test]
fn json_readers_accept_and_refuse_what_each_dialect_says() {
    let s = Scratch::repo("json-syntax");
    s.write("tools/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    s.write("tools/gen/tool.buri", crate::json_schema_suite::GENERATOR);
    s.write("lib/j/lib.buri", "export let nothing: Int = 0;\n");
    s.write("lib/j/any.schema.json", "{\"$schema\": \"https://json-schema.org/draft/2020-12/schema\"}\n");
    let mut rows: Vec<(String, String, Option<String>)> =
        ROWS.iter().map(|(ext, text, problem)| (ext.to_string(), text.replace('S', S), problem.map(str::to_string))).collect();
    rows.push(("json".into(), "[".repeat(300), Some("values nest more than 256 deep here".into())));
    let mut inputs = String::new();
    for (i, (ext, text, _)) in rows.iter().enumerate() {
        s.write(&format!("lib/j/r{i}.{ext}"), text);
        inputs.push_str(&format!("\"r{i}.{ext}\", "));
    }
    s.write(
        "lib/j/BUILD.buri",
        &format!("library {{\n    generators: [\n        {{ tool: \"//tools/gen\", inputs: [{inputs}] }},\n    ]\n}}\n"),
    );

    let run = s.run(&["lint", "//lib/j", "--error-format=json"]);
    run.exits(1);
    let mut found: Vec<(String, String)> = Vec::new();
    for line in run.stdout.lines().chain(run.stderr.lines()).filter(|l| l.starts_with('{')) {
        let d = buri::json::parse(line).unwrap();
        let text = |path: &str| d.at(path).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        found.push((text("location.file"), format!("{} {}", text("code"), text("message"))));
    }
    let mut failures = Vec::new();
    for (i, (ext, text, problem)) in rows.iter().enumerate() {
        let file = format!("lib/j/r{i}.{ext}");
        let said: Vec<&str> = found.iter().filter(|(f, _)| *f == file).map(|(_, m)| m.as_str()).collect();
        let want: Vec<String> = problem.iter().map(|p| format!("json-syntax {p}")).collect();
        if said != want {
            failures.push(format!("{ext} {text:?}: expected {want:?}, found {said:?}"));
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}
