//! The embedded standard library.
//!
//! `core/*` ships with the toolchain and is never listed in a `dependencies`.
//! It is available to every target, and the purity tiers in SPEC 11.1 govern
//! what any given import of it can do.
//!
//! There is no directory layout on disk here — the library is the one table of
//! `include_str!`s below — so `core/effect` is a name rather than a place. It
//! is spelled without a file because an import that crosses a module boundary
//! names the module, and every import of the standard library crosses one:
//! nothing in a repository is ever *inside* `core/effect`. [`find`] still
//! answers to `core/effect/lib.buri`, which names the same module the long way
//! round, and canonicalises it — see there for why one spelling has to win.
//!
//! Modules here may declare a `fn` with no body. Those are the operations the
//! backend supplies — string and array primitives, the platform's effect
//! implementations, the test runner's — and every one of them must have an
//! entry in the backend's runtime or code generation fails loudly.
//!
//! Everything the rest of the compiler asks about a standard library module is
//! answered from the one table below. It used to be five hand-maintained lists
//! of the same strings — a `source()` match, a `MODULES` array, an
//! `EAGER_MODULES` array, a `PRELUDE_MODULES` array and a `PRELUDE` array of
//! pairs — so a path in one and not another was representable: a module in
//! `MODULES` with no `source()` arm made the diagnostics suggest a near miss
//! that then failed to load, and a name in `PRELUDE` whose module was not
//! loaded eagerly was silently not in scope.

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
    /// and comparison traits are here because `derive Eq for Point;` appears in
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
        prelude: &["Order", "Eq", "Ord", "Show", "Hash"],
        eager: true,
        ..m("core/order", include_str!("sources/order.buri"))
    },
    StdModule {
        prelude: &[
            "Add",
            "Sub",
            "Mul",
            "Div",
            "Rem",
            "Neg",
            "Bounded",
            "Checked",
            "Wrapping",
            "Saturating",
            "RangeError",
        ],
        eager: true,
        ..m("core/num", include_str!("sources/num.buri"))
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
    StdModule { eager: true, ..m("core/char", include_str!("sources/char.buri")) },
    StdModule { eager: true, ..m("core/bool", include_str!("sources/bool.buri")) },
    m("core/queue", include_str!("sources/queue.buri")),
    m("core/bitset", include_str!("sources/bitset.buri")),
    m("core/json", include_str!("sources/json.buri")),
    m("core/proto", include_str!("sources/proto.buri")),
    m("core/map", include_str!("sources/map.buri")),
    m("core/set", include_str!("sources/set.buri")),
    m("core/ordmap", include_str!("sources/ordmap.buri")),
    m("core/ordset", include_str!("sources/ordset.buri")),
    m("core/bytes", include_str!("sources/bytes.buri")),
    m("core/hash", include_str!("sources/hash.buri")),
    m("core/crypto", include_str!("sources/crypto.buri")),
    m("core/math", include_str!("sources/math.buri")),
    m("core/simd", include_str!("sources/simd.buri")),
    m("core/bits", include_str!("sources/bits.buri")),
    StdModule { platform: true, ..m("core/effect", include_str!("sources/effect.buri")) },
    StdModule { platform: true, ..m("core/host", include_str!("sources/host.buri")) },
    // `core/host`'s surface for a test source: the same names, called rather
    // than referred to, so each call is a fresh runner-side handle. It is a
    // *different path* rather than a second export list on `core/host`, and
    // that is what carries its import rule — `is_test_only_path` sees the
    // `testing` segment, and `HOST_MODULE`'s `Role::Entry` gate keys on the
    // exact path `core/host` and so does not catch this one.
    StdModule {
        platform: true,
        ..m("core/host/testing", include_str!("sources/host_testing.buri"))
    },
    // Not a platform module, deliberately. It *implements* `Alloc` rather than
    // declaring it, and `Alloc` is the one effect whose implementation carries
    // no authority — a `Region` is a number, so a library that builds its own
    // allocator has been granted nothing (SPEC 10.5). That is why this is
    // importable anywhere and `core/host` is not.
    m("core/alloc", include_str!("sources/alloc.buri")),
    m("core/io", include_str!("sources/io.buri")),
    m("core/fs", include_str!("sources/fs.buri")),
    m("core/env", include_str!("sources/env.buri")),
    // The parsed half of `core/env`'s `args`. Not a platform module and not an
    // eager one: it declares no effect — `run` *names* `Env`, `Stdout` and
    // `Stderr` in its bound the way `core/fs` names `Fs` — and it declares
    // methods only on its own `Arguments`, so a program that has never heard
    // of it does not pay to parse it.
    m("core/cli", include_str!("sources/cli.buri")),
    m("core/time", include_str!("sources/time.buri")),
    m("core/date", include_str!("sources/date.buri")),
    m("core/random", include_str!("sources/random.buri")),
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
    m("core/proc", include_str!("sources/proc.buri")),
    // Not a platform module: it *names* `Tasks` in its bounds rather than
    // declaring or implementing it, exactly as `core/fs` names `Fs`. The
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
    StdModule {
        platform: true,
        ..m("core/testing/assert", include_str!("sources/assert.buri"))
    },
    // `ui/*`. A user interface is not one of the deliberately small
    // essentials, and its vocabulary is large, so it gets its own reserved
    // root rather than growing `core/`. Only `ui/effect` is a platform module —
    // it declares the effects; everything else is ordinary Buri over inert
    // handles and could move to a real library once external repositories
    // land.
    StdModule { platform: true, ..m("ui/effect", include_str!("sources/ui_effect.buri")) },
    m("ui/signal", include_str!("sources/ui_signal.buri")),
    m("ui/prop", include_str!("sources/ui_prop.buri")),
    m("ui/style", include_str!("sources/ui_style.buri")),
    m("ui/theme", include_str!("sources/ui_theme.buri")),
    m("ui/node", include_str!("sources/ui_node.buri")),
    m("ui/testing", include_str!("sources/ui_testing.buri")),
];

