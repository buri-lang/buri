//! Compiling the documentation.
//!
//! Every fenced Buri block in every document is a real module, compiled by the
//! real front end against the real standard library. A block that stops
//! compiling fails `cargo test`, which is the whole point: the examples cannot
//! drift from the language because they *are* the language, run.
//!
//! The hard part is that a document shows fragments. `let a = 5;` is not a
//! module, `fn total(xs: [N]): N { ... }` is not a body, and a section about
//! errors deliberately shows code that must not compile. Four mechanisms
//! absorb that, in the order you should reach for them:
//!
//!   * `wrap=body` puts a statement fragment inside a synthetic `main`, with a
//!     context built from whatever `ctx=` names, so a free `ctx` resolves.
//!   * `sig` mode parses the block as a standard-library module, where a
//!     declaration may have no body — which is what a signature list is.
//!   * `use=` splices a named preamble from `doc_harness`, and `name=` makes a
//!     block itself available to later ones, so a narrative that builds up a
//!     type over three blocks compiles as three modules.
//!   * a line ending `// ERROR: <substring>` is compiled *separately* and must
//!     produce that diagnostic. That is how a block can show the wrong thing
//!     beside the right thing and have both checked.
//!
//! What is deliberately absent is any rewriting of the block's text before it
//! is shown. Whatever a reader copies is what was compiled, character for
//! character, apart from lines beginning `# `, which are hidden imports and
//! are marked as such in the source document.

