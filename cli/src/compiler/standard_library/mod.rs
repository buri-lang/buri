//! The embedded standard library.
//!
//! `core/*` ships with the toolchain and is never listed in a `dependencies`.
//! It is available to every target, and the purity tiers in SPEC 11.1 govern
//! what any given import of it can do.
//!
//! There is no directory layout on disk here — the library is the one table of
//! `include_str!`s below — so `platform/effect` is a name rather than a place. It
//! is spelled without a file because an import that crosses a module boundary
//! names the module, and every import of the standard library crosses one:
//! nothing in a repository is ever *inside* `platform/effect`. [`find`] still
//! answers to `platform/effect/lib.buri`, which names the same module the long way
//! round, and canonicalises it — see there for why one spelling has to win.
//!
//! Modules here may declare a `fn` with no body. Those are the operations the
//! backend supplies — string and array primitives, the platform's effect
//! implementations, the test runner's — and every one of them must have an
//! entry in the backend's runtime or code generation fails loudly.
//!
//! # Two rules for anything added to `core/*`
//!
//! Neither is checked by a compiler pass, and both have been paid for once.
//!
//! 1. **Every body-less declaration needs a conformance test that calls it.**
//!    `cli/tests/language/standard_library.rs` stops after type checking, so a
//!    declaration with no runtime function behind it passes that suite silently
//!    and fails only when a real program reaches it. The suites under
//!    `cli/tests/conformance/lib/` are what actually run the code.
//! 2. **State the cost.** Every structure here is persistent, and persistent
//!    structures have costs that mutable ones do not. Saying `set` is O(n/32) is
//!    better than implying the O(1) a mutable bit set would have. The cost goes
//!    on the user-facing page — `cli/src/docs/reference/standard-library.md`, in
//!    the table — and in the module's own `//!` header, not in a `//` comment
//!    that nothing renders.
//!
//! The module's own documentation is the reference for it: `buri docs core/list`
//! and the website's `reference/std/core/list` are both rendered from the `//!`
//! and `///` comments in `sources/`, so there is no second copy of the API to
//! keep in step and a signature on a page is a signature that exists.
//!
//! Everything the rest of the compiler asks about a standard library module is
//! answered from the one table below. It used to be five hand-maintained lists
//! of the same strings — a `source()` match, a `MODULES` array, an
//! `EAGER_MODULES` array, a `PRELUDE_MODULES` array and a `PRELUDE` array of
//! pairs — so a path in one and not another was representable: a module in
//! `MODULES` with no `source()` arm made the diagnostics suggest a near miss
//! that then failed to load, and a name in `PRELUDE` whose module was not
//! loaded eagerly was silently not in scope.

pub mod renamed;

use crate::compiler::semantics::types::Prim;

/// One module of the embedded standard library.
pub struct StdModule {
    pub path: &'static str,
    /// The source text, embedded at build time. Present by construction, so
    /// there is no module the toolchain names and cannot load.
    pub source: &'static str,
    /// Loaded whether or not anything imports it. See `EAGER` below.
    pub eager: bool,
    /// Only platform modules may declare effects, so the set of things a Buri
    /// program can do to the world is fixed by its platform rather than
    /// open-ended (SPEC 10.1).
    pub platform: bool,
    /// Names this module puts into every module's scope without an import.
    /// `Option`, `Result` and `Order` are the prelude of SPEC 5.7; the operator
    /// and comparison traits are here because `derive Equal for Point;` appears in
    /// programs that import nothing from `core/order`, and because `a + b`
    /// means `Add.add` whether or not anyone wrote the name down.
    ///
    /// A module with a non-empty prelude must be eager — its names are in
    /// scope everywhere, so it is part of every compilation. `MODULES_AGREE`
    /// below checks that rather than leaving it to review.
    pub prelude: &'static [&'static str],
}

const fn m(path: &'static str, source: &'static str) -> StdModule {
    StdModule { path, source, eager: false, platform: false, prelude: &[] }
}

