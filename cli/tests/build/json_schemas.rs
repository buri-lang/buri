//! What a JSON schema may say, through `buri lint`: each mistake a schema can
//! make and the finding it draws, and the checks of a value that the official
//! test suite does not reach.
//!
//! [`PROBLEMS`] puts each subschema under `$defs` in one schema file, a line
//! apiece, so a finding's line names its row. `{N}` stands for the row's own
//! number, for a row that points into itself. [`VALUES`] does the same with
//! one property per row, and a JSON5 data file giving each its value.

use crate::harness::*;

const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

/// A subschema, and what it draws: `code message`, or nothing.
const PROBLEMS: &[(&str, &str)] = &[
    (r#"{"$id": "rel"}"#, r#"json-invalid-schema `"$id": "rel"` is relative, which only a schema's root may be"#),
    (r#"{"$id": 5}"#, "json-invalid-schema `$id` is a string"),
    (r#"{"$schema": "http://json-schema.org/draft-07/schema#"}"#, "json-schema-draft-unsupported `http://json-schema.org/draft-07/schema#` is not JSON Schema 2020-12"),
    (r#"{"$schema": 7}"#, "json-schema-draft-unsupported `7` is not JSON Schema 2020-12"),
    (r#"{"$schema": "https://json-schema.org/draft/2020-12/schema#"}"#, ""),
    (r#"{"$anchor": "1a"}"#, "json-invalid-schema `$anchor` is a name: a letter or `_`, then letters, digits, `-`, `.` or `_`"),
    (r#"{"$dynamicAnchor": ""}"#, "json-invalid-schema `$dynamicAnchor` is a name: a letter or `_`, then letters, digits, `-`, `.` or `_`"),
    (r#"{"$anchor": "a!"}"#, "json-invalid-schema `$anchor` is a name: a letter or `_`, then letters, digits, `-`, `.` or `_`"),
    (r#"{"$anchor": "_ok-1.x"}"#, ""),
    (r#"{"$ref": 1}"#, "json-invalid-schema `$ref` is a string"),
    (r#"{"$dynamicRef": 1}"#, "json-invalid-schema `$dynamicRef` is a string"),
    (r#"{"allOf": []}"#, "json-invalid-schema `allOf` is a non-empty array of schemas"),
    (r#"{"anyOf": {}}"#, "json-invalid-schema `anyOf` is a non-empty array of schemas"),
    (r#"{"properties": []}"#, "json-invalid-schema `properties` is an object of schemas"),
    (r#"{"type": []}"#, "json-invalid-schema `type` is one of null, boolean, object, array, number, string, integer, or a list of them"),
    (r#"{"type": ["string", "string"]}"#, "json-invalid-schema `type` is one of null, boolean, object, array, number, string, integer, or a list of them"),
    (r#"{"type": ["string", 1]}"#, "json-invalid-schema `type` is one of null, boolean, object, array, number, string, integer, or a list of them"),
    (r#"{"type": ["string", "text"]}"#, "json-invalid-schema `type` is one of null, boolean, object, array, number, string, integer, or a list of them"),
    (r#"{"type": "strin"}"#, "json-invalid-schema `type` is one of null, boolean, object, array, number, string, integer, or a list of them"),
    (r#"{"type": 1}"#, "json-invalid-schema `type` is one of null, boolean, object, array, number, string, integer, or a list of them"),
    (r#"{"enum": 1}"#, "json-invalid-schema `enum` is an array"),
    (r#"{"multipleOf": 0}"#, "json-invalid-schema `multipleOf` is a number greater than zero"),
    (r#"{"multipleOf": -2}"#, "json-invalid-schema `multipleOf` is a number greater than zero"),
    (r#"{"multipleOf": "x"}"#, "json-invalid-schema `multipleOf` is a number greater than zero"),
    (r#"{"minimum": "1"}"#, "json-invalid-schema `minimum` is a number"),
    (r#"{"minLength": 1.5}"#, "json-invalid-schema `minLength` is a whole number, zero or more"),
    (r#"{"maxItems": -1}"#, "json-invalid-schema `maxItems` is a whole number, zero or more"),
    (r#"{"minItems": "1"}"#, "json-invalid-schema `minItems` is a whole number, zero or more"),
    (r#"{"uniqueItems": 1}"#, "json-invalid-schema `uniqueItems` is `true` or `false`"),
    (r#"{"pattern": 1}"#, "json-invalid-schema `pattern` is a string"),
    (r#"{"required": ["a", "a"]}"#, "json-invalid-schema `required` is an array of property names, each once"),
    (r#"{"required": [1]}"#, "json-invalid-schema `required` is an array of property names, each once"),
    (r#"{"required": "a"}"#, "json-invalid-schema `required` is an array of property names, each once"),
    (r#"{"dependentRequired": {"a": [1]}}"#, "json-invalid-schema `dependentRequired` maps a property name to an array of names"),
    (r#"{"dependentRequired": []}"#, "json-invalid-schema `dependentRequired` maps a property name to an array of names"),
    (r#"{"items": [{}]}"#, "json-invalid-schema `items` holds one schema in 2020-12"),
    (r#"{"additionalItems": {}}"#, "json-invalid-schema `additionalItems` is not a JSON Schema 2020-12 keyword"),
    (r#"{"not": 5}"#, "json-invalid-schema a schema is an object or `true` or `false`"),
    (r##"{"$ref": "#/nope"}"##, "json-invalid-schema `#/nope` points at nothing in the schema"),
    (r##"{"$ref": "#/$defs/d{N}/required/x", "required": ["a"]}"##, "json-invalid-schema `#/$defs/d{N}/required/x` points at nothing in the schema"),
    (r##"{"$ref": "#/$defs/d{N}/type/x", "type": "string"}"##, "json-invalid-schema `#/$defs/d{N}/type/x` points at nothing in the schema"),
    (r##"{"$ref": "#/%zz"}"##, "json-invalid-schema `#/%zz` points at nothing in the schema"),
    (r##"{"$ref": "#nope"}"##, "json-invalid-schema `#nope` points at nothing in the schema"),
    (r#"{"$ref": "1a:x"}"#, "unknown-schema there is no schema at `lib/s/1a:x`"),
    (r#"{"$ref": "a_b:c"}"#, "unknown-schema there is no schema at `lib/s/a_b:c`"),
    (r#"{"$ref": "urn:nothing"}"#, "schema-outside-repository `urn:nothing` is not a file in this repository"),
    (r#"{"$ref": "//"}"#, "schema-outside-repository `//` is not a file in this repository"),
    (r#"{"$ref": "/abs.json"}"#, "schema-outside-repository `/abs.json` is not a file in this repository"),
    (r##"{"$ref": "#/%24defs/d{N}"}"##, ""),
    // A relative `$id` under an absolute one, resolved each way RFC 3986 does.
    (r#"{"$id": "http://example.com", "$defs": {"a": {"$id": "x.json"}}}"#, ""),
    (r#"{"$id": "urn:abc", "$defs": {"a": {"$id": "def"}}}"#, ""),
    (r#"{"$id": "http://e.com/a/b/", "$defs": {"a": {"$id": "../c/"}, "b": {"$id": "./d"}, "c": {"$id": "x/.."}}}"#, ""),
    (r#"{"$id": "http://e.com/", "$defs": {"a": {"$id": "../../x"}}}"#, ""),
    (r#"{"$id": "http://e.com/a", "$defs": {"a": {"$id": "//other.com/y"}, "b": {"$id": ""}, "c": {"$id": "/abs"}}}"#, ""),
    (r#"{"$id": "http://e.com/d/s.json", "$ref": "ok.schema.json"}"#, ""),
    (r#"{"$id": "http://e.com/d/t.json", "$ref": "nothing.schema.json"}"#, "unknown-schema there is no schema at `lib/s/nothing.schema.json`"),
    // A schema reached only through a `$ref` into a member that is no keyword,
    // first at its inside and then as a whole.
    (r##"{"x": {"properties": {"y": {"minimum": "a"}}}, "allOf": [{"$ref": "#/$defs/d{N}/x/properties/y"}, {"$ref": "#/$defs/d{N}/x"}]}"##, "json-invalid-schema `minimum` is a number"),
];

/// A property's schema, its value in the JSON5 data file, and the violation it
/// draws, or nothing.
const VALUES: &[(&str, &str, &str)] = &[
    (r##"{"$ref": "#/properties/v{N}"}"##, "1", "the schema refers to itself without end at this value"),
    (r#"{"enum": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]}"#, "12", "`12` is not one of the values `enum` allows: `1`, `2`, `3`, `4`, `5`, `6`, `7`, `8`, `9`, `10`, …"),
    (r#"{"minimum": 1}"#, "Infinity", "`Infinity` is not a finite number, and `minimum` asks for one"),
    (r#"{"maximum": 1}"#, "-Infinity", "`-Infinity` is not a finite number, and `maximum` asks for one"),
    (r#"{"multipleOf": 2}"#, "NaN", "`NaN` is not a finite number, and `multipleOf` asks for one"),
    (r#"{"$dynamicRef": "ok.schema.json"}"#, "1", ""),
    (r##"{"$ref": "#/%24defs/str"}"##, "1", "expected a string, found an integer"),
    (r#"{"type": "string"}"#, "'a long string of more than forty characters, cut short'", ""),
    (r#"{"const": 1}"#, "'a long string of more than forty characters, cut short'", "expected `1`, found `'a long string of more than forty charac…`"),
];

#[test]
fn json_schema_problems_and_values() {
    let s = Scratch::repo("json-schemas");
    s.write("tools/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    s.write("tools/gen/tool.buri", crate::json_schema_suite::GENERATOR);
    s.write("lib/s/lib.buri", "export let nothing: Int = 0;\n");
    s.write(
        "lib/s/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"//tools/gen\", inputs: [\"problems.json\", \"values.json5\"] },\n    ]\n}\n",
    );
    // A root may name itself with a relative `$id`.
    s.write("lib/s/ok.schema.json", &format!("{{\"$schema\": \"{DRAFT}\", \"$id\": \"ok.schema.json\"}}\n"));

    let mut problems = format!("{{\"$schema\": \"{DRAFT}\", \"$defs\": {{\n");
    for (i, (sub, _)) in PROBLEMS.iter().enumerate() {
        problems.push_str(&format!("\"d{i}\": {},\n", sub.replace("{N}", &i.to_string())));
    }
    problems.push_str("\"end\": true}}\n");
    s.write("lib/s/problems.schema.json", &problems);
    s.write("lib/s/problems.json", "{\"$schema\": \"problems.schema.json\"}\n");

    let mut schema = format!("{{\"$schema\": \"{DRAFT}\", \"$defs\": {{\"str\": {{\"type\": \"string\"}}}}, \"properties\": {{\n");
    let mut data = String::from("{\"$schema\": \"values.schema.json\",\n");
    for (i, (sub, value, _)) in VALUES.iter().enumerate() {
        schema.push_str(&format!("\"v{i}\": {},\n", sub.replace("{N}", &i.to_string())));
        data.push_str(&format!("\"v{i}\": {value},\n"));
    }
    schema.push_str("\"$schema\": true}}\n");
    data.push_str("}\n");
    s.write("lib/s/values.schema.json", &schema);
    s.write("lib/s/values.json5", &data);

    let run = s.run(&["lint", "//lib/s", "--error-format=json"]);
    run.exits(1);
    let mut said: Vec<(String, u32, String)> = Vec::new();
    for line in run.stdout.lines().chain(run.stderr.lines()).filter(|l| l.starts_with('{')) {
        let d = buri::json::parse(line).unwrap();
        let text = |path: &str| d.at(path).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let line = d.at("location.line").and_then(|v| v.as_u32()).unwrap();
        said.push((text("location.file"), line, format!("{} {}", text("code"), text("message"))));
    }
    let at = |file: &str, line: usize| -> Vec<String> {
        let mut found: Vec<String> = said.iter().filter(|(f, l, _)| f == file && *l as usize == line).map(|(_, _, m)| m.clone()).collect();
        found.dedup();
        found
    };

    let mut failures = Vec::new();
    // Row `i` is on line `i + 2`, after the line that opens `$defs`.
    for (i, (sub, want)) in PROBLEMS.iter().enumerate() {
        let want: Vec<String> = Some(want.replace("{N}", &i.to_string())).filter(|w| !w.is_empty()).into_iter().collect();
        let found = at("lib/s/problems.schema.json", i + 2);
        if found != want {
            failures.push(format!("{sub}: expected {want:?}, found {found:?}"));
        }
    }
    for (i, (sub, value, want)) in VALUES.iter().enumerate() {
        let want: Vec<String> = Some(want).filter(|w| !w.is_empty()).map(|w| format!("json-schema-violation {w}")).into_iter().collect();
        let found = at("lib/s/values.json5", i + 2);
        if found != want {
            failures.push(format!("{sub} against {value}: expected {want:?}, found {found:?}"));
        }
    }
    let elsewhere: Vec<_> = said.iter().filter(|(f, _, _)| f != "lib/s/problems.schema.json" && f != "lib/s/values.json5").collect();
    assert!(failures.is_empty() && elsewhere.is_empty(), "{} failures:\n{}\nelsewhere: {elsewhere:#?}", failures.len(), failures.join("\n"));
}

/// What a data file's `"$schema"` may name, and how far a check follows the
/// files a schema reaches: a value that is no string, a draft by its URL, a
/// schema that names itself, a file that does not read, and a chain of more
/// files than a check reads.
#[test]
fn json_schema_paths() {
    let s = Scratch::repo("json-schema-paths");
    s.write("tools/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    s.write("tools/gen/tool.buri", crate::json_schema_suite::GENERATOR);
    s.write("lib/p/lib.buri", "export let nothing: Int = 0;\n");
    let inputs = ["number.json", "draft.json", "self.schema.json", "broken.json", "chain.json"];
    s.write(
        "lib/p/BUILD.buri",
        &format!("library {{\n    generators: [\n        {{ tool: \"//tools/gen\", inputs: {inputs:?} }},\n    ]\n}}\n"),
    );
    s.write("lib/p/number.json", "{\"$schema\": 1}\n");
    s.write("lib/p/draft.json", "{\"$schema\": \"http://json-schema.org/draft-07/schema#\"}\n");
    s.write(
        "lib/p/self.schema.json",
        &format!("{{\"$schema\": \"{DRAFT}\", \"$defs\": {{\"a\": {{\"$ref\": \"self.schema.json#/$defs/b\"}}, \"b\": {{}}}}}}\n"),
    );
    s.write("lib/p/broken.json", "{\"$schema\": \"broken.schema.json\"}\n");
    s.write("lib/p/broken.schema.json", &format!("{{\"$schema\": \"{DRAFT}\", \"$ref\": \"bad.schema.json\"}}\n"));
    s.write("lib/p/bad.schema.json", "{\n");
    // A check reads at most a hundred schema files, so the hundredth's `$ref`
    // reaches nothing, and nothing reads the last.
    s.write("lib/p/chain.json", "{\"$schema\": \"chain0.schema.json\"}\n");
    for i in 0..100 {
        s.write(&format!("lib/p/chain{i}.schema.json"), &format!("{{\"$ref\": \"chain{}.schema.json\"}}\n", i + 1));
    }
    s.write("lib/p/chain100.schema.json", "{}\n");

    let run = s.run(&["lint", "//lib/p", "--error-format=json"]);
    run.exits(1);
    let mut said: Vec<String> = Vec::new();
    for line in run.stdout.lines().chain(run.stderr.lines()).filter(|l| l.starts_with('{')) {
        let d = buri::json::parse(line).unwrap();
        let text = |path: &str| d.at(path).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        said.push(format!("{} {} {}", text("location.file"), text("code"), text("message")));
    }
    said.sort();
    said.dedup();
    let want = [
        "lib/p/BUILD.buri unused-source lib/p/chain100.schema.json belongs to no library or binary, so nothing builds it",
        "lib/p/bad.schema.json json-syntax this object is never closed",
        "lib/p/chain99.schema.json unknown-schema there is no schema at `lib/p/chain100.schema.json`",
        "lib/p/draft.json json-schema-draft-unsupported `http://json-schema.org/draft-07/schema#` is not JSON Schema 2020-12",
        "lib/p/number.json schema-outside-repository `1` is not a file in this repository",
    ];
    assert_eq!(said, want);
}
