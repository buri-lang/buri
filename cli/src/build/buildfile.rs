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

/// A platform that produces a machine artifact.
///
/// `Platform::Js` is deliberately not one of these. JavaScript is not built per
/// machine, so an `arch` alongside it is meaningless — and the way to say that
/// is for the two to live in different variants of [`OutputTarget`] rather than
/// for a diagnostic to be the only thing standing between the reader and a
/// value whose `arch` half the rest of the toolchain then disagrees about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NativePlatform {
    Linux,
    Macos,
}

impl NativePlatform {
    pub fn platform(self) -> Platform {
        match self {
            NativePlatform::Linux => Platform::Linux,
            NativePlatform::Macos => Platform::Macos,
        }
    }
}

/// What an output is built for. The two variants carry exactly the fields their
/// side has: an `arch` only exists for a native build, and a module kind only
/// exists for a JavaScript one.
#[derive(Clone, Debug)]
pub enum OutputTarget {
    Native { platform: NativePlatform, arch: Option<Sp<Arch>> },
    Js { module: JsModule },
}

/// One entry of a binary's `outputs`.
///
/// A platform is required — the reader rejects an entry without one rather than
/// producing a value three modules would then each invent a different default
/// for.
#[derive(Clone, Debug)]
pub struct Output {
    pub target: OutputTarget,
    pub artifact_name: Option<String>,
    pub span: Span,
}

impl Output {
    /// The default output: this toolchain emits JavaScript.
    pub fn js(span: Span) -> Output {
        Output { target: OutputTarget::Js { module: JsModule::Esm }, artifact_name: None, span }
    }

    /// An output for a platform chosen at run time, as `buri test` does.
    pub fn for_platform(platform: Platform, span: Span) -> Output {
        let target = match platform {
            Platform::Js => OutputTarget::Js { module: JsModule::Esm },
            Platform::Linux => {
                OutputTarget::Native { platform: NativePlatform::Linux, arch: None }
            }
            Platform::Macos => {
                OutputTarget::Native { platform: NativePlatform::Macos, arch: None }
            }
        };
        Output { target, artifact_name: None, span }
    }

    pub fn platform(&self) -> Platform {
        match &self.target {
            OutputTarget::Native { platform, .. } => platform.platform(),
            OutputTarget::Js { .. } => Platform::Js,
        }
    }

    pub fn arch(&self) -> Option<Arch> {
        match &self.target {
            OutputTarget::Native { arch, .. } => arch.as_ref().map(|a| a.value),
            OutputTarget::Js { .. } => None,
        }
    }

    /// `linux-x86_64`, `js` — the directory under `.buri/out/`.
    pub fn dir(&self) -> String {
        match &self.target {
            OutputTarget::Js { .. } => "js".to_string(),
            OutputTarget::Native { platform, arch: Some(a) } => {
                format!("{}-{}", platform.platform().slug(), a.value.slug())
            }
            OutputTarget::Native { platform, arch: None } => {
                platform.platform().slug().to_string()
            }
        }
    }