/// Every module the standard library provides. The single source of truth for
/// what exists, what its text is, when it loads, and what it publishes into
/// every scope.
pub const MODULES: &[StdModule] = &[
    StdModule {
        prelude: &["Option"],
        eager: true,
        ..m("core/option", include_str!("sources/option.buri"))
    },
    StdModule {
        prelude: &["Result"],
        eager: true,
        ..m("core/result", include_str!("sources/result.buri"))
    },
    StdModule {
        prelude: &["Order", "Equal", "Ordered", "Show", "Hash"],
        eager: true,
        ..m("core/order", include_str!("sources/order.buri"))
    },
    StdModule {
        prelude: &[
            "Add",
            "Subtract",
            "Multiply",
            "Divide",
            "Remainder",
            "Negate",
            "Bounded",
            "Checked",
            "Wrapping",
            "Saturating",
            "RangeError",
        ],
        eager: true,
        ..m("core/number", include_str!("sources/number.buri"))
    },
    // `[T]`, `Str`, `Char` and `Bool` need their defining modules present in a
    // program that never names them, because a method needs no import
    // (SPEC 6.7.3): `xs.map(...)` resolves in `core/list` and `s.trim()` in
    // `core/str`. Everything below declares methods only on its *own* types,
    // which cannot exist in a program that did not import it — so it loads on
    // import, and a repository does not pay to parse `core/crypto` to compile
    // a program that has never heard of it.
    StdModule { eager: true, ..m("core/list", include_str!("sources/list.buri")) },
    StdModule { eager: true, ..m("core/str", include_str!("sources/str.buri")) },
    StdModule { eager: true, ..m("core/character", include_str!("sources/character.buri")) },
    StdModule { eager: true, ..m("core/bool", include_str!("sources/bool.buri")) },
    m("core/queue", include_str!("sources/queue.buri")),
    m("core/heap", include_str!("sources/heap.buri")),
    m("core/bitset", include_str!("sources/bitset.buri")),
    m("core/json", include_str!("sources/json.buri")),
    m("core/csv", include_str!("sources/csv.buri")),
    m("core/proto", include_str!("sources/proto.buri")),
    // The grammar as data, and the printer that turns it back into source. A
    // generator builds one of these rather than a string, so it cannot emit a
    // parse error, and every node carries the input span it came from.
    m("core/buri/ast", include_str!("sources/buri_ast.buri")),
    // What a generator is: `main` names `Stdin` and `Stdout` and nothing else,
    // so a generator cannot read the clock or the filesystem.
    m("core/codegen", include_str!("sources/codegen.buri")),
    // The `.proto` reader and the generator built on it. A schema becomes a
    // module through the same protocol any generator speaks, which is the
    // rule for every generator the toolchain ships: a user could have written
    // this one.
    m("std/codegen/proto/schema", include_str!("sources/codegen_proto_schema.buri")),
    m("std/codegen/proto", include_str!("sources/codegen_proto.buri")),
    // What a `tool` rule's entry points are handed and answer, and the doc a
    // formatter returns. `std/proto` is the program behind the `proto` tool,
    // written against it.
    m("core/format", include_str!("sources/format.buri")),
    m("core/tool", include_str!("sources/tool.buri")),
    m("std/proto", include_str!("sources/proto_tool.buri")),
    // The text format: a reader that holds a file against a message of the
    // `.proto` reader above, and the tool built on it. A contract module's
    // `decode` calls the reader at run time.
    m("std/textproto/read", include_str!("sources/textproto_read.buri")),
    m("std/textproto", include_str!("sources/textproto_tool.buri")),
    m("core/map", include_str!("sources/map.buri")),
    m("core/set", include_str!("sources/set.buri")),
    m("core/orderedmap", include_str!("sources/orderedmap.buri")),
    m("core/orderedset", include_str!("sources/orderedset.buri")),
    m("core/bytes", include_str!("sources/bytes.buri")),
    // DEFLATE and gzip, and pure Buri all the way down: there is no compression
    // crate in the runtime's manifest to bind to, and the archive's own
    // hand-written deflate is the PNG writer's and answers to no Buri name.
    m("core/compression", include_str!("sources/compression.buri")),
    m("core/hash", include_str!("sources/hash.buri")),
    m("core/crypto", include_str!("sources/crypto.buri")),
    // Above `core/crypto` rather than beside `core/bytes`: `version4` is
    // sixteen octets from `randomBytes`, so the module that mints one sits on
    // the module that owns `Entropy`'s door.
    m("core/uuid", include_str!("sources/uuid.buri")),
    m("core/math", include_str!("sources/math.buri")),
    m("core/simd", include_str!("sources/simd.buri")),
    m("core/bits", include_str!("sources/bits.buri")),
    // Integers with no width, and numbers whose value is their digits. Both are
    // ordinary Buri over `Int` and `[Int]` — no runtime entry, no platform —
    // and both declare methods only on their own type, so a program that has
    // never heard of either does not pay to parse it.
    m("core/bigint", include_str!("sources/bigint.buri")),
    m("core/decimal", include_str!("sources/decimal.buri")),
    // `platform/*`: the effects every platform shares, apart from any one
    // platform. `platform/effect` declares them, `platform/host` holds the
    // backends' production implementations a platform's host type names, and
    // `platform/effect/testing` is their test implementations.
    StdModule { platform: true, ..m("platform/effect", include_str!("sources/effect.buri")) },
    StdModule { platform: true, ..m(HOST_STRUCTS_MODULE, include_str!("sources/platform_host.buri")) },
    // The test implementations: one per effect, called rather than referred
    // to, so each call is a fresh runner-side handle. The `testing` segment is
    // what carries its import rule — `is_test_only_path` sees it — and it is an
    // effect's testing surface, so it may keep state in
    // `core/platforms/testing/state`.
    StdModule {
        platform: true,
        ..m("platform/effect/testing", include_str!("sources/host_testing.buri"))
    },
    // The bundled platforms' `platform.buri`, each declaring its host type and
    // its bodiless entry. A program imports its host type from here by the
    // platform's bare name: `from "native" import { NativeHost };`.
    m("native", include_str!("../../platforms/native/platform.buri")),
    m("node", include_str!("../../platforms/node/platform.buri")),
    m("web", include_str!("../../platforms/web/platform.buri")),
    // Not a platform module, deliberately. It *implements* `Allocator` rather than
    // declaring it, and `Allocator` is the one effect whose implementation carries
    // no authority — a `Region` is a number, so a library that builds its own
    // allocator has been granted nothing (SPEC 10.5). That is why this is
    // importable anywhere and `platform/host` is not.
    m("core/alloc", include_str!("sources/alloc.buri")),
    m("core/io", include_str!("sources/io.buri")),
    // Pure string work, and below `core/fs` rather than inside it: every
    // function in `core/fs` names a filesystem effect in its bounds and its
    // module doc says the disk is visible in the signature, so a `join` that
    // touches nothing would be the first exception. `Path` is the type
    // `core/fs` takes, and this is where it and its methods live.
    m("core/path", include_str!("sources/path.buri")),
    // A **platform module**, and one of two outside `platform/effect`, with
    // `core/process`, that declares effects. `FileSystemRead` and `FileSystemWrite` name a `Path`
    // in every method, `core/path` names `Allocator`, and `platform/effect` is below
    // `core/path` — so the declarations live here, where they can say what
    // they mean, rather than one module down where they could only say `Str`.
    StdModule { platform: true, ..m("core/fs", include_str!("sources/fs.buri")) },
    m("core/env", include_str!("sources/env.buri")),
    // The parsed half of `core/env`'s `args`. Not a platform module and not an
    // eager one: it declares no effect — `run` *names* `Environment`, `Stdout` and
    // `Stderr` in its bound the way `core/fs` names `FileSystem` — and it declares
    // methods only on its own `Arguments`, so a program that has never heard
    // of it does not pay to parse it.
    m("core/cli", include_str!("sources/cli.buri")),
    m("core/time", include_str!("sources/time.buri")),
    m("core/date", include_str!("sources/date.buri")),
    m("core/random", include_str!("sources/random.buri")),
    // Percent-encoding and a `Url`, and neither half of `core/net` owns it: a
    // `Url` is a type with methods, where `core/net/http` re-exports `Request`
    // rather than declaring one. Pure Buri over strings, so it names no effect
    // and a program that never parses a URL does not pay to load it.
    m("core/net/url", include_str!("sources/url.buri")),
    m("core/net/http", include_str!("sources/http.buri")),
    // The other half of being a server: `Server`, `bind`, `run`, `serve`, and
    // the accept loop those three are written out of. The loop is Buri rather
    // than the runtime's, which is what lets a request handler run under the
    // caller's own context — see the module's own header, and `effect Listen`.
    // The socket half is here too — `Socket`, `Message`, `CloseReason` and the
    // `WebSocket` hooks a `Server` carries — and it is the same arrangement one
    // level down: a socket's own loop is Buri's, its state is a local threaded
    // through a tail call, and the runtime holds a queue rather than a value.
    m("core/net/server", include_str!("sources/server.buri")),
    // The client half of the socket story, and a separate module for the reason
    // `core/net/http` and `core/net/server` are separate: dialling out and
    // accepting in are two authorities. It is `core/net/server`'s three hooks
    // over `core/net/server`'s own `Socket`, `Message` and `CloseReason`, which
    // it re-exports rather than declaring again — one program can serve on one
    // end and dial on the other, and the two ends use one vocabulary.
    m("core/net/websocket", include_str!("sources/websocket.buri")),
    // A **platform module**, for `core/fs`'s reason: `Spawn.spawnProcess`
    // answers this module's own `Output`, and a `Command` is built out of a
    // `Path`, so the declaration has to live where those names are. `Process` is
    // still `platform/effect`'s — ending this process names nothing but an integer.
    StdModule { platform: true, ..m("core/process", include_str!("sources/process.buri")) },
    // The layer under both of those: a connection dialled out, bytes each way,
    // and no protocol over them. It is a third authority for the reason the
    // other two are two — `Tcp` is granted where `Listen` is and nowhere else,
    // because a page has no sockets of its own.
    m("core/net/tcp", include_str!("sources/tcp.buri")),
    // Not a platform module: it *names* `Tasks` in its bounds rather than
    // declaring or implementing it, exactly as `core/fs` names `FileSystem`. The
    // authority is still `core/host`'s to hand out.
    m("core/tasks", include_str!("sources/tasks.buri")),
    // The other half of concurrency: state that outlives one call, reachable
    // only through the protocol its own enum declares. Not a platform module
    // either, and for `core/tasks`'s reason — it *names* `Tasks` in its bounds
    // and declares no effect of its own. Its nine runtime operations are
    // module functions keyed `actor.*` rather than the methods of an effect,
    // which is `core/list`'s shape: the authority is the bound in the
    // signature, and there is no second implementation of a mailbox for a test
    // to bind. That is also why it appears in no [`WRAPPERS`] row — it opens no
    // door, because it declares no effect.
    m("core/actor", include_str!("sources/actor.buri")),
    // One declaration, and it is a fact about the *artifact* rather than about
    // the program: `load(f)` answers `f`, and on a backend that writes more
    // than one file it also decides which file `f` is in. Not a platform
    // module — it declares no effect and names none — and its key is rewritten
    // by `middle::chunks` before any backend sees it.
    m("core/lazy", include_str!("sources/lazy.buri")),
    StdModule {
        platform: true,
        ..m("core/testing/assert", include_str!("sources/assert.buri"))
    },
    // Property testing. Not a platform module: it declares no effect and no
    // bodyless `fn` — every failure it reports goes through
    // `core/testing/assert`, which is the module that owns the runner's door.
    // The `testing` segment in the path is what keeps it out of a library
    // source, exactly as it does for `core/testing/assert`.
    m("core/testing/check", include_str!("sources/check.buri")),
    // State for an effect's test implementation. Not a platform module: it
    // declares no effect. The `testing` segment keeps it out of production code,
    // and `platform-testing-only-import` keeps it out of ordinary tests too.
    m("core/platforms/testing/state", include_str!("sources/platforms_testing_state.buri")),
    // `ui/*`. A user interface is not one of the deliberately small
    // essentials, and its vocabulary is large, so it gets its own reserved
    // root rather than growing `core/`. None of it is a platform module: the
    // effects it is written over, `Ui`, `Watch` and `Location`, are
    // `platform/effect`'s, and their test implementations are
    // `platform/effect/testing`'s. Everything here is ordinary Buri over inert
    // handles and could move to a real library once external repositories
    // land.
    m("ui/signal", include_str!("sources/ui_signal.buri")),
    m("ui/prop", include_str!("sources/ui_prop.buri")),
    m("ui/style", include_str!("sources/ui_style.buri")),
    m("ui/theme", include_str!("sources/ui_theme.buri")),
    m("ui/node", include_str!("sources/ui_node.buri")),
    // A website: the same tree, rendered to HTML on a worker and resumed on
    // the page. Not a platform module — it declares no effect.
    m("ui/web", include_str!("sources/web.buri")),
];

