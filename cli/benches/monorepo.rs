//! The application monorepo the throughput goals are measured on
//! (`design/PERFORMANCE.md` §6.82), beside `workspace.rs`'s library-heavy one.
//!
//! What a large application repository has that the library workspace lacks,
//! from a fixed seed:
//!
//! * **Many small packages** in a deep graph: each depends on up to five of
//!   the twenty packages before it, and uses a record of each.
//! * **Test suites**, one a package, about as many test files as sources.
//! * **Generators the build runs**: a repository tool that embeds JSON fixtures,
//!   and the toolchain's `proto`. Both run as JavaScript programs, so the
//!   build starts the JavaScript runtime once a generator.
//! * **JavaScript outputs**: `node` programs and `web` pages, beside native
//!   programs.
//! * **Application source**: records read from and written to JSON, enums
//!   named by strings, literal tables, JSON fixtures, and long plain functions.
//!   Few generics, fewer traits, and a fraction of the code reachable from any
//!   one program.
//! * **Runs generic over their state**: each package's steps take a run, match
//!   on an event and step into the one before, logging through a dependency's
//!   run as well as their own. A page's buttons step a package's run, so style
//!   extraction follows what a node reads into all of them, over the rounds
//!   it takes a generic parameter to be found going further.
//! * **A design system** (`monorepo_ui.rs`) the pages are built from.

use super::super::generate::{laid_out, nonblank_lines, Rng};
use super::File;

#[path = "monorepo_ui.rs"]
mod design;

/// How large a monorepo to write.
pub struct Monorepo {
    /// Library packages.
    pub packages: usize,
    /// Source modules in each.
    pub modules: usize,
    /// Non-blank lines a source module is filled to.
    pub module_lines: usize,
    /// Native programs, `node` programs and `web` pages.
    pub programs: usize,
    pub scripts: usize,
    pub pages: usize,
    /// One package in this many embeds JSON fixtures, and one in
    /// `protos` reads a protocol buffer schema.
    pub fixtures: usize,
    pub protos: usize,
    /// How many of a package's nearest dependencies it calls the whole of,
    /// through their `entry`: how much of the graph a program reaches.
    pub calls: usize,
    /// Messages in a protocol buffer schema.
    pub messages: usize,
    /// Design system packages, the component modules in each, and the
    /// components in a module (at least one of each kind).
    pub ui_packages: usize,
    pub ui_modules: usize,
    pub components: usize,
    /// Steps of a run generic over its state in each module, each stepping
    /// into the one before it, the first into the module before's last.
    pub machines: usize,
}

/// The dependencies of package `k`: up to five of the twenty before it.
fn dependencies(rng: &mut Rng, k: usize) -> Vec<usize> {
    let mut deps = Vec::new();
    let want = k.min(2 + rng.below(4) as usize);
    let window = k.min(20) as u64;
    while deps.len() < want {
        let d = k - 1 - rng.below(window) as usize;
        if !deps.contains(&d) {
            deps.push(d);
        }
    }
    deps.sort_unstable();
    deps
}

fn package(k: usize) -> String {
    format!("//pkg/p{k:03}")
}

fn quoted(xs: impl Iterator<Item = String>) -> String {
    xs.map(|x| format!("\"{x}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every file of the monorepo, in a fixed order, the `REPO.buri` included.
pub fn files(m: &Monorepo) -> Vec<File> {
    let mut rng = Rng::new(0x4d4f_4e4f_5245_504f);
    let mut out = vec![File {
        path: "REPO.buri".into(),
        text: "# A generated monorepo (cli/benches/monorepo.rs).\n\
               lint {\n    check_during_build: true\n}\n"
            .into(),
    }];
    out.push(File {
        path: "tools/embed/BUILD.buri".into(),
        text: "tool {\n    generate {}\n}\n".into(),
    });
    out.push(File {
        path: "tools/embed/tool.buri".into(),
        text: EMBED.into(),
    });
    out.push(File {
        path: "schemas/fixture.schema.json".into(),
        text: FIXTURE_SCHEMA.into(),
    });
    let mut entries = Vec::new();
    for k in 0..m.packages {
        let deps = dependencies(&mut rng, k);
        let fx = library(&mut out, m, k, &deps, &entries, &mut rng);
        entries.push(fx);
    }
    let fx = |used: &[usize]| used.iter().fold(0, |a, d| a | entries[*d]);
    let draw = |rng: &mut Rng, uses: usize| {
        let mut used = Vec::new();
        while used.len() < uses.min(m.packages) {
            let d = rng.below(m.packages as u64) as usize;
            if !used.contains(&d) {
                used.push(d);
            }
        }
        used.sort_unstable();
        used
    };
    for j in 0..m.programs {
        let used = draw(&mut rng, 3);
        program(&mut out, j, &used, fx(&used));
    }
    for j in 0..m.scripts {
        let used = draw(&mut rng, 2);
        script(&mut out, j, &used, fx(&used));
    }
    let components = design::files(&mut out, m, &mut rng);
    for j in 0..m.pages {
        let used = draw(&mut rng, 2);
        // One component of each kind, from any package.
        let shown: Vec<design::Component> = design::KINDS
            .iter()
            .filter(|_| !components.is_empty())
            .map(|kind| {
                let theirs = &components[rng.below(components.len() as u64) as usize];
                let of: Vec<&design::Component> =
                    theirs.iter().filter(|c| c.kind == *kind).collect();
                of[rng.below(of.len() as u64) as usize].clone()
            })
            .collect();
        page(&mut out, j, &used, &shown, fx(&used));
    }
    for f in &mut out {
        if f.path.ends_with(".buri") && !f.path.ends_with("BUILD.buri") && f.path != "REPO.buri" {
            f.text = laid_out(&f.path, std::mem::take(&mut f.text));
        }
    }
    out
}

/// The repository's generator: each JSON fixture it is handed becomes two
/// constants, how many members the document has and the document itself.
const EMBED: &str = r#"//! Embeds JSON fixtures as constants.

from "core/buri/ast" import * as ast;
from "core/json" import * as json;
from "core/json" import { Json };
from "core/str" import * as str;
from "core/tool" import { Generated, GenerateRequest, Input };
from "platform/effect" import { Allocator };

/// The name a fixture's constants are declared under: its file name, less `.json`.
fn stem<C: Allocator>(ctx: C, path: Str): Str {
    let name = path.split(ctx, "/").last().withDefault(path);
    name.stripSuffix(".json").withDefault(name)
}

/// Members of an object, elements of an array, and nothing else.
fn members(doc: Json): Int {
    match (doc) {
        .Object(fields) => fields.length(),
        .Array(items) => items.length(),
        _ => 0,
    }
}

/// The two constants of one fixture.
fn declare<C: Allocator>(ctx: C, input: Input<Str>): Str {
    let name = stem(ctx, input.path);
    match (json.parse(ctx, input.value)) {
        .Ok(doc) => {
            let text = json.quote(ctx, json.stringify(ctx, doc));
            str.format(
                ctx,
                "export let ${name}Members: Int = ${members(doc)};\nexport let ${name}Text: Str = ${text};\n",
            )
        },
        .Err(_) => str.format(ctx, "export let ${name}Members: Int = 0;\n"),
    }
}

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    let source = request.inputs.mapCtx(ctx, fn(c, input) => declare(c, input)).join(ctx, "");
    let modules = match (ast.parse(ctx, "", source)) {
        .Ok(parsed) => [("fixtures", parsed)],
        .Err(_) => [],
    };
    Generated { modules: modules, diagnostics: [], needs: [] }
}
"#;

/// The schema every JSON fixture names.
const FIXTURE_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["kind", "items"],
  "properties": {
    "$schema": { "type": "string" },
    "kind": { "type": "string" },
    "items": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "tags"],
        "properties": {
          "id": { "type": "string" },
          "tags": { "type": "array", "items": { "type": "string" } }
        }
      }
    }
  }
}
"#;

