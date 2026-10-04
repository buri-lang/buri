//! Compiling the documentation.
//!
//! Every fenced Buri block in every document is a real module, compiled by the
//! real front end against the real standard library. A block that stops
//! compiling fails `cargo test`, which is the whole point: the examples cannot
//! drift from the language because they *are* the language, run.
//!
//! The hard part is that a document shows fragments. `let a = 5;` is not a
//! module, `fn total(xs: [N]): N { ... }` is not a body, and a section about
//! errors deliberately shows code that must not compile. Five mechanisms
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
//!   * `file=<path>` says the fence *is* one file of a repository: the one
//!     `repo=` names, copied, or one the page's own `file=` fences make from an
//!     empty `REPO.buri`. Every such fence is written into it and the result is
//!     built the way `buri build` builds it, so a platform's `platform.buri`, a
//!     data file a generator reads, and a program importing what it generates
//!     are each checked where they can mean something. A ```textproto fence
//!     tagged `fail code=` is written in on its own and must fail the build
//!     with that code.
//!
//! There is no way to leave a fence out: a textproto fence is a build file, a
//! `REPO.buri`, or a file of a repository.
//!
//! What is deliberately absent is any rewriting of the block's text before it
//! is shown. Whatever a reader copies is what was compiled, character for
//! character, apart from lines beginning `# `, which are hidden imports and
//! are marked as such in the source document.

use crate::compiler::driver;
use crate::compiler::modules::Role;
use crate::diagnostics::{Invariant as _, SourceMap};
use crate::documentation::markdown::{self, Fence, Info};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// What a block claims about itself
// ---------------------------------------------------------------------------

/// A `// ERROR: <substring>` line: `(0-based line within `source`, substring)`.
pub type Annotation = (usize, String);

/// What a block claims about itself, holding the evidence for the claim.
///
/// Each mode's required payload lives *in* its variant, so the four rules that
/// used to be enforced only by `parse_block` are now enforced by the compiler:
/// a `run` block cannot exist without pinned output (it used to fall back to
/// `unwrap_or_default()`, so a `run` block that pinned nothing *passed*), a
/// `fail` block cannot carry a stdout transcript nothing would ever compare,
/// and `run` and `sig` cannot carry `// ERROR:` annotations they would
/// silently not honour.
#[derive(Clone, Debug)]
pub enum Claim {
    /// Must typecheck. The default, and what most blocks are.
    Check { errors: Vec<Annotation> },
    /// A list of signatures. Parsed as a standard-library module, so a
    /// declaration may have no body.
    Sig,
    /// Must typecheck, compile, and run, printing exactly this. The text comes
    /// from the ```stdout fence beneath the block.
    Run { stdout: String },
    /// Must be rejected. `code` is the diagnostic code the error catalog
    /// demands, `messages` the substrings from the ```error fence beneath the
    /// block, and `errors` the block's own `// ERROR:` annotations. At least
    /// one of the three is present, or the block says nothing about how it
    /// fails.
    Fail { code: Option<String>, messages: Vec<String>, errors: Vec<Annotation> },
    /// Must typecheck, and `buri lint` must report `code` about it. The block
    /// is the one source of a scratch library, so a lint page's example is
    /// held to the finding it explains.
    Lint { code: String },
}

impl Claim {
    /// The `// ERROR:` annotations this claim honours, each compiled on its
    /// own. `sig`, `run` and `lint` do not take them, so for those there are
    /// none to have.
    pub fn errors(&self) -> &[Annotation] {
        match self {
            Claim::Check { errors } | Claim::Fail { errors, .. } => errors,
            Claim::Sig | Claim::Run { .. } | Claim::Lint { .. } => &[],
        }
    }
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
    /// What the block claims, and what it offers as evidence.
    pub claim: Claim,
    pub wrap: Wrap,
    /// Named preambles spliced ahead of this block.
    pub uses: Vec<String>,
    /// This block's own name, for a later block to `use=`.
    pub name: Option<String>,
    /// Effects the synthetic `main` builds a context from.
    pub effects: Vec<String>,
    /// A repository to compile against, relative to the documentation root.
    /// Without one a snippet may not name a `//...` module.
    pub repo: Option<String>,
    /// The package this example stands in, as a label. A document about a
    /// library shows the library's own files, and their imports are legal only
    /// from inside it.
    pub package: Option<String>,
    /// What the block is being compiled *as*, when the default is wrong: a
    /// document showing an `effect` declaration is showing a platform module,
    /// and one showing a `test` is showing a test source.
    pub role: Option<Role>,
    /// The path of the repository file this fence is, when it is one. It is
    /// then built with the rest of that repository rather than compiled on
    /// its own.
    pub file: Option<String>,
    /// The output's platform this block is built for, when the block is
    /// *about* a platform's host.
    ///
    /// `None` — the default, and every block but one — accepts any bundled
    /// host, because a snippet builds no output. Writing `platform=node` is how
    /// a document says "and this is what does not compile there", which is the
    /// only way the error page for `entry-host-mismatch` can carry a program
    /// that provokes it.
    pub platform: Option<crate::build::buildfile::Platform>,
    /// The fence exactly as it stands in the document: hidden `# ` markers,
    /// `// ERROR:` annotations and all.
    ///
    /// What the *reader* gets out of it is `rendered(&body)`, which is what
    /// `documentation::layout` holds to the formatter's layout, and what a
    /// rewrite of it puts back.
    pub body: String,
    /// The columns the fence itself is indented by, for one written inside a
    /// list item.
    pub indent: usize,
    /// The text to compile: hidden `# ` markers removed, `// ERROR:` lines
    /// blanked (they are compiled separately).
    pub source: String,
    /// The same text with the `// ERROR:` lines still in place, so a variant
    /// can put one of them back.
    pub original: String,
    pub origin: Origin,
}