/// The module-path roots the standard library owns.
///
/// `core/` is the deliberately small set of essentials; `ui/` is the reactive
/// and styling vocabulary, which is a different kind of thing and a much
/// larger surface, so it gets its own root rather than diluting what `core/`
/// means (SPEC rule 35). `std/` is the tools the toolchain ships as ordinary
/// Buri programs — `std/codegen/proto` and the schema reader under it — which
/// are neither essentials nor vocabulary. `platform/` is the effects every
/// platform shares and the backends' implementations of them. All four are
/// reserved: a repository path is always `//...`, so nothing here can collide
/// with user code.
pub const ROOTS: &[&str] = &["core/", "ui/", "std/", "platform/"];

/// The bundled platforms, whose `platform.buri` a program imports by the
/// platform's bare name.
pub const PLATFORMS: &[&str] = &["native", "node", "web"];

/// The module `platform/host`: the backends' production structs, which only a
/// platform's `platform.buri` may import.
pub const HOST_STRUCTS_MODULE: &str = "platform/host";

/// Whether a module path names the embedded standard library at all.
///
/// This is a question about the *path*, not about whether the module exists —
/// `"core/nope"` answers `true`, so that naming a module the standard library
/// does not have is a `unknown-module` error rather than a search of the
/// repository that reports something else. A bundled platform's bare name is
/// one of these too.
pub fn is_std_path(path: &str) -> bool {
    ROOTS.iter().any(|r| path.starts_with(r)) || is_bundled_platform(path)
}

/// A bundled platform's name and the host type its `platform.buri` declares:
/// `("native", "NativeHost")`.
pub fn host_type_of(platform: &str) -> Option<(&'static str, &'static str)> {
    match platform {
        "native" => Some(("native", "NativeHost")),
        "node" => Some(("node", "NodeHost")),
        "web" => Some(("web", "WebHost")),
        _ => None,
    }
}

/// Whether `name` is one of the entries a bundled platform's `platform.buri`
/// declares without a body, for a program to fill.
pub fn is_entry_declaration(module: &str, name: &str) -> bool {
    let canonical = module.strip_suffix("/lib.buri").unwrap_or(module);
    crate::build::buildfile::PlatformName::bundled(canonical)
        .is_some_and(|p| p.entries().contains(&name))
}

/// The same, for what gets built.
pub fn host_type(platform: crate::build::buildfile::Platform) -> Option<(&'static str, &'static str)> {
    host_type_of(platform.proto())
}

/// The type of the field called `field` on a bundled platform's host, read
/// off its `platform.buri`: `HostFileSystem` for `native`'s `fs`.
pub fn host_field(platform: &str, field: &str) -> Option<&'static str> {
    let source = source(platform)?;
    let prefix = format!("export {field}: ");
    source.lines().find_map(|line| line.trim().strip_prefix(prefix.as_str())?.strip_suffix(','))
}

/// The bundled effects a production struct in `platform/host` implements, in
/// declaration order: `FileSystemRead` and `FileSystemWrite` for
/// `HostFileSystem`.
pub fn effects_of_host_struct(name: &str) -> Vec<&'static str> {
    let Some(source) = source(HOST_STRUCTS_MODULE) else { return Vec::new() };
    let suffix = format!(" for {name} {{");
    source
        .lines()
        .filter_map(|line| line.strip_prefix("impl ")?.strip_suffix(suffix.as_str()))
        .collect()
}

/// Whether a module path is a bundled platform's `platform.buri`, by either
/// spelling.
pub fn is_bundled_platform(path: &str) -> bool {
    let canonical = path.strip_suffix("/lib.buri").unwrap_or(path);
    PLATFORMS.contains(&canonical)
}

/// The roots as they read in a diagnostic: `` `core/...` or `ui/...` ``.
pub fn roots_phrase() -> String {
    let quoted: Vec<String> = ROOTS.iter().map(|r| format!("`{r}...`")).collect();
    match quoted.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => quoted.join(""),
    }
}

/// The paths this library used to answer to, and what each is called now.
///
/// A rename is not an alias: the old path stops resolving, and the point of
/// this table is that the *diagnostic* names the new one rather than leaving a
/// reader to guess. Most rows are an abbreviation and the same module spelled
/// out: `core/char` is `core/character`, `core/proc` is `core/process`,
/// `core/num` is `core/number`, and `core/ordmap` and `core/ordset` are
/// `core/orderedmap` and `core/orderedset`. The rest moved: effects live
/// apart from any platform, under `platform/effect`, `Ui`, `Watch` and
/// `Location` among them, with their test implementations in
/// `platform/effect/testing`; and an entry takes its platform's host rather
/// than importing `core/host`'s values, which [`retired_note`] says.
///
/// Nothing here is loadable, and [`find`] is asked first, so a name that came
/// back into service would shadow its own row rather than collide with it.
/// `no_retired_path_is_also_a_module` is what says a row cannot be both.
pub const RETIRED: &[(&str, &str)] = &[
    ("core/char", "core/character"),
    ("core/num", "core/number"),
    ("core/ordmap", "core/orderedmap"),
    ("core/ordset", "core/orderedset"),
    ("core/proc", "core/process"),
    ("core/effect", "platform/effect"),
    ("core/host/testing", "platform/effect/testing"),
    ("core/host", "platform/effect"),
    ("ui/effect", "platform/effect"),
    ("ui/testing", "platform/effect/testing"),
];

