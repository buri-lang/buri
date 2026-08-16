//! `BUILD.buri` and `REPO.buri`, typed.
//!
//! The normative schemas are `cli/src/docs/schema/build.proto` and
//! `repo.proto`. This module is the reader for them: it walks the textproto
//! tree and produces typed values, rejecting an unknown field with a line
//! number rather than ignoring it. That is the point of the schema being a real
//! artifact — a typo in a field name is an error, not a silent no-op.

use crate::build::textproto::{self, Doc, Msg, Value};
use crate::diagnostics::{Diagnostic, FileId, Span};

#[derive(Clone, Debug)]
pub struct Sp<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Sp<T> {
    pub fn new(value: T, span: Span) -> Sp<T> {
        Sp { value, span }
    }
}

impl<T: Default> Default for Sp<T> {
    fn default() -> Sp<T> {
        Sp { value: T::default(), span: Span::NONE }
    }
}

impl<T: PartialEq> PartialEq for Sp<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Platform {
    Linux,
    Macos,
    Js,
}

impl Platform {
    pub fn parse(s: &str) -> Option<Platform> {
        Some(match s {
            "LINUX" => Platform::Linux,
            "MACOS" => Platform::Macos,
            "JS" => Platform::Js,
            _ => return None,
        })
    }

    /// The spelling used in `--output=` and in artifact paths.
    pub fn slug(self) -> &'static str {
        match self {
            Platform::Linux => "linux",
            Platform::Macos => "macos",
            Platform::Js => "js",
        }
    }

    pub fn proto(self) -> &'static str {
        match self {
            Platform::Linux => "LINUX",
            Platform::Macos => "MACOS",
            Platform::Js => "JS",
        }
    }

    pub const ALL: [Platform; 3] = [Platform::Linux, Platform::Macos, Platform::Js];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Arch {
    X86_64,
    Arm64,
}

impl Arch {
    pub fn parse(s: &str) -> Option<Arch> {
        Some(match s {
            "X86_64" => Arch::X86_64,
            "ARM64" => Arch::Arm64,
            _ => return None,
        })
    }