/// The module-path roots the standard library owns.
///
/// `core/` is the deliberately small set of essentials; `ui/` is the reactive
/// and styling vocabulary, which is a different kind of thing and a much
/// larger surface, so it gets its own root rather than diluting what `core/`
/// means (SPEC rule 35). Both are reserved: a repository path is always
/// `//...`, so nothing here can collide with user code.
pub const ROOTS: &[&str] = &["core/", "ui/"];

/// Whether a module path names the embedded standard library at all.
///
/// This is a question about the *path*, not about whether the module exists —
/// `"core/nope"` answers `true`, so that naming a module the standard library
/// does not have is a `no-such-module` error rather than a search of the
/// repository that reports something else.
pub fn is_std_path(path: &str) -> bool {
    ROOTS.iter().any(|r| path.starts_with(r))
}

/// The roots as they read in a diagnostic: `` `core/...` or `ui/...` ``.
pub fn roots_phrase() -> String {
    let quoted: Vec<String> = ROOTS.iter().map(|r| format!("`{r}...`")).collect();
    match quoted.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => quoted.join(""),
    }
}

/// The module a path names, whichever of its two spellings was written.
///
/// `core/effect` is the canonical one and the one the table holds.
/// `core/effect/lib.buri` names the same module — a cross-module import may
/// name the surface file honestly, it is merely the long way round — and it
/// has to arrive at the *same* [`StdModule`], because the loader keys a
/// module by its path and two keys would be two copies of `Alloc`.
pub fn find(path: &str) -> Option<&'static StdModule> {
    let canonical = path.strip_suffix("/lib.buri").unwrap_or(path);
    MODULES.iter().find(|m| m.path == canonical)
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
/// `core/num`.
pub fn defining_module(p: Prim) -> &'static str {
    match p {
        Prim::Str => "core/str",
        Prim::Char => "core/char",
        Prim::Bool => "core/bool",
        // A template is a `Str` with holes, and its operations are the
        // numeric-rendering ones, so it shares `core/num`'s module the way
        // every numeric type does.
        Prim::Template => "core/num",
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
        | Prim::F64 => "core/num",
    }
}