/// What a retired path's diagnostic adds, where the new path is not the whole
/// answer: `core/host`'s values are an entry's host now, and only the effects
/// moved to `platform/effect`. A note, and the fix that replaces the
/// template's.
pub fn retired_note(path: &str) -> Option<(&'static str, &'static str)> {
    let canonical = path.strip_suffix("/lib.buri").unwrap_or(path);
    (canonical == "core/host").then_some((
        "an entry takes its platform's host and binds the effects it needs from the host's \
         fields; the effects themselves are declared in `platform/effect`",
        "take the host in the entry and bind its fields:\n     \
         export fn main(host: NodeHost): Result<(), Str> {\n         \
         run(context { Allocator: host.alloc, Stdout: host.stdout })\n     \
         }",
    ))
}

/// What a retired path is called now, or `None` for a path that never named a
/// module here. Read with the same `/lib.buri` canonicalisation [`find`] uses,
/// because both spellings of a retired module are equally retired.
pub fn retired(path: &str) -> Option<&'static str> {
    let canonical = path.strip_suffix("/lib.buri").unwrap_or(path);
    RETIRED.iter().find(|(old, _)| *old == canonical).map(|(_, now)| *now)
}

/// The module a path names, whichever of its two spellings was written.
///
/// `platform/effect` is the canonical one and the one the table holds.
/// `platform/effect/lib.buri` names the same module — a cross-module import may
/// name the surface file honestly, it is merely the long way round — and it
/// has to arrive at the *same* [`StdModule`], because the loader keys a
/// module by its path and two keys would be two copies of `Allocator`.
pub fn find(path: &str) -> Option<&'static StdModule> {
    find_index(path).and_then(|i| MODULES.get(i))
}

/// [`find`], answering with the module's position in [`MODULES`], which is
/// what names its file ([`crate::diagnostics::FileId::standard`]).
pub fn find_index(path: &str) -> Option<usize> {
    let canonical = path.strip_suffix("/lib.buri").unwrap_or(path);
    MODULES.iter().position(|m| m.path == canonical)
}

/// The `index`th module as a source file: the process's one copy, built the
/// first time anything asks for it.
///
/// An index past the end is the empty file, for the reason
/// [`SourceMap::get`](crate::diagnostics::SourceMap::get) answers one for an
/// id it never minted: rendering a diagnostic is not allowed to crash.
pub fn file(index: usize) -> &'static crate::diagnostics::SourceFile {
    static FILES: [std::sync::OnceLock<crate::diagnostics::SourceFile>; MODULES.len()] =
        [const { std::sync::OnceLock::new() }; MODULES.len()];
    static MISSING: std::sync::OnceLock<crate::diagnostics::SourceFile> = std::sync::OnceLock::new();
    let new = |name: &str, text: &str| {
        crate::diagnostics::SourceFile::new(name.to_string(), std::path::PathBuf::new(), text.to_string())
    };
    match (FILES.get(index), MODULES.get(index)) {
        (Some(slot), Some(m)) => slot.get_or_init(|| new(m.path, m.source)),
        _ => MISSING.get_or_init(|| new("<none>", "")),
    }
}

/// The canonical spelling of a standard library path, or `None` when the
/// library has no such module. This is [`find`] with the answer narrowed to
/// the one thing a caller comparing paths needs.
pub fn canonical(path: &str) -> Option<&'static str> {
    find(path).map(|m| m.path)
}

/// Module path -> source text.
pub fn source(path: &str) -> Option<&'static str> {
    find(path).map(|m| m.source)
}

pub fn is_platform_module(path: &str) -> bool {
    find(path).is_some_and(|m| m.platform)
}

/// The modules a compilation always needs: the prelude, and the defining
/// module of every built-in type.
///
/// Adding a module here is safe. *Removing* one is not, unless nothing in it
/// declares a method on a built-in type — which `semantics::resolve` enforces
/// rather than leaving to review.
pub fn eager_modules() -> impl Iterator<Item = &'static str> {
    MODULES.iter().filter(|m| m.eager).map(|m| m.path)
}

/// The modules whose names are in scope in every module without an import.
pub fn prelude_modules() -> impl Iterator<Item = &'static str> {
    MODULES.iter().filter(|m| !m.prelude.is_empty()).map(|m| m.path)
}

/// `(module, exported name)` pairs injected into every module's scope, at
/// lower priority than the module's own declarations and its imports — so a
/// module may shadow any of them, and importing one explicitly is harmless.
pub fn prelude() -> impl Iterator<Item = (&'static str, &'static str)> {
    MODULES.iter().flat_map(|m| m.prelude.iter().map(move |n| (m.path, *n)))
}

/// The defining module of each built-in type (SPEC 6.7.3). A type's operations
/// travel with it, so this is where `xs.map(...)` and `s.trim()` resolve.
///
/// Total over `Prim` rather than a `&str` match with a catch-all: a new
/// primitive is now a compile error here instead of silently landing in
/// `core/number`.
pub fn defining_module(p: Prim) -> &'static str {
    match p {
        Prim::Str => "core/str",
        Prim::Char => "core/character",
        Prim::Bool => "core/bool",
        // A template is a `Str` with holes, and its operations are the
        // numeric-rendering ones, so it shares `core/number`'s module the way
        // every numeric type does.
        Prim::Template => "core/number",
        Prim::I8
        | Prim::I16
        | Prim::I32
        | Prim::I64
        | Prim::I128
        | Prim::U8
        | Prim::U16
        | Prim::U32
        | Prim::U64
        | Prim::U128
        | Prim::F32
        | Prim::F64 => "core/number",
    }
}

/// `Char.isPrintable`, for the compiler's own use: whether `c` is outside
/// General Categories `C` and `Z`, or is the space.
///
/// Read from `core/character`'s own `PRINTABLE` table rather than a copy of
/// it, so the formatter and `core/buri/ast`'s printer, which both escape a
/// literal's unprintable characters, cannot disagree about which ones those
/// are. The table is ranges of eight base-36 digits, as `unicode_tables.py`
/// writes it. A table that does not read answers `false` for everything
/// outside printable ASCII, which escapes more than it needs to and never
/// writes an invisible character into a file.
pub fn is_printable(c: char) -> bool {
    static RANGES: std::sync::OnceLock<Vec<(u32, u32)>> = std::sync::OnceLock::new();
    if (' '..='~').contains(&c) {
        return true;
    }
    let ranges = RANGES.get_or_init(|| printable_ranges().unwrap_or_default());
    in_ranges(ranges, c)
}

/// Whether `c` is Extended_Pictographic and assigned: what a zero-width joiner
/// joins in an emoji ZWJ sequence.
///
/// Read from `core/buri/ast`'s own `PICTOGRAPHIC` table, for the reason
/// `is_printable` reads `core/character`'s: the formatter and that printer
/// keep an emoji's joiners by one rule, and this is its data. A table that
/// does not read answers `false`, which escapes the joiner.
pub fn is_pictographic(c: char) -> bool {
    static RANGES: std::sync::OnceLock<Vec<(u32, u32)>> = std::sync::OnceLock::new();
    let ranges = RANGES.get_or_init(|| pictographic_ranges().unwrap_or_default());
    in_ranges(ranges, c)
}