    pub fn slug(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
        }
    }

    pub fn proto(self) -> &'static str {
        match self {
            Arch::X86_64 => "X86_64",
            Arch::Arm64 => "ARM64",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum JsModule {
    #[default]
    Esm,
    Cjs,
}

#[derive(Clone, Debug, Default)]
pub struct Output {
    pub platform: Option<Sp<Platform>>,
    pub arch: Option<Sp<Arch>>,
    pub artifact_name: Option<String>,
    pub js_module: JsModule,
    pub span: Span,
}

impl Output {
    /// `linux-x86_64`, `js` — the directory under `.buri/out/`.
    pub fn dir(&self) -> String {
        let p = self.platform.as_ref().map(|p| p.value).unwrap_or(Platform::Linux);
        match (p, &self.arch) {
            (Platform::Js, _) => "js".to_string(),
            (p, Some(a)) => format!("{}-{}", p.slug(), a.value.slug()),
            (p, None) => p.slug().to_string(),
        }
    }

    /// Whether `--output=<sel>` selects this entry. Accepts `js`,
    /// `linux/x86_64`, and `linux-x86_64`.
    pub fn matches_selector(&self, sel: &str) -> bool {
        let sel = sel.replace('/', "-");
        self.dir() == sel
            || self.platform.as_ref().is_some_and(|p| p.value.slug() == sel)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TestSuite {
    pub sources: Vec<Sp<String>>,
    pub dependencies: Vec<Sp<String>>,
    pub data: Vec<Sp<String>>,
    pub timeout_seconds: Option<u32>,
    pub platforms: Vec<Sp<Platform>>,
    pub span: Span,
    pub present: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TestingSurface {
    pub sources: Vec<Sp<String>>,
    pub dependencies: Vec<Sp<String>>,
    pub span: Span,
    pub present: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Library {
    pub sources: Vec<Sp<String>>,
    /// The `.proto` schemas this rule owns. Each one becomes a module, and the
    /// module belongs to this rule exactly as a `.buri` source does.
    pub proto_sources: Vec<Sp<String>>,
    pub dependencies: Vec<Sp<String>>,
    pub tags: Vec<Sp<String>>,
    pub platforms: Vec<Sp<Platform>>,
    pub visibility: Vec<Sp<String>>,
    pub test: TestSuite,
    pub testing: TestingSurface,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Binary {
    pub sources: Vec<Sp<String>>,
    pub proto_sources: Vec<Sp<String>>,
    pub dependencies: Vec<Sp<String>>,
    pub tags: Vec<Sp<String>>,
    pub outputs: Vec<Output>,
    pub test: TestSuite,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct BuildFile {
    pub library: Option<Library>,
    pub binary: Option<Binary>,
    pub file: Option<FileId>,
}

#[derive(Clone, Debug, Default)]
pub struct Toolchain {
    pub version: String,
    pub sha256: String,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Tag {
    pub name: Sp<String>,
    pub doc: String,
    pub forbids_tags: Vec<Sp<String>>,
    pub requires_platforms: Vec<Sp<Platform>>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct RepoConfig {
    pub toolchain: Toolchain,
    pub tags: Vec<Tag>,
    pub file: Option<FileId>,
}

impl RepoConfig {
    pub fn tag(&self, name: &str) -> Option<&Tag> {
        self.tags.iter().find(|t| t.name.value == name)
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

struct Reader {
    errors: Vec<Diagnostic>,
}

impl Reader {
    /// Every build-file error carries the edit that resolves it, the same way
    /// every source diagnostic does.
    fn err(&mut self, span: Span, msg: impl Into<String>, fix: impl Into<String>) {
        self.errors.push(Diagnostic::error(span, msg).with_fix(fix));
    }

    /// The common shape: a field holds one kind of value and was given
    /// another.
    fn wrong_kind(&mut self, span: Span, name: &str, want: &str, found: &str) {
        self.errors.push(
            Diagnostic::error(span, format!("`{name}` holds {want}, found {found}"))
                .with_mismatch(want.to_string(), found.to_string())
                .with_fix(format!("write {want} for `{name}`")),
        );
    }

    /// Rejects any field the schema does not declare, naming the nearest
    /// known field when there is one.
    fn check_known(&mut self, msg: &Msg, known: &[&str], what: &str) {
        for f in &msg.fields {
            if !known.contains(&f.name.as_str()) {
                let mut d = Diagnostic::error(
                    f.name_span,
                    format!("unknown field `{}` in {what}", f.name),
                );
                if let Some(near) = nearest(&f.name, known) {
                    d = d.with_fix(format!("did you mean `{near}`?"));
                } else {
                    d = d.with_fix(format!("{what} accepts: {}", known.join(", ")));
                }
                self.errors.push(d);
            }
        }
    }

    fn strings(&mut self, msg: &Msg, name: &str) -> Vec<Sp<String>> {
        let mut out = Vec::new();
        for f in msg.all(name) {
            match &f.value {
                Value::List(items, _) => {
                    for item in items {
                        match item {
                            Value::Str(s, sp) => out.push(Sp::new(s.clone(), *sp)),
                            other => {
                                let kind = other.kind().to_string();
                                self.wrong_kind(other.span(), name, "strings", &kind)
                            }
                        }
                    }
                }
                Value::Str(s, sp) => out.push(Sp::new(s.clone(), *sp)),
                other => {
                    let kind = other.kind().to_string();
                    self.wrong_kind(other.span(), name, "a list of strings", &kind)
                }
            }
        }
        out
    }

    fn string(&mut self, msg: &Msg, name: &str) -> Option<String> {
        let f = msg.get(name)?;
        match &f.value {
            Value::Str(s, _) => Some(s.clone()),
            other => {
                let kind = other.kind().to_string();
                self.wrong_kind(other.span(), name, "a string", &kind);
                None
            }
        }
    }

    fn u32_field(&mut self, msg: &Msg, name: &str) -> Option<u32> {
        let f = msg.get(name)?;
        match &f.value {
            Value::Int(n, sp) if *n >= 0 && *n <= u32::MAX as i64 => Some(*n as u32),
            other => {
                let kind = other.kind().to_string();
                self.wrong_kind(other.span(), name, "a non-negative number", &kind);
                None
            }
        }
    }

    fn platforms(&mut self, msg: &Msg, name: &str) -> Vec<Sp<Platform>> {
        let mut out = Vec::new();
        for f in msg.all(name) {
            let items: Vec<&Value> = match &f.value {
                Value::List(items, _) => items.iter().collect(),
                other => vec![other],
            };
            for item in items {
                match item {
                    Value::Ident(s, sp) => match Platform::parse(s) {
                        Some(p) => out.push(Sp::new(p, *sp)),
                        None => {
                            let mut d =
                                Diagnostic::error(*sp, format!("`{s}` is not a platform"));
                            let known = ["LINUX", "MACOS", "JS"];
                            d = match nearest(s, &known) {
                                Some(n) => d.with_fix(format!("did you mean `{n}`?")),
                                None => d.with_fix("the platforms are LINUX, MACOS, JS"),
                            };
                            // `Platform` is a closed enum in the schema. Adding
                            // one is a compiler change, not a configuration
                            // change.
                            self.errors.push(d);
                        }
                    },
                    other => {
                        let kind = other.kind().to_string();
                        self.wrong_kind(other.span(), name, "platform names", &kind)
                    }
                }
            }
        }
        out
    }

    fn sub_msg<'a>(&mut self, msg: &'a Msg, name: &str) -> Option<(&'a Msg, Span)> {
        let f = msg.get(name)?;
        match &f.value {
            Value::Msg(m, sp) => Some((m, *sp)),
            other => {
                let kind = other.kind().to_string();
                self.wrong_kind(other.span(), name, "a block", &kind);
                None
            }
        }
    }

    fn test_suite(&mut self, parent: &Msg) -> TestSuite {
        let Some((m, span)) = self.sub_msg(parent, "test") else {
            return TestSuite::default();
        };
        self.check_known(
            m,
            &["sources", "dependencies", "data", "timeout_seconds", "platforms"],
            "a `test` block",
        );
        TestSuite {
            sources: self.strings(m, "sources"),
            dependencies: self.strings(m, "dependencies"),
            data: self.strings(m, "data"),
            timeout_seconds: self.u32_field(m, "timeout_seconds"),
            platforms: self.platforms(m, "platforms"),
            span,
            present: true,
        }
    }

    fn testing_surface(&mut self, parent: &Msg) -> TestingSurface {
        let Some((m, span)) = self.sub_msg(parent, "testing") else {
            return TestingSurface::default();
        };
        self.check_known(m, &["sources", "dependencies"], "a `testing` block");
        TestingSurface {
            sources: self.strings(m, "sources"),
            dependencies: self.strings(m, "dependencies"),
            span,
            present: true,
        }
    }

    fn outputs(&mut self, msg: &Msg) -> Vec<Output> {
        let mut out = Vec::new();
        for f in msg.all("outputs") {
            let items: Vec<&Value> = match &f.value {
                Value::List(items, _) => items.iter().collect(),
                other => vec![other],
            };
            for item in items {
                let Value::Msg(m, span) = item else {
                    let kind = item.kind().to_string();
                    self.wrong_kind(item.span(), "outputs", "a block", &kind);
                    continue;
                };
                self.check_known(m, &["platform", "arch", "artifact_name", "js"], "an output");

                let platform = m.get("platform").and_then(|pf| match &pf.value {
                    Value::Ident(s, sp) => match Platform::parse(s) {
                        Some(p) => Some(Sp::new(p, *sp)),
                        None => {
                            self.err(
                                *sp,
                                format!("`{s}` is not a platform"),
                                "the platforms are LINUX, MACOS, JS",
                            );
                            None
                        }
                    },
                    other => {
                        self.err(
                            other.span(),
                            "`platform` names a platform",
                            "write a bare word: LINUX, MACOS, or JS",
                        );
                        None
                    }
                });
                let arch = m.get("arch").and_then(|af| match &af.value {
                    Value::Ident(s, sp) => match Arch::parse(s) {
                        Some(a) => Some(Sp::new(a, *sp)),
                        None => {
                            self.err(
                                *sp,
                                format!("`{s}` is not an architecture"),
                                "the architectures are X86_64 and ARM64",
                            );
                            None
                        }
                    },
                    other => {
                        self.err(
                            other.span(),
                            "`arch` names an architecture",
                            "write a bare word: X86_64 or ARM64",
                        );
                        None
                    }
                });

                // `arch` is ignored, and must be unset, when platform is JS.
                if let (Some(p), Some(a)) = (&platform, &arch) {
                    if p.value == Platform::Js {
                        self.errors.push(
                            Diagnostic::error(a.span, "a JS output has no architecture")
                                .with_fix("remove `arch`; JavaScript is not built per machine"),
                        );
                    }
                }
                if platform.is_none() {
                    self.err(
                        *span,
                        "an output must name a platform",
                        "add `platform: LINUX`, `MACOS`, or `JS`",
                    );
                }

                let artifact_name = self.string(m, "artifact_name");
                let mut js_module = JsModule::Esm;
                if let Some((jm, _)) = self.sub_msg(m, "js") {
                    self.check_known(jm, &["module"], "a `js` block");
                    if let Some(mf) = jm.get("module") {
                        match &mf.value {
                            Value::Ident(s, sp) => match s.as_str() {
                                "ESM" | "MODULE_UNSPECIFIED" => js_module = JsModule::Esm,
                                "CJS" => js_module = JsModule::Cjs,
                                _ => self.err(
                                    *sp,
                                    format!("`{s}` is not a module kind"),
                                    "the module kinds are ESM and CJS",
                                ),
                            },
                            other => self.err(
                                other.span(),
                                "`module` names ESM or CJS",
                                "write a bare word: ESM or CJS",
                            ),
                        }
                    }
                }
                out.push(Output { platform, arch, artifact_name, js_module, span: *span });
            }
        }
        out
    }
}

/// Levenshtein-nearest known name, for "did you mean" notes.
///
/// `known` usually arrives from a hash map, so ties are broken by name rather
/// than by whichever candidate came first. A diagnostic that changes between
/// two runs of the same compiler is a diagnostic nobody can test or diff.
pub fn nearest<'a>(word: &str, known: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(usize, &'a str)> = None;
    for k in known {
        let d = edit_distance(word, k);
        // Only suggest something genuinely close.
        let limit = (word.len().max(k.len()) / 3).max(1) + 1;
        if d > limit {
            continue;
        }
        let better = match best {
            None => true,
            Some((bd, bk)) => (d, *k) < (bd, bk),
        };
        if better {
            best = Some((d, k));
        }
    }
    best.map(|(_, k)| k)
}

pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

pub struct ReadResult<T> {
    pub value: T,
    pub doc: Doc,
    pub errors: Vec<Diagnostic>,
}

pub fn read_build_file(text: &str, file: FileId) -> ReadResult<BuildFile> {
    let parsed = textproto::parse(text, file);
    let mut r = Reader { errors: parsed.errors };
    let msg = parsed.doc.as_msg();
    r.check_known(&msg, &["library", "binary"], "a build file");

    let library = r.sub_msg(&msg, "library").map(|(m, span)| {
        r.check_known(
            m,
            &[
                "sources",
                "proto_sources",
                "dependencies",
                "tags",
                "platforms",
                "visibility",
                "test",
                "testing",
            ],
            "a `library` rule",
        );
        Library {
            sources: r.strings(m, "sources"),
            proto_sources: r.strings(m, "proto_sources"),
            dependencies: r.strings(m, "dependencies"),
            tags: r.strings(m, "tags"),
            platforms: r.platforms(m, "platforms"),
            visibility: r.strings(m, "visibility"),
            test: r.test_suite(m),
            testing: r.testing_surface(m),
            span,
        }
    });

    let binary = r.sub_msg(&msg, "binary").map(|(m, span)| {
        // A binary has no `platforms` field of its own — `outputs` already
        // says — and no `visibility`, because nothing can depend on a binary.
        r.check_known(
            m,
            &["sources", "proto_sources", "dependencies", "tags", "outputs", "test"],
            "a `binary` rule",
        );
        for bad in ["platforms", "visibility"] {
            if let Some(f) = m.get(bad) {
                let note = if bad == "platforms" {
                    "a binary's `outputs` already name its platforms"
                } else {
                    "nothing can depend on a binary, so there is no one to be visible to"
                };
                r.errors.push(
                    Diagnostic::error(f.name_span, format!("a `binary` has no `{bad}` field"))
                        .with_fix(format!("remove `{bad}`"))
                        .with_note(note),
                );
            }
        }
        Binary {
            sources: r.strings(m, "sources"),
            proto_sources: r.strings(m, "proto_sources"),
            dependencies: r.strings(m, "dependencies"),
            tags: r.strings(m, "tags"),
            outputs: r.outputs(m),
            test: r.test_suite(m),
            span,
        }
    });

    ReadResult {
        value: BuildFile { library, binary, file: Some(file) },
        doc: parsed.doc,
        errors: r.errors,
    }
}

pub fn read_repo_config(text: &str, file: FileId) -> ReadResult<RepoConfig> {
    let parsed = textproto::parse(text, file);
    let mut r = Reader { errors: parsed.errors };
    let msg = parsed.doc.as_msg();
    r.check_known(&msg, &["toolchain", "tag"], "REPO.buri");

    let toolchain = match r.sub_msg(&msg, "toolchain") {
        Some((m, span)) => {
            r.check_known(m, &["version", "sha256"], "a `toolchain` block");
            Toolchain {
                version: r.string(m, "version").unwrap_or_default(),
                sha256: r.string(m, "sha256").unwrap_or_default(),
                span,
            }
        }
        None => Toolchain::default(),
    };

    let mut tags: Vec<Tag> = Vec::new();
    for f in parsed.doc.all("tag") {
        let Value::Msg(m, span) = &f.value else {
            r.err(f.value.span(), "`tag` is a block", "write `tag { name: \"...\" ... }`");
            continue;
        };
        r.check_known(m, &["name", "doc", "forbids", "requires"], "a `tag` block");

        let name_field = m.get("name");
        let name = match name_field.map(|nf| &nf.value) {
            Some(Value::Str(s, sp)) => Sp::new(s.clone(), *sp),
            Some(other) => {
                r.err(other.span(), "`name` holds a string", "quote it, as in `name: \"server\"`");
                continue;
            }
            None => {
                r.err(*span, "a `tag` block must have a `name`", "add `name: \"...\"`; a tag is identified by it");
                continue;
            }
        };

        let mut forbids_tags = Vec::new();
        if let Some((fm, _)) = r.sub_msg(m, "forbids") {
            // There is deliberately no `platforms` under `forbids`: a platform
            // restriction is always a whitelist under `requires`.
            r.check_known(fm, &["tags"], "a `forbids` block");
            if let Some(p) = fm.get("platforms") {
                r.errors.push(
                    Diagnostic::error(p.name_span, "`forbids` takes no `platforms`")
                        .with_fix("move the list under `requires { platforms: [...] }`")
                        .with_note(
                            "a platform restriction is always a whitelist under `requires`, so \
                             that adding a platform to the toolchain cannot silently widen code \
                             written before it existed",
                        ),
                );
            }
            forbids_tags = r.strings(fm, "tags");
        }

        let mut requires_platforms = Vec::new();
        if let Some((rm, _)) = r.sub_msg(m, "requires") {
            r.check_known(rm, &["platforms"], "a `requires` block");
            if let Some(t) = rm.get("tags") {
                r.errors.push(
                    Diagnostic::error(t.name_span, "`requires` takes no `tags`")
                        .with_fix("what this usually means is `forbids { tags: [...] }`")
                        .with_note(
                        "carrying no tags is the common case, so requiring a tag transitively \
                         would force it onto every library; what this usually means is \
                         `forbids { tags: [...] }`",
                    ),
                );
            }
            requires_platforms = r.platforms(rm, "platforms");
        }

        // Tags form one flat namespace, so a name declared twice is rejected
        // rather than quietly meaning whichever came first.
        if let Some(prev) = tags.iter().find(|t| t.name.value == name.value) {
            r.errors.push(
                Diagnostic::error(name.span, format!("tag `{}` is declared twice", name.value))
                    .with_fix("rename one, or delete the duplicate")
                    .with_sub(prev.name.span, "first declared here")
                    .with_note("tags are one flat namespace, so a name means one thing"),
            );
            continue;
        }

        tags.push(Tag {
            name,
            doc: r.string(m, "doc").unwrap_or_default(),
            forbids_tags,
            requires_platforms,
            span: *span,
        });
    }

    ReadResult {
        value: RepoConfig { toolchain, tags, file: Some(file) },
        doc: parsed.doc,
        errors: r.errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_library_rule() {
        let src = r#"
library {
  sources: ["cents.buri", "parse.buri"]
  visibility: ["//visibility:public"]

  test {
    sources: ["test/cents.buri"]
  }
}
"#;
        let r = read_build_file(src, FileId(0));
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        let lib = r.value.library.unwrap();
        assert_eq!(lib.sources.len(), 2);
        assert_eq!(lib.visibility[0].value, "//visibility:public");
        assert_eq!(lib.test.sources.len(), 1);
    }

    #[test]
    fn reads_outputs() {
        let src = "binary {\n  outputs: [\n    { platform: LINUX, arch: X86_64 },\n    { platform: JS, js { module: ESM } },\n  ]\n}\n";
        let r = read_build_file(src, FileId(0));
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        let b = r.value.binary.unwrap();
        assert_eq!(b.outputs.len(), 2);
        assert_eq!(b.outputs[0].dir(), "linux-x86_64");
        assert_eq!(b.outputs[1].dir(), "js");
    }

    #[test]
    fn unknown_field_is_an_error_with_a_suggestion() {
        let r = read_build_file("library {\n  source: []\n}\n", FileId(0));
        assert!(r.errors[0].message.contains("unknown field `source`"));
        // The suggestion is the fix, not background: it is the edit to make.
        assert!(r.errors[0].fix.as_deref().is_some_and(|f| f.contains("sources")));
    }

    #[test]
    fn a_binary_has_no_visibility() {
        let r = read_build_file("binary {\n  visibility: []\n}\n", FileId(0));
        assert!(r.errors.iter().any(|e| e.message.contains("no `visibility` field")));
    }

    #[test]
    fn js_output_rejects_arch() {
        let r = read_build_file("binary {\n  outputs: [{ platform: JS, arch: ARM64 }]\n}\n", FileId(0));
        assert!(r.errors.iter().any(|e| e.message.contains("no architecture")));
    }

    #[test]
    fn forbids_takes_no_platforms() {
        let src = "tag {\n  name: \"a\"\n  forbids { platforms: [JS] }\n}\n";
        let r = read_repo_config(src, FileId(0));
        assert!(r.errors.iter().any(|e| e.message.contains("no `platforms`")));
    }

    #[test]
    fn duplicate_tags_are_rejected() {
        let src = "tag { name: \"a\" }\ntag { name: \"a\" }\n";
        let r = read_repo_config(src, FileId(0));
        assert!(r.errors.iter().any(|e| e.message.contains("declared twice")));
    }
}