const WORDS: [&str; 24] = [
    "account", "invoice", "order", "shipment", "ticket", "session", "member", "report", "payment",
    "booking", "device", "region", "channel", "message", "project", "task", "release", "policy",
    "contract", "vendor", "budget", "survey", "license", "route",
];

const FIELDS: [&str; 16] = [
    "title",
    "owner",
    "status",
    "region",
    "source",
    "target",
    "label",
    "kind",
    "remark",
    "summary",
    "reference",
    "currency",
    "locale",
    "contact",
    "origin",
    "category",
];

const NUMBERS: [&str; 8] = [
    "count", "limit", "total", "version", "priority", "attempts", "quantity", "weight",
];

const SENTENCES: [&str; 12] = [
    "is waiting for a review before it can be published",
    "was moved to the archive after ninety days without activity",
    "has been reassigned to the team that owns the region",
    "needs a second approval because the amount is over the limit",
    "is blocked until the upstream service answers again",
    "was imported from the previous system and kept as it was",
    "has a reference that no longer resolves to a record",
    "is ready to send once the attachments are uploaded",
    "will be retried with a longer timeout on the next run",
    "has been split into two parts that are tracked on their own",
    "belongs to an account that was closed last quarter",
    "uses the default settings because none were given",
];

/// A record's shape: its string and number fields, beside the four every
/// record has.
struct Record {
    name: String,
    word: &'static str,
    texts: Vec<&'static str>,
    numbers: Vec<&'static str>,
}

impl Record {
    fn draw(rng: &mut Rng, name: String) -> Record {
        let word = *rng.pick(&WORDS);
        let mut texts = Vec::new();
        for _ in 0..(2 + rng.below(3)) {
            let f = *rng.pick(&FIELDS);
            if !texts.contains(&f) {
                texts.push(f);
            }
        }
        let mut numbers = Vec::new();
        for _ in 0..(1 + rng.below(3)) {
            let f = *rng.pick(&NUMBERS);
            if !numbers.contains(&f) {
                numbers.push(f);
            }
        }
        Record {
            name,
            word,
            texts,
            numbers,
        }
    }

    /// A literal of the record, every field given.
    fn literal(&self, rng: &mut Rng) -> String {
        let mut s = format!("{} {{ id: \"{}-{}\"", self.name, self.word, rng.below(1000));
        for f in &self.texts {
            s.push_str(&format!(", {f}: \"{}\"", rng.pick(&SENTENCES)));
        }
        for f in &self.numbers {
            s.push_str(&format!(", {f}: {}", rng.below(500)));
        }
        s.push_str(&format!(
            ", active: true, tags: [\"{}\", \"{}\"], note: .Some(\"{}\") }}",
            rng.pick(&WORDS),
            rng.pick(&FIELDS),
            rng.pick(&SENTENCES)
        ));
        s
    }
}

/// What a module declared that its tests and its summary call.
#[derive(Default)]
struct Declared {
    /// Lines of `summary`'s sum, each an `Int`, and the effects each needs.
    reach: Vec<(String, Fx)>,
    /// Test declarations.
    tests: Vec<String>,
    /// Names the package's surface exports for the tests.
    exports: Vec<String>,
    json: bool,
    decode: bool,
    /// Whether the module reads the clock and draws random numbers.
    effects: bool,
    /// The module's last step of its package's run.
    machine: Option<String>,
}

impl Declared {
    /// A line of the sum that needs only an allocator.
    fn sum(&mut self, line: String) {
        self.reach.push((line, 0));
    }
}

/// Effects beyond an allocator, as bits: [`CLOCK`] and [`RANDOM`].
type Fx = u8;
const CLOCK: Fx = 1;
const RANDOM: Fx = 2;

/// The context bound of a function needing `fx`.
fn bound(fx: Fx) -> String {
    let mut s = String::from("Allocator");
    if fx & CLOCK != 0 {
        s.push_str(" + Clock");
    }
    if fx & RANDOM != 0 {
        s.push_str(" + Random");
    }
    s
}

/// The context fields a host gives for `fx`, beside its allocator.
fn fields(fx: Fx) -> String {
    let mut s = String::new();
    if fx & CLOCK != 0 {
        s.push_str("        Clock: host.clock,\n");
    }
    if fx & RANDOM != 0 {
        s.push_str("        Random: host.rand,\n");
    }
    s
}

fn record(out: &mut String, d: &mut Declared, r: &Record, rng: &mut Rng) {
    let n = &r.name;
    let w = r.word;
    let cap = 3 + rng.below(6);
    let first_number = r.numbers[0];
    out.push_str(&format!(
        "/// A {w} as the service stores it.\n\
         derive Equal, Show, ToJson, FromJson for {n};\n\
         export struct {n} {{\n\
         \x20   export id: Str,\n"
    ));
    for f in &r.texts {
        out.push_str(&format!("    export {f}: Str,\n"));
    }
    for f in &r.numbers {
        out.push_str(&format!("    export {f}: Int,\n"));
    }
    out.push_str(
        "    export active: Bool,\n    export tags: [Str],\n    export note: Option<Str>,\n}\n\n",
    );
    let blanks: String = r
        .texts
        .iter()
        .map(|f| format!("{f}: \"\", "))
        .chain(r.numbers.iter().map(|f| format!("{f}: 0, ")))
        .collect();
    out.push_str(&format!(
        "/// A {w} with every field at its default.\n\
         export fn blank{n}(id: Str): {n} {{\n\
         \x20   {n} {{ id: id, {blanks}active: false, tags: [], note: .None }}\n\
         }}\n\n\
         /// The {w}, or why it cannot be stored.\n\
         export fn check{n}(r: {n}): Result<{n}, Str> {{\n\
         \x20   if (r.id.isEmpty()) {{\n\
         \x20       .Err(\"a {w} needs an id before it can be stored\")\n\
         \x20   }} else if (r.{first_number} < 0) {{\n\
         \x20       .Err(\"the {first_number} of a {w} cannot be negative\")\n\
         \x20   }} else if (r.tags.length() > {cap}) {{\n\
         \x20       .Err(\"a {w} carries at most {cap} tags\")\n\
         \x20   }} else {{\n\
         \x20       .Ok(r)\n\
         \x20   }}\n\
         }}\n\n"
    ));
    let shown: Vec<String> = r
        .texts
        .iter()
        .chain(&r.numbers)
        .map(|f| format!("{f}=${{r.{f}}}"))
        .collect();
    out.push_str(&format!(
        "/// One line about the {w}, for the log.\n\
         export fn describe{n}<C: Allocator>(ctx: C, r: {n}): Str {{\n\
         \x20   str.format(ctx, \"{w} ${{r.id}}: {}, tags=${{r.tags.length()}}\")\n\
         }}\n\n",
        shown.join(", ")
    ));
    d.sum(format!("describe{n}(ctx, blank{n}(\"a\")).length()"));
    d.sum(format!(
        "check{n}(blank{n}(\"\")).withDefault(blank{n}(\"b\")).id.length()"
    ));
    d.sum(format!(
        "json.stringify(ctx, json.encode(ctx, blank{n}(\"j\"))).length()"
    ));
    d.json = true;
    d.exports.extend([
        n.clone(),
        format!("blank{n}"),
        format!("check{n}"),
        format!("describe{n}"),
    ]);
    d.decode = true;
    let full = r.literal(rng);
    d.tests.push(format!(
        "test \"{n}: a {w} is checked and survives a trip through JSON\" {{\n\
         \x20   let ctx = alloc();\n\
         \x20   assert.equal(check{n}(blank{n}(\"\")), .Err(\"a {w} needs an id before it can be stored\"));\n\
         \x20   let r = {full};\n\
         \x20   let back: Result<{n}, DecodeError> = json.decode(ctx, json.encode(ctx, r));\n\
         \x20   assert.equal(back, .Ok(r));\n\
         \x20   assert.containsText(describe{n}(ctx, r), \"{w} \");\n\
         }}\n\n"
    ));
}

