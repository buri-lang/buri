//! The embedded standard library.
//!
//! `core/*` ships with the toolchain and is never listed in a `dependencies`.
//! It is available to every target, and the purity tiers in SPEC 11.1 govern
//! what any given import of it can do.
//!
//! There is no directory layout on disk here — the library is the one table of
//! `include_str!`s below — so `core/effect/lib.buri` is a name rather than a
//! place. It is spelled that way because every import names a file, and the
//! standard library is not the one corner of the language where that is untrue.
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
        ..m("core/option/lib.buri", include_str!("sources/option.buri"))
    },
    StdModule {
        prelude: &["Result"],
        eager: true,
        ..m("core/result/lib.buri", include_str!("sources/result.buri"))
    },
    StdModule {
        prelude: &["Order", "Eq", "Ord", "Show", "Hash"],
        eager: true,
        ..m("core/order/lib.buri", include_str!("sources/order.buri"))
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
        ..m("core/num/lib.buri", include_str!("sources/num.buri"))
    },
    // `[T]`, `Str`, `Char` and `Bool` need their defining modules present in a
    // program that never names them, because a method needs no import
    // (SPEC 6.7.3): `xs.map(...)` resolves in `core/list` and `s.trim()` in
    // `core/str`. Everything below declares methods only on its *own* types,
    // which cannot exist in a program that did not import it — so it loads on
    // import, and a repository does not pay to parse `core/crypto` to compile
    // a program that has never heard of it.
    StdModule { eager: true, ..m("core/list/lib.buri", include_str!("sources/list.buri")) },
    StdModule { eager: true, ..m("core/str/lib.buri", include_str!("sources/str.buri")) },
    StdModule { eager: true, ..m("core/char/lib.buri", include_str!("sources/char.buri")) },
    StdModule { eager: true, ..m("core/bool/lib.buri", include_str!("sources/bool.buri")) },
    m("core/queue/lib.buri", include_str!("sources/queue.buri")),
    m("core/bitset/lib.buri", include_str!("sources/bitset.buri")),
    m("core/json/lib.buri", include_str!("sources/json.buri")),
    m("core/proto/lib.buri", include_str!("sources/proto.buri")),
    m("core/map/lib.buri", include_str!("sources/map.buri")),
    m("core/set/lib.buri", include_str!("sources/set.buri")),
    m("core/ordmap/lib.buri", include_str!("sources/ordmap.buri")),
    m("core/ordset/lib.buri", include_str!("sources/ordset.buri")),
    m("core/bytes/lib.buri", include_str!("sources/bytes.buri")),
    m("core/crypto/lib.buri", include_str!("sources/crypto.buri")),
    m("core/math/lib.buri", include_str!("sources/math.buri")),
    m("core/simd/lib.buri", include_str!("sources/simd.buri")),
    m("core/bits/lib.buri", include_str!("sources/bits.buri")),
    StdModule { platform: true, ..m("core/effect/lib.buri", include_str!("sources/effect.buri")) },
    StdModule { platform: true, ..m("core/host/lib.buri", include_str!("sources/host.buri")) },
    // `core/host`'s surface for a test source: the same names, called rather
    // than referred to, so each call is a fresh runner-side handle. It is a
    // *different path* rather than a second export list on `core/host`, and
    // that is what carries its import rule — `is_test_only_path` sees the
    // `testing` segment, and `HOST_MODULE`'s `Role::Entry` gate keys on the
    // exact path `core/host/lib.buri` and so does not catch this one.
    StdModule {
        platform: true,
        ..m("core/host/testing/lib.buri", include_str!("sources/host_testing.buri"))
    },
    // Not a platform module, deliberately. It *implements* `Alloc` rather than
    // declaring it, and `Alloc` is the one effect whose implementation carries
    // no authority — a `Region` is a number, so a library that builds its own
    // allocator has been granted nothing (SPEC 10.5). That is why this is
    // importable anywhere and `core/host` is not.
    m("core/alloc/lib.buri", include_str!("sources/alloc.buri")),
    m("core/io/lib.buri", include_str!("sources/io.buri")),
    m("core/fs/lib.buri", include_str!("sources/fs.buri")),
    m("core/env/lib.buri", include_str!("sources/env.buri")),
    m("core/time/lib.buri", include_str!("sources/time.buri")),
    m("core/date/lib.buri", include_str!("sources/date.buri")),
    m("core/random/lib.buri", include_str!("sources/random.buri")),
    m("core/net/http/lib.buri", include_str!("sources/http.buri")),
    StdModule {
        platform: true,
        ..m("core/testing/assert/lib.buri", include_str!("sources/assert.buri"))
    },
    StdModule {
        platform: true,
        ..m("core/testing/context/lib.buri", include_str!("sources/testing_context.buri"))
    },
    // `ui/*`. A user interface is not one of the deliberately small
    // essentials, and its vocabulary is large, so it gets its own reserved
    // root rather than growing `core/`. Only `ui/effect` is a platform module —
    // it declares the effects; everything else is ordinary Buri over inert
    // handles and could move to a real library once external repositories
    // land.
    StdModule { platform: true, ..m("ui/effect/lib.buri", include_str!("sources/ui_effect.buri")) },
    m("ui/signal/lib.buri", include_str!("sources/ui_signal.buri")),
    m("ui/prop/lib.buri", include_str!("sources/ui_prop.buri")),
    m("ui/style/lib.buri", include_str!("sources/ui_style.buri")),
    m("ui/theme/lib.buri", include_str!("sources/ui_theme.buri")),
    m("ui/node/lib.buri", include_str!("sources/ui_node.buri")),
    m("ui/testing/lib.buri", include_str!("sources/ui_testing.buri")),
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
/// `"core/nope/lib.buri"` answers `true`, so that naming a module the standard library
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

