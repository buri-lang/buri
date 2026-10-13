//! A repository platform's `js` file, written every way JavaScript allows.
//!
//! The build reads the file with a small tokenizer rather than a parser, so
//! these feed it the syntax a tokenizer can trip on: comments, strings,
//! templates, regular expressions, every export form, and files cut off in
//! the middle of each. The walkthrough's `fetch.mjs` stands in for the file;
//! everything else is `repositories/custom-platforms/cloudflare_kv`.
use crate::harness::*;

fn walkthrough() -> Scratch {
    Scratch::copy_of(
        "host-files",
        &tests_dir().join("repositories/custom-platforms/cloudflare_kv/repo"),
    )
}

const FILE: &str = "platform/cloudflare_worker/fetch.mjs";

/// Every token kind and export form at once, and the module the build writes
/// from it still answers the worker runtime, and exports what the file did.
const EVERY_FORM: &str = r#"// platform/cloudflare_worker/fetch.mjs, written every way the reader allows.
/* A block comment, with a * inside, a "quote", a 'tick' and a `template`. */
import { fetch as answer } from "buri:program";
import { sep } from "node:path"
const café = 'it\'s';
const $tag = `a ${ { x: `${1}` }.x } b \` c`;
const _price = `cost $5`;
const éclair = 1, snake_case = 2;
const pattern = /[/]x\/y/gi;
const half = 10 / 2;
const big = 1_000.5e3;
let bindings = {};
function check() {
  return pattern.test("/x/y") && half === 5 && big > 0 && éclair + snake_case === 3;
}

export default { fetch(request, env) { bindings = env; return answer(request); } };
export const HostKv = {
  "get": async function (self, namespace, key) { return (await bindings[namespace].get(key)) ?? undefined; },
  async put(self, namespace, key, value,) { return bindings[namespace].put(key, value); },
};
export { café as cafe, $tag, _price };
export * as path from "node:path";
export function helper({ a, b }, [c, d] = [], ...rest) { return check() ? a : sep; }
export let first = 1, { second, third: renamed = 2 } = { second: 2 };
export class Thing {}
export async function* stream() {}
// The file ends in a comment with no line break."#;

const CALL: &str = r#"
import worker, * as all from "./.buri/out/platform/cloudflare_worker/cmd/site/fetch.mjs";
const store = new Map([["session:t2", "ada"]]);
const kv = { get: async (key) => store.get(key) ?? null, put: async (key, value) => { store.set(key, value); } };
for (const token of ["t2", "t1"]) {
  const request = new Request("https://site.example/", { headers: { "x-session": token } });
  const response = await worker.fetch(request, { SESSIONS: kv });
  console.log(`${response.status} ${await response.text()}`);
}
console.log(Object.keys(all).sort().join(" "));
console.log(all.cafe, all.$tag, all._price, all.helper({ a: "a" }), all.first, all.second, all.renamed, typeof all.path.join);
"#;

#[test]
fn a_file_written_every_way_is_read_and_still_answers() {
    let scratch = walkthrough();
    scratch.write(FILE, EVERY_FORM);
    scratch.run(&["build", "//cmd/site"]).ok();
    scratch.write("call.mjs", CALL);
    let out = std::process::Command::new(js_runtime())
        .arg(scratch.path("call.mjs"))
        .current_dir(&scratch.root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the worker failed:\n{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout,
        "200 hello ada\n401 \n\
         $tag Thing _price cafe default first helper path renamed second stream\n\
         it's a 1 b ` c cost $5 a 1 2 2 function\n"
    );
}

/// The walkthrough's imports and default export, which every row below keeps.
const HEAD: &str = r#"import { fetch } from "buri:program";
let bindings = {};
export default { fetch(request, env) { bindings = env; return fetch(request); } };
"#;

/// The walkthrough's `HostKv`, which implements `Kv` as `platform.buri` asks.
const KV: &str = r#"export const HostKv = {
  get: async (self, namespace, key) => (await bindings[namespace].get(key)) ?? undefined,
  put: (self, namespace, key, value) => bindings[namespace].put(key, value),
};
"#;