fn states(out: &mut String, d: &mut Declared, name: &str, rng: &mut Rng) {
    let w = *rng.pick(&WORDS);
    let extra = rng.below(4) as usize;
    let more = ["Paused", "Expired", "Rejected", "Escalated"];
    out.push_str(&format!(
        "/// Where a {w} is in its life.\n\
         derive Equal, Show for {name};\n\
         export enum {name} {{\n\
         \x20   Draft,\n\
         \x20   Pending(Str),\n\
         \x20   Active {{ since: Int, by: Str }},\n\
         \x20   Closed,\n"
    ));
    for m in &more[..extra] {
        out.push_str(&format!("    {m},\n"));
    }
    out.push_str("}\n\n");
    out.push_str(&format!(
        "/// The state's name, as the API spells it.\n\
         export fn nameOf{name}(s: {name}): Str {{\n\
         \x20   match (s) {{\n\
         \x20       .Draft => \"draft\",\n\
         \x20       .Pending(reason) => reason,\n\
         \x20       .Active {{ by, .. }} => by,\n\
         \x20       .Closed => \"closed\",\n"
    ));
    for m in &more[..extra] {
        out.push_str(&format!("        .{m} => \"{}\",\n", m.to_lowercase()));
    }
    out.push_str("    }\n}\n\n");
    out.push_str(&format!(
        "/// The state an API name stands for, if any.\n\
         export fn parse{name}(text: Str): Option<{name}> {{\n\
         \x20   if (text == \"draft\") {{\n\
         \x20       .Some(.Draft)\n\
         \x20   }} else if (text == \"closed\") {{\n\
         \x20       .Some(.Closed)\n"
    ));
    for m in &more[..extra] {
        out.push_str(&format!(
            "    }} else if (text == \"{}\") {{\n        .Some(.{m})\n",
            m.to_lowercase()
        ));
    }
    out.push_str(
        "    } else if (text.startsWith(\"pending:\")) {\n\
         \x20       .Some(.Pending(text.slice(8, text.length())))\n\
         \x20   } else {\n\
         \x20       .None\n\
         \x20   }\n\
         }\n\n",
    );
    d.sum(format!("nameOf{name}(.Closed).length()"));
    d.sum(format!(
        "parse{name}(\"draft\").map(fn(s) => nameOf{name}(s).length()).withDefault(0)"
    ));
    d.exports.extend([
        name.to_string(),
        format!("nameOf{name}"),
        format!("parse{name}"),
    ]);
    let mut t = format!(
        "test \"{name}: every {w} state has a name it reads back from\" {{\n\
         \x20   assert.equal(nameOf{name}(.Draft), \"draft\");\n\
         \x20   assert.equal(nameOf{name}(.Pending(\"on hold\")), \"on hold\");\n\
         \x20   assert.equal(nameOf{name}(.Active {{ since: 3, by: \"ops\" }}), \"ops\");\n\
         \x20   assert.equal(nameOf{name}(.Closed), \"closed\");\n"
    );
    for m in &more[..extra] {
        t.push_str(&format!(
            "    assert.equal(nameOf{name}(.{m}), \"{}\");\n",
            m.to_lowercase()
        ));
    }
    t.push_str(&format!(
        "    assert.equal(parse{name}(\"closed\"), .Some(.Closed));\n\
         \x20   assert.equal(parse{name}(\"pending:review\"), .Some(.Pending(\"review\")));\n\
         \x20   assert.equal(parse{name}(\"unknown\"), .None);\n\
         }}\n\n"
    ));
    d.tests.push(t);
}

fn table(out: &mut String, d: &mut Declared, name: &str, rng: &mut Rng) {
    let w = *rng.pick(&WORDS);
    let rows = 8 + rng.below(16) as usize;
    let fallback = 10 + rng.below(90);
    out.push_str(&format!(
        "/// One row of the {w} limits.\n\
         struct Row{name} {{\n\
         \x20   code: Str,\n\
         \x20   limit: Int,\n\
         \x20   text: Str,\n\
         }}\n\n\
         /// The {w} limits, by code.\n\
         let rows{name}: [Row{name}] = [\n"
    ));
    let mut first = (String::new(), 0);
    for r in 0..rows {
        let code = format!("{}-{}-{r}", w, rng.pick(&FIELDS));
        let limit = rng.below(10_000);
        if r == 0 {
            first = (code.clone(), limit);
        }
        out.push_str(&format!(
            "    Row{name} {{ code: \"{code}\", limit: {limit}, text: \"the {w} {}\" }},\n",
            rng.pick(&SENTENCES)
        ));
    }
    out.push_str(&format!(
        "];\n\n\
         /// The limit for `code`, or {fallback} for a code the table does not list.\n\
         export fn limit{name}(code: Str): Int {{\n\
         \x20   match (rows{name}.find(fn(row) => row.code == code)) {{\n\
         \x20       .Some(row) => row.limit,\n\
         \x20       .None => {fallback},\n\
         \x20   }}\n\
         }}\n\n\
         /// What the {w} limit for `code` is for, or why there is none.\n\
         export fn reason{name}(code: Str): Str {{\n\
         \x20   match (rows{name}.find(fn(row) => row.code == code)) {{\n\
         \x20       .Some(row) => row.text,\n\
         \x20       .None => \"no {w} limit is set for this code\",\n\
         \x20   }}\n\
         }}\n\n"
    ));
    d.sum(format!("limit{name}(\"{}\")", first.0));
    d.sum(format!("reason{name}(\"{}\").length()", first.0));
    d.exports
        .extend([format!("limit{name}"), format!("reason{name}")]);
    d.tests.push(format!(
        "test \"{name}: the {w} limits are read by code\" {{\n\
         \x20   assert.equal(limit{name}(\"{}\"), {});\n\
         \x20   assert.equal(limit{name}(\"not-a-code\"), {fallback});\n\
         \x20   assert.equal(reason{name}(\"not-a-code\"), \"no {w} limit is set for this code\");\n\
         }}\n\n",
        first.0, first.1
    ));
}