pub fn find(path: &str) -> Option<&'static StdModule> {
    MODULES.iter().find(|m| m.path == path)
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
        Prim::Str => "core/str/lib.buri",
        Prim::Char => "core/char/lib.buri",
        Prim::Bool => "core/bool/lib.buri",
        // A template is a `Str` with holes, and its operations are the
        // numeric-rendering ones, so it shares `core/num`'s module the way
        // every numeric type does.
        Prim::Template => "core/num/lib.buri",
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
        | Prim::F64 => "core/num/lib.buri",
    }
}

// ---------------------------------------------------------------------------
// What each platform's host grants
// ---------------------------------------------------------------------------

/// The path of the one module whose exports vary by platform.
pub const HOST_MODULE: &str = "core/host/lib.buri";

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
    // Declared, and granted by nobody. `Tasks` has no scheduler behind it on
    // any platform yet, so the honest row is the empty one: the names exist,
    // the signature is fixed, and asking for them is refused everywhere with
    // the reason. Granting it later is an edit to this line and to nothing
    // else, which is the whole point of landing the declaration first.
    HostGrant {
        effect: "Tasks",
        exports: &["HostTasks", "tasks"],
        platforms: &[],
        because: "no platform runs tasks yet; `Tasks` is declared so that its signature is \
                  fixed, and the scheduler that answers `parallel` lands with the servers",
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

#[cfg(test)]
mod tests {
    use super::*;

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
    /// A row may name **no platform** — `Tasks` is one — and that is not the
    /// error it looks like. An effect nothing grants is an effect nothing can
    /// bind, which is exactly what a declaration landing ahead of its runtime
    /// wants, and the row is what makes the refusal say why instead of "no
    /// such name". An empty *exports* list is still an error, because a row
    /// that names no export withholds nothing on every platform.
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

    /// `Tasks` is granted by no platform, and every platform withholds both of
    /// its names.
    ///
    /// The reject corpus can ask for `JS` and `WEB` and no more — a case's
    /// platform comes from its `// PLATFORM:` line, and the two native ones
    /// would want a linker — so *every* platform is proved here, over
    /// `Platform::ALL`, and the reject fixtures pin what the refusal reads
    /// like.
    #[test]
    fn tasks_is_granted_by_no_platform() {
        let grant = host_grant_of("tasks").expect("`tasks` is in the grant table");
        assert_eq!(grant.effect, "Tasks");
        assert!(grant.platforms.is_empty(), "`Tasks` is granted by {}", grant.platforms_phrase());
        for platform in Platform::ALL {
            for name in ["HostTasks", "tasks"] {
                assert!(
                    host_withholds(platform, name),
                    "`{}` grants `{name}`, which no platform implements",
                    platform.proto()
                );
            }
        }
    }

    /// A row with no platform offers no elsewhere, and a row with platforms
    /// offers the sentence it always did.
    #[test]
    fn an_ungrantable_effect_is_not_told_to_build_elsewhere() {
        let tasks = host_grant_of("tasks").expect("`tasks` is in the grant table");
        assert_eq!(tasks.elsewhere_clause(), "");
        // `fs` and not `net`: B5 moved `Net` into the every-platform group, and
        // a clause naming all four platforms would not show that the sentence
        // is the *subset* a target could be built for instead.
        let fs = host_grant_of("fs").expect("`fs` is in the grant table");
        assert_eq!(
            fs.elsewhere_clause(),
            ", or build this target for a platform that grants it: LINUX, MACOS, JS"
        );
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