    /// Whether `--output=<sel>` selects this entry. Accepts `js`,
    /// `linux/x86_64`, and `linux-x86_64`.
    pub fn matches_selector(&self, sel: &str) -> bool {
        let sel = sel.replace('/', "-");
        self.dir() == sel || self.platform().slug() == sel
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
}

#[derive(Clone, Debug, Default)]
pub struct TestingSurface {
    pub sources: Vec<Sp<String>>,
    pub dependencies: Vec<Sp<String>>,
    pub span: Span,
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
    /// Parsed here rather than at every consumer: an entry that is not a
    /// visibility is a diagnostic, in the same place and the same way a bad
    /// `platforms` entry is, instead of an unparseable string that silently
    /// makes the library visible to nobody.
    pub visibility: Vec<Sp<crate::build::workspace::Visibility>>,
    /// The rule's suite, present exactly when the build file writes a `test`
    /// block. An absent block and an empty one are different claims, and the
    /// `empty-test-suite` lint exists to tell them apart.
    pub test: Option<TestSuite>,
    pub testing: Option<TestingSurface>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Binary {
    pub sources: Vec<Sp<String>>,
    pub proto_sources: Vec<Sp<String>>,
    pub dependencies: Vec<Sp<String>>,
    pub tags: Vec<Sp<String>>,
    pub outputs: Vec<Output>,
    pub test: Option<TestSuite>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct BuildFile {
    pub library: Option<Library>,
    pub binary: Option<Binary>,
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
    pub tags: Vec<Tag>,
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

    /// `visibility`, parsed. The shape mirrors `platforms`: a bad entry is
    /// reported where it is written and dropped, rather than carried forward as
    /// a string.
    fn visibility(&mut self, msg: &Msg) -> Vec<Sp<crate::build::workspace::Visibility>> {
        let mut out = Vec::new();
        for entry in self.strings(msg, "visibility") {
            match crate::build::workspace::Visibility::parse(&entry.value) {
                Ok(v) => out.push(Sp::new(v, entry.span)),
                Err(why) => self.err(
                    entry.span,
                    why,
                    "the forms are `//visibility:public`, `//visibility:private`, `//pkg`, \
                     `//pkg/...` and `//...`",
                ),
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

    fn test_suite(&mut self, parent: &Msg) -> Option<TestSuite> {
        let (m, span) = self.sub_msg(parent, "test")?;
        self.check_known(
            m,
            &["sources", "dependencies", "data", "timeout_seconds", "platforms"],
            "a `test` block",
        );
        Some(TestSuite {
            sources: self.strings(m, "sources"),
            dependencies: self.strings(m, "dependencies"),
            data: self.strings(m, "data"),
            timeout_seconds: self.u32_field(m, "timeout_seconds"),
            platforms: self.platforms(m, "platforms"),
            span,
        })
    }

    fn testing_surface(&mut self, parent: &Msg) -> Option<TestingSurface> {
        let (m, span) = self.sub_msg(parent, "testing")?;
        self.check_known(m, &["sources", "dependencies"], "a `testing` block");
        Some(TestingSurface {
            sources: self.strings(m, "sources"),
            dependencies: self.strings(m, "dependencies"),
            span,
        })
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

                // A platform is what an output *is*, so an entry without one is
                // rejected and dropped rather than carried forward for each
                // consumer to guess about.
                let Some(platform) = platform else {
                    self.err(
                        *span,
                        "an output must name a platform",
                        "add `platform: LINUX`, `MACOS`, or `JS`",
                    );
                    continue;
                };
                let target = match platform.value {
                    Platform::Js => {
                        // `arch` is ignored, and must be unset, when the
                        // platform is JS.
                        if let Some(a) = &arch {
                            self.errors.push(
                                Diagnostic::error(a.span, "a JS output has no architecture")
                                    .with_fix("remove `arch`; JavaScript is not built per machine"),
                            );
                        }
                        OutputTarget::Js { module: js_module }
                    }
                    Platform::Linux => {
                        OutputTarget::Native { platform: NativePlatform::Linux, arch }
                    }
                    Platform::Macos => {
                        OutputTarget::Native { platform: NativePlatform::Macos, arch }
                    }
                };
                out.push(Output { target, artifact_name, span: *span });
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
        let limit = (word.len().max(k.len()) / 3).max(1).saturating_add(1);
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
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur: Vec<usize> = Vec::with_capacity(prev.len());
    for (i, ca) in a.chars().enumerate() {
        cur.clear();
        // `left` is the cell to the left in the row being built, and
        // `windows(2)` hands out the two cells above it — the whole
        // recurrence, with no index to get off by one.
        let mut left = i.saturating_add(1);
        cur.push(left);
        for (cb, above) in b.iter().zip(prev.windows(2)) {
            let [diagonal, up] = above else { break };
            let cost = usize::from(ca != *cb);
            left = up
                .saturating_add(1)
                .min(left.saturating_add(1))
                .min(diagonal.saturating_add(cost));
            cur.push(left);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev.last().copied().unwrap_or(0)
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
            visibility: r.visibility(m),
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
        value: BuildFile { library, binary },
        doc: parsed.doc,
        errors: r.errors,
    }
}

pub fn read_repo_config(text: &str, file: FileId) -> ReadResult<RepoConfig> {
    let parsed = textproto::parse(text, file);
    let mut r = Reader { errors: parsed.errors };
    let msg = parsed.doc.as_msg();
    r.check_known(&msg, &["tag"], "REPO.buri");

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
        value: RepoConfig { tags },
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
        assert_eq!(lib.visibility[0].value, crate::build::workspace::Visibility::Public);
        assert_eq!(lib.test.unwrap().sources.len(), 1);
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

    /// A typo in a `visibility` entry is reported where it is written. Before,
    /// it was silently discarded — which made the library private to everything
    /// and then printed the typo back as if it were in force.
    #[test]
    fn a_visibility_that_is_not_one_is_an_error() {
        let r = read_build_file("library {\n  visibility: [\"//visibility:pubic\"]\n}\n", FileId(0));
        assert!(r.errors.iter().any(|e| e.message.contains("//visibility:pubic")), "{:#?}", r.errors);
        assert!(r.value.library.unwrap().visibility.is_empty());
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

    /// The toolchain pin was removed, so a `REPO.buri` still carrying one is a
    /// `REPO.buri` naming a field that does not exist — the same diagnostic any
    /// other unknown field gets, with no special case remembering the pin.
    ///
    /// The fix names what the file *does* accept rather than guessing: `tag` is
    /// nowhere near `toolchain`, and a suggestion that far away would read as a
    /// rename that never happened.
    #[test]
    fn a_leftover_toolchain_block_is_an_unknown_field() {
        let src = "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n";
        let r = read_repo_config(src, FileId(0));
        let d = r.errors.first().expect("a removed field is still a field REPO.buri does not have");
        assert_eq!(d.message, "unknown field `toolchain` in REPO.buri");
        assert_eq!(d.fix.as_deref(), Some("REPO.buri accepts: tag"));
        assert!(nearest("toolchain", &["tag"]).is_none(), "`tag` was suggested for `toolchain`");
        // The block's contents are not read at all: one diagnostic, on the
        // field that does not exist, rather than one per field inside it.
        assert_eq!(r.errors.len(), 1, "{:#?}", r.errors);
    }
}