fn fixture(out: &mut String, d: &mut Declared, name: &str, rng: &mut Rng) {
    let w = *rng.pick(&WORDS);
    let members = 4 + rng.below(6) as usize;
    let mut doc = format!("{{\\\"id\\\": \\\"{w}-{}\\\"", rng.below(10_000));
    for m in 1..members {
        let f = FIELDS[(m + rng.below(4) as usize) % FIELDS.len()];
        if m % 3 == 0 {
            doc.push_str(&format!(", \\\"{f}{m}\\\": {}", rng.below(1_000_000)));
        } else {
            doc.push_str(&format!(
                ", \\\"{f}{m}\\\": \\\"{}\\\"",
                rng.pick(&SENTENCES)
            ));
        }
    }
    doc.push('}');
    out.push_str(&format!(
        "/// A {w} as the service sends it.\n\
         let sample{name}: Str = \"{doc}\";\n\n\
         /// How many members the sample has, or -1 when it does not parse.\n\
         export fn members{name}<C: Allocator>(ctx: C): Int {{\n\
         \x20   match (json.parse(ctx, sample{name})) {{\n\
         \x20       .Ok(.Object(fields)) => fields.length(),\n\
         \x20       .Ok(_) => 0,\n\
         \x20       .Err(_) => -1,\n\
         \x20   }}\n\
         }}\n\n"
    ));
    d.json = true;
    d.sum(format!("members{name}(ctx)"));
    d.exports.push(format!("members{name}"));
    d.tests.push(format!(
        "test \"{name}: the {w} sample parses\" {{\n\
         \x20   assert.equal(members{name}(alloc()), {members});\n\
         }}\n\n"
    ));
}

fn slug(out: &mut String, d: &mut Declared, name: &str, rng: &mut Rng) {
    let w = *rng.pick(&WORDS);
    let max = 20 + rng.below(30);
    out.push_str(&format!(
        "/// A {w} name as a URL segment: trimmed, lower case, words joined by\n\
         /// dashes, and at most {max} characters.\n\
         export fn slug{name}<C: Allocator>(ctx: C, raw: Str): Str {{\n\
         \x20   let lower = raw.trim().toLower(ctx);\n\
         \x20   let words = lower.split(ctx, \" \").filter(ctx, fn(p) => !p.isEmpty());\n\
         \x20   let joined = words.join(ctx, \"-\");\n\
         \x20   if (joined.isEmpty()) {{\n\
         \x20       \"{w}\"\n\
         \x20   }} else if (joined.length() > {max}) {{\n\
         \x20       joined.slice(0, {max})\n\
         \x20   }} else {{\n\
         \x20       joined\n\
         \x20   }}\n\
         }}\n\n"
    ));
    d.sum(format!("slug{name}(ctx, \"  A New {w} \").length()"));
    d.exports.push(format!("slug{name}"));
    d.tests.push(format!(
        "test \"{name}: a {w} slug is lower case and dashed\" {{\n\
         \x20   let ctx = alloc();\n\
         \x20   assert.equal(slug{name}(ctx, \"  The First  {w} \"), \"the-first-{w}\");\n\
         \x20   assert.equal(slug{name}(ctx, \"   \"), \"{w}\");\n\
         }}\n\n"
    ));
}

fn totals(out: &mut String, d: &mut Declared, name: &str, rng: &mut Rng) {
    let w = *rng.pick(&WORDS);
    let fee = 1 + rng.below(9);
    let cap = 1000 + rng.below(9000);
    let steps = 3 + rng.below(6) as usize;
    out.push_str(&format!(
        "/// What a batch of {w} amounts comes to: the positive ones, less a fee of\n\
         /// {fee} each, held under {cap}.\n\
         export fn total{name}<C: Allocator>(ctx: C, amounts: [Int]): Int {{\n\
         \x20   let positive = amounts.filter(ctx, fn(a) => a > 0);\n\
         \x20   let gross = positive.fold(fn(sum, a) => sum + a, 0);\n\
         \x20   let fees = positive.length() * {fee};\n\
         \x20   let net = gross - fees;\n"
    ));
    let mut last = String::from("net");
    for s in 0..steps {
        let op = *rng.pick(&["+", "-"]);
        out.push_str(&format!(
            "    let step{s} = if ({last} > {}) {{ {last} {op} {} }} else {{ {last} }};\n",
            rng.below(500),
            1 + rng.below(20)
        ));
        last = format!("step{s}");
    }
    out.push_str(&format!(
        "    if ({last} > {cap}) {{\n\
         \x20       {cap}\n\
         \x20   }} else if ({last} < 0) {{\n\
         \x20       0\n\
         \x20   }} else {{\n\
         \x20       {last}\n\
         \x20   }}\n\
         }}\n\n"
    ));
    d.sum(format!("total{name}(ctx, [3, -1, 40])"));
    d.exports.push(format!("total{name}"));
    d.tests.push(format!(
        "test \"{name}: a batch of {w} amounts never totals below zero\" {{\n\
         \x20   let ctx = alloc();\n\
         \x20   assert.greaterOrEqual(total{name}(ctx, [-5, -10]), 0);\n\
         \x20   assert.lessOrEqual(total{name}(ctx, [{cap}, {cap}, {cap}]), {cap});\n\
         }}\n\n"
    ));
}

fn lease(out: &mut String, d: &mut Declared, name: &str, rng: &mut Rng) {
    let w = *rng.pick(&WORDS);
    let minutes = 5 + rng.below(55);
    let tokens = 1000 + rng.below(1_000_000);
    out.push_str(&format!(
        "/// Who holds a {w} and until when.\n\
         export struct Lease{name} {{\n\
         \x20   export holder: Str,\n\
         \x20   export token: Int,\n\
         \x20   export until: Instant,\n\
         }}\n\n\
         /// A {minutes}-minute lease on a {w} for `holder`, with a random token.\n\
         export fn grant{name}<C: Clock + Random>(ctx: C, holder: Str): Lease{name} {{\n\
         \x20   let start = time.now(ctx);\n\
         \x20   Lease{name} {{\n\
         \x20       holder: holder,\n\
         \x20       token: random.int(ctx, 1, {tokens}),\n\
         \x20       until: start.plus(time.minutes({minutes})),\n\
         \x20   }}\n\
         }}\n\n\
         /// Why the lease needs renewing, or nothing while it holds.\n\
         export fn renewal{name}<C: Allocator + Clock>(ctx: C, lease: Lease{name}): Option<Str> {{\n\
         \x20   if (time.now(ctx).hasPassed(lease.until)) {{\n\
         \x20       .Some(str.format(ctx, \"the {w} lease of ${{lease.holder}} ran out\"))\n\
         \x20   }} else if (lease.token <= 0) {{\n\
         \x20       .Some(str.format(ctx, \"the {w} lease of ${{lease.holder}} has no token\"))\n\
         \x20   }} else {{\n\
         \x20       .None\n\
         \x20   }}\n\
         }}\n\n"
    ));
    d.effects = true;
    d.reach
        .push((format!("grant{name}(ctx, \"r\").token"), CLOCK | RANDOM));
    d.reach.push((
        format!(
            "renewal{name}(ctx, grant{name}(ctx, \"q\")).map(fn(s) => s.length()).withDefault(0)"
        ),
        CLOCK | RANDOM,
    ));
    d.exports.extend([
        format!("Lease{name}"),
        format!("grant{name}"),
        format!("renewal{name}"),
    ]);
    d.tests.push(format!(
        "test \"{name}: a fresh {w} lease holds\" {{\n\
         \x20   let ctx = context {{ Allocator: alloc(), Clock: clock(), Random: rand() }};\n\
         \x20   let lease = grant{name}(ctx, \"ops\");\n\
         \x20   assert.equal(lease.holder, \"ops\");\n\
         \x20   assert.equal(renewal{name}(ctx, lease), .None);\n\
         }}\n\n"
    ));
}

