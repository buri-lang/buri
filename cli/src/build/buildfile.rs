//! `BUILD.buri` and `REPO.buri`, typed.
//!
//! The normative schemas are `cli/src/docs/reference/schema/build.proto` and
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
const BUILD_FILE_RULES: &[&str] = &["library", "binary", "tool", "platform"];
const REPO_FILE_RULES: &[&str] = &["tag", "lint", "language"];

/// The fields a `test` block used to declare and no longer does.
///
/// A retired field is not an unknown one. `unknown-field` offers the nearest
/// name it does know, and to somebody who wrote what the last release
/// documented that reads as a typo they did not make; a retired field has a
/// page of its own instead, saying what replaced it. `check_known` passes the
/// names here over, and `test_suite` — which is where the block is known —
/// emits the code.
///
/// Per block rather than one list for the file, because a `data` entry in a
/// `library` rule is still an unknown field and still gets told so.
const RETIRED_TEST_FIELDS: &[&str] = &["data", "platforms"];

/// The fields a `library` rule used to declare and no longer does. Same rule as
/// [`RETIRED_TEST_FIELDS`]: the name is passed over by `check_known` and gets
/// its own page instead of a near miss.
const RETIRED_LIBRARY_FIELDS: &[&str] = &["proto_sources"];

/// The same, for a `binary` rule.
const RETIRED_BINARY_FIELDS: &[&str] = &["proto_sources"];

/// The same, for an `outputs` entry: `variant` replaced `arch`, `entries`
/// replaced `entry`, and every JavaScript output is an ES module.
const RETIRED_OUTPUT_FIELDS: &[&str] = &["arch", "js", "entry"];

/// The platform names a build file wrote before platforms were strings, and
/// the list that says the same thing now.
const RETIRED_PLATFORM_NAMES: &[(&str, &str)] = &[
    ("LINUX", "backends: [NATIVE]"),
    ("MACOS", "backends: [NATIVE]"),
    ("JS", "backends: [JS]"),
    ("WEB", "platforms: [\"web\"]"),
];

/// The tool names this toolchain used to answer to, and what each is called
/// now. A built-in tool is its language's bare name, as a built-in platform
/// is; it was `std/<language>` before that, and the proto generator was named
/// for what it did before it was named for its language.
pub const RETIRED_TOOL_NAMES: &[(&str, &str)] = &[
    ("std/codegen/proto", "proto"),
    ("std/json", "json"),
    ("std/proto", "proto"),
    ("std/textproto", "textproto"),
];

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

/// The signature a platform fixes for the function an output enters through.
///
/// The compiler checks the entry against this, so declaring an output for a
/// platform whose shape the function does not have is a type error at the
/// function rather than a failure at run time. A platform added later is a row
/// here and a row in the checker's table, and nothing else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryShape {
    /// `fn <entry>(): Result<(), Str>`. The program runs itself: `.Ok(())`
    /// exits 0, `.Err(msg)` prints `msg` and exits 1.
    Program,
    /// `fn <entry>(request: Request): Response`. The platform calls it, once
    /// per request.
    Fetch,
}

/// What gets built: a bundled platform, with `native` split by operating
/// system because a backend needs to know which one.
///
/// A build file never names one of these. It names a platform (`"native"`,
/// `"node"`, `"web"`) and, for `native`, a variant; [`Output`] turns that
/// into one of these.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Platform {
    Linux,
    Macos,
    /// `node`.
    Js,
    /// A page in a browser. The artifact is JavaScript, so it is built by the
    /// same backend `Js` is, but it is a different *platform* because a
    /// platform is the set of effects its host exports: `Web` grants the
    /// reactive graph, and grants no filesystem, no standard input, no
    /// environment and no process to exit.
    Web,
    /// A Cloudflare Worker. The artifact is JavaScript, and the platform calls
    /// it: the entry is a `fetch` the worker runtime invokes per request,
    /// rather than a `main` that runs itself. It grants what a request handler
    /// away from a machine has — a clock, randomness, an outbound request —
    /// and grants no filesystem, no standard input, no environment, no process
    /// to exit, no port to hold open and no document.
    CloudflareWorker,
}

impl Platform {
    /// The spelling used in `--output=`, in artifact paths and in messages.
    pub fn slug(self) -> &'static str {
        match self {
            Platform::Linux | Platform::Macos => "native",
            Platform::Js => "node",
            Platform::Web => "web",
            Platform::CloudflareWorker => "cloudflare-worker",
        }
    }

    /// The spelling a build file writes.
    pub fn proto(self) -> &'static str {
        match self {
            Platform::Linux | Platform::Macos => "native",
            Platform::Js => "node",
            Platform::Web => "web",
            Platform::CloudflareWorker => "CLOUDFLARE_WORKER",
        }
    }

    /// `linux`, `macos`, `node`: the operating system for a native platform,
    /// for a message about a machine.
    pub fn machine(self) -> &'static str {
        self.os().unwrap_or(self.slug())
    }

    /// The operating system half of a `native` variant.
    pub fn os(self) -> Option<&'static str> {
        match self {
            Platform::Linux => Some("linux"),
            Platform::Macos => Some("macos"),
            Platform::Js | Platform::Web | Platform::CloudflareWorker => None,
        }
    }

    /// The backend that builds this platform's artifact.
    pub fn backend(self) -> Backend {
        match self {
            Platform::Linux | Platform::Macos => Backend::Native,
            Platform::Js | Platform::Web | Platform::CloudflareWorker => Backend::Js,
        }
    }

    /// Whether this platform's artifact is JavaScript: emitted by the `js`
    /// backend, written as an `.mjs`, linked by nothing.
    pub fn is_javascript(self) -> bool {
        self.backend() == Backend::Js
    }

    /// Whether this platform is built by a native backend, linked, and run as
    /// a process.
    pub fn is_native(self) -> bool {
        self.backend() == Backend::Native
    }

    pub const ALL: [Platform; 5] = [
        Platform::Linux,
        Platform::Macos,
        Platform::Js,
        Platform::Web,
        Platform::CloudflareWorker,
    ];

    /// The signature this platform fixes for the function an output enters
    /// through.
    pub fn entry_shape(self) -> EntryShape {
        match self {
            Platform::CloudflareWorker => EntryShape::Fetch,
            Platform::Linux | Platform::Macos | Platform::Js | Platform::Web => {
                EntryShape::Program
            }
        }
    }

    /// `native, node, web`: the bundled platforms, as a diagnostic lists them.
    pub fn names_phrase() -> String {
        PlatformName::BUNDLED.iter().map(|p| p.name()).collect::<Vec<_>>().join(", ")
    }

    /// `the web platform`, `the native and node platforms`: platforms named
    /// inside a sentence. Linux and macOS are both `native`, so a name is
    /// written once.
    pub fn sentence_phrase(platforms: &[Platform]) -> String {
        let mut names: Vec<&str> = Vec::new();
        for p in platforms {
            if !names.contains(&p.proto()) {
                names.push(p.proto());
            }
        }
        match names.split_last() {
            None => String::new(),
            Some((last, [])) => format!("the {last} platform"),
            Some((last, rest)) => format!("the {} and {last} platforms", rest.join(", ")),
        }
    }
}

/// How a program is compiled. Closed: a backend is built into the CLI.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Backend {
    Native,
    Js,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Backend> {
        Some(match s {
            "NATIVE" => Backend::Native,
            "JS" => Backend::Js,
            _ => return None,
        })
    }

    pub fn proto(self) -> &'static str {
        match self {
            Backend::Native => "NATIVE",
            Backend::Js => "JS",
        }
    }

    const NAMES: &'static [&'static str] = &["NATIVE", "JS"];

    /// The platforms this backend builds.
    pub fn platforms(self) -> &'static [Platform] {
        match self {
            Backend::Native => &[Platform::Linux, Platform::Macos],
            Backend::Js => &[Platform::Js, Platform::Web, Platform::CloudflareWorker],
        }
    }
}

/// A platform as a build file names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum PlatformName {
    Native,
    Node,
    Web,
    /// The one old spelling still accepted, until Cloudflare is a platform a
    /// repository writes itself.
    CloudflareWorker,
}

impl PlatformName {
    pub const BUNDLED: [PlatformName; 3] = [PlatformName::Native, PlatformName::Node, PlatformName::Web];