/// Files cut off inside a token or a statement, and exports the reader can't
/// make sense of: the build still finds `HostKv` and builds the worker.
#[test]
fn a_file_cut_off_or_malformed_anywhere_still_builds() {
    let whole = format!("{HEAD}{KV}");
    let rows: Vec<(&str, String)> = vec![
        ("a block comment cut off", format!("{whole}/* cut off")),
        ("a string cut off", format!("{whole}const s = 'cut off")),
        ("a string broken by a line", format!("{whole}const s = 'broken\nconst t = 1;\n")),
        ("a template cut off", format!("{whole}const t = `cut ${{off")),
        ("a regular expression broken by a line", format!("{whole}const r = /cut\nconst t = 1;\n")),
        ("a regular expression cut off", format!("{whole}const r = /cut")),
        ("an identifier at the end", format!("{whole}last")),
        ("a number at the end", format!("{whole}1")),
        (
            "an object cut off",
            format!("{HEAD}export const HostKv = {{ get: (self, ns, key) => 1, put: (self, ns, key, value) => 1"),
        ),
        ("a statement cut off", format!("{whole}export let last = 1")),
        ("a pattern cut off", format!("{whole}export const {{ a")),
        ("an export list cut off", format!("{whole}export {{ a")),
        ("an export list without its commas", format!("{whole}const a = 1, c = 2;\nexport {{ a b, c as }};\n")),
        ("an export of nothing", format!("{whole}export")),
        ("an exported function without a name", format!("{whole}export function (a) {{}}\n")),
        ("an exported constant without a name", format!("{whole}export const = 1;\n")),
        ("an exported function without parameters", format!("{whole}export function f;\n")),
        (
            "a namespace import of the program",
            whole
                .replace("import { fetch } from", "import * as program from")
                .replace("return fetch(request)", "return program.fetch(request)"),
        ),
        ("a namespace import without a name", format!("import * from \"buri:program\";\n{whole}")),
        ("a default generator", format!("{whole}export default function* gen() {{}}\n")),
        (
            "destructured exports",
            format!(
                "{whole}const obj = {{ a: 1, b: 2 }}, arr = [1, [2], {{}}];\n\
                 export const {{ a, b }} = obj;\nexport const [c, [d], {{ e = a }}] = arr;\n"
            ),
        ),
        ("an import whose `;` is on the next line", format!("import {{ sep }} from \"node:path\"\n;\n{whole}")),
        ("keys named `async`", format!("{HEAD}{}", KV.replace("get:", "async: 1,\n  async(self) {},\n  get:"))),
    ];
    let scratch = walkthrough();
    for (name, text) in rows {
        scratch.write(FILE, &text);
        let run = scratch.run(&["build", "//cmd/site"]);
        assert_eq!(
            run.code,
            0,
            "{name}: the build refused the file:\n{}",
            indent(&run.all())
        );
    }
}

/// A method written as a one-parameter arrow takes one parameter, `self`, and
/// the build says so against the three `Kv` declares.
#[test]
fn a_one_parameter_arrow_is_counted() {
    let scratch = walkthrough();
    scratch.write(
        FILE,
        &format!(
            "{HEAD}{}",
            KV.replace("get: async (self, namespace, key) =>", "get: self =>")
        ),
    );
    scratch
        .run(&["build", "//cmd/site"])
        .exits(1)
        .says("`fetch.mjs` exports `HostKv` whose `get` takes 1 parameters, where `Kv` declares 3");
}

/// `as` is a name like any other in a default import, so `import as from
/// "buri:program"` is a default import, which `buri:program` doesn't have.
#[test]
fn a_default_import_named_as_is_refused() {
    let scratch = walkthrough();
    scratch.write(
        FILE,
        &format!("import as from \"buri:program\";\n{HEAD}{KV}"),
    );
    scratch
        .run(&["build", "//cmd/site"])
        .exits(1)
        .says("\"buri:program\" does not export `default`");
}