fn generic(out: &mut String, d: &mut Declared, name: &str) {
    out.push_str(&format!(
        "/// The item at `index`, or `fallback` past the end.\n\
         export fn pick{name}<T>(items: [T], index: Int, fallback: T): T {{\n\
         \x20   match (items.get(index)) {{\n\
         \x20       .Some(item) => item,\n\
         \x20       .None => fallback,\n\
         \x20   }}\n\
         }}\n\n"
    ));
    d.sum(format!("pick{name}([1, 2], 1, 0)"));
    d.sum(format!("pick{name}([\"a\"], 3, \"none\").length()"));
    d.exports.push(format!("pick{name}"));
    d.tests.push(format!(
        "test \"{name}: an item past the end is the fallback\" {{\n\
         \x20   assert.equal(pick{name}([\"a\", \"b\"], 1, \"z\"), \"b\");\n\
         \x20   assert.equal(pick{name}([\"a\"], 4, \"z\"), \"z\");\n\
         }}\n\n"
    ));
}

/// Package `k`'s run, generic over its state, and what starts and logs one.
fn run(out: &mut String, d: &mut Declared, k: usize, rng: &mut Rng) {
    let w = *rng.pick(&WORDS);
    let cap = 4 + rng.below(8);
    out.push_str(&format!(
        "/// A {w} run: the state so far, what happened, and how many steps it took.\n\
         export struct Run{k}<S> {{\n\
         \x20   export state: S,\n\
         \x20   export log: [Str],\n\
         \x20   export steps: Int,\n\
         }}\n\n\
         /// A run that has not stepped yet.\n\
         export fn start{k}<S>(state: S): Run{k}<S> {{\n\
         \x20   Run{k} {{ state: state, log: [], steps: 0 }}\n\
         }}\n\n\
         /// The last {cap} lines of `log`, `line` the newest.\n\
         export fn noted{k}<C: Allocator>(ctx: C, log: [Str], line: Str): [Str] {{\n\
         \x20   if (log.length() < {cap}) {{\n\
         \x20       log.push(ctx, line)\n\
         \x20   }} else {{\n\
         \x20       log.drop(ctx, 1).push(ctx, line)\n\
         \x20   }}\n\
         }}\n\n"
    ));
    d.exports
        .extend([format!("Run{k}"), format!("start{k}"), format!("noted{k}")]);
}

/// One step of package `k`'s run, logged through package `src`'s `noted`,
/// that steps into `prev` after it. `steps` is how many it takes in all.
#[allow(clippy::too_many_arguments)]
fn machine(
    out: &mut String,
    d: &mut Declared,
    k: usize,
    name: &str,
    src: usize,
    prev: Option<&str>,
    steps: usize,
    rng: &mut Rng,
) {
    let w = *rng.pick(&WORDS);
    let f = *rng.pick(&FIELDS);
    let tail = match prev {
        Some(p) => format!("stepped{p}(ctx, next, .Started, step)"),
        None => "next".to_string(),
    };
    out.push_str(&format!(
        "/// What can happen to a {w} run.\n\
         export enum Event{name} {{\n\
         \x20   Started,\n\
         \x20   Moved {{ by: Int, note: Str }},\n\
         \x20   Failed(Str),\n\
         \x20   Settled {{ at: Int }},\n\
         }}\n\n\
         /// A {w} step: `step` moves the state on, and the log says what happened.\n\
         export fn stepped{name}<S, C: Allocator>(\n\
         \x20   ctx: C,\n\
         \x20   run: Run{k}<S>,\n\
         \x20   event: Event{name},\n\
         \x20   step: fn(S) => S,\n\
         ): Run{k}<S> {{\n\
         \x20   let moved = match (event) {{\n\
         \x20       .Started => run,\n\
         \x20       .Moved {{ by, note }} => {{\n\
         \x20           let logged = noted{k}(ctx, run.log, note);\n\
         \x20           Run{k} {{ state: run.state, log: logged, steps: run.steps + by }}\n\
         \x20       }},\n\
         \x20       .Failed(why) => {{\n\
         \x20           let logged = noted{src}(ctx, run.log, why);\n\
         \x20           Run{k} {{ state: run.state, log: logged, steps: run.steps }}\n\
         \x20       }},\n\
         \x20       .Settled {{ at }} => Run{k} {{ state: run.state, log: run.log, steps: at }},\n\
         \x20   }};\n\
         \x20   let {f} = step(moved.state);\n\
         \x20   let next = Run{k} {{\n\
         \x20       state: {f},\n\
         \x20       log: noted{k}(ctx, moved.log, \"{w} {name}\"),\n\
         \x20       steps: moved.steps + 1,\n\
         \x20   }};\n\
         \x20   {tail}\n\
         }}\n\n"
    ));
    d.sum(format!(
        "stepped{name}(ctx, start{k}(1), .Moved {{ by: 2, note: \"{w}\" }}, fn(n) => n + 1).steps"
    ));
    d.exports.push(format!("stepped{name}"));
    d.tests.push(format!(
        "test \"{name}: a {w} run takes every step once\" {{\n\
         \x20   let run = stepped{name}(alloc(), start{k}(1), .Settled {{ at: 10 }}, fn(n) => n + 2);\n\
         \x20   assert.equal(run.steps, {});\n\
         \x20   assert.equal(run.state, {});\n\
         \x20   assert.equal(run.log.isEmpty(), false);\n\
         }}\n\n",
        10 + steps,
        1 + 2 * steps
    ));
}

/// Lines of a module's sum each of its parts adds.
const PART: usize = 25;

/// The kinds of declaration group a module is filled with, by weight:
/// records, states, tables, fixtures, slugs, totals, generics, leases.
const WEIGHTS: [u64; 8] = [5, 3, 2, 3, 2, 2, 1, 3];