use crate::compiler::driver;
use crate::compiler::modules::Role;
use crate::diagnostics::SourceMap;
use crate::documentation::markdown::{self, Fence};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// What a block claims about itself
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Must typecheck. The default, and what most blocks are.
    Check,
    /// A list of signatures. Parsed as a standard-library module, so a
    /// declaration may have no body.
    Sig,
    /// Must typecheck, compile, and run. The next ```stdout fence pins what it
    /// prints.
    Run,
    /// Must be rejected. The next ```error fence, or the block's own
    /// `// ERROR:` annotations, pin the message.
    Fail,
    /// Not compiled. Requires `why=`, and is listed in the ratchet file so a
    /// new one is a reviewable line rather than a silent omission.
    Ignore,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wrap {
    /// The block is a whole module. The default.
    Module,
    /// The block is statements; they go inside a synthetic `main`.
    Body,
    /// The block is one expression; it is bound inside a synthetic `main`.
    Expr,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Schema {
    Build,
    Repo,
}

#[derive(Clone, Debug)]
pub struct Origin {
    pub file: String,
    /// 1-based line of the fence's first content line.
    pub line: usize,
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    pub mode: Mode,
    pub wrap: Wrap,
    /// Named preambles spliced ahead of this block.
    pub uses: Vec<String>,
    /// This block's own name, for a later block to `use=`.
    pub name: Option<String>,
    /// Why an `ignore` block is ignored. Required for `Mode::Ignore`.
    pub why: Option<String>,
    /// Effects the synthetic `main` builds a context from.
    pub effects: Vec<String>,
    /// A repository to compile against, relative to the documentation root.
    /// Without one a snippet may not name a `//...` module.
    pub repo: Option<String>,
    /// The package this example stands in, as a label. A document about a
    /// library shows the library's own files, and their imports are legal only
    /// from inside it.
    pub pkg: Option<String>,
    /// What the block is being compiled *as*, when the default is wrong: a
    /// document showing an `effect` declaration is showing a platform module,
    /// and one showing a `test` is showing a test source.
    pub role: Option<Role>,
    /// For a `fail` block in the error catalog: the code it must produce.
    pub expect_code: Option<String>,
    /// Which build-file schema a ```textproto block must satisfy.
    pub schema: Option<Schema>,
    /// The text to compile: hidden `# ` markers removed, `// ERROR:` lines
    /// blanked (they are compiled separately).
    pub source: String,
    /// The same text with the `// ERROR:` lines still in place, so a variant
    /// can put one of them back.
    pub original: String,
    /// The `// ERROR:` lines, as `(0-based line within `source`, substring)`.
    pub errors: Vec<(usize, String)>,
    /// Pinned stdout, from the ```stdout fence beneath a `run` block.
    pub expect_stdout: Option<String>,
    /// Pinned diagnostics, from the ```error fence beneath a `fail` block.
    pub expect_errors: Vec<String>,
    pub origin: Origin,
}

/// A block a document says is textproto rather than Buri.
#[derive(Clone, Debug)]
pub struct ProtoBlock {
    pub schema: Schema,
    pub source: String,
    pub origin: Origin,
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub origin: Origin,
    /// One line: what went wrong.
    pub what: String,
    /// The evidence, indented under it.
    pub detail: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}: {}", self.origin, self.what)?;
        for line in self.detail.lines() {
            writeln!(f, "    {line}")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

pub struct Extracted {
    pub blocks: Vec<Block>,
    pub protos: Vec<ProtoBlock>,
    /// Malformed fence info strings and missing `why=` reasons.
    pub failures: Vec<Failure>,
}

/// Reads every fenced block in one document.
///
/// `stdout` and `error` fences are consumed by the block above them rather
/// than standing alone, which is what makes the expected output sit where a
/// reader expects to see it.
pub fn extract(file: &str, text: &str) -> Extracted {
    let fences = markdown::fences(text);
    let mut out =
        Extracted { blocks: Vec::new(), protos: Vec::new(), failures: Vec::new() };

    for (i, fence) in fences.iter().enumerate() {
        let origin = Origin { file: file.to_string(), line: fence.body_line };
        match fence.info.lang.as_str() {
            "buri" => match parse_block(fence, &origin, fences.get(i + 1)) {
                Ok(b) => out.blocks.push(b),
                Err(f) => out.failures.push(f),
            },
            "textproto" => {
                // A fragment of a build file is not a build file; it says so
                // the same way a Buri fragment does.
                if fence.info.mode.as_deref() == Some("ignore") {
                    if fence.info.get("why").is_none() {
                        out.failures.push(Failure {
                            origin,
                            what: "an `ignore` block must say why".into(),
                            detail: "add `why=\"...\"`".into(),
                        });
                    }
                    continue;
                }
                let Some(schema) = schema_of(fence) else {
                    out.failures.push(Failure {
                        origin,
                        what: "this textproto block does not say which schema it is".into(),
                        detail: "tag it `textproto schema=build` or `textproto schema=repo`, \
                                 or `textproto ignore why=\"...\"` if it is a fragment"
                            .into(),
                    });
                    continue;
                };
                out.protos.push(ProtoBlock {
                    schema,
                    source: fence.body.clone(),
                    origin,
                });
            }
            // Expected-output fences belong to the block above them.
            "stdout" | "error" => {}
            _ => {}
        }
    }
    out
}

fn schema_of(fence: &Fence) -> Option<Schema> {
    match fence.info.get("schema") {
        Some("build") => Some(Schema::Build),
        Some("repo") => Some(Schema::Repo),
        _ => None,
    }
}

fn parse_block(fence: &Fence, origin: &Origin, next: Option<&Fence>) -> Result<Block, Failure> {
    let fail = |what: String, detail: &str| Failure {
        origin: origin.clone(),
        what,
        detail: detail.to_string(),
    };

    if let Err(msg) = markdown::parse_info(fence.raw_info) {
        return Err(fail(msg, "the info string is `buri [mode] [key=value]...`"));
    }

    let mode = match fence.info.mode.as_deref() {
        None | Some("check") => Mode::Check,
        Some("sig") => Mode::Sig,
        Some("run") => Mode::Run,
        Some("fail") => Mode::Fail,
        Some("ignore") => Mode::Ignore,
        Some(other) => {
            return Err(fail(
                format!("`{other}` is not a mode"),
                "the modes are check, sig, run, fail, ignore",
            ))
        }
    };

    let wrap = match fence.info.get("wrap") {
        None | Some("module") => Wrap::Module,
        Some("body") => Wrap::Body,
        Some("expr") => Wrap::Expr,
        Some(other) => {
            return Err(fail(
                format!("`wrap={other}` is not a wrapper"),
                "the wrappers are module, body, expr",
            ))
        }
    };

    let role = match fence.info.get("role") {
        None => None,
        Some("source") => Some(Role::Source),
        Some("entry") => Some(Role::Entry),
        Some("std") => Some(Role::Std),
        Some("platform") => Some(Role::Platform),
        Some("test") => Some(Role::TestSource),
        Some(other) => {
            return Err(fail(
                format!("`role={other}` is not a role"),
                "the roles are source, entry, std, platform, test",
            ))
        }
    };

    let repo = fence.info.get("repo").map(String::from);
    let pkg = fence.info.get("pkg").map(String::from);

    let why = fence.info.get("why").map(String::from);
    if mode == Mode::Ignore && why.is_none() {
        return Err(fail(
            "an `ignore` block must say why".into(),
            "add `why=\"...\"`. An untested example is a claim nobody checks, so the \
             reason belongs in the document where a reader of the diff can weigh it.",
        ));
    }

    let effects = match fence.info.get("ctx") {
        Some(_) => fence.info.list("ctx"),
        // Enough to print and to allocate, which is what a fragment that names
        // `ctx` at all almost always wants.
        None => vec!["alloc".into(), "stdout".into()],
    };
    for e in &effects {
        if effect_binding(e).is_none() {
            return Err(fail(
                format!("`{e}` is not an effect this harness can build"),
                "the effects are alloc, stdout, stderr, stdin, fs, net, clock, rand, env, proc",
            ));
        }
    }

    let (source, original, errors) = strip_annotations(&fence.body);

    // The expected-output fence directly beneath, if this mode takes one.
    let mut expect_stdout = None;
    let mut expect_errors = Vec::new();
    if let Some(next) = next {
        match next.info.lang.as_str() {
            "stdout" if mode == Mode::Run => expect_stdout = Some(next.body.clone()),
            "error" if mode == Mode::Fail => {
                expect_errors =
                    next.body.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
            }
            _ => {}
        }
    }

    if mode == Mode::Run && expect_stdout.is_none() {
        return Err(fail(
            "a `run` block must be followed by a ```stdout fence".into(),
            "add one directly beneath it holding exactly what the program prints; \
             `BURI_BLESS=1` fills it in",
        ));
    }
    if mode == Mode::Fail
        && expect_errors.is_empty()
        && errors.is_empty()
        && fence.info.get("code").is_none()
    {
        return Err(fail(
            "a `fail` block must say what it fails with".into(),
            "add a ```error fence beneath it, or annotate the offending line \
             `// ERROR: <substring of the message>`",
        ));
    }
    // In `run` and `sig` the annotation would be silently unhonoured, which is
    // worse than rejecting it. In `ignore` nothing is honoured by definition,
    // and the ratchet already records that.
    if matches!(mode, Mode::Run | Mode::Sig) && !errors.is_empty() {
        return Err(fail(
            format!("`// ERROR:` annotations are not compiled in `{mode:?}` mode"),
            "use the default mode, or `fail`",
        ));
    }

    Ok(Block {
        mode,
        wrap,
        uses: fence.info.list("use"),
        name: fence.info.get("name").map(String::from),
        why,
        effects,
        expect_code: fence.info.get("code").map(String::from),
        repo,
        pkg,
        role,
        schema: None,
        source,
        original,
        errors,
        expect_stdout,
        expect_errors,
        origin: origin.clone(),
    })
}

/// Removes the two in-band markers and reports what they said.
///
/// A leading `# ` is a hidden line: kept for the compiler, dropped from the
/// rendered document. `#` cannot begin a Buri line, so there is no ambiguity.
/// `## ` escapes to a literal `# `.
///
/// A trailing `// ERROR: msg` marks a line that must not compile. It is
/// *blanked* rather than removed so that every variant of this block has
/// identical line numbering, which is what keeps a reported location pointing
/// at the document.
fn strip_annotations(body: &str) -> (String, String, Vec<(usize, String)>) {
    let mut source = String::with_capacity(body.len());
    let mut original = String::with_capacity(body.len());
    let mut errors = Vec::new();
    for (i, raw) in body.lines().enumerate() {
        let line = match raw.strip_prefix("##") {
            Some(rest) => format!("#{rest}"),
            None => raw.strip_prefix("# ").unwrap_or(raw).to_string(),
        };
        original.push_str(&line);
        original.push('\n');
        match error_annotation(&line) {
            Some(msg) => {
                errors.push((i, msg));
                source.push('\n');
            }
            None => {
                source.push_str(&line);
                source.push('\n');
            }
        }
    }
    (source, original, errors)
}

fn error_annotation(line: &str) -> Option<String> {
    let at = line.find("// ERROR:")?;
    Some(line[at + "// ERROR:".len()..].trim().to_string())
}

/// The line as it appears in the rendered document: hidden lines gone, the
/// `// ERROR:` comments kept, since they are what makes the example legible.
pub fn rendered(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("##") {
            out.push('#');
            out.push_str(rest);
            out.push('\n');
            continue;
        }
        if line.strip_prefix("# ").is_some() || line == "#" {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

/// `(effect name, cap trait, host value)`.
fn effect_binding(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "alloc" => ("Alloc", "alloc"),
        "stdout" => ("Stdout", "stdout"),
        "stderr" => ("Stderr", "stderr"),
        "stdin" => ("Stdin", "stdin"),
        "fs" => ("Fs", "fs"),
        "net" => ("Net", "net"),
        "clock" => ("Clock", "clock"),
        "rand" => ("Rand", "rand"),
        "env" => ("Env", "env"),
        "proc" => ("Proc", "proc"),
        _ => return None,
    })
}

pub struct Compiland {
    pub text: String,
    pub role: Role,
    /// Lines the wrapper and the preambles contributed, ahead of the block's
    /// own first line.
    pub prefix_lines: usize,
}

impl Compiland {
    /// Maps a 1-based line in the compiland back to a 1-based line in the
    /// document. Lines the harness generated map to the fence's own opening
    /// line, so a diagnostic in the wrapper still points somewhere real.
    pub fn document_line(&self, origin: &Origin, reported: usize) -> usize {
        if reported <= self.prefix_lines {
            return origin.line.saturating_sub(1).max(1);
        }
        origin.line + (reported - self.prefix_lines) - 1
    }
}

/// The named blocks and harnesses a document's blocks may `use=`.
pub struct Registry {
    named: HashMap<String, String>,
}

impl Registry {
    pub fn new() -> Registry {
        let mut named = HashMap::new();
        for (name, text) in crate::documentation::harness::HARNESSES {
            named.insert((*name).to_string(), (*text).to_string());
        }
        Registry { named }
    }

    /// Makes a `name=`d block available to the blocks after it.
    pub fn record(&mut self, block: &Block) {
        if let Some(name) = &block.name {
            self.named.insert(name.clone(), block.source.clone());
        }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.named.get(name).map(String::as_str)
    }
}

impl Default for Registry {
    fn default() -> Registry {
        Registry::new()
    }
}

/// Builds the module that will actually be compiled.
///
/// `only_error` selects which `// ERROR:` line is left in place: `None`
/// compiles the block with all of them blanked, `Some(i)` restores just that
/// one. Every variant has the same line count, so one remapping serves them
/// all.
pub fn assemble(
    block: &Block,
    reg: &Registry,
    only_error: Option<usize>,
) -> Result<Compiland, Failure> {
    let mut prefix = String::new();

    for name in &block.uses {
        let Some(text) = reg.get(name) else {
            return Err(Failure {
                origin: block.origin.clone(),
                what: format!("there is no preamble named `{name}`"),
                detail: "name an earlier block with `name=...`, or add a file to \
                         `cli/src/docs/harness/` and register it in `documentation/harness.rs`"
                    .into(),
            });
        };
        prefix.push_str(text);
        if !prefix.ends_with('\n') {
            prefix.push('\n');
        }
    }

    let role = match (block.role, block.mode) {
        // An explicit `role=` wins, except that `sig` needs a role whose
        // parser allows a declaration with no body — `Std` and `Platform` are
        // the two, and a block showing an `effect` must be `Platform`.
        (Some(r @ (Role::Std | Role::Platform)), Mode::Sig) => r,
        (Some(_), Mode::Sig) | (None, Mode::Sig) => Role::Std,
        (Some(r), _) => r,
        _ if block.wrap != Wrap::Module => Role::Entry,
        // A whole-module block is ordinary source unless it declares `main`,
        // which is what makes a documented program a program.
        _ if declares_main(&block.source) => Role::Entry,
        _ => Role::Source,
    };

    let mut body = String::new();
    let mut suffix = String::new();
    match block.wrap {
        Wrap::Module => {}
        Wrap::Body | Wrap::Expr => {
            body.push_str("from \"core/cap\" import * as __cap;\n");
            body.push_str("from \"core/host\" import * as __host;\n");
            body.push_str("export fn main(): Result<(), Str> {\n");
            body.push_str("  let ctx = context {\n");
            for e in &block.effects {
                let (trait_name, host_value) = effect_binding(e).expect("checked at parse");
                body.push_str(&format!("    __cap.{trait_name}: __host.{host_value},\n"));
            }
            body.push_str("  };\n");
            if block.wrap == Wrap::Expr {
                body.push_str("  let __value = (\n");
                suffix.push_str("  );\n  .Ok(())\n}\n");
            } else {
                suffix.push_str("  .Ok(())\n}\n");
            }
        }
    }

    let prefix_lines = prefix.lines().count() + body.lines().count();
    let mut text = prefix;
    text.push_str(&body);
    text.push_str(&restore_error(&block.source, block, only_error));
    text.push_str(&suffix);
    Ok(Compiland { text, role, prefix_lines })
}

/// Puts one `// ERROR:` line back, leaving the rest blank.
fn restore_error(source: &str, block: &Block, only: Option<usize>) -> String {
    let Some(only) = only else { return source.to_string() };
    let restored = block.original.lines().nth(only).unwrap_or_default();
    let mut out = String::with_capacity(source.len());
    for (i, line) in source.lines().enumerate() {
        out.push_str(if i == only { restored } else { line });
        out.push('\n');
    }
    out
}

fn declares_main(source: &str) -> bool {
    source.lines().any(|l| {
        let t = l.trim_start();
        let Some(rest) = t.strip_prefix("export fn main").or_else(|| t.strip_prefix("fn main"))
        else {
            return false;
        };
        // `main<T>()` is still `main` — an illegal one, which is exactly what
        // the page about `main`'s shape needs to be able to show.
        rest.starts_with('(') || rest.starts_with('<')
    })
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// Compiles one block and reports every way it did not do what it claimed.
pub fn run_block(block: &Block, reg: &Registry, map: &mut SourceMap) -> Vec<Failure> {
    run_block_in(None, block, reg, map)
}

/// The same, against a repository the block named with `repo=`.
pub fn run_block_in(
    ws: Option<&crate::build::workspace::Workspace>,
    block: &Block,
    reg: &Registry,
    map: &mut SourceMap,
) -> Vec<Failure> {
    let pkg = match (&block.pkg, ws) {
        (Some(label), Some(ws)) => {
            let path = label.trim_start_matches('/');
            match ws.pkg_by_path(path) {
                Some(id) => Some(id),
                None => {
                    return vec![Failure {
                        origin: block.origin.clone(),
                        what: format!("`pkg={label}` is not a package of this repository"),
                        detail: "name a package that exists, as in `pkg=//lib/money`".into(),
                    }]
                }
            }
        }
        (Some(label), None) => {
            return vec![Failure {
                origin: block.origin.clone(),
                what: format!("`pkg={label}` needs a repository"),
                detail: "add `repo=...`, or run this inside the repository the package is in"
                    .into(),
            }]
        }
        (None, _) => None,
    };
    if block.mode == Mode::Ignore {
        return Vec::new();
    }
    let mut failures = Vec::new();

    // The block with every annotated line blanked must be clean, whatever the
    // annotations say — otherwise a `// ERROR:` would be hiding a second,
    // unrelated mistake.
    let base = match assemble(block, reg, None) {
        Ok(c) => c,
        Err(f) => return vec![f],
    };
    let name = format!("{}", block.origin);
    let analysis =
        driver::analyze_snippet_as(ws, pkg, map, &name, &base.text, base.role);
    let diags: Vec<String> = analysis
        .diags
        .items
        .iter()
        .filter(|d| d.is_error())
        .map(|d| describe(map, d, &base, &block.origin))
        .collect();

    match block.mode {
        Mode::Fail => {
            // `code=` is what makes the error catalog self-verifying: a page
            // claims to explain a rule, and its example must provoke exactly
            // that rule. A page nothing can provoke is a page that describes an
            // error the compiler no longer emits.
            if let Some(want) = &block.expect_code {
                let got: Vec<String> = analysis
                    .diags
                    .items
                    .iter()
                    .filter(|d| d.is_error())
                    .filter_map(|d| d.code.clone())
                    .collect();
                if !got.iter().any(|c| c == want) {
                    failures.push(Failure {
                        origin: block.origin.clone(),
                        what: format!("this example does not produce `{want}`"),
                        detail: if got.is_empty() {
                            format!(
                                "it produced no coded diagnostic; got:\n{}",
                                if diags.is_empty() { "nothing".into() } else { diags.join("\n") }
                            )
                        } else {
                            format!("it produced: {}", got.join(", "))
                        },
                    });
                }
            }
            if diags.is_empty() && block.errors.is_empty() {
                failures.push(Failure {
                    origin: block.origin.clone(),
                    what: "this block is marked `fail` but compiles".into(),
                    detail: "either the language changed and the document is now wrong, \
                             or the block needs to be retagged"
                        .into(),
                });
            }
            for want in &block.expect_errors {
                if !diags.iter().any(|d| d.contains(want.as_str())) {
                    failures.push(Failure {
                        origin: block.origin.clone(),
                        what: format!("no diagnostic contains `{want}`"),
                        detail: if diags.is_empty() {
                            "it compiled cleanly".into()
                        } else {
                            diags.join("\n")
                        },
                    });
                }
            }
        }
        _ => {
            if !diags.is_empty() {
                failures.push(Failure {
                    origin: block.origin.clone(),
                    what: "this example does not compile".into(),
                    detail: diags.join("\n"),
                });
            }
        }
    }

    // Each annotated line, compiled on its own.
    for (index, want) in &block.errors {
        let variant = match assemble(block, reg, Some(*index)) {
            Ok(c) => c,
            Err(f) => {
                failures.push(f);
                continue;
            }
        };
        let a =
            driver::analyze_snippet_as(ws, pkg, map, &name, &variant.text, variant.role);
        let got: Vec<String> = a
            .diags
            .items
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect();
        if !got.iter().any(|m| m.contains(want.as_str())) {
            let line = variant.document_line(&block.origin, base.prefix_lines + index + 1);
            failures.push(Failure {
                origin: Origin { file: block.origin.file.clone(), line },
                what: format!("this line should not compile: expected `{want}`"),
                detail: if got.is_empty() {
                    "it compiled cleanly".into()
                } else {
                    got.join("\n")
                },
            });
        }
    }

    if block.mode == Mode::Run && failures.is_empty() {
        match driver::run_snippet_in(ws, map, &name, &base.text) {
            Ok(stdout) => {
                let want = block.expect_stdout.clone().unwrap_or_default();
                if stdout != want {
                    failures.push(Failure {
                        origin: block.origin.clone(),
                        what: "this example prints something else".into(),
                        detail: format!(
                            "expected:\n{}\n  actual:\n{}",
                            indent(&want),
                            indent(&stdout)
                        ),
                    });
                }
            }
            Err(d) => failures.push(Failure {
                origin: block.origin.clone(),
                what: "this example does not run".into(),
                detail: d.items.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("\n"),
            }),
        }
    }

    failures
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  | {l}")).collect::<Vec<_>>().join("\n")
}

/// One diagnostic, with its location translated back to the document.
fn describe(
    map: &SourceMap,
    d: &crate::diagnostics::Diagnostic,
    c: &Compiland,
    origin: &Origin,
) -> String {
    if d.span.is_none() {
        return d.message.clone();
    }
    let file = map.get(d.span.file);
    let (line, col) = file.line_col(d.span.start);
    format!("{}:{}:{}: {}", origin.file, c.document_line(origin, line), col, d.message)
}

/// Every block in one document, in order, with `name=` bindings visible to the
/// blocks that follow.
pub fn run_file(file: &str, text: &str) -> Vec<Failure> {
    run_file_at(std::path::Path::new("."), file, text)
}

/// `root` is the directory a block's `repo=` path is resolved against — the
/// documentation repository's own root.
pub fn run_file_at(root: &std::path::Path, file: &str, text: &str) -> Vec<Failure> {
    run_file_with(root, false, file, text)
}

/// The same, but a block that names no `repo=` is compiled against the
/// repository at `root`.
///
/// This is what `buri docs test` uses: inside a repository, an example that
/// imports `//lib/money` should just work, because that is what the person
/// writing the example meant. `repo=` is then only for the other case — a
/// document in one repository showing an example from another, which is what
/// this toolchain's own documentation does.
pub fn run_file_in_repo(root: &std::path::Path, file: &str, text: &str) -> Vec<Failure> {
    run_file_with(root, true, file, text)
}

fn run_file_with(
    root: &std::path::Path,
    default_to_root: bool,
    file: &str,
    text: &str,
) -> Vec<Failure> {
    let extracted = extract(file, text);
    let mut failures = extracted.failures;
    let mut reg = Registry::new();
    let mut map = SourceMap::new();
    // One `Workspace` per repository named in this document, not per block:
    // loading a monorepo reads every build file in it.
    let mut repos: HashMap<String, Option<crate::build::workspace::Workspace>> = HashMap::new();

    for block in &extracted.blocks {
        let named = match (&block.repo, default_to_root) {
            (Some(rel), _) => Some(rel.clone()),
            (None, true) => Some(String::new()),
            (None, false) => None,
        };
        let ws = match &named {
            None => None,
            Some(rel) => {
                if !repos.contains_key(rel) {
                    let mut diags = crate::diagnostics::Diagnostics::new();
                    let loaded = crate::build::workspace::Workspace::load(
                        &root.join(rel),
                        &mut map,
                        &mut diags,
                    )
                    .ok();
                    repos.insert(rel.clone(), loaded);
                }
                match repos.get(rel).and_then(|w| w.as_ref()) {
                    Some(ws) => Some(ws),
                    None => {
                        failures.push(Failure {
                            origin: block.origin.clone(),
                            what: format!("`repo={rel}` is not a repository"),
                            detail: "the path is relative to the repository root and must \
                                     contain a REPO.buri"
                                .into(),
                        });
                        continue;
                    }
                }
            }
        };
        failures.extend(run_block_in(ws, block, &reg, &mut map));
        reg.record(block);
    }
    for proto in &extracted.protos {
        failures.extend(run_proto(proto));
    }
    failures
}

/// Renders every failure in a document as one block of text.
pub fn report(failures: &[Failure]) -> String {
    failures.iter().map(|f| f.to_string()).collect::<Vec<_>>().join("\n")
}

/// A ```textproto block has to parse *and* satisfy the schema its document
/// claims, which is what keeps `BUILD-FILES.md` honest about fields that no
/// longer exist.
pub fn run_proto(proto: &ProtoBlock) -> Vec<Failure> {
    let mut map = SourceMap::new();
    let name = format!("{}", proto.origin);
    let file = map.add(name, std::path::PathBuf::new(), proto.source.clone());
    let text = map.text(file).to_string();
    let errors = match proto.schema {
        Schema::Build => crate::build::buildfile::read_build_file(&text, file).errors,
        Schema::Repo => crate::build::buildfile::read_repo_config(&text, file).errors,
    };
    let errors: Vec<&crate::diagnostics::Diagnostic> =
        errors.iter().filter(|d| d.is_error()).collect();
    if errors.is_empty() {
        return Vec::new();
    }
    vec![Failure {
        origin: proto.origin.clone(),
        what: "this build file does not satisfy the schema".into(),
        detail: errors.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("\n"),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a synthetic document and returns its failures, rendered.
    fn check(doc: &str) -> String {
        report(&run_file("test.md", doc))
    }

    #[test]
    fn a_whole_module_compiles() {
        let doc = "```buri\n\
                   export fn double(n: Int): Int {\n  n * 2\n}\n\
                   ```\n";
        assert_eq!(check(doc), "");
    }

    #[test]
    fn a_statement_fragment_compiles_with_a_context() {
        let doc = "```buri wrap=body\n\
                   let a = 5;\n\
                   let _ = ctx.println(\"a is ${a}\");\n\
                   ```\n";
        assert_eq!(check(doc), "");
    }

    #[test]
    fn a_signature_list_compiles() {
        let doc = "```buri sig\n\
                   export fn allocate(bytes: Int): Int;\n\
                   ```\n";
        assert_eq!(check(doc), "");
    }

    /// A method signature needs its `impl`, which a document can supply with
    /// hidden lines rather than a fourth wrapper mode.
    #[test]
    fn a_method_signature_list_compiles_inside_a_hidden_impl() {
        let doc = "```buri sig\n\
                   # export struct Ring(export Int);\n\
                   # impl Ring {\n\
                   export fn size(self: Ring): Int;\n\
                   # }\n\
                   ```\n";
        assert_eq!(check(doc), "");
    }

    /// A block documenting a method on a *built-in* cannot be compiled where
    /// it is written: a type's operations may only be declared in its defining
    /// module (SPEC 6.7.3). Those blocks belong in the generated standard
    /// library reference, which renders them from `cli/src/compiler/standard_library/sources/*.buri`; until
    /// a document is converted, they carry `ignore` and a reason. This test
    /// pins the rule so the reason stays true.
    #[test]
    fn a_builtin_impl_cannot_be_documented_as_a_compilable_block() {
        let doc = "```buri sig\n\
                   # impl<T> [T] {\n\
                   export fn len(self: [T]): Int;\n\
                   # }\n\
                   ```\n";
        assert!(check(doc).contains("defining module of `[T]` is `core/list`"));
    }

    #[test]
    fn a_broken_example_is_reported_at_its_own_line() {
        let doc = "prose\n\nmore prose\n\n```buri wrap=body\n\
                   let a: Int = \"not an int\";\n\
                   ```\n";
        let out = check(doc);
        assert!(out.contains("does not compile"), "{out}");
        // The fence body starts on line 6 of the document.
        assert!(out.contains("test.md:6:"), "location should point at the fence:\n{out}");
    }

    #[test]
    fn a_fail_block_must_actually_fail() {
        let good = "```buri fail wrap=body\n\
                    let a: Int = \"not an int\";\n\
                    ```\n\
                    ```error\n\
                    expected `I64`, found `Str`\n\
                    ```\n";
        assert_eq!(check(good), "");

        let bad = "```buri fail wrap=body\n\
                   let a = 5;\n\
                   ```\n\
                   ```error\n\
                   expected `I64`, found `Str`\n\
                   ```\n";
        assert!(check(bad).contains("compiles"), "a compiling `fail` block must be caught");
    }

    #[test]
    fn an_error_annotation_is_compiled_on_its_own() {
        let doc = "```buri wrap=body\n\
                   let ok: Int = 5;\n\
                   let bad: Int = \"nope\";         // ERROR: expected `I64`, found `Str`\n\
                   let alsoOk = ok + 1;\n\
                   ```\n";
        assert_eq!(check(doc), "", "the annotated line should satisfy its own claim");
    }

    #[test]
    fn an_error_annotation_that_does_not_hold_is_caught() {
        let doc = "```buri wrap=body\n\
                   let ok: Int = 5;                // ERROR: expected `I64`, found `Str`\n\
                   ```\n";
        let out = check(doc);
        assert!(out.contains("should not compile"), "{out}");
    }

    #[test]
    fn a_named_block_is_reusable() {
        let doc = "```buri name=pt\n\
                   export struct Point { export x: Int, export y: Int }\n\
                   ```\n\
                   ```buri use=pt wrap=body\n\
                   let p = Point { x: 1, y: 2 };\n\
                   let _ = ctx.println(\"${p.x}\");\n\
                   ```\n";
        assert_eq!(check(doc), "");
    }

    #[test]
    fn a_harness_is_reusable() {
        let doc = "```buri use=shapes wrap=body\n\
                   let a = area(.Circle(2.0));\n\
                   let _ = ctx.println(\"${a}\");\n\
                   ```\n";
        assert_eq!(check(doc), "");
    }

    #[test]
    fn an_ignore_block_needs_a_reason() {
        let doc = "```buri ignore\nwhatever\n```\n";
        assert!(check(doc).contains("must say why"));
        let doc = "```buri ignore why=\"a precedence table, not a program\"\nwhatever\n```\n";
        assert_eq!(check(doc), "");
    }

    #[test]
    fn a_run_block_must_pin_its_output() {
        let doc = "```buri run wrap=body\nlet _ = ctx.println(\"hi\");\n```\n";
        assert!(check(doc).contains("```stdout"));
    }

    /// The end-to-end case: compile, execute under the JS runtime, and
    /// compare stdout with what the document promises.
    #[test]
    fn a_run_block_executes_and_its_output_is_pinned() {
        let runtime = crate::commands::test::js_runtime();
        if std::process::Command::new(runtime).arg("--version").output().is_err() {
            eprintln!("skipping: no JavaScript runtime");
            return;
        }
        let good = "```buri run wrap=body\n\
                    let _ = ctx.println(\"basket total: $36.50\");\n\
                    ```\n\
                    ```stdout\n\
                    basket total: $36.50\n\
                    ```\n";
        assert_eq!(check(good), "");

        let wrong = "```buri run wrap=body\n\
                     let _ = ctx.println(\"one thing\");\n\
                     ```\n\
                     ```stdout\n\
                     another thing\n\
                     ```\n";
        assert!(check(wrong).contains("prints something else"), "a wrong transcript must be caught");
    }

    #[test]
    fn hidden_lines_are_compiled_but_not_shown() {
        let body = "# from \"core/str\" import * as str;\nlet s = str.trim(\"  x  \");\n";
        let (source, _, _) = strip_annotations(body);
        assert!(source.starts_with("from \"core/str\""));
        assert!(!rendered(body).contains("core/str"));
        assert!(rendered(body).contains("str.trim"));
    }
}

// -----------------------------------------------------------------------------
// Documentation comments
// -----------------------------------------------------------------------------

/// The `///` and `//!` comments of a source file, as a markdown document.
///
/// The trick that makes this cheap: the result has **the same number of lines
/// as the source**, with each doc line at its own line number and everything
/// else blank. So a block's `origin.line` is already the line in the `.buri`
/// file, and there is no map to build, keep, or get wrong. The blank lines
/// between doc runs are also what separates one comment's prose from the
/// next's, which is what markdown wants anyway.
///
/// This is deliberately textual rather than a walk over the AST: a file whose
/// examples are worth checking may be one that does not currently compile, and
/// the fences in it are still worth extracting.
pub fn doc_comments(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        let t = line.trim_start();
        // `////` is not a doc comment — the lexer says so, and so does this.
        let body = if let Some(rest) = t.strip_prefix("//!") {
            Some(crate::parsing::lexer::doc_body(rest))
        } else if t.starts_with("///") && !t.starts_with("////") {
            Some(crate::parsing::lexer::doc_body(&t[3..]))
        } else {
            None
        };
        if let Some(b) = body {
            out.push_str(&b);
        }
        out.push('\n');
    }
    out
}

/// Whether a source file has anything for the doctest harness to do. A byte
/// scan, so a repository full of files with no examples costs nothing.
pub fn has_examples(source: &str) -> bool {
    source.contains("```")
}