// ---------------------------------------------------------------------------
// What each platform's host grants
// ---------------------------------------------------------------------------

/// The path of the one module whose exports vary by platform.
pub const HOST_MODULE: &str = "core/host";

/// One effect `core/host` can grant, and the platforms that grant it.
///
/// **This table is what makes a platform *be* the set of effects its host
/// exports** (`design/ui-reactivity.md` §Targets). A platform that does not
/// grant an effect does not export the names for it, so asking for one is an
/// unresolved name at the line that asked — not a run-time failure, and not a
/// convention.
pub struct HostGrant {
    /// The effect this implements, as `core/effect` or `ui/effect` spells it.
    /// It is the name a reader wrote on the left of the context binding that
    /// failed, so it is what the diagnostic leads with.
    pub effect: &'static str,
    /// The names `core/host` exports for it: the implementation struct, and
    /// the value `main` binds.
    ///
    /// **Both are withheld together**, and that is load-bearing rather than
    /// tidy. A struct with no private field can be constructed by name from
    /// anywhere that can see it, so withholding `net` while exporting
    /// `HostNet` would leave the authority one `Net: host.HostNet {}` away.
    pub exports: &'static [&'static str],
    /// The platforms that grant it. Order follows [`Platform::ALL`], so the
    /// list a diagnostic prints reads the same way the schema does.
    pub platforms: &'static [Platform],
    /// Why the platforms outside that list do not grant it, in one clause. A
    /// refusal that says only "not granted" tells a reader what happened and
    /// not what to do about it.
    pub because: &'static str,
}

use crate::build::buildfile::Platform;

/// Granted everywhere. Written once rather than repeated on five rows.
const EVERY_PLATFORM: &[Platform] = &Platform::ALL;