/// Module `g` of package `k`, numbered `i` across the monorepo; its text and
/// its tests.
#[allow(clippy::too_many_arguments)]
fn module(
    m: &Monorepo,
    k: usize,
    g: usize,
    i: usize,
    deps: &[(usize, String, Option<Fx>)],
    first: Option<(&Record, &Generated)>,
    prev_fx: Fx,
    prev_machine: Option<&str>,
    rng: &mut Rng,
) -> (String, Declared, Fx) {
    let mut body = String::new();
    let mut d = Declared::default();
    let mut n = 0usize;
    if let Some((r, generated)) = first {
        record(&mut body, &mut d, r, rng);
        for (_, f) in &generated.functions {
            body.push_str(f);
        }
        d.reach
            .extend(generated.functions.iter().map(|(r, _)| (r.clone(), 0)));
        run(&mut body, &mut d, k, rng);
    }
    // The first step logs through a dependency's run, if the module uses one.
    let mut prev = prev_machine.map(str::to_string);
    let noted_from = deps.first().map(|(dep, _, _)| *dep);
    for j in 0..m.machines {
        let name = format!("{i}s{j}");
        let src = if j == 0 { noted_from.unwrap_or(k) } else { k };
        let steps = g * m.machines + j + 1;
        machine(
            &mut body,
            &mut d,
            k,
            &name,
            src,
            prev.as_deref(),
            steps,
            rng,
        );
        prev = Some(name);
    }
    d.machine = prev;
    for (dep, word, calls) in deps {
        body.push_str(&format!(
            "/// A {word} from p{dep:03}, as this package logs it.\n\
             fn borrowed{i}_{dep}<C: Allocator>(ctx: C, id: Str): Str {{\n\
             \x20   let other = blankItem{dep}(id);\n\
             \x20   let line = describeItem{dep}(ctx, other);\n\
             \x20   str.format(ctx, \"p{k:03} sees ${{line}}\")\n\
             }}\n\n"
        ));
        d.sum(format!("borrowed{i}_{dep}(ctx, \"x\").length()"));
        if let Some(fx) = calls {
            d.reach.push((format!("entry{dep}(ctx)"), *fx));
        }
    }
    if g > 0 {
        d.sum(format!(
            "describeItem{k}(ctx, blankItem{k}(\"own\")).length()"
        ));
    }
    let total: u64 = WEIGHTS.iter().sum();
    // The first module has every kind, so the smallest monorepo builds them all.
    while nonblank_lines(&body) < m.module_lines || (i == 0 && n < WEIGHTS.len()) {
        let mut r = rng.below(total);
        let mut kind = 0;
        for (at, w) in WEIGHTS.iter().enumerate() {
            if r < *w {
                kind = at;
                break;
            }
            r -= w;
        }
        if i == 0 && n < WEIGHTS.len() {
            kind = n;
        }
        let name = format!("{i}x{n}");
        match kind {
            0 => {
                let r = Record::draw(rng, format!("Rec{name}"));
                record(&mut body, &mut d, &r, rng);
            }
            1 => states(&mut body, &mut d, &format!("State{name}"), rng),
            2 => table(&mut body, &mut d, &name, rng),
            3 => fixture(&mut body, &mut d, &name, rng),
            4 => slug(&mut body, &mut d, &name, rng),
            5 => totals(&mut body, &mut d, &name, rng),
            6 => generic(&mut body, &mut d, &name),
            _ => lease(&mut body, &mut d, &name, rng),
        }
        n += 1;
    }
    let last = g + 1 == m.modules;
    let prev = if g > 0 {
        format!("        + summary{}(ctx)\n", i - 1)
    } else {
        String::new()
    };
    // A sum of at most `PART` lines a function, so none grows past the
    // lint's length limit.
    let parts: Vec<&[(String, Fx)]> = d.reach.chunks(PART).collect();
    let mut fx = if g > 0 { prev_fx } else { 0 };
    for (p, part) in parts.iter().enumerate() {
        let part_fx = part.iter().fold(0, |a, (_, f)| a | f);
        fx |= part_fx;
        body.push_str(&format!(
            "/// Part {p} of this module's sum.\n\
             fn part{i}_{p}<C: {}>(ctx: C): Int {{\n\
             \x20   0\n{}}}\n\n",
            bound(part_fx),
            part.iter()
                .map(|(r, _)| format!("        + {r}\n"))
                .collect::<String>(),
        ));
    }
    body.push_str(&format!(
        "/// Everything in this module, from one call.\n\
         {}fn summary{i}<C: {}>(ctx: C): Int {{\n\
         \x20   0\n{}{prev}}}\n",
        if last { "" } else { "export " },
        bound(fx),
        (0..parts.len())
            .map(|p| format!("        + part{i}_{p}(ctx)\n"))
            .collect::<String>(),
    ));
    if last {
        body.push_str(&format!(
            "\n/// Everything in package p{k:03}, from one call.\n\
             export fn entry{k}<C: {}>(ctx: C): Int {{\n\
             \x20   summary{i}(ctx)\n\
             }}\n",
            bound(fx)
        ));
        if let Some(step) = &d.machine {
            body.push_str(&format!(
                "\n/// Package p{k:03}'s run, every step of it once more.\n\
                 export fn advance{k}<S, C: Allocator>(ctx: C, run: Run{k}<S>, step: fn(S) => S): Run{k}<S> {{\n\
                 \x20   stepped{step}(ctx, run, .Started, step)\n\
                 }}\n"
            ));
            d.exports.push(format!("advance{k}"));
            d.tests.push(format!(
                "test \"advance{k}: the package's run steps through every module\" {{\n\
                 \x20   let run = advance{k}(alloc(), start{k}(0), fn(n) => n + 1);\n\
                 \x20   assert.equal(run.steps, {});\n\
                 }}\n\n",
                m.modules * m.machines
            ));
        }
    }
    let mut s = format!("//! Module {g} of package p{k:03}.\n\n");
    if d.json {
        s.push_str("from \"core/json\" import * as json;\n");
    }
    if d.decode {
        s.push_str("from \"core/json\" import { FromJson, ToJson };\n");
    }
    if body.contains("str.format(") {
        s.push_str("from \"core/str\" import * as str;\n");
    }
    if d.effects {
        s.push_str(
            "from \"core/random\" import * as random;\n\
             from \"core/time\" import * as time;\n\
             from \"core/time\" import { Instant };\n",
        );
    }
    let used = if d.effects { CLOCK | RANDOM } else { fx };
    s.push_str(&format!(
        "from \"platform/effect\" import {{ {} }};\n",
        bound(used).replace(" +", ",")
    ));
    if g > 0 {
        let mut first = vec![format!("blankItem{k}"), format!("describeItem{k}")];
        let mut before = vec![format!("summary{}", i - 1)];
        if m.machines > 0 {
            first.extend([format!("Run{k}"), format!("start{k}")]);
            if body.contains(&format!("noted{k}(")) {
                first.push(format!("noted{k}"));
            }
        }
        if let Some(p) = prev_machine {
            before.push(format!("stepped{p}"));
        }
        if g == 1 {
            first.append(&mut before);
        }
        first.sort();
        s.push_str(&format!(
            "from \"{}/m1.buri\" import {{ {} }};\n",
            package(k),
            first.join(", ")
        ));
        if !before.is_empty() {
            before.sort();
            s.push_str(&format!(
                "from \"{}/m{g}.buri\" import {{ {} }};\n",
                package(k),
                before.join(", ")
            ));
        }
    }
    for (dep, _, calls) in deps {
        let entry = if calls.is_some() {
            format!(", entry{dep}")
        } else {
            String::new()
        };
        let noted = if body.contains(&format!("noted{dep}(")) {
            format!(", noted{dep}")
        } else {
            String::new()
        };
        s.push_str(&format!(
            "from \"{}\" import {{ blankItem{dep}, describeItem{dep}{entry}{noted} }};\n",
            package(*dep)
        ));
    }
    if let Some((_, generated)) = first {
        for line in &generated.imports {
            s.push_str(line);
        }
    }
    s.push('\n');
    s.push_str(&body);
    (s, d, fx)
}