    /// A bundled platform by its bare name.
    pub fn bundled(s: &str) -> Option<PlatformName> {
        PlatformName::BUNDLED.into_iter().find(|p| p.name() == s)
    }

    pub fn name(self) -> &'static str {
        match self {
            PlatformName::Native => "native",
            PlatformName::Node => "node",
            PlatformName::Web => "web",
            PlatformName::CloudflareWorker => "CLOUDFLARE_WORKER",
        }
    }

    /// What an output the CLI makes up for it builds: `native` builds the
    /// host's.
    pub fn host_platform(self) -> Platform {
        match self {
            PlatformName::Native => crate::compiler::driver::host_native_platform(),
            PlatformName::Node => Platform::Js,
            PlatformName::Web => Platform::Web,
            PlatformName::CloudflareWorker => Platform::CloudflareWorker,
        }
    }

    /// What gets built for it.
    pub fn platforms(self) -> &'static [Platform] {
        match self {
            PlatformName::Native => &[Platform::Linux, Platform::Macos],
            PlatformName::Node => &[Platform::Js],
            PlatformName::Web => &[Platform::Web],
            PlatformName::CloudflareWorker => &[Platform::CloudflareWorker],
        }
    }

    /// The platform's rule. `None` for the worker, which has no build file.
    pub fn rule(self) -> Option<&'static PlatformRule> {
        crate::build::platforms::bundled(self.name())
    }

    /// The variants an output picks between. Empty when there are none.
    pub fn variants(self) -> Vec<&'static str> {
        self.rule().map(|r| r.variants.iter().map(|v| v.value.as_str()).collect()).unwrap_or_default()
    }

    /// The names of the platform's entries.
    pub fn entries(self) -> Vec<&'static str> {
        match self.rule() {
            Some(r) => r.entries.iter().map(|e| e.name.value.as_str()).collect(),
            None => vec!["fetch"],
        }
    }
}

/// The two lists a library, a tag's `requires` and a tag's `forbids` write:
/// `backends` and `platforms`.
#[derive(Clone, Debug, Default)]
pub struct Admitted {
    pub backends: Vec<Spanned<Backend>>,
    pub platforms: Vec<Spanned<PlatformName>>,
}

impl Admitted {
    pub fn is_empty(&self) -> bool {
        self.backends.is_empty() && self.platforms.is_empty()
    }

    /// Whether every list written admits `platform`. Nothing written admits
    /// everything.
    pub fn admits(&self, platform: Platform) -> bool {
        (self.backends.is_empty() || self.backends.iter().any(|b| b.value.platforms().contains(&platform)))
            && (self.platforms.is_empty()
                || self.platforms.iter().any(|p| p.value.platforms().contains(&platform)))
    }

    /// The word written for `platform` in either list, if one names it.
    pub fn naming(&self, platform: Platform) -> Option<&'static str> {
        let backend = self.backends.iter().find(|b| b.value.platforms().contains(&platform));
        let named = self.platforms.iter().find(|p| p.value.platforms().contains(&platform));
        backend.map(|b| b.value.proto()).or(named.map(|p| p.value.name()))
    }

    /// What a written list admits, or `None` when nothing is written.
    pub fn set(&self) -> Option<std::collections::BTreeSet<Platform>> {
        (!self.is_empty()).then(|| Platform::ALL.into_iter().filter(|p| self.admits(*p)).collect())
    }

    /// `backends NATIVE`, `platforms web`: the lists as a note writes them.
    pub fn phrase(&self) -> String {
        let mut parts = Vec::new();
        if !self.backends.is_empty() {
            let names: Vec<&str> = self.backends.iter().map(|b| b.value.proto()).collect();
            parts.push(format!("backends {}", names.join(", ")));
        }
        if !self.platforms.is_empty() {
            let names: Vec<&str> = self.platforms.iter().map(|p| p.value.name()).collect();
            parts.push(format!("platforms {}", names.join(", ")));
        }
        parts.join(" and ")
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
            "x86_64" => Arch::X86_64,
            "arm64" => Arch::Arm64,
            _ => return None,
        })
    }

    pub fn slug(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
        }
    }
}

/// A platform that produces a machine artifact.
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

/// What an output is built for. Only a native build has an `arch`: it is the
/// half of the variant after the operating system.
#[derive(Clone, Debug)]
pub enum OutputTarget {
    /// `arch` is `None` for an output the CLI made up, which builds for the
    /// host's architecture.
    Native { platform: NativePlatform, arch: Option<Spanned<Arch>> },
    Js,
    Web,
    CloudflareWorker,
}

/// One entry of a binary's `outputs`.
#[derive(Clone, Debug)]
pub struct Output {
    pub target: OutputTarget,
    pub artifact_name: Option<String>,
    /// The function filling the platform's entry, where `entries` names one.
    /// `None` is the function named after the entry.
    pub entry: Option<Spanned<String>>,
    /// The repository platform this output names, `//platform/<name>`, or
    /// `None` for a bundled one.
    pub custom: Option<CustomPlatform>,
    pub span: Span,
}

/// An output of a repository's own platform, a `platform` rule under
/// `//platform/`.
///
/// The reader cannot check one: the rule is in another build file. So it keeps
/// what was written, and the workspace checks it once every build file is
/// read, filling in `point` and `backend` and turning one output into one per
/// entry of the platform ([`crate::build::workspace::Workspace::load`]).
#[derive(Clone, Debug)]
pub struct CustomPlatform {
    /// `//platform/cloudflare_worker`, where it was written.
    pub label: Spanned<String>,
    /// The `variant`, as written.
    pub variant: Option<Spanned<String>>,
    /// The `entries`, as written: the platform's entry, and the function
    /// filling it.
    pub entries: Vec<(Spanned<String>, Spanned<String>)>,
    /// The platform's entry this output builds: `fetch`.
    pub point: String,
    /// The backend that entry is built by.
    pub backend: Backend,
    /// The entry's `js` file, package-relative to the platform, where it has
    /// one.
    pub js: Option<String>,
}

impl CustomPlatform {
    /// `cloudflare_worker`: the platform's directory under `platform/`.
    pub fn name(&self) -> &str {
        self.label.value.strip_prefix("//platform/").unwrap_or(&self.label.value)
    }

    /// `platform/cloudflare_worker`: the platform's package path.
    pub fn package_path(&self) -> &str {
        self.label.value.strip_prefix("//").unwrap_or(&self.label.value)
    }
}

impl Output {
    /// The default output, `node`.
    pub fn js(span: Span) -> Output {
        Output { target: OutputTarget::Js, artifact_name: None, entry: None, custom: None, span }
    }

    /// An output for a platform chosen at run time, as `buri test` does. A
    /// native one builds the host's variant.
    pub fn for_platform(platform: Platform, span: Span) -> Output {
        let target = match platform {
            Platform::Js => OutputTarget::Js,
            Platform::Linux => {
                OutputTarget::Native { platform: NativePlatform::Linux, arch: None }
            }
            Platform::Macos => {
                OutputTarget::Native { platform: NativePlatform::Macos, arch: None }
            }
            Platform::Web => OutputTarget::Web,
            Platform::CloudflareWorker => OutputTarget::CloudflareWorker,
        };
        Output { target, artifact_name: None, entry: None, custom: None, span }
    }

    pub fn platform(&self) -> Platform {
        match &self.target {
            OutputTarget::Native { platform, .. } => platform.platform(),
            OutputTarget::Js => Platform::Js,
            OutputTarget::Web => Platform::Web,
            OutputTarget::CloudflareWorker => Platform::CloudflareWorker,
        }
    }

    /// The platform's entry this output fills: `main`, or a worker's `fetch`.
    pub fn entry_point(&self) -> &str {
        if let Some(custom) = &self.custom {
            return &custom.point;
        }
        match self.platform() {
            Platform::CloudflareWorker => "fetch",
            _ => "main",
        }
    }

    /// The function this output enters through.
    pub fn entry_name(&self) -> &str {
        self.entry.as_ref().map_or(self.entry_point(), |e| e.value.as_str())
    }

    pub fn arch(&self) -> Option<Arch> {
        match &self.target {
            OutputTarget::Native { arch, .. } => arch.as_ref().map(|a| a.value),
            OutputTarget::Js | OutputTarget::Web | OutputTarget::CloudflareWorker => None,
        }
    }