fn in_ranges(ranges: &[(u32, u32)], c: char) -> bool {
    let cp = u32::from(c);
    let at = ranges.partition_point(|&(_, last)| last < cp);
    ranges.get(at).is_some_and(|&(first, _)| first <= cp)
}

/// The `PRINTABLE` table out of `core/character`'s source.
fn printable_ranges() -> Option<Vec<(u32, u32)>> {
    range_table("core/character", "PRINTABLE")
}

/// The `PICTOGRAPHIC` table out of `core/buri/ast`'s source.
fn pictographic_ranges() -> Option<Vec<(u32, u32)>> {
    range_table("core/buri/ast", "PICTOGRAPHIC")
}

/// The range table `let {name}: Str` in `module`'s source.
fn range_table(module: &str, name: &str) -> Option<Vec<(u32, u32)>> {
    let source = source(module)?;
    let (_, after) = source.split_once(&format!("let {name}: Str ="))?;
    let (_, quoted) = after.split_once('"')?;
    let (table, _) = quoted.split_once('"')?;
    let code = |digits: &[char]| -> Option<u32> {
        digits.iter().try_fold(0u32, |n, d| n.checked_mul(36)?.checked_add(d.to_digit(36)?))
    };
    let digits: Vec<char> = table.chars().collect();
    digits
        .chunks(8)
        .map(|range| match range {
            [a, b, c, d, e, f, g, h] => Some((code(&[*a, *b, *c, *d])?, code(&[*e, *f, *g, *h])?)),
            _ => None,
        })
        .collect()
}

/// The module only an effect's testing surface may import.
pub const PLATFORM_STATE_MODULE: &str = "core/platforms/testing/state";

/// Whether a module path is part of an effect's testing surface: under
/// `platform/effect/` and with a `testing` segment, bundled or `//`.
pub fn is_effect_testing_path(path: &str) -> bool {
    let bare = path.trim_start_matches("//");
    bare.starts_with("platform/effect/") && bare.split('/').any(|seg| seg == "testing")
}

// ---------------------------------------------------------------------------
// The door onto every effect
// ---------------------------------------------------------------------------

/// The standard-library function that performs one effect method.
///
/// **An effect's methods are called through the module that wraps the effect,
/// never on the value that carries it** (SPEC 10.2): `ctx.println(t)` is
/// `io.println(ctx, t)`. Two layers are below that line and keep the method
/// form — the standard library, which is where these wrappers are, and the
/// body of an `impl` that *supplies* the effect, which is where the operation
/// is implemented. Everywhere else the call goes through a row of this table,
/// and `semantics/expressions.rs` reports `effect-method-call` when it does
/// not.
///
/// One table, three readers: the diagnostic's fix, the language server's
/// completion list, and [`tests::every_effect_method_has_a_door`], which is
/// what keeps a method from being declared with no way to call it. Six of the
/// effect methods had no wrapper at all before this table existed, and nothing
/// said so.
pub struct Wrapper {
    /// The effect, as `platform/effect`, `core/fs` or `core/process` spells it.
    pub effect: &'static str,
    /// The method it declares.
    pub method: &'static str,
    /// The module holding the door, as an import path.
    pub module: &'static str,
    /// How the call reads, with the context in the place it goes. A free
    /// function leads with its module alias; a handle's method leads with the
    /// handle, because that is the shape `ui/signal` already ships.
    pub call: &'static str,
}

const fn w(
    effect: &'static str,
    method: &'static str,
    module: &'static str,
    call: &'static str,
) -> Wrapper {
    Wrapper { effect, method, module, call }
}

/// Every method of every declared effect, and the function that calls it.
///
/// The order is `platform/effect`'s declaration order, then `core/fs`'s two, then
/// the reactive graph's, so the table reads beside the sources it is about.
pub const WRAPPERS: &[Wrapper] = &[
    w("Allocator", "allocate", "core/alloc", "alloc.allocate(ctx, bytes)"),
    w("Stdout", "print", "core/io", "io.print(ctx, text)"),
    w("Stdout", "println", "core/io", "io.println(ctx, text)"),
    w("Stdout", "writeBytes", "core/io", "io.writeBytes(ctx, bytes)"),
    w("Stderr", "eprint", "core/io", "io.eprint(ctx, text)"),
    w("Stderr", "eprintln", "core/io", "io.eprintln(ctx, text)"),
    w("Stdin", "readLine", "core/io", "io.readLine(ctx)"),
    w("Stdin", "readBytes", "core/io", "io.readBytes(ctx, n)"),
    w("FileSystemRead", "readFile", "core/fs", "fs.readText(ctx, path)"),
    w("FileSystemRead", "fileExists", "core/fs", "fs.exists(ctx, path)"),
    w("FileSystemRead", "readDir", "core/fs", "fs.listDir(ctx, path)"),
    w("FileSystemRead", "readFileBytes", "core/fs", "fs.readBytes(ctx, path)"),
    w("FileSystemRead", "metadata", "core/fs", "fs.metadata(ctx, path)"),
    w("FileSystemRead", "readRange", "core/fs", "fs.readRange(ctx, path, at, count)"),
    w("FileSystemRead", "realPath", "core/fs", "fs.canonicalize(ctx, path)"),
    w("FileSystemWrite", "writeFile", "core/fs", "fs.writeText(ctx, path, body)"),
    w("FileSystemWrite", "writeFileBytes", "core/fs", "fs.writeBytes(ctx, path, body)"),
    w("FileSystemWrite", "appendFile", "core/fs", "fs.append(ctx, path, body)"),
    w("FileSystemWrite", "renameFile", "core/fs", "fs.rename(ctx, source, destination)"),
    w("FileSystemWrite", "removeFile", "core/fs", "fs.remove(ctx, path)"),
    w("FileSystemWrite", "removeDir", "core/fs", "fs.removeDir(ctx, path)"),
    w("FileSystemWrite", "makeDir", "core/fs", "fs.makeDir(ctx, path)"),
    w("FileSystemWrite", "syncFile", "core/fs", "fs.sync(ctx, path)"),
    w("FileSystemWrite", "copyFile", "core/fs", "fs.copy(ctx, source, destination)"),
    w("Network", "fetch", "core/net/http", "http.send(ctx, request)"),
    w("Clock", "nowMilliseconds", "core/time", "time.now(ctx)"),
    w("Clock", "sleepMilliseconds", "core/time", "time.sleep(ctx, duration)"),
    w("Clock", "monotonicNanoseconds", "core/time", "time.monotonic(ctx)"),
    w("Random", "nextInt", "core/random", "random.int(ctx, lo, hi)"),
    w("Random", "nextFloat", "core/random", "random.float(ctx)"),
    // `core/crypto` rather than `core/random`, which is the whole argument
    // `core/crypto`'s header makes: the seeded module and the unguessable one
    // are different promises and a reader should have to name which they meant.
    w("Entropy", "bytes", "core/crypto", "crypto.randomBytes(ctx, count)"),
    w("Environment", "variable", "core/env", "env.get(ctx, name)"),
    w("Environment", "arguments", "core/env", "env.arguments(ctx)"),
    w("Environment", "currentDirectory", "core/env", "env.currentDirectory(ctx)"),
    w("Environment", "allVariables", "core/env", "env.all(ctx)"),
    w("Environment", "operatingSystemName", "core/env", "env.operatingSystem(ctx)"),
    w("Process", "exitWith", "core/process", "process.exit(ctx, code)"),
    w("Spawn", "spawnProcess", "core/process", "process.run(ctx, command)"),
    w("Tasks", "parallel", "core/tasks", "tasks.parallel(ctx, items, f)"),
    w("Listen", "listenBind", "core/net/server", "server.bind(ctx, aServer)"),
    w("Listen", "listenAccept", "core/net/server", "server.serve(ctx, aServer)"),
    w("Listen", "listenRequest", "core/net/server", "server.serve(ctx, aServer)"),
    w("Listen", "listenRespond", "core/net/server", "server.serve(ctx, aServer)"),
    w("Listen", "listenClose", "core/net/server", "server.run(ctx, listener, aServer)"),
    w("Listen", "listenUpgrade", "core/net/server", "server.serve(ctx, aServer)"),
    w("Listen", "listenReceive", "core/net/server", "server.serve(ctx, aServer)"),
    w("Sockets", "socketSendText", "core/net/server", "aSocket.send(ctx, .Text(text))"),
    w("Sockets", "socketSendBytes", "core/net/server", "aSocket.send(ctx, .Binary(bytes))"),
    w("Sockets", "socketClose", "core/net/server", "aSocket.close(ctx, aCloseReason)"),
    w("Tcp", "tcpConnect", "core/net/tcp", "tcp.connect(ctx, host, port)"),
    w("Tcp", "tcpRead", "core/net/tcp", "aStream.read(ctx, limit)"),
    w("Tcp", "tcpWrite", "core/net/tcp", "aStream.write(ctx, body)"),
    w("Tcp", "tcpClose", "core/net/tcp", "aStream.close(ctx)"),
    w("WebSocketClient", "connectSocket", "core/net/websocket", "websocket.connect(ctx, aClient)"),
    w(
        "WebSocketClient",
        "connectReceive",
        "core/net/websocket",
        "websocket.connect(ctx, aClient)",
    ),
    // `ui/*`. A signal handle is inert data and the authority travels through
    // the context, so the door for reading and writing one is a method on the
    // handle that *takes* the context — which is already the shape this rule
    // asks for, and is why these rows do not lead with a module alias.
    w("Watch", "read", "ui/signal", "aSignal.get(ctx)"),
    w("Ui", "signal", "ui/signal", "signal.signal(ctx, initial)"),
    w("Ui", "read", "ui/signal", "aSignal.get(ctx)"),
    w("Ui", "write", "ui/signal", "aSignal.set(ctx, value)"),
    w("Ui", "memo", "ui/prop", "prop.memo(ctx, compute)"),
    w("Ui", "watch", "ui/signal", "signal.watch(ctx, run)"),
    w("Ui", "schedule", "platform/effect", "effect.after(ctx, duration, run)"),
    w("Ui", "unschedule", "platform/effect", "effect.cancel(ctx, timer)"),
    // The address bar. Its reader answers a cell, and the door that turns that
    // into something a tree can hold is `route`; `web.path(ctx)` is the same
    // cell read once. Its two writers put an address in the bar, and each door
    // writes the cell after it — so a program never touches one without the
    // other.
    w("Location", "path", "ui/web", "web.route(ctx)"),
    w("Location", "push", "ui/web", "web.navigate(ctx, path)"),
    w("Location", "replace", "ui/web", "web.replace(ctx, path)"),
];