/// Library package `k`.
/// Library package `k`; the effects its `entry` needs. `entries` holds every
/// earlier package's.
fn library(
    out: &mut Vec<File>,
    m: &Monorepo,
    k: usize,
    deps: &[usize],
    entries: &[Fx],
    rng: &mut Rng,
) -> Fx {
    let dir = format!("pkg/p{k:03}");
    let fixtures = m.fixtures > 0 && k % m.fixtures == m.fixtures - 1;
    let protos = m.protos > 0 && k % m.protos == m.protos / 2;
    let item = Record::draw(rng, format!("Item{k}"));
    // Each dependency is used by one module, in turn.
    let words: Vec<(usize, String, Option<Fx>)> = deps
        .iter()
        .enumerate()
        .map(|(at, d)| {
            let calls = (at + m.calls >= deps.len()).then_some(entries[*d]);
            (*d, WORDS[*d % WORDS.len()].to_string(), calls)
        })
        .collect();
    let generated = generated(out, m, &dir, k, fixtures, protos, rng);
    let mut surface = generated.surface.clone();
    let mut tests = Vec::new();
    let mut entry_fx = 0;
    let mut last_machine: Option<String> = None;
    for g in 0..m.modules {
        let i = k * m.modules + g;
        let mine: Vec<(usize, String, Option<Fx>)> = words
            .iter()
            .enumerate()
            .filter(|(at, _)| at % m.modules == g)
            .map(|(_, w)| w.clone())
            .collect();
        let first = (g == 0).then_some((&item, &generated));
        let (text, d, fx) = module(
            m,
            k,
            g,
            i,
            &mine,
            first,
            entry_fx,
            last_machine.as_deref(),
            rng,
        );
        entry_fx = fx;
        last_machine = d.machine.clone();
        out.push(File {
            path: format!("{dir}/m{}.buri", g + 1),
            text,
        });
        let mut exports = d.exports.clone();
        if g + 1 == m.modules {
            exports.push(format!("entry{k}"));
        }
        exports.sort();
        surface.push_str(&format!(
            "from \"{}/m{}.buri\" export {{ {} }};\n",
            package(k),
            g + 1,
            exports.join(", ")
        ));
        tests.push((g, d));
    }
    let mut gens = Vec::new();
    if fixtures {
        gens.push(format!(
            "        {{ tool: \"//tools/embed\", inputs: [{}] }},\n",
            quoted(generated.inputs.iter().cloned())
        ));
    }
    if protos {
        gens.push("        { tool: \"proto\", inputs: [\"schema.proto\"] },\n".to_string());
    }
    let generators_field = if gens.is_empty() {
        String::new()
    } else {
        format!("    generators: [\n{}    ]\n", gens.concat())
    };
    out.push(File {
        path: format!("{dir}/BUILD.buri"),
        text: format!(
            "library {{\n    sources: [{}]\n    dependencies: [{}]\n{generators_field}    visibility: [\"//...\"]\n\n    test {{\n        sources: [{}]\n    }}\n}}\n",
            quoted((1..=m.modules).map(|g| format!("m{g}.buri"))),
            quoted(deps.iter().map(|d| package(*d))),
            quoted((1..=m.modules).map(|g| format!("test/m{g}_test.buri"))),
        ),
    });
    out.push(File {
        path: format!("{dir}/lib.buri"),
        text: format!("//! The public surface of package p{k:03}.\n\n{surface}"),
    });
    for (g, d) in tests {
        let mut t = String::from(
            "from \"core/testing/assert\" import * as assert;\n\
             from \"platform/effect/testing\" import { alloc };\n",
        );
        if d.effects {
            t = t.replace("import { alloc };", "import { alloc, clock, rand };");
            t.push_str("from \"platform/effect\" import { Allocator, Clock, Random };\n");
        }
        if d.decode {
            t.push_str("from \"core/json\" import * as json;\nfrom \"core/json\" import { DecodeError };\n");
        }
        // The tests start runs, and never name one's type or log.
        let mut names: Vec<String> = d
            .exports
            .iter()
            .filter(|n| **n != format!("Run{k}") && **n != format!("noted{k}"))
            .cloned()
            .collect();
        if g > 0 && m.machines > 0 {
            names.push(format!("start{k}"));
        }
        if g == 0 {
            names.extend(generated.exported.iter().cloned());
            t.push_str(&generated.test_imports);
        }
        names.sort();
        t.push_str(&format!(
            "from \"{}\" import {{ {} }};\n\n",
            package(k),
            names.join(", ")
        ));
        for test in &d.tests {
            t.push_str(test);
        }
        if g == 0 {
            t.push_str(&generated.tests);
        }
        out.push(File {
            path: format!("{dir}/test/m{}_test.buri", g + 1),
            text: t,
        });
    }
    entry_fx
}

/// What a package's generators add: the files they read, the modules' names
/// on the package's surface, and what its first module and tests do with them.
#[derive(Default)]
struct Generated {
    inputs: Vec<String>,
    surface: String,
    exported: Vec<String>,
    imports: Vec<String>,
    /// A line of `summary`'s sum and the function it calls.
    functions: Vec<(String, String)>,
    test_imports: String,
    tests: String,
}