/// Everything a platform's host can grant, and who grants it.
///
/// The rows in the first group are granted by every platform, so they never
/// produce a diagnostic; they are here so that the table is the whole of
/// `core/host` rather than the interesting half of it, and so that a name
/// added to `host.buri` and forgotten here is caught by
/// `every_host_export_is_in_the_grant_table`.
const HOST_GRANTS: &[HostGrant] = &[
    HostGrant {
        effect: "Alloc",
        exports: &["HostAlloc", "alloc"],
        platforms: EVERY_PLATFORM,
        because: "every platform can allocate",
    },
    HostGrant {
        effect: "Stdout",
        exports: &["HostStdout", "stdout"],
        platforms: EVERY_PLATFORM,
        because: "every platform has somewhere to write a line",
    },
    HostGrant {
        effect: "Stderr",
        exports: &["HostStderr", "stderr"],
        platforms: EVERY_PLATFORM,
        because: "every platform has somewhere to write a line",
    },
    HostGrant {
        effect: "Clock",
        exports: &["HostClock", "clock"],
        platforms: EVERY_PLATFORM,
        because: "every platform can read a clock",
    },
    HostGrant {
        effect: "Rand",
        exports: &["HostRand", "rand"],
        platforms: EVERY_PLATFORM,
        because: "every platform has a source of randomness",
    },
    // `Entropy` is granted everywhere `Rand` is, and the two rows reading alike
    // is the point rather than a copy: what separates the effects is what they
    // *promise*, not where they are available. Every platform this language
    // targets has an operating-system generator behind it — `getrandom(2)` and
    // `getentropy(2)` natively, `crypto.getRandomValues` in every JavaScript
    // runtime and every browser, secure context or not — so there is no
    // platform to withhold it from and no program that has to ask whether its
    // target can keep a secret.
    //
    // What can be missing is the *toolchain*: a runtime archive built without
    // the `crypto` feature has no body for `host.HostEntropy.bytes`, and
    // `backend::cryptography_gap` turns that into a refusal naming the
    // operation. That is the same arrangement `net` has, and it is a different
    // question from this table's.
    HostGrant {
        effect: "Entropy",
        exports: &["HostEntropy", "entropy"],
        platforms: EVERY_PLATFORM,
        because: "every platform has an operating-system generator behind it",
    },
    // `Net` was three platforms until a request stopped blocking. The reason
    // it was withheld from WEB was never authority — a page is the one place
    // that can already reach any origin it is allowed to — it was that
    // `Net.fetch` did not return until the answer arrived, and a page whose
    // one thread is waiting is a frozen page. WEB grants it now, and the
    // callback-shaped `Fetch` that stood in for it is gone.
    HostGrant {
        effect: "Net",
        exports: &["HostNet", "net"],
        platforms: EVERY_PLATFORM,
        because: "every platform can make a request",
    },
    // The two halves that vary. A page has no operating system under it, and
    // nothing but a page has a document over it.
    HostGrant {
        effect: "Fs",
        exports: &["HostFs", "fs"],
        platforms: &[Platform::Linux, Platform::Macos, Platform::Js],
        because: "a page has no filesystem to read",
    },
    HostGrant {
        effect: "Stdin",
        exports: &["HostStdin", "stdin"],
        platforms: &[Platform::Linux, Platform::Macos, Platform::Js],
        because: "a page has no standard input",
    },
    HostGrant {
        effect: "Env",
        exports: &["HostEnv", "env"],
        platforms: &[Platform::Linux, Platform::Macos, Platform::Js],
        because: "a page has no command line and no environment",
    },
    HostGrant {
        effect: "Proc",
        exports: &["HostProc", "proc"],
        platforms: &[Platform::Linux, Platform::Macos, Platform::Js],
        because: "a page has no process to exit; a mounted interface stays live",
    },
    // Granted wherever a program is a program rather than a page — the same
    // three as `Fs`, `Stdin`, `Env` and `Proc`, and withheld from WEB for a
    // reason of the same kind.
    //
    // WEB is the one platform where `parallel` would be *reachable* from a
    // running interface rather than from `main`: a page's own concurrency is
    // its event loop, and every effect a page has either answers instantly
    // (`Ui`, `Watch`) or suspends without holding that loop — which is what let
    // `Net` onto WEB once `fetch` stopped waiting. `parallel` waits by
    // construction — it returns when the last task has finished — so granting it
    // there would put the one shape a page's host is built to avoid back into a
    // page, in the one place a program cannot leave. The note says the same
    // thing from the other end: tasks are what servers are built out of, and
    // the browser's story is the one that lands with them.
    HostGrant {
        effect: "Tasks",
        exports: &["HostTasks", "tasks"],
        platforms: &[Platform::Linux, Platform::Macos, Platform::Js],
        because: "`parallel` returns only when the last task has finished, which freezes a \
                  page; a page's concurrency is its event loop, and the effect that reaches \
                  it lands with the servers",
    },
    // The two halves of being a server. They are granted *together* or not at
    // all, and never on `JS` or `WEB`. Nothing enforces the pairing beyond the
    // two rows below being edited in one commit — and
    // `the_server_effects_are_granted_together_and_never_on_a_page`, which is
    // the assertion that they were.
    //
    // These are the first two rows whose platform list is neither everything,
    // nor the three non-page platforms, nor `WEB`: a native program may hold a
    // port open and a JavaScript one may not, so `LINUX, MACOS` is a shape the
    // table now has. `design/ui-reactivity.md`'s open item about host
    // subsetting among the non-UI platforms is closed by that.
    HostGrant {
        effect: "Listen",
        exports: &["HostListen", "listen"],
        platforms: &[Platform::Linux, Platform::Macos],
        because: "holding a port open is a native program's authority; a page is served \
                  rather than serving, and its host has no way to accept a connection",
    },
    HostGrant {
        effect: "Sockets",
        exports: &["HostSockets", "sockets"],
        platforms: &[Platform::Linux, Platform::Macos],
        because: "writing to an open socket is granted with `Listen`, and a page neither \
                  accepts connections nor holds one to push on",
    },
    HostGrant {
        effect: "Ui",
        exports: &["HostUi", "ui"],
        platforms: &[Platform::Web],
        because: "the reactive graph drives a document, and only a page has one",
    },
    HostGrant {
        effect: "Watch",
        exports: &["HostWatch", "watch"],
        platforms: &[Platform::Web],
        because: "reading the reactive graph is meaningless where nothing writes it",
    },
];

