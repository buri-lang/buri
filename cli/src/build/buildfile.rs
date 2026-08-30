//! `BUILD.buri` and `REPO.buri`, typed.
//!
//! The normative schemas are `cli/src/docs/schema/build.proto` and
//! `repo.proto`. This module is the reader for them: it walks the textproto
//! tree and produces typed values, rejecting an unknown field with a line
//! number rather than ignoring it. That is the point of the schema being a real
//! artifact — a typo in a field name is an error, not a silent no-op.

use crate::build::textproto::{self, Document, Message, Value};
use crate::diagnostics::{Diagnostic, FileId, Invariant, Span};

/// The top level of a `BUILD.buri`, and the top level of a `REPO.buri`.
///
/// The one place a known-field list is not `textproto::schema_order`: the
/// formatter cannot tell the two kinds of file apart, so it carries their
/// union, while the reader must refuse a `tag` in a build file. The test at
/// the bottom of this module holds the union to these two halves.
const BUILD_FILE_RULES: &[&str] = &["library", "binary"];
const REPO_FILE_RULES: &[&str] = &["tag", "lint"];

#[derive(Clone, Debug)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Span) -> Spanned<T> {
        Spanned { value, span }
    }
}

impl<T: Default> Default for Spanned<T> {
    fn default() -> Spanned<T> {
        Spanned { value: T::default(), span: Span::NONE }
    }
}

impl<T: PartialEq> PartialEq for Spanned<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Platform {
    Linux,
    Macos,
    Js,
    /// A page in a browser. The artifact is JavaScript, so it is built by the
    /// same backend `Js` is, but it is a different *platform* because a
    /// platform is the set of effects its host exports: `Web` grants the
    /// reactive graph, and grants no filesystem, no standard input, no
    /// environment and no process to exit.
    Web,
}

impl Platform {
    pub fn parse(s: &str) -> Option<Platform> {
        Some(match s {
            "LINUX" => Platform::Linux,
            "MACOS" => Platform::Macos,
            "JS" => Platform::Js,
            "WEB" => Platform::Web,
            _ => return None,
        })
    }

    /// The spelling used in `--output=` and in artifact paths.
    pub fn slug(self) -> &'static str {
        match self {
            Platform::Linux => "linux",
            Platform::Macos => "macos",
            Platform::Js => "js",
            Platform::Web => "web",
        }
    }

    pub fn proto(self) -> &'static str {
        match self {
            Platform::Linux => "LINUX",
            Platform::Macos => "MACOS",
            Platform::Js => "JS",
            Platform::Web => "WEB",
        }
    }

    /// Whether this platform's artifact is JavaScript.
    ///
    /// **This is the question almost every `platform` test in the toolchain is
    /// really asking**, and it used to be spelled `!= Platform::Js` back when
    /// there was one JavaScript platform and the two questions could not come
    /// apart. They can now: a `Web` artifact is emitted by the `js` backend,
    /// is written as an `.mjs`, runs no native linker and needs no runtime
    /// archive — so every site that meant "not native" must ask this rather
    /// than compare against one variant.
    pub fn is_javascript(self) -> bool {
        matches!(self, Platform::Js | Platform::Web)
    }

    /// Whether this platform is built by a native backend, linked, and run as
    /// a process. The complement of [`Platform::is_javascript`], written out
    /// so that a reader of a call site does not have to negate anything.
    pub fn is_native(self) -> bool {
        !self.is_javascript()
    }

    pub const ALL: [Platform; 4] =
        [Platform::Linux, Platform::Macos, Platform::Js, Platform::Web];

    /// Every platform's schema spelling, in declaration order.
    ///
    /// Derived from [`Platform::ALL`] rather than written out beside it, so
    /// that adding a variant cannot leave a diagnostic naming three platforms
    /// when there are four. Three diagnostics offer this list and all three
    /// read it from here.
    fn proto_names() -> Vec<&'static str> {
        Platform::ALL.iter().map(|p| p.proto()).collect()
    }

    /// `LINUX, MACOS, JS, WEB` — the list as a diagnostic writes it.
    pub fn names_phrase() -> String {
        Platform::proto_names().join(", ")
    }
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