/// Writes package `k`'s fixtures and schema, when it has them.
fn generated(
    out: &mut Vec<File>,
    m: &Monorepo,
    dir: &str,
    k: usize,
    fixtures: bool,
    protos: bool,
    rng: &mut Rng,
) -> Generated {
    let mut g = Generated::default();
    if fixtures {
        let mut names = Vec::new();
        for f in 0..2 {
            let w = WORDS[(k + f) % WORDS.len()];
            let path = format!("data/{w}s{f}.json");
            out.push(File {
                path: format!("{dir}/{path}"),
                text: fixture_file(w, rng),
            });
            names.push(format!("{w}s{f}"));
            g.inputs.push(path);
        }
        let all: Vec<String> = names
            .iter()
            .flat_map(|n| [format!("{n}Members"), format!("{n}Text")])
            .collect();
        g.surface.push_str(&format!(
            "from \"{}/fixtures\" export {{ {} }};\n",
            package(k),
            all.join(", ")
        ));
        g.exported.extend(all);
        g.imports.push(format!(
            "from \"{}/fixtures\" import {{ {} }};\n",
            package(k),
            names
                .iter()
                .map(|n| format!("{n}Text"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for n in &names {
            g.functions.push((
                format!("fixtureLength{k}_{n}()"),
                format!(
                    "/// How long the embedded `{n}` fixture is.\n\
                     fn fixtureLength{k}_{n}(): Int {{\n\
                     \x20   {n}Text.length()\n\
                     }}\n\n"
                ),
            ));
            g.tests.push_str(&format!(
                "test \"the {n} fixture is embedded whole\" {{\n\
                 \x20   assert.equal({n}Members, 3);\n\
                 \x20   assert.containsText({n}Text, \"\\\"items\\\":[\");\n\
                 }}\n\n"
            ));
        }
    }
    if protos {
        out.push(File {
            path: format!("{dir}/schema.proto"),
            text: proto_file(k, m.messages, rng),
        });
        let names = [
            format!("decodeWire{k}N0"),
            format!("defaultWire{k}N0"),
            format!("encodeWire{k}N0"),
            format!("Wire{k}N0"),
        ];
        g.surface.push_str(&format!(
            "from \"{}/schema.proto\" export {{ {} }};\n",
            package(k),
            names.join(", ")
        ));
        g.exported.extend(names);
        g.imports.push(format!(
            "from \"{}/schema.proto\" import {{ defaultWire{k}N0, encodeWire{k}N0 }};\n",
            package(k)
        ));
        g.functions.push((
            format!("wireSize{k}(ctx)"),
            format!(
                "/// How many bytes an empty message takes on the wire.\n\
                 fn wireSize{k}<C: Allocator>(ctx: C): Int {{\n\
                 \x20   encodeWire{k}N0(ctx, defaultWire{k}N0()).length()\n\
                 }}\n\n"
            ),
        ));
        g.tests.push_str(&format!(
            "test \"a message survives the wire\" {{\n\
             \x20   let ctx = alloc();\n\
             \x20   let m = defaultWire{k}N0();\n\
             \x20   assert.isTrue(decodeWire{k}N0(ctx, encodeWire{k}N0(ctx, m)) == .Ok(m));\n\
             }}\n\n"
        ));
    }
    g
}

/// A JSON fixture: a list of records with a few fields each.
fn fixture_file(word: &str, rng: &mut Rng) -> String {
    let rows = 6 + rng.below(20) as usize;
    let mut s = format!(
        "{{\n  \"$schema\": \"//schemas/fixture.schema.json\",\n  \"kind\": \"{word}\",\n  \"items\": [\n"
    );
    for r in 0..rows {
        s.push_str(&format!(
            "    {{ \"id\": \"{word}-{r}\", \"{}\": \"{}\", \"{}\": {}, \"tags\": [\"{}\", \"{}\"] }}{}\n",
            rng.pick(&FIELDS),
            rng.pick(&SENTENCES),
            rng.pick(&NUMBERS),
            rng.below(10_000),
            rng.pick(&WORDS),
            rng.pick(&WORDS),
            if r + 1 == rows { "" } else { "," }
        ));
    }
    s.push_str("  ]\n}\n");
    s
}

/// A protocol buffer schema of a few messages.
fn proto_file(k: usize, messages: usize, rng: &mut Rng) -> String {
    let mut s = format!("edition = \"2026\";\n\npackage p{k:03}.v1;\n");
    for msg in 0..messages {
        s.push_str(&format!("\nmessage Wire{k}N{msg} {{\n"));
        let fields = 4 + rng.below(8) as usize;
        for f in 0..fields {
            let ty = *rng.pick(&[
                "string",
                "int32",
                "int64",
                "bool",
                "double",
                "repeated string",
            ]);
            s.push_str(&format!(
                "    {ty} {}_{f} = {};\n",
                rng.pick(&FIELDS),
                f + 1
            ));
        }
        s.push_str("}\n");
    }
    s
}

/// A native program over `used` packages.
fn program(out: &mut Vec<File>, j: usize, used: &[usize], fx: Fx) {
    let dir = format!("bin/n{j}");
    out.push(File {
        path: format!("{dir}/BUILD.buri"),
        text: format!(
            "binary {{\n    dependencies: [{}]\n\n    outputs: [\n        {{ platform: \"native\" }},\n    ]\n}}\n",
            quoted(used.iter().map(|d| package(*d))),
        ),
    });
    out.push(File {
        path: format!("{dir}/main.buri"),
        text: main_of("native", "NativeHost", used, fx),
    });
}

/// A `node` program over `used` packages.
fn script(out: &mut Vec<File>, j: usize, used: &[usize], fx: Fx) {
    let dir = format!("bin/s{j}");
    out.push(File {
        path: format!("{dir}/BUILD.buri"),
        text: format!(
            "binary {{\n    dependencies: [{}]\n\n    outputs: [\n        {{ platform: \"node\" }},\n    ]\n}}\n",
            quoted(used.iter().map(|d| package(*d))),
        ),
    });
    out.push(File {
        path: format!("{dir}/main.buri"),
        text: main_of("node", "NodeHost", used, fx),
    });
}

/// A `main` printing what each package adds up to.
fn main_of(platform: &str, host: &str, used: &[usize], fx: Fx) -> String {
    let mut s = format!(
        "from \"core/io\" import * as io;\n\
         from \"core/str\" import * as str;\n\
         from \"{platform}\" import {{ {host} }};\n\
         from \"platform/effect\" import {{ {}, IoError, Stdout }};\n",
        bound(fx).replace(" +", ",")
    );
    for d in used {
        s.push_str(&format!(
            "from \"{}\" import {{ entry{d} }};\n",
            package(*d)
        ));
    }
    s.push_str(&format!(
        "\n/// One line per package: what it adds up to.\n\
         fn report<C: {} + Stdout>(ctx: C): Result<(), IoError> {{\n",
        bound(fx)
    ));
    for d in used {
        s.push_str(&format!(
            "    let _ = io.println(ctx, str.format(ctx, \"p{d:03}: ${{entry{d}(ctx)}}\"))?;\n"
        ));
    }
    s.push_str(&format!(
        "    .Ok(())\n\
         }}\n\n\
         export fn main(host: {host}): Result<(), Str> {{\n\
         \x20   let ctx = context {{\n\
         \x20       Allocator: host.alloc,\n\
         {}\
         \x20       Stdout: host.stdout,\n\
         \x20   }};\n\
         \x20   match (report(ctx)) {{\n\
         \x20       .Ok(_) => .Ok(()),\n\
         \x20       .Err(_) => .Err(\"could not print\"),\n\
         \x20   }}\n\
         }}\n",
        fields(fx)
    ));
    s
}

/// A page over `used` packages, with styles, showing the `shown` components.
fn page(out: &mut Vec<File>, j: usize, used: &[usize], shown: &[design::Component], fx: Fx) {
    let dir = format!("web/w{j}");
    let mut designs: Vec<usize> = shown.iter().map(|c| c.package).collect();
    designs.sort_unstable();
    designs.dedup();
    out.push(File {
        path: format!("{dir}/BUILD.buri"),
        text: format!(
            "binary {{\n    dependencies: [{}]\n\n    outputs: [\n        {{ platform: \"web\" }},\n    ]\n}}\n",
            quoted(
                used.iter()
                    .map(|d| package(*d))
                    .chain(designs.iter().map(|d| design::package(*d)))
            ),
        ),
    });
    let mut m = format!(
        "from \"core/str\" import * as str;\n\
         from \"platform/effect\" import {{ {}, Ui, Watch }};\n\
         from \"ui/node\" import * as ui;\n\
         from \"ui/node\" import {{ Node }};\n\
         from \"ui/signal\" import {{ signal }};\n\
         from \"ui/style\" import {{ Style }};\n\
         from \"web\" import {{ WebHost }};\n",
        bound(fx).replace(" +", ",")
    );
    // The page's buttons step the first package's run.
    let driven = used[0];
    for d in used {
        let run = if *d == driven {
            format!(", advance{d}, start{d}")
        } else {
            String::new()
        };
        m.push_str(&format!(
            "from \"{}\" import {{ entry{d}{run} }};\n",
            package(*d)
        ));
    }
    for d in &designs {
        let mut names: Vec<String> = shown
            .iter()
            .filter(|c| c.package == *d)
            .flat_map(|c| {
                let mut v = vec![c.name.clone()];
                if c.kind == design::Kind::Panel {
                    v.push(format!("Props{d}"));
                }
                v
            })
            .collect();
        names.sort();
        names.dedup();
        m.push_str(&format!(
            "from \"{}\" import {{ {} }};\n",
            design::package(*d),
            names.join(", ")
        ));
    }
    m.push_str(&format!(
        "\nlet panel{j}: Style = .Group([\n\
         \x20   .Layout(.Column),\n\
         \x20   .Gap(.Px({gap})),\n\
         \x20   .Padding(.Rem(0.75)),\n\
         \x20   .Radius(.Px(8)),\n\
         ]);\n\n\
         /// One line per package.\n\
         fn row{j}<C: Allocator>(ctx: C, name: Str, total: Int): Node<C> {{\n\
         \x20   ui.text({{ content: .Const(str.format(ctx, \"${{name}}: ${{total}}\")) }})\n\
         }}\n\n\
         export fn main(host: WebHost): Result<(), Str> {{\n\
         \x20   let ctx = context {{\n\
         \x20       Allocator: host.alloc,\n\
         {fields}\
         \x20       Ui: host.ui,\n\
         \x20       Watch: host.watch,\n\
         \x20   }};\n\
         \x20   let run = signal(ctx, start{driven}(0));\n\
         \x20   ui.mount(ctx, ui.stack({{\n\
         \x20       styles: [panel{j}],\n\
         \x20       children: [\n",
        gap = 4 + j % 4,
        fields = fields(fx),
    ));
    for d in used {
        m.push_str(&format!(
            "            row{j}(ctx, \"p{d:03}\", entry{d}(ctx)),\n"
        ));
    }
    for (at, c) in shown.iter().enumerate() {
        m.push_str(&format!(
            "            {},\n",
            design::call(
                c,
                &format!("{} {at}", WORDS[(j + at) % WORDS.len()]),
                &format!("fn(c) => run.set(c, advance{driven}(c, run.get(c), fn(n) => n + 1))"),
            )
        ));
    }
    m.push_str("        ],\n    }), [])\n}\n");
    out.push(File {
        path: format!("{dir}/main.buri"),
        text: m,
    });
}
