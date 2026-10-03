//! The build system: what a repository declares, and what the toolchain does
//! with it.
//!
//! `workspace` is the graph — packages, targets, labels, visibility, tags,
//! platforms — read from the `BUILD.buri` and `REPO.buri` files that
//! `buildfile` types and `textproto` parses. `actions` is what a target's
//! build actually does, `cache` decides whether it has to happen at all, and
//! `regenerate` writes back the fields of a build file that merely restate the
//! sources.
//!
//! `generators` is the other direction: a rule may declare a program the build
//! runs, and what the program answers *becomes* a Buri module, so that
//! `from "//proto/person.proto" import { Person };` resolves to types and
//! codecs that no one had to write down twice. The `.proto` generator is one
//! of those programs — the `proto` tool, written in Buri — and not a path
//! of its own.
//!
//! `link` is the last action in the graph for a native artifact: the C driver
//! the objects are handed to, the `.buri/link/<key>/` directory they are
//! written into, and the manifest that says which of them this build produced
//! and which came out of the cache.
//!
//! `session` is the handle every command that needs a repository opens first:
//! the root, the loaded workspace, the source map, and the diagnostics.
//! `spawn` is the deterministic environment the one action that leaves this
//! process is started in.

pub mod actions;
pub mod buildfile;
pub mod cache;
/// `generators`: a program the build runs, whose output becomes a module. The
/// wire it speaks, the action that runs it, and the store the compiler reads
/// what it produced through.
pub mod generators;
// A repository platform's `js` file: its exports, the host-file check, and the
// module the build bundles it into with the program.
pub mod hosted;
pub mod link;
/// The musl sysroot `cli/build.rs` baked in: the `libc.a`, unwinder and crt
/// objects that finish a hermetic Linux link, and the `Libc` this toolchain's
/// runtime archive was built against. Bytes and accessors only — the flags and
/// the staging are `link`'s.
pub mod musl;
/// The bundled platforms, `native`, `node` and `web`, read from their
/// embedded build files.
pub mod platforms;
pub mod regenerate;
/// Building and caching the runtime archive and musl sysroot for a **cross**
/// target, at `buri build` time, from the sources `runtime_src` embeds.
pub mod runtime_cross;
/// The runtime's sources, embedded so a cross build can re-assemble them.
pub mod runtime_src;
pub mod session;
/// The loaded state of one repository, kept between the questions asked of
/// it: the graph, the files read so far, and the parses of them.
pub mod sources;
/// SHA-256. Its own file, and not a private one, because `cli/build.rs`
/// `#[path]`-includes it: the digests of the blobs the build script embeds are
/// taken where the bytes are written, and a build script cannot use the crate
/// it builds. `cache` re-exports it, so nothing else spells this path.
pub mod sha256;
pub mod spawn;
pub mod textproto;
/// `tool` rules: resolving a tool name, the `main` the build writes for one,
/// and asking one to check, format or generate.
pub mod tools;
pub mod workspace;