/// A block a document says is textproto rather than Buri.
#[derive(Clone, Debug)]
pub struct ProtoBlock {
    pub schema: Schema,
    pub source: String,
    pub origin: Origin,
}

/// A fence that is one file of a repository the page builds.
#[derive(Clone, Debug)]
pub struct RepoFile {
    /// The repository it is written into, relative to the documentation root,
    /// or `None` for the one the page's own `file=` fences make.
    pub repo: Option<String>,
    /// Where in that repository.
    pub path: String,
    /// The file's text: what the fence shows, with hidden lines put back.
    pub text: String,
    /// The code the build must fail with, for a fence showing a mistake.
    pub fails_with: Option<String>,
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
    /// The fences that are files of a repository, Buri and textproto alike.
    pub files: Vec<RepoFile>,
    /// Malformed fence info strings, and fences that do not say what they are.
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
        Extracted { blocks: Vec::new(), protos: Vec::new(), files: Vec::new(), failures: Vec::new() };

    for (i, fence) in fences.iter().enumerate() {
        let origin = Origin { file: file.to_string(), line: fence.body_line };
        // A malformed info string is reported whatever the language claims to
        // be. It used to be swallowed, and a fence that could not say what it
        // was got neither compiled nor complained about.
        let info = match &fence.info {
            Ok(info) => info,
            Err(msg) => {
                out.failures.push(Failure {
                    origin,
                    what: msg.clone(),
                    detail: "the info string is `<lang> [mode] [key=value]...`".into(),
                });
                continue;
            }
        };
        match fence.lang {
            "buri" => match parse_block(fence, info, &origin, fences.get(i.saturating_add(1))) {
                Ok(b) => {
                    if let Some(path) = &b.file {
                        out.files.push(RepoFile {
                            repo: b.repo.clone(),
                            path: path.clone(),
                            text: b.source.clone(),
                            fails_with: None,
                            origin: origin.clone(),
                        });
                    }
                    out.blocks.push(b);
                }
                Err(f) => out.failures.push(f),
            },
            "textproto" => {
                if let Err(f) = textproto_fence(fence, info, origin, &mut out) {
                    out.failures.push(f);
                }
            }
            // Expected-output fences belong to the block above them.
            "stdout" | "error" => {}
            _ => {}
        }
    }
    out
}

/// Reads one ```textproto fence into what it claims to be.
///
/// It is a build file or a `REPO.buri` (`schema=`), a file of a repository
/// (`file=`, which may name a schema too), or a file of a repository whose
/// build must fail with a code (`fail code=… file=…`). A fence that says none
/// of these is an error, so no fence goes unchecked by omission.
fn textproto_fence(fence: &Fence, info: &Info, origin: Origin, out: &mut Extracted) -> Result<(), Failure> {
    let fail = |what: String, detail: &str| Failure { origin: origin.clone(), what, detail: detail.to_string() };
    let schema = match info.get("schema") {
        None => None,
        Some("build") => Some(Schema::Build),
        Some("repo") => Some(Schema::Repo),
        Some(other) => {
            return Err(fail(format!("`schema={other}` is not a schema"), "the schemas are build and repo"))
        }
    };
    let file = info.get("file").map(String::from);
    if info.get("repo").is_some() && file.is_none() {
        return Err(fail("`repo=` says nothing without `file=`".into(), "name the file this fence is"));
    }
    let mode = info.mode.as_deref();
    if info.get("code").is_some() && mode != Some("fail") {
        return Err(fail("`code=` says nothing outside `fail`".into(), "tag the fence `fail`"));
    }
    let fails_with = match mode {
        None if schema.is_none() && file.is_none() => {
            return Err(fail(
                "this textproto block does not say what it is".into(),
                "tag it `schema=build` or `schema=repo`, or `file=<path>` (and `repo=`) for \
                 a file of a repository, such as a data file a generator reads",
            ))
        }
        None => None,
        Some("fail") => {
            let (Some(code), Some(_)) = (info.get("code"), &file) else {
                return Err(fail(
                    "a textproto `fail` block names its code and its file".into(),
                    "add `code=<diagnostic code>` and `file=<path>`, and `repo=` for the \
                     repository that gives it a schema",
                ));
            };
            if schema.is_some() {
                return Err(fail("a `fail` fence takes no `schema=`".into(), "its build is what fails it"));
            }
            Some(code.to_string())
        }
        Some(other) => {
            return Err(fail(format!("`{other}` is not a mode"), "the one textproto mode is fail"))
        }
    };
    if let Some(schema) = schema {
        out.protos.push(ProtoBlock { schema, source: fence.body.clone(), origin: origin.clone() });
    }
    if let Some(path) = file {
        out.files.push(RepoFile {
            repo: info.get("repo").map(String::from),
            path,
            text: fence.body.clone(),
            fails_with,
            origin,
        });
    }
    Ok(())
}