/// **`Cjs` is read and not acted on.** `module: CJS` parses, is stored in
/// `OutputTarget::Js`, and is then consulted by nobody: every destructuring of
/// that variant outside this file is `Js { .. }`, and the backend emits an ES
/// module either way. So a build file that asks for CommonJS gets an `.mjs`
/// and is not told.
///
/// Written down rather than fixed here because the fix is a decision, not a
/// cleanup: either the backend grows a CommonJS emitter, or the value leaves
/// `build.proto` and the schema's unknown-value diagnostic refuses it. The
/// second is the smaller change and would alter a pinned expectation in
/// `repositories/build-files/web_output`, which is a wave that owns the
/// schema's business rather than this one.
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
    Native { platform: NativePlatform, arch: Option<Spanned<Arch>> },
    Js { module: JsModule },
    /// A browser page. It carries neither an `arch` nor a module kind: a
    /// machine architecture is meaningless for JavaScript, and a browser
    /// loads an ES module — there is no `<script type="commonjs">` — so the
    /// one field `Js` has would have exactly one legal value here.
    Web,
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
            Platform::Web => OutputTarget::Web,
        };
        Output { target, artifact_name: None, span }
    }

    pub fn platform(&self) -> Platform {
        match &self.target {
            OutputTarget::Native { platform, .. } => platform.platform(),
            OutputTarget::Js { .. } => Platform::Js,
            OutputTarget::Web => Platform::Web,
        }
    }

    pub fn arch(&self) -> Option<Arch> {
        match &self.target {
            OutputTarget::Native { arch, .. } => arch.as_ref().map(|a| a.value),
            OutputTarget::Js { .. } | OutputTarget::Web => None,
        }
    }

    /// `linux-x86_64`, `js`, `web` — the directory under `.buri/out/`.
    pub fn dir(&self) -> String {
        match &self.target {
            OutputTarget::Js { .. } => "js".to_string(),
            OutputTarget::Web => "web".to_string(),
            OutputTarget::Native { platform, arch: Some(a) } => {
                format!("{}-{}", platform.platform().slug(), a.value.slug())
            }
            OutputTarget::Native { platform, arch: None } => {
                platform.platform().slug().to_string()
            }
        }
    }

    /// Whether `--output=<selector>` selects this entry. Accepts `js`,
    /// `linux/x86_64`, and `linux-x86_64`.
    pub fn matches_selector(&self, selector: &str) -> bool {
        let selector = selector.replace('/', "-");
        self.dir() == selector || self.platform().slug() == selector
    }
}