    /// `linux-arm64`: a native output's variant, the host's architecture when
    /// the output named none.
    pub fn variant(&self) -> Option<String> {
        let os = self.platform().os()?;
        let arch = self.arch().or_else(crate::build::link::host_arch)?;
        Some(format!("{os}-{}", arch.slug()))
    }

    /// `native/linux-arm64`, `node`, `web`: the directory under `.buri/out/`.
    /// A repository platform's is `platform/<name>`, with its variant below
    /// that where the output names one.
    pub fn dir(&self) -> String {
        if let Some(custom) = &self.custom {
            return match &custom.variant {
                Some(v) => format!("platform/{}/{}", custom.name(), v.value),
                None => format!("platform/{}", custom.name()),
            };
        }
        match self.variant() {
            Some(v) => format!("native/{v}"),
            None => self.platform().slug().to_string(),
        }
    }

    /// Whether `--output=<selector>` selects this output: its directory, or
    /// its platform's name for every output of that platform.
    pub fn matches_selector(&self, selector: &str) -> bool {
        if let Some(custom) = &self.custom {
            return self.dir() == selector || custom.label.value == selector;
        }
        self.dir() == selector || self.platform().slug() == selector
    }
}

#[derive(Clone, Debug, Default)]
pub struct TestSuite {
    pub sources: Vec<Spanned<String>>,
    pub dependencies: Vec<Spanned<String>>,
    pub timeout_seconds: Option<u32>,
    /// The backends to run the suite on, one run each. Empty is one run,
    /// natively.
    pub backends: Vec<Spanned<Backend>>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct TestingSurface {
    pub sources: Vec<Spanned<String>>,
    pub dependencies: Vec<Spanned<String>>,
    pub span: Span,
}

/// One `generators` entry: a program the build runs, and the files it is
/// handed.
///
/// The tool is a string rather than a resolved target because a label naming
/// nothing is a diagnostic the build graph gets to report, in the same place
/// and the same way a `dependencies` entry naming nothing is.
#[derive(Clone, Debug)]
pub struct Generator {
    /// A `//label` naming a binary in this repository, or the name of a
    /// generator the toolchain ships.
    pub tool: Spanned<String>,
    /// Package-relative paths, no globs.
    pub inputs: Vec<Spanned<String>>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Library {
    pub sources: Vec<Spanned<String>>,
    /// The generators this rule runs. Every module one hands back belongs to
    /// this rule exactly as a `.buri` source does.
    pub generators: Vec<Generator>,
    pub dependencies: Vec<Spanned<String>>,
    pub tags: Vec<Spanned<String>>,
    /// `backends` and `platforms`: where the library may be built. Nothing
    /// written is everywhere.
    pub admits: Admitted,
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
    /// Exactly the same meaning as on a library.
    pub generators: Vec<Generator>,
    pub dependencies: Vec<Spanned<String>>,
    pub tags: Vec<Spanned<String>>,
    pub outputs: Vec<Output>,
    pub test: Option<TestSuite>,
    pub span: Span,
}

/// A program the build runs on a language's files, rooted at `tool.buri`.
///
/// Each of `check`, `format` and `generate` is the span of the block declaring
/// the entry point of that name, or `None` where the rule has no such block.
/// `tool.buri` exports exactly the functions its blocks name.
#[derive(Clone, Debug, Default)]
pub struct Tool {
    pub sources: Vec<Spanned<String>>,
    pub dependencies: Vec<Spanned<String>>,
    pub test: Option<TestSuite>,
    pub check: Option<Span>,
    pub format: Option<Span>,
    pub generate: Option<Span>,
    /// The contracts `check` and `generate` declare. Empty means text.
    pub check_accepts: Vec<Accepts>,
    pub generate_accepts: Vec<Accepts>,
    pub span: Span,
}

/// One `accepts` entry: inputs in `language` reach the entry point typed, as
/// the root type `language`'s `generate` makes of `type_schema`.
#[derive(Clone, Debug)]
pub struct Accepts {
    pub language: Spanned<String>,
    /// Opaque to the build; the language's tools read it.
    pub type_schema: Spanned<String>,
    pub span: Span,
}

impl Tool {
    /// The block declaring the entry point `name`, if the rule has one.
    pub fn block(&self, name: &str) -> Option<Span> {
        match name {
            "check" => self.check,
            "format" => self.format,
            "generate" => self.generate,
            _ => None,
        }
    }

    /// The contracts the entry point `name` declares.
    pub fn accepts(&self, name: &str) -> &[Accepts] {
        match name {
            "check" => &self.check_accepts,
            "generate" => &self.generate_accepts,
            _ => &[],
        }
    }

    /// Every contract, over both entry points.
    pub fn contracts(&self) -> impl Iterator<Item = &Accepts> {
        self.check_accepts.iter().chain(self.generate_accepts.iter())
    }
}

/// A `platform` rule: the entries a platform offers, and how an output of it
/// may be built.
#[derive(Clone, Debug, Default)]
pub struct PlatformRule {
    pub sources: Vec<Spanned<String>>,
    pub dependencies: Vec<Spanned<String>>,
    pub variants: Vec<Spanned<String>>,
    pub entries: Vec<PlatformEntry>,
    pub assets: Vec<Spanned<String>>,
    pub span: Span,
}

/// One `entry` block of a platform rule.
#[derive(Clone, Debug)]
pub struct PlatformEntry {
    pub name: Spanned<String>,
    pub backend: Spanned<Backend>,
    pub js: Option<Spanned<String>>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct BuildFile {
    pub library: Option<Library>,
    pub binary: Option<Binary>,
    pub tool: Option<Tool>,
    pub platform: Option<PlatformRule>,
}

#[derive(Clone, Debug, Default)]
pub struct Tag {
    pub name: Spanned<String>,
    pub doc: String,
    pub forbids_tags: Vec<Spanned<String>>,
    pub forbids: Admitted,
    pub requires: Admitted,
    pub span: Span,
}

impl Tag {
    /// Whether code carrying this tag may be built for `platform`: admitted by
    /// `requires` (or nothing is required), and named by nothing in `forbids`.
    pub fn admits(&self, platform: Platform) -> bool {
        self.requires.admits(platform) && !self.forbids(platform)
    }

    pub fn forbids(&self, platform: Platform) -> bool {
        self.forbids.naming(platform).is_some()
    }
}

/// How hard the lint catalogue is run for this repository.
///
/// Both booleans false and every rule enabled is the whole of the default, and
/// is exactly what a `REPO.buri` with no `lint` block means — so this is a
/// value rather than an option, and no site has to ask whether the block was
/// written.
#[derive(Clone, Debug, Default)]
pub struct LintConfig {
    /// `buri build` and `buri test` run the catalogue too.
    pub check_during_build: bool,
    /// A finding fails whichever command reported it.
    pub fail_on_finding: bool,
    /// Which of the catalogue's rules this repository listens to.
    pub rules: LintRules,
}

/// What a rule the `rules` block does not name is.
///
/// An enum rather than a bool because it is read at a distance from the
/// overrides beneath it: `default: DISABLED` turns the block into an allow
/// list, and a reader scanning past `default: false` would have to work out
/// what a false *default* meant before knowing which way round the file was.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RuleDefault {
    #[default]
    Enabled,
    Disabled,
}

impl RuleDefault {
    fn parse(s: &str) -> Option<RuleDefault> {
        Some(match s {
            // The proto zero value. An unset enum field is the documented
            // default, which is the same thing `ENABLED` says out loud.
            "ENABLED" | "RULE_DEFAULT_UNSPECIFIED" => RuleDefault::Enabled,
            "DISABLED" => RuleDefault::Disabled,
            _ => return None,
        })
    }

    const NAMES: &'static [&'static str] = &["ENABLED", "DISABLED", "RULE_DEFAULT_UNSPECIFIED"];
}

/// `lint { rules { … } }`: which of the catalogue's rules this repository is
/// asking to be told about.
///
/// One rule of arithmetic, and everything about the block follows from it:
/// **`enabled(rule) = override.unwrap_or(default)`**. So an absent block is an
/// absent override over the `ENABLED` default and changes nothing, and
/// `default: DISABLED` with a handful of rules written `true` is an allow list
/// without needing a second spelling for one.
#[derive(Clone, Debug, Default)]
pub struct LintRules {
    /// What a rule the block does not name is.
    pub default: RuleDefault,
    /// The rules the block does name, keyed by **code** rather than by the
    /// field name that carried it: a code is what a finding prints and what
    /// every other part of the toolchain calls a rule, and the underscored
    /// spelling exists only because a textproto field name cannot hold a
    /// hyphen.
    pub overrides: std::collections::BTreeMap<&'static str, bool>,
}

impl LintRules {
    /// Whether this repository listens to a rule. The one rule of arithmetic,
    /// and the only way any part of the toolchain asks the question.
    pub fn enabled(&self, code: &str) -> bool {
        match self.overrides.get(code) {
            Some(written) => *written,
            None => self.default == RuleDefault::Enabled,
        }
    }