/// The door onto one effect method, or `None` for a name this table has never
/// heard of — which, given [`tests::every_effect_method_has_a_door`], means the
/// trait was not an effect.
pub fn wrapper(effect: &str, method: &str) -> Option<&'static Wrapper> {
    WRAPPERS.iter().find(|r| r.effect == effect && r.method == method)
}

impl Wrapper {
    /// The fix a diagnostic prints: the call, and the module it comes from.
    pub fn fix(&self) -> String {
        format!("call it through `{}`: `{}`", self.module, self.call)
    }

    /// The import line the fix needs, for a door that leads with a module
    /// alias — and nothing for one that leads with a handle, where the name is
    /// the reader's own local and no import would introduce it.
    ///
    /// The alias is the module path's last segment, which is the convention
    /// every `core/*` wrapper module already follows, and
    /// [`tests::every_wrapper_call_leads_with_its_module_or_a_handle`] is what
    /// keeps a row from quietly inventing a second one.
    pub fn import(&self) -> Option<String> {
        let alias = self.module.rsplit('/').next()?;
        self.call
            .starts_with(&format!("{alias}."))
            .then(|| format!("the import is `from \"{}\" import * as {alias};`", self.module))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both spellings: a repository's own effect and the bundled one.
    #[test]
    fn an_effect_testing_path_is_under_platform_effect_with_a_testing_segment() {
        for yes in [
            "//platform/effect/kv/testing",
            "//platform/effect/kv/testing/store.buri",
            "platform/effect/testing",
            "platform/effect/kv/testing/lib.buri",
        ] {
            assert!(is_effect_testing_path(yes), "{yes}");
        }
        for no in [
            "//platform/effect/kv",
            "//lib/kv/testing",
            "//platform/node/testing",
            "//platform/effect/kv/testing.buri",
            "core/host/testing",
            "main.buri",
        ] {
            assert!(!is_effect_testing_path(no), "{no}");
        }
    }

    /// `is_printable` falls back to escaping everything non-ASCII when the
    /// table does not read, which no test of the formatter's output would
    /// notice on ASCII and Latin text. This one would.
    #[test]
    fn the_printable_table_reads_out_of_core_character() {
        let ranges = printable_ranges().expect("`PRINTABLE` reads");
        assert!(ranges.len() > 100, "{} range(s)", ranges.len());
        for c in ['a', ' ', 'é', '中', '😀'] {
            assert!(is_printable(c), "{c:?}");
        }
        for c in ['\u{7f}', '\u{85}', '\u{a0}', '\u{200b}', '\u{202e}', '\u{2028}', '\u{feff}'] {
            assert!(!is_printable(c), "{c:?}");
        }
    }

    /// The same for `is_pictographic`, which would otherwise escape every
    /// emoji's joiners without a word.
    #[test]
    fn the_pictographic_table_reads_out_of_core_buri_ast() {
        let ranges = pictographic_ranges().expect("`PICTOGRAPHIC` reads");
        assert!(ranges.len() > 50, "{} range(s)", ranges.len());
        for c in ['\u{a9}', '\u{2764}', '\u{1f3f4}', '\u{1f468}', '\u{1f680}'] {
            assert!(is_pictographic(c), "{c:?}");
        }
        for c in ['a', '\u{200d}', '\u{fe0f}', '\u{1f3fb}', '\u{1f1e6}', '\u{1fc00}'] {
            assert!(!is_pictographic(c), "{c:?}");
        }
    }

    /// The declared effect methods, read off the two platform sources that may
    /// declare an effect: `(effect, method)`, in declaration order.
    ///
    /// Off the source text rather than off a second list, for
    /// `every_host_export_is_in_the_grant_table`'s reason — a method added to
    /// `platform/effect` and forgotten here would be a method with no way to call
    /// it, which is precisely the hole [`WRAPPERS`] exists to close.
    fn declared_effect_methods() -> Vec<(String, String)> {
        let mut out = Vec::new();
        for path in ["platform/effect", "core/fs", "core/process"] {
            let src = source(path).expect("a platform module");
            let mut effect: Option<String> = None;
            for line in src.lines() {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("export effect ") {
                    effect = Some(rest.split([' ', '{', '<']).next().unwrap_or("").to_string());
                    continue;
                }
                // A declaration is one line and ends the block it is in only at
                // a closing brace in column one, which is the shape every
                // effect in both files is written in.
                if t == "}" {
                    effect = None;
                    continue;
                }
                let Some(e) = &effect else { continue };
                let Some(rest) = t.strip_prefix("fn ") else { continue };
                let name = rest.split(['(', '<']).next().unwrap_or("");
                out.push((e.clone(), name.to_string()));
            }
        }
        out
    }

    /// A retired path names no module, and the module it points at is real.
    ///
    /// Both halves matter. A row whose old path still loads would make
    /// `load_std` unreachable for it and the rename a lie; a row pointing at a
    /// module that does not exist would send a reader to a second failure.
    #[test]
    fn no_retired_path_is_also_a_module() {
        for (old, now) in RETIRED {
            assert!(find(old).is_none(), "`{old}` is retired and still loads");
            assert!(find(now).is_some(), "`{old}` points at `{now}`, which is no module");
            assert_eq!(retired(old), Some(*now));
            assert_eq!(retired(&format!("{old}/lib.buri")), Some(*now));
        }
        assert_eq!(retired("core/list"), None);
    }

    /// A renamed name is gone, its module is real, and what it points at is
    /// spelled the way the library spells it.
    ///
    /// The table is read only after a lookup has already failed, so a row for a
    /// name that is still there would be dead at best and a lie at worst — it
    /// would tell a reader to rewrite working code. `sqrt` is the shape: the
    /// old spelling appears nowhere in `core/math`, and `squareRoot` does.
    #[test]
    fn no_renamed_name_is_still_exported() {
        for row in renamed::RENAMED {
            let src = source(row.module)
                .unwrap_or_else(|| panic!("`{}` is no module", row.module));
            // On the surface: anything `export`ed, plus a trait's or an
            // effect's own members, which are indented and carry no `export`.
            // A module-level private `let` is neither — which is exactly what
            // `core/time`'s counts became.
            let on_the_surface = |name: &str| {
                src.lines().any(|l| {
                    let t = l.trim_start();
                    let indented = l.starts_with(' ');
                    let keywords: &[&str] = match t.starts_with("export ") {
                        true => &["fn ", "let ", "struct ", "enum ", "trait ", "effect ", "type "],
                        false if indented => &["fn "],
                        false => &[],
                    };
                    let rest = t.strip_prefix("export ").unwrap_or(t);
                    keywords
                        .iter()
                        .filter_map(|k| rest.strip_prefix(k))
                        .filter_map(|r| r.strip_prefix(name))
                        .any(|r| r.starts_with(['(', '<', ':', ' ']))
                })
            };
            assert!(
                !on_the_surface(row.old),
                "`{}` still has `{}`, so the rename row is a lie",
                row.module,
                row.old
            );
            if let renamed::Now::Named(now) = row.now {
                assert!(
                    on_the_surface(now),
                    "`{}` says `{}` is `{now}`, which it does not have",
                    row.module,
                    row.old
                );
            }
        }
    }

    /// The one thing a bare name has to get right: two modules that renamed the
    /// same name differently answer with nothing rather than with one of them.
    #[test]
    fn a_bare_name_answers_only_where_the_modules_agree() {
        assert!(renamed::anywhere("args").is_none(), "`args` has two answers");
        let (note, fix) = renamed::anywhere("len").expect("`len` is `length` everywhere");
        assert_eq!(note, "`len` was renamed to `length`");
        assert_eq!(fix, "write `length`");
        let (note, _) = renamed::in_module("core/time", "sleepMs").expect("a removed name");
        assert_eq!(note, "`sleepMs` was removed; write `time.sleep(ctx, time.milliseconds(n))`");
        assert!(renamed::in_module("core/list", "map").is_none());
    }

    /// `core/actor` declares no effect, so it opens no door — and the two
    /// halves of that are asserted rather than left to be noticed.
    ///
    /// It is the one module whose runtime operations are **module functions**
    /// rather than effect methods: nine bodyless `fn`s keyed `actor.*`, each
    /// with the authority in its bound (`C: Tasks`) exactly as `core/list`'s
    /// allocating combinators carry `C: Allocator`. SPEC 10.2 is about reaching
    /// *the outside world* through a context, and a mailbox is neither the
    /// outside world nor something a test would want a second implementation
    /// of — which is also why `platform/effect/testing` gains nothing for it.
    ///
    /// So [`WRAPPERS`] has no `actor` row, and `every_effect_method_has_a_door`
    /// above still passes over the whole effect surface. A future `effect
    /// Actors` would have to add nine rows there, and this test is what would
    /// fail first if somebody declared one and forgot.
    #[test]
    fn core_actor_declares_no_effect_and_so_needs_no_door() {
        let source = find("core/actor").expect("`core/actor` is in the table").source;
        assert!(
            !source.contains("export effect "),
            "`core/actor` declares an effect now; it needs `WRAPPERS` rows, and this test \
             is the reminder rather than the rule"
        );
        assert!(
            source.contains("fn mailboxOpen<C: Tasks, S>"),
            "the nine runtime operations are bodyless module functions with the authority \
             in their bound; a signature that lost the bound would be an operation with no \
             authority behind it"
        );
        assert!(
            !WRAPPERS.iter().any(|row| row.module == "core/actor"),
            "a `WRAPPERS` row points at `core/actor`, which declares no effect"
        );
    }

    /// The mailbox bound is one number, written twice, and the two spellings
    /// must agree.
    ///
    /// `cli/runtime/rt.rs` refuses to take a message past the bound, and
    /// `core/actor` is where the number a reader of the module is told about
    /// lives. A number quoted in the documentation that could drift from the
    /// one the runtime enforces is a claim nobody can check.
    ///
    /// Read out of the two sources rather than shared as a constant, because
    /// they are two crates that never link against each other — the archive is
    /// `include_bytes!`d — which is the same reason `BURI_OK` is transcribed in
    /// `backend/runtime_table.rs` rather than imported.
    #[test]
    fn the_default_mailbox_is_the_one_core_actor_names() {
        const RUNTIME: &str = include_str!("../../../runtime/rt.rs");
        let buri = find("core/actor").expect("`core/actor` is in the table").source;
        // `split` rather than `find` and a range, because a byte range into a
        // `&str` is `clippy::string_slice` and this needs no offset — what
        // follows the needle is what the second piece begins with.
        let named = |text: &str, needle: &str| -> String {
            text.split(needle)
                .nth(1)
                .unwrap_or_else(|| panic!("no `{needle}`"))
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        };
        let module = named(buri, "let MAILBOX: Int = ");
        let runtime = named(RUNTIME, "pub const MAILBOX: i64 = ");
        assert!(!module.is_empty(), "`core/actor` names no default mailbox");
        assert_eq!(
            module, runtime,
            "`core/actor`'s MAILBOX is {module} and `cli/runtime/rt.rs`'s is {runtime}"
        );
    }

    /// Every method of every declared effect has a standard-library function
    /// that calls it.
    ///
    /// This is the invariant the rule rests on: an effect method is no longer
    /// callable outside the standard library and the `impl` that supplies it,
    /// so a method with no wrapper is a method nothing can reach. `Allocator`,
    /// `Process`, `Listen` and `Sockets` failed this the day the table was
    /// written — six methods of thirty-eight with no door — and it is the
    /// reason `core/process` and `core/net/server` exist.
    #[test]
    fn every_effect_method_has_a_door() {
        let declared = declared_effect_methods();
        assert!(declared.len() > 30, "the scan found only {} methods", declared.len());
        for (effect, method) in &declared {
            let row = wrapper(effect, method);
            assert!(
                row.is_some(),
                "`{effect}.{method}` is declared and no standard-library function calls it, so \
                 nothing outside `core/*` can perform it"
            );
            let row = row.expect("checked");
            assert!(
                find(row.module).is_some(),
                "`{effect}.{method}`'s door is in `{}`, which is not a module",
                row.module
            );
        }
    }

    /// A door's call either leads with its module's alias — the path's last
    /// segment, which is what every wrapper module is imported as — or with a
    /// handle the reader already has. Nothing in between: a call leading with
    /// a third name would print an import nobody could write.
    ///
    /// **The handles are named here rather than pattern-matched**, because
    /// "starts with a lowercase `a`" would admit `aliased.` and the point of
    /// the second arm is that a reader already holds the value. Two types are
    /// on it and both are the same arrangement — an effect that speaks in
    /// integer handles, and a module one level up that wraps one in a value
    /// with methods: `ui/signal`'s `Signal<T>` over `Ui`'s signal ids,
    /// `core/net/server`'s `Socket` over `Sockets`' socket ids, and
    /// `core/net/tcp`'s `Stream` over `Tcp`'s stream handles.
    #[test]
    fn every_wrapper_call_leads_with_its_module_or_a_handle() {
        const HANDLES: &[&str] = &["aSignal.", "aSocket.", "aStream."];
        for row in WRAPPERS {
            let alias = row.module.rsplit('/').next().expect("a path has a segment");
            let leads = row.call.starts_with(&format!("{alias}."));
            assert_eq!(
                leads,
                row.import().is_some(),
                "`{}.{}` disagrees with itself about leading with `{alias}`",
                row.effect,
                row.method
            );
            assert!(
                leads || HANDLES.iter().any(|handle| row.call.starts_with(handle)),
                "`{}.{}`'s call `{}` leads with neither `{alias}` nor a handle",
                row.effect,
                row.method,
                row.call
            );
        }
    }

    /// And the other direction: a row for a method nobody declares would offer
    /// a fix that does not compile.
    #[test]
    fn every_wrapper_names_a_declared_method() {
        let declared = declared_effect_methods();
        for row in WRAPPERS {
            assert!(
                declared.iter().any(|(e, m)| e == row.effect && m == row.method),
                "`{}.{}` has a wrapper row and no declaration",
                row.effect,
                row.method
            );
        }
    }

    /// A prelude name is in scope in every module, so its module has to be in
    /// every compilation.
    #[test]
    fn every_prelude_module_is_eager() {
        for m in MODULES {
            assert!(
                m.prelude.is_empty() || m.eager,
                "`{}` publishes a prelude name but does not load eagerly",
                m.path
            );
        }
    }

    /// The roots are what module resolution, the reference and the intrinsic
    /// gate all key off, so a module outside them would load and then be
    /// invisible to all three.
    #[test]
    fn every_module_is_under_a_reserved_root() {
        for m in MODULES {
            assert!(is_std_path(m.path), "`{}` is under no reserved root", m.path);
        }
    }

    /// A cross-module import may name the surface file honestly — it is only
    /// the long way round — and it has to arrive at the same module. Two
    /// entries would be two `Allocator`s, and a value of one would not be a value
    /// of the other.
    #[test]
    fn both_spellings_of_a_module_are_the_same_module() {
        for m in MODULES {
            let long = format!("{}/lib.buri", m.path);
            assert_eq!(canonical(&long), Some(m.path), "`{long}` is not `{}`", m.path);
            assert_eq!(canonical(m.path), Some(m.path));
        }
        assert_eq!(canonical("core/nope"), None);
        assert_eq!(canonical("core/nope/lib.buri"), None);
    }

    #[test]
    fn every_module_path_is_distinct() {
        let mut seen: Vec<&str> = MODULES.iter().map(|m| m.path).collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(before, seen.len(), "two entries share a path");
    }

    /// No method `Listen` or `Sockets` declares is declared by any other
    /// effect.
    ///
    /// This is the `find_in_bounds` hazard written down (`semantics/
    /// expressions.rs`): a call through a context searches *every* bound
    /// effect, and two matches are `ambiguous-trait-method` at the call site
    /// rather than at either declaration. So a method name is not local to the
    /// effect that declares it — it is claimed out of a namespace shared by
    /// everything a program might bind beside it, and the claim cannot be
    /// withdrawn once programs are written.
    ///
    /// The tree already carries the lesson twice. `Ui.read` and `Watch.read`
    /// are designed to be bound together and `ctx.read(id)` is ambiguous for
    /// everybody who does; `Network.fetch` and `Fetch.fetch` are the same word for
    /// nearly the same thing, saved only by no platform granting both. Neither
    /// can be fixed now, so neither is asserted about here — what is asserted
    /// is that the two server effects do not add a third. `Listen` grew from
    /// one method to four when its accept loop moved into `core/net/server`,
    /// and to five when that loop grew a worker per handler, and to seven when
    /// it learned to upgrade a connection into a socket — and every one of the
    /// seven kept the `listen` prefix for exactly this reason: a namespace is
    /// claimed once, and seven common verbs would have been seven names taken
    /// from every effect a server binds beside it. `listenRequest` and
    /// `listenReceive` are the clearest cases of all — a bare `request` is a
    /// word half the standard library could want and `Network` is bound beside this
    /// one by design, and a bare `receive` would read as either a socket or a
    /// mailbox depending on what else happened to be in scope.
    #[test]
    fn the_server_effects_claim_no_method_name_another_effect_claims() {
        let mut mine: Vec<(&str, String)> = Vec::new();
        let mut theirs: Vec<(String, String)> = Vec::new();
        for path in ["platform/effect/lib.buri"] {
            let src = source(path).expect("a platform module");
            let mut effect: Option<String> = None;
            for line in src.lines() {
                if let Some(rest) = line.strip_prefix("export effect ") {
                    effect = Some(rest.trim_end_matches(" {").trim().to_string());
                } else if line == "}" {
                    effect = None;
                } else if let (Some(owner), Some(rest)) =
                    (effect.as_ref(), line.trim_start().strip_prefix("fn "))
                {
                    let method = rest.split(['(', '<']).next().unwrap_or("").to_string();
                    match owner.as_str() {
                        "Listen" => mine.push(("Listen", method)),
                        "Sockets" => mine.push(("Sockets", method)),
                        _ => theirs.push((owner.clone(), method)),
                    }
                }
            }
        }
        assert_eq!(mine.len(), 10, "the two effects declare ten methods between them: {mine:?}");
        for (owner, method) in &mine {
            for (other, name) in &theirs {
                assert!(
                    name != method,
                    "`{owner}.{method}` collides with `{other}.{name}`: a context binding both \
                     cannot call either by name"
                );
            }
        }
    }

    /// Every type a primitive can be must have a module that exists.
    #[test]
    fn every_primitive_has_a_defining_module() {
        for p in Prim::all() {
            let path = defining_module(*p);
            assert!(find(path).is_some(), "`{}` names a module that does not exist", p.name());
        }
    }
}