#[derive(Clone, Debug, Default)]
pub struct TestSuite {
    pub sources: Vec<Spanned<String>>,
    pub dependencies: Vec<Spanned<String>>,
    pub data: Vec<Spanned<String>>,
    pub timeout_seconds: Option<u32>,
    pub platforms: Vec<Spanned<Platform>>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct TestingSurface {
    pub sources: Vec<Spanned<String>>,
    pub dependencies: Vec<Spanned<String>>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Library {
    pub sources: Vec<Spanned<String>>,
    /// The `.proto` schemas this rule owns. Each one becomes a module, and the
    /// module belongs to this rule exactly as a `.buri` source does.
    pub proto_sources: Vec<Spanned<String>>,
    pub dependencies: Vec<Spanned<String>>,
    pub tags: Vec<Spanned<String>>,
    pub platforms: Vec<Spanned<Platform>>,
    /// Parsed here rather than at every consumer: an entry that is not a
    /// visibility is a diagnostic, in the same place and the same way a bad
    /// `platforms` entry is, instead of an unparseable string that silently
    /// makes the library visible to nobody.
    pub visibility: Vec<Spanned<crate::build::workspace::Visibility>>,
    /// The rule's suite, present exactly when the build file writes a `test`
    /// block. An absent block and an empty one are different claims, and the
    /// `empty-test-suite` lint exists to tell them apart.
    pub test: Option<TestSuite>,
    pub testing: Option<TestingSurface>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Binary {
    pub sources: Vec<Spanned<String>>,
    pub proto_sources: Vec<Spanned<String>>,
    pub dependencies: Vec<Spanned<String>>,
    pub tags: Vec<Spanned<String>>,
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
    pub name: Spanned<String>,
    pub doc: String,
    pub forbids_tags: Vec<Spanned<String>>,
    pub requires_platforms: Vec<Spanned<Platform>>,
    pub span: Span,
}

/// How hard the lint catalogue is run for this repository.
///
/// Both fields false is the whole of the default, and is exactly what a
/// `REPO.buri` with no `lint` block means — so this is a value rather than an
/// option, and no site has to ask whether the block was written.
#[derive(Clone, Debug, Default)]
pub struct LintConfig {
    /// `buri build` and `buri test` run the catalogue too.
    pub check_during_build: bool,
    /// A finding fails whichever command reported it.
    pub fail_on_finding: bool,
}

#[derive(Clone, Debug, Default)]
pub struct RepoConfig {
    pub tags: Vec<Tag>,
    pub lint: LintConfig,
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
    /// A diagnostic whose wording lives on its page. What follows is
    /// `.bind(…)` for each `{placeholder}` the page names.
    fn templated(&mut self, code: &str, span: Span) -> &mut Diagnostic {
        self.errors.push(Diagnostic::templated(code, span));
        self.errors.last_mut().or_ice("the diagnostic just pushed is the last one")
    }

    /// The common shape: a field holds one kind of value and was given
    /// another.
    fn wrong_kind(&mut self, span: Span, name: &str, want: &str, found: &str) {
        self.templated("field-wrong-kind", span)
            .bind("field", name)
            .bind("expected", want)
            .bind("found", found)
            .mismatch(want.to_string(), found.to_string());
    }

    /// Rejects any field the schema does not declare, naming the nearest
    /// known field when there is one.
    fn check_known(&mut self, message: &Message, known: &[&str], what: &str) {
        for f in &message.fields {
            if !known.contains(&f.name.as_str()) {
                let near = nearest(&f.name, known);
                let d = self
                    .templated("unknown-field", f.name_span)
                    .bind("field", f.name.clone())
                    .bind("block", what)
                    .bind("known_fields", known.join(", "));
                // A near miss replaces the page's fix: the two sentences share
                // no phrase, and it is one rule with one message.
                if let Some(near) = near {
                    d.fix(format!("did you mean `{near}`?"));
                }
            }
        }
    }

    fn strings(&mut self, message: &Message, name: &str) -> Vec<Spanned<String>> {
        let mut out = Vec::new();
        for f in message.all(name) {
            match &f.value {
                Value::List(items, _) => {
                    for item in items {
                        match item {
                            Value::Str(s, sp) => out.push(Spanned::new(s.clone(), *sp)),
                            other => {
                                let kind = other.kind().to_string();
                                self.wrong_kind(other.span(), name, "strings", &kind)
                            }
                        }
                    }
                }
                Value::Str(s, sp) => out.push(Spanned::new(s.clone(), *sp)),
                other => {
                    let kind = other.kind().to_string();
                    self.wrong_kind(other.span(), name, "a list of strings", &kind)
                }
            }
        }
        out
    }

    fn string(&mut self, message: &Message, name: &str) -> Option<String> {
        let f = message.get(name)?;
        match &f.value {
            Value::Str(s, _) => Some(s.clone()),
            other => {
                let kind = other.kind().to_string();
                self.wrong_kind(other.span(), name, "a string", &kind);
                None
            }
        }
    }

    fn u32_field(&mut self, message: &Message, name: &str) -> Option<u32> {
        let f = message.get(name)?;
        match &f.value {
            Value::Int(n, sp) if *n >= 0 && *n <= u32::MAX as i64 => Some(*n as u32),
            other => {
                let kind = other.kind().to_string();
                self.wrong_kind(other.span(), name, "a non-negative number", &kind);
                None
            }
        }
    }

    /// A bool, which textproto spells as a bare `true` or `false`.
    fn bool_field(&mut self, message: &Message, name: &str) -> Option<bool> {
        let f = message.get(name)?;
        match &f.value {
            Value::Ident(s, _) if s == "true" || s == "false" => Some(s == "true"),
            other => {
                let kind = other.kind().to_string();
                self.wrong_kind(other.span(), name, "`true` or `false`", &kind);
                None
            }
        }
    }

    fn platforms(&mut self, message: &Message, name: &str) -> Vec<Spanned<Platform>> {
        let mut out = Vec::new();
        for f in message.all(name) {
            let items: Vec<&Value> = match &f.value {
                Value::List(items, _) => items.iter().collect(),
                other => vec![other],
            };
            for item in items {
                match item {
                    Value::Ident(s, sp) => match Platform::parse(s) {
                        Some(p) => out.push(Spanned::new(p, *sp)),
                        None => {
                            let near = nearest(s, &Platform::proto_names());
                            let d = self
                                .templated("unknown-bare-word", *sp)
                                .bind("value", s.clone())
                                .bind("expected", "a platform")
                                .bind("expected_plural", "platforms")
                                .bind("choices", Platform::names_phrase());
                            // `Platform` is a closed enum in the schema. Adding
                            // one is a compiler change, not a configuration
                            // change.
                            if let Some(n) = near {
                                d.fix(format!("did you mean `{n}`?"));
                            }
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
    fn visibility(
        &mut self,
        message: &Message,
    ) -> Vec<Spanned<crate::build::workspace::Visibility>> {
        let mut out = Vec::new();
        for entry in self.strings(message, "visibility") {
            match crate::build::workspace::Visibility::parse(&entry.value) {
                Ok(v) => out.push(Spanned::new(v, entry.span)),
                // The sentence is the parser's: it names which of the five
                // forms the entry came closest to.
                Err(why) => {
                    self.templated("unknown-visibility", entry.span).bind("problem", why);
                }
            }
        }
        out
    }

    fn sub_message<'a>(&mut self, message: &'a Message, name: &str) -> Option<(&'a Message, Span)> {
        let f = message.get(name)?;
        match &f.value {
            Value::Message(m, sp) => Some((m, *sp)),
            other => {
                let kind = other.kind().to_string();
                self.wrong_kind(other.span(), name, "a block", &kind);
                None
            }
        }
    }

    fn test_suite(&mut self, parent: &Message) -> Option<TestSuite> {
        let (m, span) = self.sub_message(parent, "test")?;
        self.check_known(
            m,
            textproto::schema_order("test"),
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

    fn testing_surface(&mut self, parent: &Message) -> Option<TestingSurface> {
        let (m, span) = self.sub_message(parent, "testing")?;
        self.check_known(m, textproto::schema_order("testing"), "a `testing` block");
        Some(TestingSurface {
            sources: self.strings(m, "sources"),
            dependencies: self.strings(m, "dependencies"),
            span,
        })
    }

    fn outputs(&mut self, message: &Message) -> Vec<Output> {
        let mut out = Vec::new();
        for f in message.all("outputs") {
            let items: Vec<&Value> = match &f.value {
                Value::List(items, _) => items.iter().collect(),
                other => vec![other],
            };
            for item in items {
                let Value::Message(m, span) = item else {
                    let kind = item.kind().to_string();
                    self.wrong_kind(item.span(), "outputs", "a block", &kind);
                    continue;
                };
                self.check_known(m, textproto::schema_order("outputs"), "an output");

                let platform = m.get("platform").and_then(|field| match &field.value {
                    Value::Ident(s, sp) => match Platform::parse(s) {
                        Some(p) => Some(Spanned::new(p, *sp)),
                        None => {
                            self.templated("unknown-bare-word", *sp)
                                .bind("value", s.clone())
                                .bind("expected", "a platform")
                                .bind("expected_plural", "platforms")
                                .bind("choices", Platform::names_phrase());
                            None
                        }
                    },
                    other => {
                        self.templated("not-a-bare-word", other.span())
                            .bind("field", "platform")
                            .bind("expected", "a platform")
                            .bind("choices", Platform::names_phrase());
                        None
                    }
                });
                let arch = m.get("arch").and_then(|field| match &field.value {
                    Value::Ident(s, sp) => match Arch::parse(s) {
                        Some(a) => Some(Spanned::new(a, *sp)),
                        None => {
                            self.templated("unknown-bare-word", *sp)
                                .bind("value", s.clone())
                                .bind("expected", "an architecture")
                                .bind("expected_plural", "architectures")
                                .bind("choices", "X86_64 and ARM64");
                            None
                        }
                    },
                    other => {
                        self.templated("not-a-bare-word", other.span())
                            .bind("field", "arch")
                            .bind("expected", "an architecture")
                            .bind("choices", "X86_64 or ARM64");
                        None
                    }
                });

                let artifact_name = self.string(m, "artifact_name");
                let mut js_module = JsModule::Esm;
                let mut js_block: Option<Span> = None;
                if let Some((js_message, js_span)) = self.sub_message(m, "js") {
                    js_block = Some(js_span);
                    self.check_known(js_message, textproto::schema_order("js"), "a `js` block");
                    if let Some(module_field) = js_message.get("module") {
                        match &module_field.value {
                            Value::Ident(s, sp) => match s.as_str() {
                                "ESM" | "MODULE_UNSPECIFIED" => js_module = JsModule::Esm,
                                "CJS" => js_module = JsModule::Cjs,
                                _ => {
                                    self.templated("unknown-bare-word", *sp)
                                        .bind("value", s.clone())
                                        .bind("expected", "a module kind")
                                        .bind("expected_plural", "module kinds")
                                        .bind("choices", "ESM and CJS");
                                }
                            },
                            other => {
                                self.templated("not-a-bare-word", other.span())
                                    .bind("field", "module")
                                    .bind("expected", "ESM or CJS")
                                    .bind("choices", "ESM or CJS");
                            }
                        }
                    }
                }

                // A platform is what an output *is*, so an entry without one is
                // rejected and dropped rather than carried forward for each
                // consumer to guess about.
                let Some(platform) = platform else {
                    self.templated("output-without-a-platform", *span);
                    continue;
                };
                let target = match platform.value {
                    Platform::Js => {
                        // `arch` is ignored, and must be unset, when the
                        // platform is JS.
                        if let Some(a) = &arch {
                            self.templated("output-with-an-architecture", a.span)
                                .bind("platform", "JS")
                                .bind("artifact", "JavaScript");
                        }
                        OutputTarget::Js { module: js_module }
                    }
                    Platform::Linux => {
                        OutputTarget::Native { platform: NativePlatform::Linux, arch }
                    }
                    Platform::Macos => {
                        OutputTarget::Native { platform: NativePlatform::Macos, arch }
                    }
                    Platform::Web => {
                        // The same two refusals as JS, and one more. An `arch`
                        // is meaningless for JavaScript; a `js { module }` is
                        // meaningless for a page, because a browser loads an
                        // ES module and there is no other kind of script tag
                        // to put a CommonJS artifact in. Saying so here is
                        // what keeps a field the rest of the toolchain then
                        // ignores from being writable.
                        if let Some(a) = &arch {
                            self.templated("output-with-an-architecture", a.span)
                                .bind("platform", "WEB")
                                .bind("artifact", "a page");
                        }
                        if let Some(js_span) = js_block {
                            self.templated("web-output-with-a-js-block", js_span);
                        }
                        OutputTarget::Web
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

fn edit_distance(a: &str, b: &str) -> usize {
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
    pub document: Document,
    pub errors: Vec<Diagnostic>,
}

pub fn read_build_file(text: &str, file: FileId) -> ReadResult<BuildFile> {
    let parsed = textproto::parse(text, file);
    let mut reader = Reader { errors: parsed.errors };
    let message = parsed.document.as_message();
    reader.check_known(&message, BUILD_FILE_RULES, "a build file");

    let library = reader.sub_message(&message, "library").map(|(m, span)| {
        reader.check_known(m, textproto::schema_order("library"), "a `library` rule");
        Library {
            sources: reader.strings(m, "sources"),
            proto_sources: reader.strings(m, "proto_sources"),
            dependencies: reader.strings(m, "dependencies"),
            tags: reader.strings(m, "tags"),
            platforms: reader.platforms(m, "platforms"),
            visibility: reader.visibility(m),
            test: reader.test_suite(m),
            testing: reader.testing_surface(m),
            span,
        }
    });

    let binary = reader.sub_message(&message, "binary").map(|(m, span)| {
        // A binary has no `platforms` field of its own — `outputs` already
        // says — and no `visibility`, because nothing can depend on a binary.
        reader.check_known(m, textproto::schema_order("binary"), "a `binary` rule");
        for bad in ["platforms", "visibility"] {
            if let Some(f) = m.get(bad) {
                let note = if bad == "platforms" {
                    "a binary's `outputs` already name its platforms"
                } else {
                    "nothing can depend on a binary, so there is no one to be visible to"
                };
                // The note is the site's: which of the two fields it is
                // decides the sentence, and a page holds one note.
                reader
                    .templated("binary-field-not-allowed", f.name_span)
                    .bind("field", bad)
                    .note(note);
            }
        }
        Binary {
            sources: reader.strings(m, "sources"),
            proto_sources: reader.strings(m, "proto_sources"),
            dependencies: reader.strings(m, "dependencies"),
            tags: reader.strings(m, "tags"),
            outputs: reader.outputs(m),
            test: reader.test_suite(m),
            span,
        }
    });

    ReadResult {
        value: BuildFile { library, binary },
        document: parsed.document,
        errors: reader.errors,
    }
}

pub fn read_repo_config(text: &str, file: FileId) -> ReadResult<RepoConfig> {
    let parsed = textproto::parse(text, file);
    let mut reader = Reader { errors: parsed.errors };
    let message = parsed.document.as_message();
    reader.check_known(&message, REPO_FILE_RULES, "REPO.buri");

    let mut tags: Vec<Tag> = Vec::new();
    for f in parsed.document.all("tag") {
        let Value::Message(m, span) = &f.value else {
            reader.templated("tag-not-a-block", f.value.span());
            continue;
        };
        reader.check_known(m, textproto::schema_order("tag"), "a `tag` block");

        let name_field = m.get("name");
        let name = match name_field.map(|field| &field.value) {
            Some(Value::Str(s, sp)) => Spanned::new(s.clone(), *sp),
            Some(other) => {
                reader.templated("tag-name-not-a-string", other.span());
                continue;
            }
            None => {
                reader.templated("tag-without-a-name", *span);
                continue;
            }
        };

        let mut forbids_tags = Vec::new();
        if let Some((forbids, _)) = reader.sub_message(m, "forbids") {
            // There is deliberately no `platforms` under `forbids`: a platform
            // restriction is always a whitelist under `requires`.
            reader.check_known(forbids, textproto::schema_order("forbids"), "a `forbids` block");
            if let Some(p) = forbids.get("platforms") {
                reader.templated("platforms-under-forbids", p.name_span);
            }
            forbids_tags = reader.strings(forbids, "tags");
        }

        let mut requires_platforms = Vec::new();
        if let Some((requires, _)) = reader.sub_message(m, "requires") {
            reader.check_known(requires, textproto::schema_order("requires"), "a `requires` block");
            if let Some(t) = requires.get("tags") {
                reader.templated("tags-under-requires", t.name_span);
            }
            requires_platforms = reader.platforms(requires, "platforms");
        }

        // Tags form one flat namespace, so a name declared twice is rejected
        // rather than quietly meaning whichever came first.
        if let Some(prev) = tags.iter().find(|t| t.name.value == name.value) {
            reader
                .templated("duplicate-tag", name.span)
                .bind("tag", name.value.clone())
                .secondary_span(prev.name.span, "first declared here");
            continue;
        }

        tags.push(Tag {
            name,
            doc: reader.string(m, "doc").unwrap_or_default(),
            forbids_tags,
            requires_platforms,
            span: *span,
        });
    }

    // Singular, and read like `library` and `binary` are: the first block wins,
    // and an unwritten field is the same as the field written false.
    let mut lint = LintConfig::default();
    if let Some((m, _)) = reader.sub_message(&message, "lint") {
        reader.check_known(m, textproto::schema_order("lint"), "a `lint` block");
        lint.check_during_build = reader.bool_field(m, "check_during_build").unwrap_or(false);
        lint.fail_on_finding = reader.bool_field(m, "fail_on_finding").unwrap_or(false);
    }

    ReadResult {
        value: RepoConfig { tags, lint },
        document: parsed.document,
        errors: reader.errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one known-field list this module still owns, held to the formatter's.
    ///
    /// Every other list is `textproto::schema_order` itself, so there is
    /// nothing to keep in step. The top level is two lists here and one there —
    /// the formatter cannot tell a `BUILD.buri` from a `REPO.buri` — so this
    /// is what makes the union a fact rather than a comment.
    #[test]
    fn the_two_top_level_lists_are_the_formatter_s_union() {
        let mut halves: Vec<&str> =
            BUILD_FILE_RULES.iter().chain(REPO_FILE_RULES).copied().collect();
        halves.sort_unstable();
        let mut whole: Vec<&str> = textproto::schema_order("").to_vec();
        whole.sort_unstable();
        assert_eq!(halves, whole);
    }

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
        let read = read_build_file(src, FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        let lib = read.value.library.unwrap();
        assert_eq!(lib.sources.len(), 2);
        assert_eq!(lib.visibility[0].value, crate::build::workspace::Visibility::Public);
        assert_eq!(lib.test.unwrap().sources.len(), 1);
    }

    #[test]
    fn reads_outputs() {
        let src = "binary {\n  outputs: [\n    { platform: LINUX, arch: X86_64 },\n    { platform: JS, js { module: ESM } },\n  ]\n}\n";
        let read = read_build_file(src, FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        let b = read.value.binary.unwrap();
        assert_eq!(b.outputs.len(), 2);
        assert_eq!(b.outputs[0].dir(), "linux-x86_64");
        assert_eq!(b.outputs[1].dir(), "js");
    }

    /// Every platform round-trips through its schema spelling, and the two
    /// questions a call site can ask about one partition it.
    ///
    /// The second half is the whole reason `is_javascript` exists: `!= Js` was
    /// a correct spelling of "native" only while there was one JavaScript
    /// platform, and a variant that answered neither question — or both —
    /// would make every site that asks one of them wrong in silence.
    #[test]
    fn the_platform_enum_is_total() {
        for p in Platform::ALL {
            assert_eq!(Platform::parse(p.proto()), Some(p), "`{}` does not round-trip", p.proto());
            assert!(!p.slug().is_empty());
            assert_ne!(
                p.is_javascript(),
                p.is_native(),
                "`{}` is neither a JavaScript platform nor a native one, or is both",
                p.proto()
            );
        }
        assert!(Platform::Web.is_javascript());
        assert!(!Platform::Web.is_native());
        assert_eq!(Platform::names_phrase(), "LINUX, MACOS, JS, WEB");
    }

    /// A WEB output carries neither field a JS output can, and both refusals
    /// name the reason rather than dropping the value silently.
    #[test]
    fn a_web_output_has_no_arch_and_no_module_kind() {
        let src = "binary {\n  outputs: [\n    { platform: WEB },\n  ]\n}\n";
        let read = read_build_file(src, FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        let b = read.value.binary.unwrap();
        assert_eq!(b.outputs[0].dir(), "web");
        assert_eq!(b.outputs[0].platform(), Platform::Web);
        assert_eq!(b.outputs[0].arch(), None);
        assert!(b.outputs[0].matches_selector("web"));

        let src = "binary {\n  outputs: [{ platform: WEB  arch: ARM64  js { module: CJS } }]\n}\n";
        let read = read_build_file(src, FileId(0));
        assert_eq!(read.errors.len(), 2, "{:#?}", read.errors);
        assert!(read.errors.iter().any(|e| e.message.contains("no architecture")));
        assert!(read.errors.iter().any(|e| e.message.contains("no `js` block")));
    }

    #[test]
    fn unknown_field_is_an_error_with_a_suggestion() {
        let read = read_build_file("library {\n  source: []\n}\n", FileId(0));
        assert!(read.errors[0].message.contains("unknown field `source`"));
        // The suggestion is the fix, not background: it is the edit to make.
        assert!(read.errors[0].fix.as_deref().is_some_and(|f| f.contains("sources")));
    }

    /// A typo in a `visibility` entry is reported where it is written. Before,
    /// it was silently discarded — which made the library private to everything
    /// and then printed the typo back as if it were in force.
    #[test]
    fn a_visibility_that_is_not_one_is_an_error() {
        let source = "library {\n  visibility: [\"//visibility:pubic\"]\n}\n";
        let read = read_build_file(source, FileId(0));
        let named = read.errors.iter().any(|e| e.message.contains("//visibility:pubic"));
        assert!(named, "{:#?}", read.errors);
        assert!(read.value.library.unwrap().visibility.is_empty());
    }

    #[test]
    fn a_binary_has_no_visibility() {
        let read = read_build_file("binary {\n  visibility: []\n}\n", FileId(0));
        assert!(read.errors.iter().any(|e| e.message.contains("no `visibility` field")));
    }

    #[test]
    fn js_output_rejects_arch() {
        let source = "binary {\n  outputs: [{ platform: JS, arch: ARM64 }]\n}\n";
        let read = read_build_file(source, FileId(0));
        assert!(read.errors.iter().any(|e| e.message.contains("no architecture")));
    }

    #[test]
    fn forbids_takes_no_platforms() {
        let src = "tag {\n  name: \"a\"\n  forbids { platforms: [JS] }\n}\n";
        let read = read_repo_config(src, FileId(0));
        assert!(read.errors.iter().any(|e| e.message.contains("no `platforms`")));
    }

    #[test]
    fn duplicate_tags_are_rejected() {
        let src = "tag { name: \"a\" }\ntag { name: \"a\" }\n";
        let read = read_repo_config(src, FileId(0));
        assert!(read.errors.iter().any(|e| e.message.contains("declared twice")));
    }

    /// Both fields, both spelled out, both landing where the struct says.
    #[test]
    fn a_lint_block_is_read() {
        let src = "lint {\n  check_during_build: true\n  fail_on_finding: true\n}\n";
        let read = read_repo_config(src, FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        assert!(read.value.lint.check_during_build);
        assert!(read.value.lint.fail_on_finding);
    }

    /// No block is not a missing answer: it is both fields false, which is the
    /// behaviour the toolchain had before the block existed.
    #[test]
    fn no_lint_block_is_both_fields_false() {
        let read = read_repo_config("tag { name: \"a\" }\n", FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        assert!(!read.value.lint.check_during_build);
        assert!(!read.value.lint.fail_on_finding);

        // And a block that names only one field says nothing about the other.
        let read = read_repo_config("lint { fail_on_finding: true }\n", FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        assert!(!read.value.lint.check_during_build);
        assert!(read.value.lint.fail_on_finding);
    }

    /// The block is closed like every other, and the near miss is the fix.
    #[test]
    fn an_unknown_field_in_the_lint_block_is_rejected() {
        let read = read_repo_config("lint { fail_on_findings: true }\n", FileId(0));
        let d = read.errors.first().expect("`fail_on_findings` is not a field");
        assert_eq!(d.message, "unknown field `fail_on_findings` in a `lint` block");
        assert!(d.fix.as_deref().is_some_and(|f| f.contains("fail_on_finding")), "{:#?}", d.fix);
        assert!(!read.value.lint.fail_on_finding);
    }

    /// A bool is a bare `true` or `false`, so a quoted one is the wrong kind of
    /// value rather than a truthy string.
    #[test]
    fn a_lint_field_that_is_not_a_bool_is_rejected() {
        for src in ["lint { check_during_build: \"yes\" }\n", "lint { check_during_build: 1 }\n"] {
            let read = read_repo_config(src, FileId(0));
            let named = read.errors.iter().any(|e| e.message.contains("`true` or `false`"));
            assert!(named, "{src:?}: {:#?}", read.errors);
            assert!(!read.value.lint.check_during_build);
        }
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
        let read = read_repo_config(src, FileId(0));
        let d = read
            .errors
            .first()
            .expect("a removed field is still a field REPO.buri does not have");
        assert_eq!(d.message, "unknown field `toolchain` in REPO.buri");
        assert_eq!(d.fix.as_deref(), Some("REPO.buri accepts: tag, lint"));
        assert!(
            nearest("toolchain", REPO_FILE_RULES).is_none(),
            "a field REPO.buri has was suggested for `toolchain`"
        );
        // The block's contents are not read at all: one diagnostic, on the
        // field that does not exist, rather than one per field inside it.
        assert_eq!(read.errors.len(), 1, "{:#?}", read.errors);
    }
}