    /// Every code this repository has turned off, in catalogue order.
    ///
    /// Over the catalogue rather than over the overrides, because
    /// `default: DISABLED` turns off the rules nobody wrote down.
    pub fn disabled(&self) -> Vec<&'static str> {
        crate::documentation::lints::LINTS
            .iter()
            .map(|l| l.code)
            .filter(|code| !self.enabled(code))
            .collect()
    }

    /// Whether every rule in the catalogue runs, which is what a `REPO.buri`
    /// with no `rules` block says and what an empty one says too.
    ///
    /// Asked of the answer rather than of the fields: a block that writes
    /// `default: DISABLED` and then turns every rule back on has said nothing,
    /// and a reader of a report should not be told about a block that changed
    /// its mind.
    pub fn everything_runs(&self) -> bool {
        self.disabled().is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct RepoConfig {
    pub tags: Vec<Tag>,
    pub lint: LintConfig,
    /// The built-in languages, with the extensions `language` blocks added.
    pub languages: crate::languages::Languages,
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
    ///
    /// `retired` is passed over here and answered where the block is read,
    /// because a retired field is not an unknown one.
    fn check_known(
        &mut self,
        message: &Message,
        known: &[&str],
        retired: &[&str],
        what: &str,
    ) {
        for f in &message.fields {
            if retired.contains(&f.name.as_str()) {
                continue;
            }
            if !known.contains(&f.name.as_str()) {
                let near = nearest(&f.name, known);
                let d = self
                    .templated("unknown-field", f.name_span)
                    .bind("field", f.name.clone())
                    .bind("block", what)
                    .bind(
                        "known_fields",
                        if known.is_empty() { "nothing".to_string() } else { known.join(", ") },
                    );
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

    /// The same, keeping where it was written. An `entry` names a function
    /// the compiler then reports about, so the span has to survive the read.
    fn spanned_string(&mut self, message: &Message, name: &str) -> Option<Spanned<String>> {
        let f = message.get(name)?;
        match &f.value {
            Value::Str(s, sp) => Some(Spanned::new(s.clone(), *sp)),
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

    /// Refuses a retired platform or field, naming what replaced it.
    fn retired(&mut self, span: Span, name: &str, replacement: impl Into<String>) {
        self.templated("retired-platform-name", span)
            .bind("name", name)
            .bind("replacement", replacement);
    }

    /// `backends: [NATIVE, JS]`.
    fn backends(&mut self, message: &Message) -> Vec<Spanned<Backend>> {
        let mut out = Vec::new();
        for f in message.all("backends") {
            let items: Vec<&Value> = match &f.value {
                Value::List(items, _) => items.iter().collect(),
                other => vec![other],
            };
            for item in items {
                match item {
                    Value::Ident(s, sp) => match Backend::parse(s) {
                        Some(b) => out.push(Spanned::new(b, *sp)),
                        None => {
                            let near = nearest(s, Backend::NAMES);
                            let d = self
                                .templated("unknown-bare-word", *sp)
                                .bind("value", s.clone())
                                .bind("expected", "a backend")
                                .bind("expected_plural", "backends")
                                .bind("choices", "NATIVE and JS");
                            if let Some(n) = near {
                                d.fix(format!("did you mean `{n}`?"));
                            }
                        }
                    },
                    other => {
                        let kind = other.kind().to_string();
                        self.wrong_kind(other.span(), "backends", "backend names", &kind)
                    }
                }
            }
        }
        out
    }

    /// One platform as a build file names it: a bundled name in a string, or
    /// the worker's old bare word. `None` once refused.
    fn platform_name(&mut self, value: &Value) -> Option<Spanned<PlatformName>> {
        match value {
            Value::Str(s, sp) => match PlatformName::bundled(s) {
                Some(p) => Some(Spanned::new(p, *sp)),
                None => {
                    let names: Vec<&str> = PlatformName::BUNDLED.iter().map(|p| p.name()).collect();
                    let d = self.templated("no-such-platform", *sp).bind("platform", s.clone());
                    if let Some(n) = nearest(s, &names) {
                        d.fix(format!("did you mean `\"{n}\"`?"));
                    }
                    None
                }
            },
            Value::Ident(s, sp) if s == "CLOUDFLARE_WORKER" => {
                Some(Spanned::new(PlatformName::CloudflareWorker, *sp))
            }
            Value::Ident(s, sp) => {
                match RETIRED_PLATFORM_NAMES.iter().find(|(old, _)| old == s) {
                    Some((_, now)) => self.retired(*sp, s, format!("write `{now}`")),
                    None => {
                        self.templated("no-such-platform", *sp).bind("platform", s.clone());
                    }
                }
                None
            }
            other => {
                let kind = other.kind().to_string();
                self.wrong_kind(other.span(), "platform", "a platform name", &kind);
                None
            }
        }
    }

    /// `platforms: ["native", "web"]`.
    fn platform_names(&mut self, message: &Message) -> Vec<Spanned<PlatformName>> {
        let mut out = Vec::new();
        for f in message.all("platforms") {
            let items: Vec<&Value> = match &f.value {
                Value::List(items, _) => items.iter().collect(),
                other => vec![other],
            };
            for item in items {
                out.extend(self.platform_name(item));
            }
        }
        out
    }

    fn admitted(&mut self, message: &Message) -> Admitted {
        Admitted { backends: self.backends(message), platforms: self.platform_names(message) }
    }

    /// A tag's two lists: each names a backend or a platform once, and none is
    /// both required and forbidden.
    fn tag_platforms(&mut self, tag: &str, requires: &Admitted, forbids: &Admitted) {
        fn words(a: &Admitted) -> Vec<(&'static str, Span)> {
            a.backends
                .iter()
                .map(|b| (b.value.proto(), b.span))
                .chain(a.platforms.iter().map(|p| (p.value.name(), p.span)))
                .collect()
        }
        let (requires, forbids) = (words(requires), words(forbids));
        for (list, field) in [(&requires, "requires"), (&forbids, "forbids")] {
            for (i, (word, span)) in list.iter().enumerate() {
                if let Some((_, first)) = list.iter().take(i).find(|(w, _)| w == word) {
                    self.templated("duplicate-platform", *span)
                        .bind("platform", *word)
                        .bind("field", field)
                        .secondary_span(*first, "first listed here");
                }
            }
        }
        for (word, span) in &forbids {
            if let Some((_, r)) = requires.iter().find(|(w, _)| w == word) {
                self.templated("platform-required-and-forbidden", *span)
                    .bind("tag", tag)
                    .bind("platform", *word)
                    .secondary_span(*r, "required here");
            }
        }
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

    /// A `rules` block: the `default` every override is read against, and one
    /// bool per lint code.
    ///
    /// The field set is `documentation::lints`' own — see
    /// [`crate::documentation::lints::rule_fields`] — so a code added to the
    /// catalogue is nameable here on the same commit, and a name the catalogue
    /// does not have is the `unknown-field` any other undeclared field gets,
    /// with the nearest rule offered as the fix.
    fn lint_rules(&mut self, message: &Message) -> LintRules {
        use crate::documentation::lints;
        self.check_known(message, textproto::schema_order("rules"), &[], "a `rules` block");
        let mut rules = LintRules::default();
        if let Some(f) = message.get("default") {
            match &f.value {
                Value::Ident(s, sp) => match RuleDefault::parse(s) {
                    Some(d) => rules.default = d,
                    None => {
                        let near = nearest(s, RuleDefault::NAMES);
                        let d = self
                            .templated("unknown-bare-word", *sp)
                            .bind("value", s.clone())
                            .bind("expected", "a rule default")
                            .bind("expected_plural", "rule defaults")
                            .bind("choices", "ENABLED and DISABLED");
                        if let Some(n) = near {
                            d.fix(format!("did you mean `{n}`?"));
                        }
                    }
                },
                other => {
                    self.templated("not-a-bare-word", other.span())
                        .bind("field", "default")
                        .bind("expected", "a rule default")
                        .bind("choices", "ENABLED or DISABLED");
                }
            }
        }
        for l in lints::LINTS {
            let field = lints::rule_field(l.code);
            if let Some(written) = self.bool_field(message, &field) {
                rules.overrides.insert(l.code, written);
            }
        }
        rules
    }

    /// The `language` blocks, over the built-in languages.
    ///
    /// A block may add extensions to a built-in language and do nothing else:
    /// replacing a built-in's check would make one `.json` mean different
    /// things in different repositories. A block naming any other language
    /// declares it, once, with the `tool` rules that check, format and
    /// generate from it; whether each names a tool with that entry point is
    /// the graph's question, asked once every build file is read.
    fn languages(&mut self, document: &Document) -> crate::languages::Languages {
        use crate::languages::{Kind, Language, Tools};
        let mut languages = crate::languages::Languages::default();
        let mut declared: Vec<(String, Span)> = Vec::new();
        for f in document.all("language") {
            let Value::Message(m, span) = &f.value else {
                let kind = f.value.kind().to_string();
                self.wrong_kind(f.value.span(), "language", "a block", &kind);
                continue;
            };
            self.check_known(m, textproto::schema_order("language"), &[], "a `language` block");
            let Some(name) = self.spanned_string(m, "name") else {
                if m.get("name").is_none() {
                    self.templated("language-without-a-name", *span);
                }
                continue;
            };
            let built_in = languages.named(&name.value).is_some_and(crate::languages::Language::is_built_in);
            if built_in {
                for tool in ["check", "format", "generate"] {
                    if let Some(field) = m.get(tool) {
                        self.templated("built-in-language-tool", field.name_span)
                            .bind("field", tool)
                            .bind("language", name.value.clone());
                    }
                }
            } else {
                if let Some((_, first)) = declared.iter().find(|(n, _)| *n == name.value) {
                    self.templated("language-declared-twice", name.span)
                        .bind("language", name.value.clone())
                        .secondary_span(*first, "declared here");
                    continue;
                }
                declared.push((name.value.clone(), name.span));
                let mut tool = |field: &str| {
                    let named = self.spanned_string(m, field)?;
                    self.retired_tool_name(&named);
                    Some(named)
                };
                let tools =
                    Tools { check: tool("check"), format: tool("format"), generate: tool("generate") };
                languages.all.push(Language {
                    name: name.value.clone(),
                    extensions: Vec::new(),
                    kind: Kind::Custom(tools),
                });
            }
            for extension in self.strings(m, "extensions") {
                let e = &extension.value;
                let valid = e.len() > 1
                    && e.starts_with('.')
                    && !e.contains('/')
                    && !e.contains(char::is_whitespace);
                if !valid {
                    self.templated("language-extension-invalid", extension.span).bind("extension", e.clone());
                    continue;
                }
                let owner = languages
                    .all
                    .iter()
                    .find_map(|l| {
                        l.extensions.iter().find(|x| x.value == *e).map(|x| (l.name.clone(), x.span))
                    });
                if let Some((owner, first)) = owner {
                    let d = self
                        .templated("language-extension-taken", extension.span)
                        .bind("extension", e.clone())
                        .bind("language", owner);
                    if first != Span::NONE {
                        d.secondary_span(first, "claimed here");
                    }
                    continue;
                }
                if let Some(language) = languages.all.iter_mut().find(|l| l.name == name.value) {
                    language.extensions.push(extension);
                }
            }
        }
        languages
    }

    /// Refuses a tool name this toolchain has retired, naming what replaced it.
    ///
    /// A retired name is not an unknown one: somebody wrote what the last
    /// release documented, and the page says what it is called now.
    fn retired_tool_name(&mut self, tool: &Spanned<String>) {
        if let Some((_, now)) = RETIRED_TOOL_NAMES.iter().find(|(old, _)| *old == tool.value) {
            self.templated("retired-tool-name", tool.span)
                .bind("tool", tool.value.clone())
                .bind("replacement", *now);
        }
    }

    /// A block's `accepts`, a list of `{ language, type_schema }`. An entry
    /// missing either is refused and dropped.
    fn accepts(&mut self, message: &Message) -> Vec<Accepts> {
        let mut out = Vec::new();
        for f in message.all("accepts") {
            let items: Vec<&Value> = match &f.value {
                Value::List(items, _) => items.iter().collect(),
                other => vec![other],
            };
            for item in items {
                let Value::Message(m, span) = item else {
                    let kind = item.kind().to_string();
                    self.wrong_kind(item.span(), "accepts", "a block", &kind);
                    continue;
                };
                self.check_known(m, textproto::schema_order("accepts"), &[], "an `accepts` entry");
                let language = self.spanned_string(m, "language");
                let type_schema = self.spanned_string(m, "type_schema");
                let (Some(language), Some(type_schema)) = (language, type_schema) else {
                    for field in ["language", "type_schema"] {
                        if m.get(field).is_none() {
                            self.templated("accepts-incomplete", *span).bind("field", field);
                        }
                    }
                    continue;
                };
                out.push(Accepts { language, type_schema, span: *span });
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
        // Before `check_known`, and not one of its near misses: `data` is not a
        // field this schema never had, it is one this schema *retired*, and
        // "unknown field `data` in a `test` block" would send a reader looking
        // for a typo. The page is the whole of the answer, so the emission
        // carries no binds.
        for f in m.all("data") {
            self.templated("retired-test-data", f.name_span);
        }
        // A suite runs on a backend, so the list it used to write names one.
        for f in m.all("platforms") {
            let items: Vec<&Value> = match &f.value {
                Value::List(items, _) => items.iter().collect(),
                other => vec![other],
            };
            let mut backends: Vec<&str> = Vec::new();
            for item in items {
                let backend = match item {
                    Value::Ident(s, _) if s == "LINUX" || s == "MACOS" => "NATIVE",
                    _ => "JS",
                };
                if !backends.contains(&backend) {
                    backends.push(backend);
                }
            }
            self.retired(f.name_span, "platforms", format!("write `backends: [{}]`", backends.join(", ")));
        }
        self.check_known(m, textproto::schema_order("test"), RETIRED_TEST_FIELDS, "a `test` block");
        Some(TestSuite {
            sources: self.strings(m, "sources"),
            dependencies: self.strings(m, "dependencies"),
            timeout_seconds: self.u32_field(m, "timeout_seconds"),
            backends: self.backends(m),
            span,
        })
    }

    fn testing_surface(&mut self, parent: &Message) -> Option<TestingSurface> {
        let (m, span) = self.sub_message(parent, "testing")?;
        self.check_known(m, textproto::schema_order("testing"), &[], "a `testing` block");
        Some(TestingSurface {
            sources: self.strings(m, "sources"),
            dependencies: self.strings(m, "dependencies"),
            span,
        })
    }

    /// `generators`, parsed. A repeated message field, written either as a
    /// list of blocks or as repeated blocks — the same two spellings
    /// [`Reader::outputs`] takes, and read the same way.
    fn generators(&mut self, message: &Message) -> Vec<Generator> {
        let mut out = Vec::new();
        for f in message.all("generators") {
            let items: Vec<&Value> = match &f.value {
                Value::List(items, _) => items.iter().collect(),
                other => vec![other],
            };
            for item in items {
                let Value::Message(m, span) = item else {
                    let kind = item.kind().to_string();
                    self.wrong_kind(item.span(), "generators", "a block", &kind);
                    continue;
                };
                self.check_known(m, textproto::schema_order("generators"), &[], "a generator");
                let inputs = self.strings(m, "inputs");
                // A generator *is* its tool, so an entry without one is
                // rejected and dropped rather than carried forward for the
                // build to guess about — the same rule an `outputs` entry with
                // no platform is held to.
                let tool = match m.get("tool") {
                    Some(field) => match &field.value {
                        Value::Str(s, sp) => Spanned::new(s.clone(), *sp),
                        other => {
                            let kind = other.kind().to_string();
                            self.wrong_kind(other.span(), "tool", "a string", &kind);
                            continue;
                        }
                    },
                    None => {
                        self.templated("generator-without-a-tool", *span);
                        continue;
                    }
                };
                self.retired_tool_name(&tool);
                out.push(Generator { tool, inputs, span: *span });
            }
        }
        out
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
                if let Some(output) = self.output(m, *span) {
                    out.push(output);
                }
            }
        }
        out
    }

    /// One `outputs` entry, or `None` once refused.
    fn output(&mut self, m: &Message, span: Span) -> Option<Output> {
        self.check_known(m, textproto::schema_order("outputs"), RETIRED_OUTPUT_FIELDS, "an output");
        let artifact_name = self.string(m, "artifact_name");
        let variant = self.spanned_string(m, "variant");
        if let Some(js) = m.get("js") {
            self.retired(js.name_span, "js", "remove it; every JavaScript output is an ES module");
        }
        // A platform is what an output *is*, so an entry without one is
        // rejected and dropped rather than carried forward for each consumer
        // to guess about.
        let Some(field) = m.get("platform") else {
            self.templated("output-without-a-platform", span);
            return None;
        };
        let arch = m.get("arch");
        // A repository's own platform. Its rule is in another build file, so
        // what was written is kept and checked once every build file is read.
        if let Value::Str(label, label_span) = &field.value {
            if label.starts_with("//") {
                if let Some(a) = arch {
                    self.retired(a.name_span, "arch", "name it in `variant`, as `variant: \"linux-arm64\"`");
                }
                let entries = self.written_entries(m);
                let custom = CustomPlatform {
                    label: Spanned::new(label.clone(), *label_span),
                    variant,
                    entries,
                    point: String::new(),
                    backend: Backend::Js,
                    js: None,
                };
                return Some(Output {
                    target: OutputTarget::Js,
                    artifact_name,
                    entry: None,
                    custom: Some(custom),
                    span,
                });
            }
        }
        let platform = match &field.value {
            Value::Ident(s, sp) if matches!(s.as_str(), "LINUX" | "MACOS" | "JS" | "WEB") => {
                let replacement = match s.as_str() {
                    "JS" => "write `platform: \"node\"`".to_string(),
                    "WEB" => "write `platform: \"web\"`".to_string(),
                    _ => {
                        let os = s.to_lowercase();
                        let written = arch.and_then(|a| match &a.value {
                            Value::Ident(a, _) => Arch::parse(&a.to_lowercase()),
                            _ => None,
                        });
                        match written {
                            Some(a) => format!("write `platform: \"native\", variant: \"{os}-{}\"`", a.slug()),
                            None => format!(
                                "write `platform: \"native\", variant: \"{os}-arm64\"` or `\"{os}-x86_64\"`"
                            ),
                        }
                    }
                };
                self.retired(*sp, s, replacement);
                return None;
            }
            value => self.platform_name(value),
        };
        if let Some(a) = arch {
            self.retired(a.name_span, "arch", "name it in `variant`, as `variant: \"linux-arm64\"`");
        }
        let platform = platform?;
        let entry_names = platform.value.entries();
        if let Some(e) = m.get("entry") {
            let written = match &e.value {
                Value::Str(s, _) => s.clone(),
                _ => String::from("main"),
            };
            let point = entry_names.first().copied().unwrap_or("main");
            self.retired(e.name_span, "entry", format!("write `entries: {{ {point}: \"{written}\" }}`"));
        }

        let variants = platform.value.variants();
        let arch = match &variant {
            None if !variants.is_empty() => {
                self.templated("variant-required", span)
                    .bind("platform", platform.value.name())
                    .bind("variants", variants.join(", "))
                    .bind("example", variants.first().copied().unwrap_or_default());
                return None;
            }
            None => None,
            Some(v) if !variants.contains(&v.value.as_str()) => {
                let choices = if variants.is_empty() {
                    format!("remove it; `{}` has no variants", platform.value.name())
                } else {
                    format!("the variants are {}", variants.join(", "))
                };
                let d = self
                    .templated("no-such-platform-variant", v.span)
                    .bind("variant", v.value.clone())
                    .bind("platform", platform.value.name())
                    .bind("choices", choices);
                if let Some(n) = nearest(&v.value, &variants) {
                    d.fix(format!("did you mean `\"{n}\"`?"));
                }
                return None;
            }
            Some(v) => v.value.split_once('-').and_then(|(_, a)| Arch::parse(a)).map(|a| Spanned::new(a, v.span)),
        };

        let entry = self.entries(m, platform.value, &entry_names);
        let target = match platform.value {
            PlatformName::Node => OutputTarget::Js,
            PlatformName::Web => OutputTarget::Web,
            PlatformName::CloudflareWorker => OutputTarget::CloudflareWorker,
            PlatformName::Native => {
                let os = match variant.as_ref().map(|v| v.value.as_str()) {
                    Some(v) if v.starts_with("macos-") => NativePlatform::Macos,
                    _ => NativePlatform::Linux,
                };
                OutputTarget::Native { platform: os, arch }
            }
        };
        Some(Output { target, artifact_name, entry, custom: None, span })
    }

    /// `entries`, as written, for a platform whose entries this reader cannot
    /// see. Each value still has to look like a function name.
    fn written_entries(&mut self, m: &Message) -> Vec<(Spanned<String>, Spanned<String>)> {
        let Some((entries, _)) = self.sub_message(m, "entries") else { return Vec::new() };
        let mut out = Vec::new();
        for f in &entries.fields {
            let Value::Str(s, sp) = &f.value else {
                let kind = f.value.kind().to_string();
                self.wrong_kind(f.value.span(), &f.name, "a function name", &kind);
                continue;
            };
            let ok = !s.is_empty()
                && s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !ok {
                self.templated("entry-not-a-name", *sp).bind("entry", s.clone());
                continue;
            }
            out.push((Spanned::new(f.name.clone(), f.name_span), Spanned::new(s.clone(), *sp)));
        }
        out
    }

    /// `entries: { main: "mainForNode" }`: the function filling each of the
    /// platform's entries, where it is not the one named after it.
    fn entries(
        &mut self,
        m: &Message,
        platform: PlatformName,
        names: &[&str],
    ) -> Option<Spanned<String>> {
        let (entries, _) = self.sub_message(m, "entries")?;
        let mut found = None;
        for f in &entries.fields {
            if !names.contains(&f.name.as_str()) {
                let d = self
                    .templated("no-such-entry", f.name_span)
                    .bind("entry", f.name.clone())
                    .bind("platform", platform.name())
                    .bind("entries", names.join(", "));
                if let Some(n) = nearest(&f.name, names) {
                    d.fix(format!("did you mean `{n}`?"));
                }
                continue;
            }
            let Value::Str(s, sp) = &f.value else {
                let kind = f.value.kind().to_string();
                self.wrong_kind(f.value.span(), &f.name, "a function name", &kind);
                continue;
            };
            // An entry names an exported function, so it has to look like
            // one. The compiler reports a name that is spelled right and does
            // not exist; a name that could not be a function at all is this
            // reader's own refusal.
            let ok = !s.is_empty()
                && s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !ok {
                self.templated("entry-not-a-name", *sp).bind("entry", s.clone());
                continue;
            }
            found = Some(Spanned::new(s.clone(), *sp));
        }
        found
    }

    /// A `platform` rule.
    fn platform_rule(&mut self, m: &Message, span: Span) -> PlatformRule {
        self.check_known(m, textproto::schema_order("platform"), &[], "a `platform` rule");
        let mut entries = Vec::new();
        for f in m.all("entry") {
            let Value::Message(e, entry_span) = &f.value else {
                let kind = f.value.kind().to_string();
                self.wrong_kind(f.value.span(), "entry", "a block", &kind);
                continue;
            };
            self.check_known(e, textproto::schema_order("entry"), &[], "an `entry` block");
            let name = self.spanned_string(e, "name");
            let backend = e.get("backend").and_then(|b| match &b.value {
                Value::Ident(s, sp) => match Backend::parse(s) {
                    Some(backend) => Some(Spanned::new(backend, *sp)),
                    None => {
                        self.templated("unknown-bare-word", *sp)
                            .bind("value", s.clone())
                            .bind("expected", "a backend")
                            .bind("expected_plural", "backends")
                            .bind("choices", "NATIVE and JS");
                        None
                    }
                },
                other => {
                    self.templated("not-a-bare-word", other.span())
                        .bind("field", "backend")
                        .bind("expected", "a backend")
                        .bind("choices", "NATIVE or JS");
                    None
                }
            });
            let js = self.spanned_string(e, "js");
            if let (Some(name), Some(backend)) = (name, backend) {
                entries.push(PlatformEntry { name, backend, js, span: *entry_span });
            }
        }
        PlatformRule {
            sources: self.strings(m, "sources"),
            dependencies: self.strings(m, "dependencies"),
            variants: self.strings(m, "variants"),
            entries,
            assets: self.strings(m, "assets"),
            span,
        }
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
    reader.check_known(&message, BUILD_FILE_RULES, &[], "a build file");

    let library = reader.sub_message(&message, "library").map(|(m, span)| {
        // Before `check_known`, for the reason `test`'s `data` is: a schema is
        // a generator's input now, and `proto_sources` is a field this schema
        // retired rather than one it never had.
        for f in m.all("proto_sources") {
            reader.templated("retired-proto-sources", f.name_span);
        }
        reader.check_known(
            m,
            textproto::schema_order("library"),
            RETIRED_LIBRARY_FIELDS,
            "a `library` rule",
        );
        Library {
            sources: reader.strings(m, "sources"),
            generators: reader.generators(m),
            dependencies: reader.strings(m, "dependencies"),
            tags: reader.strings(m, "tags"),
            admits: reader.admitted(m),
            visibility: reader.visibility(m),
            test: reader.test_suite(m),
            testing: reader.testing_surface(m),
            span,
        }
    });

    let binary = reader.sub_message(&message, "binary").map(|(m, span)| {
        // A binary has no `platforms` field of its own — `outputs` already
        // says — and no `visibility`, because nothing can depend on a binary.
        for f in m.all("proto_sources") {
            reader.templated("retired-proto-sources", f.name_span);
        }
        reader.check_known(
            m,
            textproto::schema_order("binary"),
            RETIRED_BINARY_FIELDS,
            "a `binary` rule",
        );
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
            generators: reader.generators(m),
            dependencies: reader.strings(m, "dependencies"),
            tags: reader.strings(m, "tags"),
            outputs: reader.outputs(m),
            test: reader.test_suite(m),
            span,
        }
    });

    let tool = reader.sub_message(&message, "tool").map(|(m, span)| {
        reader.check_known(m, textproto::schema_order("tool"), &[], "a `tool` rule");
        let mut block = |name: &str| {
            let (b, span) = reader.sub_message(m, name)?;
            reader.check_known(b, textproto::schema_order(name), &[], &format!("a `{name}` block"));
            let accepts = reader.accepts(b);
            Some((span, accepts))
        };
        let (check, format, generate) = (block("check"), block("format"), block("generate"));
        Tool {
            sources: reader.strings(m, "sources"),
            dependencies: reader.strings(m, "dependencies"),
            test: reader.test_suite(m),
            check: check.as_ref().map(|(s, _)| *s),
            format: format.map(|(s, _)| s),
            generate: generate.as_ref().map(|(s, _)| *s),
            check_accepts: check.map(|(_, a)| a).unwrap_or_default(),
            generate_accepts: generate.map(|(_, a)| a).unwrap_or_default(),
            span,
        }
    });

    let platform = reader.sub_message(&message, "platform").map(|(m, span)| reader.platform_rule(m, span));

    ReadResult {
        value: BuildFile { library, binary, tool, platform },
        document: parsed.document,
        errors: reader.errors,
    }
}

pub fn read_repo_config(text: &str, file: FileId) -> ReadResult<RepoConfig> {
    let parsed = textproto::parse(text, file);
    let mut reader = Reader { errors: parsed.errors };
    let message = parsed.document.as_message();
    reader.check_known(&message, REPO_FILE_RULES, &[], "REPO.buri");

    let mut tags: Vec<Tag> = Vec::new();
    for f in parsed.document.all("tag") {
        let Value::Message(m, span) = &f.value else {
            reader.templated("tag-not-a-block", f.value.span());
            continue;
        };
        reader.check_known(m, textproto::schema_order("tag"), &[], "a `tag` block");

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
        let mut forbids = Admitted::default();
        if let Some((block, _)) = reader.sub_message(m, "forbids") {
            reader.check_known(block, textproto::schema_order("forbids"), &[], "a `forbids` block");
            forbids_tags = reader.strings(block, "tags");
            forbids = reader.admitted(block);
        }

        let mut requires = Admitted::default();
        if let Some((block, _)) = reader.sub_message(m, "requires") {
            reader.check_known(block, textproto::schema_order("requires"), &[], "a `requires` block");
            if let Some(t) = block.get("tags") {
                reader.templated("tags-under-requires", t.name_span);
            }
            requires = reader.admitted(block);
        }
        reader.tag_platforms(&name.value, &requires, &forbids);

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
            forbids,
            requires,
            span: *span,
        });
    }

    // Singular, and read like `library` and `binary` are: the first block wins,
    // and an unwritten field is the same as the field written false.
    let mut lint = LintConfig::default();
    if let Some((m, _)) = reader.sub_message(&message, "lint") {
        reader.check_known(m, textproto::schema_order("lint"), &[], "a `lint` block");
        lint.check_during_build = reader.bool_field(m, "check_during_build").unwrap_or(false);
        lint.fail_on_finding = reader.bool_field(m, "fail_on_finding").unwrap_or(false);
        if let Some((rules, _)) = reader.sub_message(m, "rules") {
            lint.rules = reader.lint_rules(rules);
        }
    }

    let languages = reader.languages(&parsed.document);

    ReadResult {
        value: RepoConfig { tags, lint, languages },
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

    /// The phrase `effect-not-on-platform` writes its platforms with. Linux
    /// and macOS are both `native`, so it is written once.
    #[test]
    fn platforms_are_named_inside_a_sentence() {
        assert_eq!(Platform::sentence_phrase(&[]), "");
        assert_eq!(Platform::sentence_phrase(&[Platform::Web]), "the web platform");
        assert_eq!(
            Platform::sentence_phrase(&[Platform::Linux, Platform::Macos, Platform::Js]),
            "the native and node platforms"
        );
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
        let src = "binary {\n  outputs: [\n    { platform: \"native\", variant: \"linux-x86_64\" },\n    \
                   { platform: \"node\", entries { main: \"run\" } },\n    { platform: \"web\" },\n  ]\n}\n";
        let read = read_build_file(src, FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        let b = read.value.binary.unwrap();
        assert_eq!(b.outputs.len(), 3);
        assert_eq!(b.outputs[0].dir(), "native/linux-x86_64");
        assert_eq!(b.outputs[0].platform(), Platform::Linux);
        assert!(b.outputs[0].matches_selector("native"));
        assert!(b.outputs[0].matches_selector("native/linux-x86_64"));
        assert_eq!(b.outputs[1].dir(), "node");
        assert_eq!(b.outputs[1].entry_name(), "run");
        assert_eq!(b.outputs[2].dir(), "web");
        assert_eq!(b.outputs[2].entry_name(), "main");
    }

    /// A page names its own tab from code, so a binary rule has no `web` block
    /// at all: the block and both fields it held are unknown now.
    #[test]
    fn a_binary_has_no_web_block() {
        let src = "binary {\n  web {\n    title: \"Buri Design\"\n  }\n}\n";
        let read = read_build_file(src, FileId(0));
        assert!(!read.errors.is_empty(), "a `web` block must be refused");
    }

    /// The two questions a call site can ask about a platform partition it.
    #[test]
    fn the_platform_enum_is_total() {
        for p in Platform::ALL {
            assert!(!p.slug().is_empty());
            assert_ne!(p.is_javascript(), p.is_native(), "`{}`", p.proto());
        }
        assert!(Platform::Web.is_javascript());
        assert!(Platform::CloudflareWorker.is_javascript());
        assert_eq!(Platform::names_phrase(), "native, node, web");
    }

    fn codes(src: &str) -> Vec<String> {
        read_build_file(src, FileId(0)).errors.iter().filter_map(|e| e.code.clone()).collect()
    }

    /// Every spelling a build file wrote before platforms were strings is
    /// refused, with what replaced it.
    #[test]
    fn retired_spellings_are_refused() {
        for src in [
            "binary {\n  outputs: [{ platform: LINUX }]\n}\n",
            "binary {\n  outputs: [{ platform: \"native\", variant: \"linux-arm64\", arch: ARM64 }]\n}\n",
            "binary {\n  outputs: [{ platform: \"node\", js { module: ESM } }]\n}\n",
            "binary {\n  outputs: [{ platform: \"node\", entry: \"run\" }]\n}\n",
            "library {\n  platforms: [JS]\n}\n",
            "library {\n  test { platforms: [JS] }\n}\n",
        ] {
            assert_eq!(codes(src), ["retired-platform-name"], "{src}");
        }
        let read = read_build_file("binary {\n  outputs: [{ platform: MACOS, arch: ARM64 }]\n}\n", FileId(0));
        let fix = read.errors[0].fix.clone().unwrap_or_default();
        assert!(fix.contains("variant: \"macos-arm64\""), "{fix}");
    }

    #[test]
    fn a_native_output_names_one_of_its_variants() {
        assert_eq!(codes("binary {\n  outputs: [{ platform: \"native\" }]\n}\n"), ["variant-required"]);
        assert_eq!(
            codes("binary {\n  outputs: [{ platform: \"native\", variant: \"linux-mips\" }]\n}\n"),
            ["no-such-platform-variant"]
        );
        assert_eq!(
            codes("binary {\n  outputs: [{ platform: \"node\", variant: \"linux-arm64\" }]\n}\n"),
            ["no-such-platform-variant"]
        );
        assert_eq!(codes("binary {\n  outputs: [{ platform: \"deno\" }]\n}\n"), ["no-such-platform"]);
        assert_eq!(
            codes("binary {\n  outputs: [{ platform: \"node\", entries { fetch: \"go\" } }]\n}\n"),
            ["no-such-entry"]
        );
    }

    #[test]
    fn a_worker_keeps_its_old_spelling_until_it_is_a_platform_of_its_own() {
        let read = read_build_file("binary {\n  outputs: [{ platform: CLOUDFLARE_WORKER }]\n}\n", FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        let output = &read.value.binary.unwrap().outputs[0];
        assert_eq!(output.platform(), Platform::CloudflareWorker);
        assert_eq!(output.entry_name(), "fetch");
    }

    #[test]
    fn a_platform_rule_is_read() {
        let src = "platform {\n  variants: [\"a\"]\n  entry {\n    name: \"fetch\"\n    backend: JS\n    js: \"fetch.mjs\"\n  }\n}\n";
        let read = read_build_file(src, FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        let rule = read.value.platform.unwrap();
        assert_eq!(rule.entries[0].name.value, "fetch");
        assert_eq!(rule.entries[0].backend.value, Backend::Js);
        assert_eq!(rule.entries[0].js.as_ref().map(|j| j.value.as_str()), Some("fetch.mjs"));
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
    fn forbids_takes_backends_and_platforms() {
        let src = "tag {\n  name: \"a\"\n  forbids { backends: [JS] }\n}\n";
        let read = read_repo_config(src, FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        let tag = &read.value.tags[0];
        assert!(!tag.admits(Platform::Js));
        assert!(!tag.admits(Platform::Web));
        assert!(tag.admits(Platform::Linux));

        let src = "tag {\n  name: \"a\"\n  forbids { platforms: [\"web\"] }\n}\n";
        let tag = &read_repo_config(src, FileId(0)).value.tags[0];
        assert!(tag.admits(Platform::Js));
        assert!(!tag.admits(Platform::Web));
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

    /// The block as the schema declares it, read: a default, and two rules
    /// written off by name.
    #[test]
    fn a_rules_block_is_read() {
        let src = "lint {\n  check_during_build: true\n  rules {\n    default: ENABLED\n    \
                   discarded_result: false\n    hex_digit_table: false\n  }\n}\n";
        let read = read_repo_config(src, FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        let rules = &read.value.lint.rules;
        assert_eq!(rules.default, RuleDefault::Enabled);
        assert!(!rules.enabled("discarded-result"));
        assert!(!rules.enabled("hex-digit-table"));
        // Everything else is what it was.
        assert!(rules.enabled("unused-import"));
        assert!(!rules.everything_runs());
        assert_eq!(rules.disabled(), ["discarded-result", "hex-digit-table"]);
    }

    /// `enabled(rule) = override.unwrap_or(default)`, and the one interesting
    /// consequence: a disabled default plus a rule written `true` is an allow
    /// list, spelled in the same two fields rather than in a mode of its own.
    #[test]
    fn a_disabled_default_is_an_allow_list() {
        let src = "lint {\n  rules {\n    default: DISABLED\n    unused_import: true\n  }\n}\n";
        let read = read_repo_config(src, FileId(0));
        assert!(read.errors.is_empty(), "{:#?}", read.errors);
        let rules = &read.value.lint.rules;
        assert_eq!(rules.default, RuleDefault::Disabled);
        assert!(rules.enabled("unused-import"));
        assert!(!rules.enabled("unused-variable"));
        assert_eq!(rules.disabled().len(), crate::documentation::lints::LINTS.len() - 1);
    }

    /// No block, and an empty one, are the same claim: the whole catalogue
    /// runs. So is a block that writes every rule it names `true`.
    #[test]
    fn every_rule_runs_until_one_is_written_off() {
        for src in [
            "lint { fail_on_finding: true }\n",
            "lint { rules {} }\n",
            "lint { rules { default: ENABLED } }\n",
            "lint { rules { unused_import: true } }\n",
        ] {
            let read = read_repo_config(src, FileId(0));
            assert!(read.errors.is_empty(), "{src:?}: {:#?}", read.errors);
            assert!(read.value.lint.rules.everything_runs(), "{src:?}");
            assert!(read.value.lint.rules.disabled().is_empty(), "{src:?}");
            for l in crate::documentation::lints::LINTS {
                assert!(read.value.lint.rules.enabled(l.code), "{src:?}: {}", l.code);
            }
        }
    }

    /// A misspelled rule is the same closed-field refusal every other block
    /// gets, with the rule it is one letter from as the fix. This is the whole
    /// reason the field set is generated from the catalogue: a typo cannot
    /// read as a rule quietly left on.
    #[test]
    fn a_misspelled_rule_is_an_unknown_field() {
        let read = read_repo_config("lint { rules { unused_improt: false } }\n", FileId(0));
        let d = read.errors.first().expect("`unused_improt` is not a rule");
        assert_eq!(d.message, "unknown field `unused_improt` in a `rules` block");
        assert!(d.fix.as_deref().is_some_and(|f| f.contains("unused_import")), "{:#?}", d.fix);
        assert!(read.value.lint.rules.everything_runs());

        // A code spelled the way a finding prints it is not a field name
        // either: a textproto field cannot hold a hyphen, and the near miss
        // says which spelling this file wants.
        let read = read_repo_config("lint { rules { unused-import: false } }\n", FileId(0));
        assert!(!read.errors.is_empty());
    }

    /// The default is one of two words, and a third is refused where it is
    /// written rather than read as either of them.
    #[test]
    fn a_rule_default_that_is_not_one_is_rejected() {
        let read = read_repo_config("lint { rules { default: ENABLE } }\n", FileId(0));
        let d = read.errors.first().expect("`ENABLE` is not a rule default");
        assert!(d.message.contains("`ENABLE` is not a rule default"), "{}", d.message);
        assert!(d.fix.as_deref().is_some_and(|f| f.contains("ENABLED")), "{:#?}", d.fix);
        // And the file is read as if it had not been written, rather than as
        // an allow list nobody asked for.
        assert!(read.value.lint.rules.everything_runs());

        let read = read_repo_config("lint { rules { default: false } }\n", FileId(0));
        assert!(!read.errors.is_empty(), "a bool is not a rule default");
        assert!(read.value.lint.rules.everything_runs());
    }

    /// A rule takes `true` or `false` and nothing else, the same way the two
    /// booleans above it do.
    #[test]
    fn a_rule_that_is_not_a_bool_is_rejected() {
        let read = read_repo_config("lint { rules { dead_code: \"no\" } }\n", FileId(0));
        let named = read.errors.iter().any(|e| e.message.contains("`true` or `false`"));
        assert!(named, "{:#?}", read.errors);
        assert!(read.value.lint.rules.enabled("dead-code"));
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
        assert_eq!(d.fix.as_deref(), Some("REPO.buri accepts: tag, lint, language"));
        assert!(
            nearest("toolchain", REPO_FILE_RULES).is_none(),
            "a field REPO.buri has was suggested for `toolchain`"
        );
        // The block's contents are not read at all: one diagnostic, on the
        // field that does not exist, rather than one per field inside it.
        assert_eq!(read.errors.len(), 1, "{:#?}", read.errors);
    }
}