/// The grant a `core/host` export belongs to, or `None` for a name that is not
/// one of them.
pub fn host_grant_of(export: &str) -> Option<&'static HostGrant> {
    HOST_GRANTS.iter().find(|g| g.exports.contains(&export))
}

/// Whether `platform` withholds `export` from `core/host`.
pub fn host_withholds(platform: Platform, export: &str) -> bool {
    host_grant_of(export).is_some_and(|g| !g.platforms.contains(&platform))
}

impl HostGrant {
    /// `LINUX, MACOS, JS` — the platforms that do grant this, as a diagnostic
    /// writes them. Empty for a row no platform grants.
    pub fn platforms_phrase(&self) -> String {
        self.platforms.iter().map(|p| p.proto()).collect::<Vec<_>>().join(", ")
    }

    /// The half of the fix that offers a platform to build for, or nothing.
    ///
    /// **A row may name no platform**, and then there is no target to send a
    /// reader to: "build this for a platform that grants it:" followed by an
    /// empty list is advice nobody can take, and reads like a bug in the
    /// compiler rather than a fact about the effect. The whole clause is
    /// therefore bound rather than only the list, so an ungrantable effect's
    /// fix stops after the one thing that *is* actionable — drop it from the
    /// context — and the `note` carries the reason.
    pub fn elsewhere_clause(&self) -> String {
        if self.platforms.is_empty() {
            return String::new();
        }
        format!(
            ", or build this target for a platform that grants it: {}",
            self.platforms_phrase()
        )
    }
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
    /// The effect, as `core/effect` or `ui/effect` spells it.
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
/// The order is `core/effect`'s declaration order followed by `ui/effect`'s, so
/// the table reads beside the sources it is about.
pub const WRAPPERS: &[Wrapper] = &[
    w("Alloc", "allocate", "core/alloc", "alloc.allocate(ctx, bytes)"),
    w("Stdout", "print", "core/io", "io.print(ctx, text)"),
    w("Stdout", "println", "core/io", "io.println(ctx, text)"),
    w("Stdout", "writeBytes", "core/io", "io.writeBytes(ctx, bytes)"),
    w("Stderr", "eprint", "core/io", "io.eprint(ctx, text)"),
    w("Stderr", "eprintln", "core/io", "io.eprintln(ctx, text)"),
    w("Stdin", "readLine", "core/io", "io.readLine(ctx)"),
    w("Stdin", "readBytes", "core/io", "io.readBytes(ctx, n)"),
    w("Fs", "readFile", "core/fs", "fs.readText(ctx, path)"),
    w("Fs", "writeFile", "core/fs", "fs.writeText(ctx, path, body)"),
    w("Fs", "fileExists", "core/fs", "fs.exists(ctx, path)"),
    w("Fs", "readDir", "core/fs", "fs.listDir(ctx, path)"),
    w("Fs", "readFileBytes", "core/fs", "fs.readBytes(ctx, path)"),
    w("Fs", "writeFileBytes", "core/fs", "fs.writeBytes(ctx, path, body)"),
    w("Fs", "appendFile", "core/fs", "fs.append(ctx, path, body)"),
    w("Fs", "renameFile", "core/fs", "fs.rename(ctx, source, destination)"),
    w("Fs", "removeFile", "core/fs", "fs.remove(ctx, path)"),
    w("Fs", "removeDir", "core/fs", "fs.removeDir(ctx, path)"),
    w("Fs", "makeDir", "core/fs", "fs.makeDir(ctx, path)"),
    w("Fs", "syncFile", "core/fs", "fs.sync(ctx, path)"),
    w("Net", "fetch", "core/net/http", "http.send(ctx, request)"),
    w("Clock", "nowMillis", "core/time", "time.now(ctx)"),
    w("Clock", "sleepMillis", "core/time", "time.sleepMs(ctx, millis)"),
    w("Rand", "nextInt", "core/random", "random.int(ctx, lo, hi)"),
    w("Rand", "nextFloat", "core/random", "random.float(ctx)"),
    // `core/crypto` rather than `core/random`, which is the whole argument
    // `core/crypto`'s header makes: the seeded module and the unguessable one
    // are different promises and a reader should have to name which they meant.
    w("Entropy", "bytes", "core/crypto", "crypto.randomBytes(ctx, count)"),
    w("Env", "variable", "core/env", "env.get(ctx, name)"),
    w("Env", "args", "core/env", "env.args(ctx)"),
    w("Proc", "exitWith", "core/proc", "proc.exit(ctx, code)"),
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


    /// The declared effect methods, read off the two platform sources that may
    /// declare an effect: `(effect, method)`, in declaration order.
    ///
    /// Off the source text rather than off a second list, for
    /// `every_host_export_is_in_the_grant_table`'s reason — a method added to
    /// `core/effect` and forgotten here would be a method with no way to call
    /// it, which is precisely the hole [`WRAPPERS`] exists to close.
    fn declared_effect_methods() -> Vec<(String, String)> {
        let mut out = Vec::new();
        for path in ["core/effect", "ui/effect"] {
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

    /// `core/actor` declares no effect, so it opens no door — and the two
    /// halves of that are asserted rather than left to be noticed.
    ///
    /// It is the one module whose runtime operations are **module functions**
    /// rather than effect methods: nine bodyless `fn`s keyed `actor.*`, each
    /// with the authority in its bound (`C: Tasks`) exactly as `core/list`'s
    /// allocating combinators carry `C: Alloc`. SPEC 10.2 is about reaching
    /// *the outside world* through a context, and a mailbox is neither the
    /// outside world nor something a test would want a second implementation
    /// of — which is also why `core/host/testing` gains nothing for it.
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

    /// The default mailbox is one number, written twice, and the two spellings
    /// must agree.
    ///
    /// `core/actor` enforces the bound — `send` runs the mailbox down when a
    /// post reaches it — and `cli/runtime/rt.rs` refuses to take a message past
    /// it. A bound only one side knew would be a bound the other could not
    /// respect: an actor that filled up would wait on the runtime for a drain
    /// the driver was never going to do.
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
        let module = named(buri, "export let MAILBOX: Int = ");
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
    /// so a method with no wrapper is a method nothing can reach. `Alloc`,
    /// `Proc`, `Listen` and `Sockets` failed this the day the table was
    /// written — six methods of thirty-eight with no door — and it is the
    /// reason `core/proc` and `core/net/server` exist.
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
    /// with methods: `ui/signal`'s `Signal<T>` over `Ui`'s signal ids, and
    /// `core/net/server`'s `Socket` over `Sockets`' socket ids.
    #[test]
    fn every_wrapper_call_leads_with_its_module_or_a_handle() {
        const HANDLES: &[&str] = &["aSignal.", "aSocket."];
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
    /// entries would be two `Alloc`s, and a value of one would not be a value
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

    /// Every name `core/host` exports is in the grant table.
    ///
    /// The table decides what a platform withholds, so a name missing from it
    /// is granted by every platform silently — which is exactly the shape of
    /// the bug this whole mechanism exists to make impossible. Read off the
    /// source text rather than from a second list, so the two cannot drift.
    #[test]
    fn every_host_export_is_in_the_grant_table() {
        let src = source(HOST_MODULE).expect("core/host is a module");
        for line in src.lines() {
            let name = if let Some(rest) = line.strip_prefix("export struct ") {
                rest.split([' ', '{', '(', ';']).next().unwrap_or("")
            } else if let Some(rest) = line.strip_prefix("export let ") {
                rest.split([':', ' ']).next().unwrap_or("")
            } else {
                continue;
            };
            assert!(
                host_grant_of(name).is_some(),
                "`{name}` is exported by core/host and is in no HostGrant row, so every \
                 platform grants it by omission"
            );
        }
    }

    /// And the other direction: a row naming an export that is not there would
    /// withhold nothing.
    ///
    /// A row may name **no platform**, and that is not the error it looks
    /// like. An effect nothing grants is an effect nothing can bind, which is
    /// exactly what a declaration landing ahead of its runtime wants, and the
    /// row is what makes the refusal say why instead of "no such name". No row
    /// is empty today — `Tasks` was, for two waves, and now names three
    /// platforms — so what is asserted here is the *exports*, which a row
    /// naming none of would withhold nothing on every platform.
    #[test]
    fn every_grant_names_exports_that_exist() {
        let src = source(HOST_MODULE).expect("core/host is a module");
        for grant in HOST_GRANTS {
            assert!(!grant.exports.is_empty(), "`{}` withholds nothing", grant.effect);
            for name in grant.exports {
                assert!(
                    src.contains(&format!("export struct {name} "))
                        || src.contains(&format!("export let {name}:")),
                    "`{name}` is in a HostGrant row and core/host does not export it"
                );
            }
        }
    }

    /// `Tasks` is granted where a program is a program, and withheld from the
    /// page — the same three platforms as `Fs`, `Net`, `Stdin`, `Env` and
    /// `Proc`, and both of its names move together.
    ///
    /// The reject corpus can ask for `JS` and `WEB` and no more — a case's
    /// platform comes from its `// PLATFORM:` line, and the two native ones
    /// would want a linker — so *every* platform is proved here, over
    /// `Platform::ALL`, and `reject/tasks_not_granted_on_web` pins what the
    /// refusal reads like.
    ///
    /// The assertion is two-sided, which is what makes it able to fail in both
    /// directions: a platform quietly losing the grant is caught by the first
    /// half, and WEB quietly gaining it by the second.
    #[test]
    fn tasks_is_granted_off_the_page_and_nowhere_else() {
        let grant = host_grant_of("tasks").expect("`tasks` is in the grant table");
        assert_eq!(grant.effect, "Tasks");
        assert_eq!(grant.platforms_phrase(), "LINUX, MACOS, JS");
        for platform in Platform::ALL {
            let granted = platform != Platform::Web;
            for name in ["HostTasks", "tasks"] {
                assert_eq!(
                    !host_withholds(platform, name),
                    granted,
                    "`{}` and `{name}` disagree about the grant",
                    platform.proto()
                );
            }
        }
        // Asserted against `Fs`'s row rather than written out a second time:
        // the claim is that `Tasks` joined the group that varies with the
        // platform, so it moves if that group ever splits.
        let fs = host_grant_of("fs").expect("`fs` is in the grant table");
        assert_eq!(grant.platforms, fs.platforms, "`Tasks` and `Fs` are granted together");
    }

    /// `Listen` and `Sockets` are granted on the two native platforms, never
    /// on a page, and always by *the same* set of platforms as each other.
    ///
    /// The pairing is the invariant worth asserting. "I accept connections"
    /// and "I can write to open sockets" are the two halves of being a server:
    /// a platform granting only the first could accept a websocket upgrade and
    /// then never answer on it, and one granting only the second would hand
    /// out an authority over sockets nothing there can produce. Nothing but
    /// this test enforces it, because the pairing is a claim about two rows
    /// and a row cannot say anything about its neighbour.
    ///
    /// The withholding half is asserted **by platform** rather than by
    /// emptiness, because `JS` and `WEB` are a permanent no and not a
    /// not-yet — a page is served rather than serving. That is the difference
    /// between this row and every other one that varies: `Fs` is absent from
    /// `WEB` because a page has no filesystem *today*, and these two are
    /// absent because a page is not a server.
    ///
    /// It is also the first row whose platform list is neither everything, nor
    /// the three non-page platforms, nor `WEB` alone — so it is asserted
    /// literally rather than against a neighbour's list the way `Tasks`' is.
    #[test]
    fn the_server_effects_are_granted_together_and_never_on_a_page() {
        let listen = host_grant_of("listen").expect("`listen` is in the grant table");
        let sockets = host_grant_of("sockets").expect("`sockets` is in the grant table");
        assert_eq!(listen.effect, "Listen");
        assert_eq!(sockets.effect, "Sockets");
        assert_eq!(
            listen.platforms, sockets.platforms,
            "`Listen` is granted by [{}] and `Sockets` by [{}]; being a server is one \
             authority in two halves and a platform has both or neither",
            listen.platforms_phrase(),
            sockets.platforms_phrase()
        );
        assert_eq!(
            listen.platforms,
            &[Platform::Linux, Platform::Macos],
            "granted by {}",
            listen.platforms_phrase()
        );
        for platform in [Platform::Js, Platform::Web] {
            for name in ["HostListen", "listen", "HostSockets", "sockets"] {
                assert!(
                    host_withholds(platform, name),
                    "`{}` grants `{name}`; a page is served rather than serving, and that is \
                     a permanent row rather than an empty one waiting to be filled",
                    platform.proto()
                );
            }
        }
        for platform in [Platform::Linux, Platform::Macos] {
            for name in ["HostListen", "listen", "HostSockets", "sockets"] {
                assert!(
                    !host_withholds(platform, name),
                    "`{}` withholds `{name}`, which `cli/runtime/net.rs` implements",
                    platform.proto()
                );
            }
        }
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
    /// everybody who does; `Net.fetch` and `Fetch.fetch` are the same word for
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
    /// word half the standard library could want and `Net` is bound beside this
    /// one by design, and a bare `receive` would read as either a socket or a
    /// mailbox depending on what else happened to be in scope.
    #[test]
    fn the_server_effects_claim_no_method_name_another_effect_claims() {
        let mut mine: Vec<(&str, String)> = Vec::new();
        let mut theirs: Vec<(String, String)> = Vec::new();
        for path in ["core/effect/lib.buri", "ui/effect/lib.buri"] {
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

    /// A row with no platform offers no elsewhere, and a row with platforms
    /// offers the sentence it always did.
    ///
    /// **Two rows are empty today**, `Listen` and `Sockets`, and `Tasks` was
    /// the one before them — declared with an empty list, then granted on three
    /// platforms by editing that row. The empty case is still tested against a
    /// row written here rather than against either of the two in the table,
    /// which is deliberate: `elsewhere_clause` is what makes *any* effect
    /// declared ahead of its runtime refuse honestly, so the branch has to hold
    /// when today's empty rows graduate the way `Tasks` did. Deleting it once
    /// the table happened to have no empty row would mean rediscovering the
    /// same "build this for a platform that grants it:" with nothing after the
    /// colon on the day the next one lands.
    #[test]
    fn an_ungrantable_effect_is_not_told_to_build_elsewhere() {
        let ungrantable = HostGrant {
            effect: "Nothing",
            exports: &["HostNothing", "nothing"],
            platforms: &[],
            because: "nothing implements it",
        };
        assert_eq!(ungrantable.elsewhere_clause(), "");
        assert_eq!(ungrantable.platforms_phrase(), "");
        // `fs` and not `net`: B5 moved `Net` into the every-platform group, and
        // a clause naming all four platforms would not show that the sentence
        // is the *subset* a target could be built for instead.
        let fs = host_grant_of("fs").expect("`fs` is in the grant table");
        assert_eq!(
            fs.elsewhere_clause(),
            ", or build this target for a platform that grants it: LINUX, MACOS, JS"
        );
        let tasks = host_grant_of("tasks").expect("`tasks` is in the grant table");
        assert_eq!(tasks.elsewhere_clause(), fs.elsewhere_clause());
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