fn parse_block(
    fence: &Fence,
    info: &Info,
    origin: &Origin,
    next: Option<&Fence>,
) -> Result<Block, Failure> {
    let fail = |what: String, detail: &str| Failure {
        origin: origin.clone(),
        what,
        detail: detail.to_string(),
    };

    let wrap = match info.get("wrap") {
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

    let role = match info.get("role") {
        None => None,
        Some("source") => Some(Role::Source),
        Some("entry") => Some(Role::Entry),
        Some("std") => Some(Role::Std),
        Some("platform") => Some(Role::Platform),
        Some("test") => Some(Role::TestSource),
        // A `testing/` module is not a test source: it is ordinary library
        // code that only test sources may import, so it may `export`, and it
        // is the one place a `context` may be exported from.
        Some("testing") => Some(Role::TestOnly),
        Some(other) => {
            return Err(fail(
                format!("`role={other}` is not a role"),
                "the roles are source, entry, std, platform, test, testing",
            ))
        }
    };

    let repo = info.get("repo").map(String::from);
    let package = info.get("package").map(String::from);

    let effects = match info.get("ctx") {
        Some(_) => info.list("ctx"),
        // Enough to print and to allocate, which is what a fragment that names
        // `ctx` at all almost always wants.
        None => vec!["alloc".into(), "stdout".into()],
    };
    for e in &effects {
        if effect_binding(e).is_none() {
            return Err(fail(
                format!("`{e}` is not an effect this harness can build"),
                "the effects are alloc, stdout, stderr, stdin, fsread, fswrite, net, clock, \
                 rand, entropy, env, proc, tasks, listen, sockets",
            ));
        }
    }

    let (source, original, errors) = strip_annotations(&fence.body);
    let mode = info.mode.as_deref().unwrap_or("check");

    // An expected-output fence directly beneath belongs to exactly one mode.
    // Attaching it anywhere else would drop it on the floor, which is how a
    // `fail` block used to be able to carry a transcript nobody compared.
    if let Some(next) = next.filter(|f| matches!(f.lang, "stdout" | "error")) {
        let wants = if next.lang == "stdout" { "run" } else { "fail" };
        if mode != wants {
            return Err(fail(
                format!(
                    "a ```{} fence under a `{mode}` block is never compared against anything",
                    next.lang
                ),
                &format!("tag the block `{wants}`, or drop the fence"),
            ));
        }
    }
    // In `run` and `sig` an annotation would be silently unhonoured, which is
    // worse than rejecting it.
    let no_annotations = |errors: &[Annotation]| {
        if errors.is_empty() {
            return Ok(());
        }
        Err(fail(
            format!("`// ERROR:` annotations are not compiled in `{mode}` mode"),
            "use the default mode, or `fail`",
        ))
    };
    let attached = |lang: &str| next.filter(|f| f.lang == lang);

    let claim = match mode {
        "check" => Claim::Check { errors },
        "sig" => {
            no_annotations(&errors)?;
            Claim::Sig
        }
        "run" => {
            no_annotations(&errors)?;
            let Some(out) = attached("stdout") else {
                return Err(fail(
                    "a `run` block must be followed by a ```stdout fence".into(),
                    "add one directly beneath it holding exactly what the program prints; \
                     `BURI_BLESS=1` fills it in",
                ));
            };
            Claim::Run { stdout: out.body.clone() }
        }
        "fail" => {
            let messages: Vec<String> = attached("error")
                .map(|f| {
                    f.body.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
                })
                .unwrap_or_default();
            let code = info.get("code").map(String::from);
            if messages.is_empty() && errors.is_empty() && code.is_none() {
                return Err(fail(
                    "a `fail` block must say what it fails with".into(),
                    "add a ```error fence beneath it, or annotate the offending line \
                     `// ERROR: <substring of the message>`",
                ));
            }
            Claim::Fail { code, messages, errors }
        }
        "lint" => {
            no_annotations(&errors)?;
            let Some(code) = info.get("code") else {
                return Err(fail(
                    "a `lint` block must name the finding it shows".into(),
                    "add `code=<lint code>`",
                ));
            };
            Claim::Lint { code: code.to_string() }
        }
        other => {
            return Err(fail(
                format!("`{other}` is not a mode"),
                "the modes are check, sig, run, fail, lint. Every example is compiled: one \
                 that needs a repository names a file of one with `file=`",
            ))
        }
    };
    // `code=` is read by two modes. Anywhere else it is a claim the harness
    // would never check.
    if info.get("code").is_some() && !matches!(mode, "fail" | "lint") {
        return Err(fail(
            format!("`code=` says nothing in a `{mode}` block"),
            "it is read only by `fail` and `lint`",
        ));
    }
    // A repository's file is built with the rest of the repository, which is
    // the whole of what it claims. Everything that shapes a snippet's own
    // compilation would be a claim nothing checks.
    let file = info.get("file").map(String::from);
    if file.is_some() {
        if mode != "check" || !claim.errors().is_empty() {
            return Err(fail(
                "a `file=` fence is built, not compiled on its own".into(),
                "drop the mode and any `// ERROR:` annotations: it must build cleanly",
            ));
        }
        for key in ["wrap", "use", "name", "ctx", "package", "role", "platform"] {
            if info.get(key).is_some() {
                return Err(fail(
                    format!("`{key}=` says nothing in a `file=` fence"),
                    "the repository and the file's path say how it is compiled",
                ));
            }
        }
    }
    // `platform=` names one of the closed enum's values. An unknown one is a
    // mistake in the document rather than a silent fall back to "grant
    // everything", which is the answer that would make the block pass while
    // proving nothing.
    let platform = match info.get("platform") {
        None => None,
        Some(name) => match crate::build::buildfile::PlatformName::bundled(name) {
            Some(p) => Some(p.host_platform()),
            None => {
                return Err(fail(
                    format!("`{name}` is not a platform"),
                    &format!(
                        "the platforms are {}",
                        crate::build::buildfile::Platform::names_phrase()
                    ),
                ))
            }
        },
    };
    Ok(Block {
        claim,
        wrap,
        body: fence.body.clone(),
        indent: fence.indent,
        uses: info.list("use"),
        name: info.get("name").map(String::from),
        effects,
        repo,
        package,
        role,
        file,
        platform,
        source,
        original,
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
            // A lone `#` is a hidden *blank* line, which is what separates two
            // hidden declarations. `rendered` has always read it that way; this
            // is the other half, so the compiler sees the blank line rather
            // than a `#`.
            None if raw == "#" => String::new(),
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
    let (_, want) = line.split_once("// ERROR:")?;
    Some(want.trim().to_string())
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

/// `(the effect type, qualified by the alias the wrapper imports it under, and
/// the host value)`.
///
/// The set is what a fence's generated `main` can actually *bind*, which is a
/// smaller thing than what `platform/effect` declares. The three UI effects are
/// absent because a fence builds no output and so has no platform, and a fence
/// naming one would fail at a line its author never wrote.
///
/// `Tasks`, `Listen` and `Sockets` are all here now and none of them was: each
/// was withheld while its row named no platform, and each row landed here the
/// day the grant did. A fence may write `ctx=listen` and call
/// `core/net/server` — though a fence that actually *served* would never
/// return, so what the rows buy is the shape of a call rather than a running
/// server.
///
/// **The type is qualified because the effects are not all in one module.**
/// `core/fs` declares `FileSystemRead` and `FileSystemWrite` — their methods name a `Path`, and
/// `core/path` names `Allocator`, so `platform/effect` is below the type they take —
/// and a row that named only `FileSystemRead` would build a wrapper importing it from
/// the module that does not have it. `fs` is deliberately not a row: the
/// filesystem is two grants, and a fence says which one it wants.
fn effect_binding(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "alloc" => ("__effect.Allocator", "alloc"),
        "stdout" => ("__effect.Stdout", "stdout"),
        "stderr" => ("__effect.Stderr", "stderr"),
        "stdin" => ("__effect.Stdin", "stdin"),
        "fsread" => ("__fs.FileSystemRead", "fs"),
        "fswrite" => ("__fs.FileSystemWrite", "fs"),
        "net" => ("__effect.Network", "net"),
        "clock" => ("__effect.Clock", "clock"),
        "rand" => ("__effect.Random", "rand"),
        "entropy" => ("__effect.Entropy", "entropy"),
        "env" => ("__effect.Environment", "env"),
        "proc" => ("__effect.Process", "proc"),
        "tasks" => ("__effect.Tasks", "tasks"),
        "listen" => ("__effect.Listen", "listen"),
        "sockets" => ("__effect.Sockets", "sockets"),
        "websocketclient" => ("__effect.WebSocketClient", "websocketClient"),
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
    fn document_line(&self, origin: &Origin, reported: usize) -> usize {
        if reported <= self.prefix_lines {
            return origin.line.saturating_sub(1).max(1);
        }
        // Past the prefix, so the subtraction below is at least one and the
        // whole expression cannot go under `origin.line`.
        origin.line.saturating_add(reported.saturating_sub(self.prefix_lines)).saturating_sub(1)
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
    registry: &Registry,
    only_error: Option<usize>,
) -> Result<Compiland, Failure> {
    let mut prefix = String::new();

    for name in &block.uses {
        let Some(text) = registry.get(name) else {
            return Err(Failure {
                origin: block.origin.clone(),
                what: format!("there is no preamble named `{name}`"),
                detail: "name an earlier block with `name=...`, or add a file to \
                         `crates/docs/src/docs/harness/` and register it in `documentation/harness.rs`"
                    .into(),
            });
        };
        prefix.push_str(text);
        if !prefix.ends_with('\n') {
            prefix.push('\n');
        }
    }

    let sig = matches!(block.claim, Claim::Sig);
    let role = match (block.role, sig) {
        // An explicit `role=` wins, except that `sig` needs a role whose
        // parser allows a declaration with no body — `Std` and `Platform` are
        // the two, and a block showing an `effect` must be `Platform`.
        (Some(r @ (Role::Std | Role::Platform)), true) => r,
        (_, true) => Role::Std,
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
            body.push_str("from \"platform/effect\" import * as __effect;\n");
            // `core/fs` beside it, and hidden for the same reason: it is where
            // `FileSystemRead` and `FileSystemWrite` are declared, so a fence naming either has
            // to reach a second module for a name it never writes.
            body.push_str("from \"core/fs\" import * as __fs;\n");
            // The host of the platform the block is pinned to, and `native`'s —
            // every effect a fragment can name — where it is pinned to none.
            let (owner, host) = block
                .platform
                .and_then(crate::compiler::standard_library::host_type)
                .unwrap_or(("native", "NativeHost"));
            body.push_str(&format!("from \"{owner}\" import {{ {host} as __Host }};\n"));
            // `core/io` under its ordinary name, and it is the one import here
            // that is not `__`-prefixed — deliberately, because a fragment
            // writes `io.println(ctx, ...)` and that spelling is the thing
            // being documented. An effect is performed by handing the context
            // to a function (SPEC 10.2), so a body fragment that prints needs
            // this module and has nowhere of its own to import it: an `import`
            // inside `main` is not a statement. The two above are hidden
            // because nothing writes them; this one is written constantly.
            body.push_str("from \"core/io\" import * as io;\n");
            body.push_str("export fn main(__host: __Host): Result<(), Str> {\n");
            body.push_str("  let ctx = context {\n");
            for e in &block.effects {
                let (trait_name, host_value) = effect_binding(e)
                    .or_ice("`parse_block` refuses a block naming an effect with no binding");
                body.push_str(&format!("    {trait_name}: __host.{host_value},\n"));
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

    let prefix_lines = prefix.lines().count().saturating_add(body.lines().count());
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

/// Compiles one block, against a repository the block named with `repo=`, and
/// reports every way it did not do what it claimed.
fn run_block_in(
    workspace: Option<&crate::build::workspace::Workspace>,
    block: &Block,
    registry: &Registry,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
) -> Vec<Failure> {
    let package = match (&block.package, workspace) {
        (Some(label), Some(workspace)) => {
            let path = label.trim_start_matches('/');
            match workspace.package_by_path(path) {
                Some(id) => Some(id),
                None => {
                    return vec![Failure {
                        origin: block.origin.clone(),
                        what: format!("`package={label}` is not a package of this repository"),
                        detail: "name a package that exists, as in `package=//lib/money`".into(),
                    }]
                }
            }
        }
        (Some(label), None) => {
            return vec![Failure {
                origin: block.origin.clone(),
                what: format!("`package={label}` needs a repository"),
                detail: "add `repo=...`, or run this inside the repository the package is in"
                    .into(),
            }]
        }
        (None, _) => None,
    };
    let mut failures = Vec::new();

    // The block with every annotated line blanked must be clean, whatever the
    // annotations say — otherwise a `// ERROR:` would be hiding a second,
    // unrelated mistake.
    let base = match assemble(block, registry, None) {
        Ok(c) => c,
        Err(f) => return vec![f],
    };
    let name = format!("{}", block.origin);
    let analysis = driver::analyze_snippet_on(
        workspace,
        package,
        map,
        cache,
        &name,
        &base.text,
        base.role,
        block.platform,
    );
    // Some rules are only checked once the program is compiled for the types
    // it is actually called with, so a `fail code=` block that checks cleanly
    // is compiled too. Its annotated lines are blanked in `base`, so a block
    // with any is held to checking alone.
    let compiled = match &block.claim {
        Claim::Fail { code: Some(_), errors, .. }
            if errors.is_empty() && !analysis.diagnostics.has_errors() =>
        {
            driver::compile_snippet_js_as(workspace, package, map, &name, &base.text).err()
        }
        _ => None,
    };
    let reported = compiled.as_ref().unwrap_or(&analysis.diagnostics);
    let diagnostics: Vec<String> = reported
        .items
        .iter()
        .filter(|d| d.is_error())
        .map(|d| describe(map, d, &base, &block.origin))
        .collect();

    match &block.claim {
        Claim::Fail { code, messages, errors } => {
            // `code=` is what makes the error catalog self-verifying: a page
            // claims to explain a rule, and its example must provoke exactly
            // that rule. A page nothing can provoke is a page that describes an
            // error the compiler no longer emits.
            if let Some(want) = code {
                let got: Vec<String> = reported
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
                                if diagnostics.is_empty() {
                                    "nothing".into()
                                } else {
                                    diagnostics.join("\n")
                                }
                            )
                        } else {
                            format!("it produced: {}", got.join(", "))
                        },
                    });
                }
            }
            if diagnostics.is_empty() && errors.is_empty() {
                failures.push(Failure {
                    origin: block.origin.clone(),
                    what: "this block is marked `fail` but compiles".into(),
                    detail: "either the language changed and the document is now wrong, \
                             or the block needs to be retagged"
                        .into(),
                });
            }
            for want in messages {
                if !diagnostics.iter().any(|d| d.contains(want.as_str())) {
                    failures.push(Failure {
                        origin: block.origin.clone(),
                        what: format!("no diagnostic contains `{want}`"),
                        detail: if diagnostics.is_empty() {
                            "it compiled cleanly".into()
                        } else {
                            diagnostics.join("\n")
                        },
                    });
                }
            }
        }
        _ => {
            if !diagnostics.is_empty() {
                failures.push(Failure {
                    origin: block.origin.clone(),
                    what: "this example does not compile".into(),
                    detail: diagnostics.join("\n"),
                });
            }
        }
    }
    if let Claim::Lint { code } = &block.claim {
        if failures.is_empty() {
            match lint_codes(&base.text) {
                Ok(got) if got.contains(code) => {}
                Ok(got) => failures.push(Failure {
                    origin: block.origin.clone(),
                    what: format!("`buri lint` does not report `{code}` about this example"),
                    detail: if got.is_empty() {
                        "it reported nothing".into()
                    } else {
                        format!("it reported: {}", got.join(", "))
                    },
                }),
                Err(why) => failures.push(Failure {
                    origin: block.origin.clone(),
                    what: "this example could not be linted".into(),
                    detail: why,
                }),
            }
        }
    }

    // Each annotated line, compiled on its own.
    for (index, want) in block.claim.errors() {
        let variant = match assemble(block, registry, Some(*index)) {
            Ok(c) => c,
            Err(f) => {
                failures.push(f);
                continue;
            }
        };
        let a = driver::analyze_snippet_on(
            workspace,
            package,
            map,
            cache,
            &name,
            &variant.text,
            variant.role,
            block.platform,
        );
        let got: Vec<String> = a
            .diagnostics
            .items
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect();
        if !got.iter().any(|m| m.contains(want.as_str())) {
            let line = variant.document_line(
                &block.origin,
                base.prefix_lines.saturating_add(*index).saturating_add(1),
            );
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

    let pinned = match &block.claim {
        Claim::Run { stdout } if failures.is_empty() => Some(stdout),
        _ => None,
    };
    if let Some(want) = pinned {
        match driver::run_snippet(workspace, map, &name, &base.text) {
            Ok(stdout) => {
                if &stdout != want {
                    failures.push(Failure {
                        origin: block.origin.clone(),
                        what: "this example prints something else".into(),
                        detail: format!(
                            "expected:\n{}\n  actual:\n{}",
                            indent(want),
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

/// Every code `buri lint` reports about `source`, written as the `lib.buri` of
/// the one library in a scratch repository.
fn lint_codes(source: &str) -> Result<Vec<String>, String> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("buri-doc-lint-{}-{n}", std::process::id()));
    let codes = lint_codes_in(&root, source);
    let _ = std::fs::remove_dir_all(&root);
    codes
}

fn lint_codes_in(root: &std::path::Path, source: &str) -> Result<Vec<String>, String> {
    let package = root.join("lib/example");
    std::fs::create_dir_all(&package).map_err(|e| e.to_string())?;
    std::fs::write(root.join("REPO.buri"), "").map_err(|e| e.to_string())?;
    std::fs::write(package.join("BUILD.buri"), "library {}\n").map_err(|e| e.to_string())?;
    std::fs::write(package.join("lib.buri"), source).map_err(|e| e.to_string())?;
    let mut map = SourceMap::new();
    let mut diagnostics = crate::diagnostics::Diagnostics::new();
    let workspace = crate::build::workspace::Workspace::load(root, &mut map, &mut diagnostics)
        .map_err(|e| e.to_string())?;
    let mut session = crate::build::session::Session {
        root: root.to_path_buf(),
        map,
        parsed: crate::parsing::parser::Cache::new(),
        diagnostics,
        workspace: std::sync::Arc::new(workspace),
        rendering: crate::build::session::Rendering::Human { color: false },
    };
    let mut codes = Vec::new();
    for target in session.resolve_targets(&[])? {
        let analysis = crate::commands::lint::analysis_of(&mut session, target);
        let found = crate::commands::lint::findings_for_target(&session, target, &analysis);
        codes.extend(found.items.iter().filter_map(|d| d.code.clone()));
    }
    Ok(codes)
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
///
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

/// A loaded repository with its generators run, so that an example importing a
/// generated module has one to import.
///
/// The session is built here rather than through `session::open_at` because the
/// workspace was loaded into the caller's source map: a second load would give
/// every build file a second file id, and a diagnostic pointing at one would
/// then be rendered against the wrong text.
fn prepared(
    root: &std::path::Path,
    workspace: crate::build::workspace::Workspace,
    map: &SourceMap,
) -> std::sync::Arc<crate::build::workspace::Workspace> {
    let workspace = std::sync::Arc::new(workspace);
    let mut session = crate::build::session::Session {
        root: root.to_path_buf(),
        map: map.clone(),
        parsed: crate::parsing::parser::Cache::new(),
        diagnostics: crate::diagnostics::Diagnostics::new(),
        workspace: std::sync::Arc::clone(&workspace),
        rendering: crate::build::session::Rendering::Human { color: false },
    };
    crate::build::generators::prepare(
        &mut session,
        &crate::commands::arguments::Flags::default(),
        &crate::build::sources::Overlay::new(),
    );
    workspace
}

fn run_file_with(
    root: &std::path::Path,
    default_to_root: bool,
    file: &str,
    text: &str,
) -> Vec<Failure> {
    let extracted = extract(file, text);
    let mut failures = extracted.failures;
    let mut registry = Registry::new();
    let mut map = SourceMap::new();
    let mut cache = crate::parsing::parser::Cache::new();
    // One `Workspace` per repository named in this document, not per block:
    // loading a monorepo reads every build file in it, and running its
    // generators runs a program per rule that declares one.
    let mut repos: HashMap<String, Option<std::sync::Arc<crate::build::workspace::Workspace>>> =
        HashMap::new();

    for block in &extracted.blocks {
        // A repository's file is built with its repository below, so it
        // loads nothing here. Its layout is still a fence somebody copies.
        if block.file.is_some() {
            failures.extend(crate::documentation::layout::check(block));
            continue;
        }
        let named = match (&block.repo, default_to_root) {
            (Some(rel), _) => Some(rel.clone()),
            (None, true) => Some(String::new()),
            (None, false) => None,
        };
        let workspace = match &named {
            None => None,
            Some(rel) => {
                if !repos.contains_key(rel) {
                    let mut diagnostics = crate::diagnostics::Diagnostics::new();
                    let loaded = crate::build::workspace::Workspace::load(
                        &root.join(rel),
                        &mut map,
                        &mut diagnostics,
                    )
                    .ok()
                    .map(|loaded| prepared(&root.join(rel), loaded, &map));
                    repos.insert(rel.clone(), loaded);
                }
                match repos.get(rel).and_then(|w| w.as_ref()) {
                    Some(workspace) => Some(&**workspace),
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
        failures.extend(crate::documentation::layout::check(block));
        failures.extend(run_block_in(workspace, block, &registry, &mut map, &mut cache));
        registry.record(block);
    }
    for proto in &extracted.protos {
        failures.extend(run_proto(proto));
    }
    failures.extend(run_repositories(root, &extracted.files));
    failures
}

/// Renders every failure in a document as one block of text.
pub fn report(failures: &[Failure]) -> String {
    failures.iter().map(|f| f.to_string()).collect::<Vec<_>>().join("\n")
}

/// A ```textproto block has to parse *and* satisfy the schema its document
/// claims, which is what keeps `BUILD-FILES.md` honest about fields that no
/// longer exist.
fn run_proto(proto: &ProtoBlock) -> Vec<Failure> {
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

// -----------------------------------------------------------------------------
// Repositories a page builds
// -----------------------------------------------------------------------------

/// Builds every repository a page's `file=` fences are files of.
///
/// One build per repository holds every fence that should build cleanly, so
/// they are checked together, as the files of one repository are. Each
/// `fail` fence is then written over that and built on its own, because a
/// mistake shown on one fence must not be what another fence's build trips
/// on. The clean build comes first even for a page whose only fence fails:
/// it is what says the code came from the fence and not the repository.
fn run_repositories(root: &std::path::Path, files: &[RepoFile]) -> Vec<Failure> {
    let mut repos: Vec<Option<&str>> = Vec::new();
    for f in files {
        if !repos.contains(&f.repo.as_deref()) {
            repos.push(f.repo.as_deref());
        }
    }
    let mut failures = Vec::new();
    for repo in repos {
        let mine: Vec<&RepoFile> = files.iter().filter(|f| f.repo.as_deref() == repo).collect();
        let mut seen: Vec<&str> = Vec::new();
        for f in &mine {
            if seen.contains(&f.path.as_str()) {
                failures.push(Failure {
                    origin: f.origin.clone(),
                    what: format!("`{}` is already a fence on this page", f.path),
                    detail: "a repository has one file at a path, so only one fence may be it".into(),
                });
            }
            seen.push(&f.path);
        }
        let clean: Vec<&RepoFile> = mine.iter().copied().filter(|f| f.fails_with.is_none()).collect();
        let Some(first) = mine.first() else { continue };
        match build_with(root, repo, &clean) {
            Err(why) => {
                failures.push(Failure {
                    origin: first.origin.clone(),
                    what: "the repository these fences are files of could not be built".into(),
                    detail: why,
                });
                continue;
            }
            Ok(found) => {
                for e in found {
                    // Laid at the fence that is the file, where there is one.
                    let at = clean.iter().find(|f| e.file == f.path);
                    let (origin, detail) = match at {
                        Some(f) => (
                            Origin {
                                file: f.origin.file.clone(),
                                line: f.origin.line.saturating_add(e.line).saturating_sub(1),
                            },
                            e.message.clone(),
                        ),
                        None => (first.origin.clone(), e.to_string()),
                    };
                    failures.push(Failure { origin, what: "this repository does not build".into(), detail });
                }
            }
        }
        for f in mine.iter().filter(|f| f.fails_with.is_some()) {
            let want = f.fails_with.as_deref().unwrap_or_default();
            let mut with = clean.clone();
            with.push(f);
            let found = match build_with(root, repo, &with) {
                Ok(found) => found,
                Err(why) => {
                    failures.push(Failure {
                        origin: f.origin.clone(),
                        what: "the repository this fence is a file of could not be built".into(),
                        detail: why,
                    });
                    continue;
                }
            };
            if found.iter().any(|e| e.code.as_deref() == Some(want)) {
                continue;
            }
            failures.push(Failure {
                origin: f.origin.clone(),
                what: format!("this file does not fail the build with `{want}`"),
                detail: if found.is_empty() {
                    "the repository built cleanly".into()
                } else {
                    found.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
                },
            });
        }
    }
    failures
}

/// One error a repository's build reported, located in the repository.
struct BuildError {
    /// The repository-relative path of the file it is in, or `""`.
    file: String,
    line: usize,
    col: usize,
    message: String,
    code: Option<String>,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.file.is_empty() {
            write!(f, "{}:{}:{}: ", self.file, self.line, self.col)?;
        }
        write!(f, "{}", self.message)?;
        if let Some(code) = &self.code {
            write!(f, " [{code}]")?;
        }
        Ok(())
    }
}

/// Copies `repo` (or starts from nothing), writes `files` into it, builds it,
/// and throws it away.
fn build_with(
    root: &std::path::Path,
    repo: Option<&str>,
    files: &[&RepoFile],
) -> Result<Vec<BuildError>, String> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = std::env::temp_dir().join(format!("buri-doc-repo-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let built = write_and_build(root, repo, files, &scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    built
}

fn write_and_build(
    root: &std::path::Path,
    repo: Option<&str>,
    files: &[&RepoFile],
    scratch: &std::path::Path,
) -> Result<Vec<BuildError>, String> {
    match repo {
        Some(rel) => {
            copy_tree(&root.join(rel), scratch).map_err(|e| format!("`repo={rel}` cannot be copied: {e}"))?
        }
        None => std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?,
    }
    if !scratch.join("REPO.buri").exists() {
        std::fs::write(scratch.join("REPO.buri"), "").map_err(|e| e.to_string())?;
    }
    for f in files {
        let path = scratch.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, &f.text).map_err(|e| e.to_string())?;
    }
    build_repository(scratch)
}

/// A directory, copied, without the build's own output.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_name() == ".buri" {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Every error `buri build` reports over a whole repository.
///
/// The same steps the command takes, target by target: a library or a tool is
/// checked, and a binary is built for each of its outputs. A native output is
/// checked for that output and not linked, so the answer does not hang on
/// whether this host can link one.
fn build_repository(root: &std::path::Path) -> Result<Vec<BuildError>, String> {
    use crate::build::actions;
    use crate::build::workspace::RuleKind;
    use crate::compiler::modules::Unit;
    let flags = crate::commands::arguments::Flags::default();
    let mut session = crate::build::session::open_at(root, &flags)?;
    let mut reported = std::mem::take(&mut session.diagnostics);
    for target in session.resolve_targets(&[])? {
        if target.kind != RuleKind::Binary {
            let mut diagnostics = crate::diagnostics::Diagnostics::new();
            actions::check_visibility(&session, target, &mut diagnostics);
            actions::check_tags(&session, target, &mut diagnostics);
            if !diagnostics.has_errors() {
                let unit = Unit { target: Some(target), platform: None, entry: None, with_tests: false };
                let analysis =
                    driver::analyze(Some(&session.workspace), &mut session.map, &mut session.parsed, &unit);
                diagnostics.extend(analysis.diagnostics.items);
            }
            reported.extend(diagnostics.items);
            continue;
        }
        for output in actions::selected_outputs(&session, target, &flags) {
            if !output.platform().is_native() {
                if let Err(d) = actions::build_target(&mut session, target, &output, &flags) {
                    reported.extend(d.items);
                }
                continue;
            }
            let mut diagnostics = crate::diagnostics::Diagnostics::new();
            actions::check_policy(&session, target, &output.output_platform(), &mut diagnostics);
            if !diagnostics.has_errors() {
                let unit = Unit {
                    target: Some(target),
                    platform: Some(output.platform()),
                    entry: Some(output.entry_name().to_string()),
                    with_tests: false,
                };
                let analysis =
                    driver::analyze(Some(&session.workspace), &mut session.map, &mut session.parsed, &unit);
                if !analysis.diagnostics.has_errors() && !analysis.checked.entries.contains_key(output.entry_name()) {
                    diagnostics.push(actions::missing_entry(&session, target, &output, &analysis.checked));
                }
                diagnostics.extend(analysis.diagnostics.items);
            }
            reported.extend(diagnostics.items);
        }
    }
    Ok(reported
        .items
        .iter()
        .filter(|d| d.is_error())
        .map(|d| {
            let (file, line, col) = if d.span.is_none() {
                (String::new(), 0, 0)
            } else {
                let source = session.map.get(d.span.file);
                let (line, col) = source.line_col(d.span.start);
                (source.name.clone(), line, col)
            };
            BuildError { file, line, col, message: d.message.clone(), code: d.code.clone() }
        })
        .collect())
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
        let marker = t
            .strip_prefix("//!")
            .or_else(|| t.strip_prefix("///").filter(|rest| !rest.starts_with('/')));
        if let Some(rest) = marker {
            out.push_str(&crate::parsing::lexer::doc_body(rest));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a synthetic document and returns its failures, rendered.
    fn check(doc: &str) -> String {
        // The repository root, as the docs' own `repo=` paths are written, and
        // not the working directory, which depends on how the test is run.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        report(&run_file_at(&root, "test.md", doc))
    }

    #[test]
    fn a_whole_module_compiles() {
        let doc = "```buri\n\
                   export fn double(n: Int): Int {\n    n * 2\n}\n\
                   ```\n";
        assert_eq!(check(doc), "");
    }

    #[test]
    fn a_statement_fragment_compiles_with_a_context() {
        let doc = "```buri wrap=body\n\
                   let a = 5;\n\
                   let _ = io.println(ctx, \"a is ${a}\").ignore();\n\
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
                   #\n\
                   # impl Ring {\n\
                   \x20   export fn size(self): Int;\n\
                   # }\n\
                   ```\n";
        assert_eq!(check(doc), "");
    }

    /// A block documenting a method on a *built-in* cannot be compiled where
    /// it is written: a type's operations may only be declared in its defining
    /// module (SPEC 6.7.3). Those blocks belong in the generated standard
    /// library reference, which renders them from `crates/stdlib/src/compiler/standard_library/sources/*.buri`; until
    /// a document is converted, they carry `ignore` and a reason. This test
    /// pins the rule so the reason stays true.
    #[test]
    fn a_builtin_impl_cannot_be_documented_as_a_compilable_block() {
        let doc = "```buri sig\n\
                   # impl<T> [T] {\n\
                   export fn length(self): Int;\n\
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
                   let bad: Int = \"nope\"; // ERROR: expected `I64`, found `Str`\n\
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
                   export struct Point {\n    export x: Int,\n    export y: Int,\n}\n\
                   ```\n\
                   ```buri use=pt wrap=body\n\
                   let p = Point { x: 1, y: 2 };\n\
                   let _ = io.println(ctx, \"${p.x}\").ignore();\n\
                   ```\n";
        assert_eq!(check(doc), "");
    }

    #[test]
    fn a_harness_is_reusable() {
        let doc = "```buri use=shapes wrap=body\n\
                   let a = area(.Circle(2.0));\n\
                   let _ = io.println(ctx, \"${a}\").ignore();\n\
                   ```\n";
        assert_eq!(check(doc), "");
    }

    /// Every example is compiled, so there is no mode that leaves one out.
    #[test]
    fn there_is_no_way_to_leave_an_example_out() {
        let doc = "```buri ignore\nwhatever\n```\n";
        assert!(check(doc).contains("`ignore` is not a mode"), "{}", check(doc));
        let doc = "```textproto ignore\nname: \"api\"\n```\n";
        assert!(check(doc).contains("`ignore` is not a mode"), "{}", check(doc));
        let doc = "```textproto\nname: \"api\"\n```\n";
        assert!(check(doc).contains("does not say what it is"), "{}", check(doc));
    }

    /// The fences labelled as files make one repository, and it is built: a
    /// build file naming an entry is what makes that function an entry.
    #[test]
    fn a_page_s_files_are_built_as_one_repository() {
        let build = "```textproto schema=build file=cmd/hello/BUILD.buri\n\
                     binary {\n    outputs: [\n        { platform: \"node\", entries: [{ name: \"main\", function: \"start\" }] },\n    ]\n}\n```\n";
        let main = "```buri file=cmd/hello/main.buri\n\
                    from \"node\" import { NodeHost };\n\n\
                    export fn start(host: NodeHost): Result<(), Str> {\n    .Ok(())\n}\n```\n";
        assert_eq!(check(&format!("{build}\n{main}")), "");

        let wrong = main.replace("export fn start", "export fn begin");
        let out = check(&format!("{build}\n{wrong}"));
        assert!(out.contains("this repository does not build"), "{out}");
    }

    /// A `fail` fence is written in on its own and must fail the build with
    /// its code; one that builds is caught.
    #[test]
    fn a_failing_file_must_fail_with_its_code() {
        let broken = "```textproto fail code=textproto-syntax repo=cli/tests/docs/repositories/deploy \
                      file=lib/deploy/server.txtpb\n\
                      # proto-file: server.proto\n# proto-message: Server\nports: [80 443]\n```\n";
        assert_eq!(check(broken), "");

        let fine = broken.replace("[80 443]", "[80, 443]");
        let out = check(&fine);
        assert!(out.contains("does not fail the build with `textproto-syntax`"), "{out}");
    }

    /// A fence whose info string does not parse used to be silently downgraded
    /// to "a fence in no language", which meant it was neither compiled nor
    /// reported — the worst of both. It is now a failure whatever it claims to
    /// be.
    #[test]
    fn a_malformed_info_string_is_reported_rather_than_swallowed() {
        let doc = "```buri file=\"unterminated\nwhatever\n```\n";
        assert!(check(doc).contains("unterminated"), "{}", check(doc));

        // Not only for `buri`: nothing else can be trusted to notice.
        let doc = "```textproto schema=build schema=repo\nx: 1\n```\n";
        assert!(check(doc).contains("given twice"), "{}", check(doc));

        // And a well-formed fence in a language nobody compiles stays silent.
        assert_eq!(check("```text\nfree prose\n```\n"), "");
    }

    #[test]
    fn a_run_block_must_pin_its_output() {
        let doc = "```buri run wrap=body\nlet _ = io.println(ctx, \"hi\").ignore();\n```\n";
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
                    let _ = io.println(ctx, \"basket total: $36.50\").ignore();\n\
                    ```\n\
                    ```stdout\n\
                    basket total: $36.50\n\
                    ```\n";
        assert_eq!(check(good), "");

        let wrong = "```buri run wrap=body\n\
                     let _ = io.println(ctx, \"one thing\").ignore();\n\
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
