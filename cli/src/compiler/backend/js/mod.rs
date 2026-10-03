//! The JavaScript backend, and the first implementor of [`Backend`].
//!
//! `generate` turns the middle end's tree into the JavaScript AST that
//! `javascript` prints and minifies, and `intrinsics` supplies the bodies of
//! the operations the standard library declares without one. `runtime.js` is
//! the hand-written half, next to the code that emits calls into it.
//!
//! There is no `backend-js` cargo feature. This backend is always compiled in:
//! it needs nothing, it is what `driver::host_platform` still returns, and a
//! feature whose only possible value is "on" is a flag nobody should have to
//! read (`design/native/BUILD-AND-WATCH.md` §2).

pub mod crossing;
pub mod generate;
pub mod intrinsics;
pub mod javascript;
pub mod park;

use crate::compiler::backend::{Backend, Emitted, Options, Profile};
use crate::compiler::middle::monomorphize::Program;
use crate::compiler::semantics::types::Tables;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};

/// The hand-written half of the JavaScript backend. Every global in it is
/// `$`-prefixed so the minifier can rename it and drop what a program does not
/// reach.
pub fn runtime_source() -> &'static str {
    include_str!("runtime.js")
}

/// Emits one ES module per program.
///
/// Stateless — the `&mut self` on [`Backend::emit`] is there for the backend
/// that needs it (an LLVM `Context` is not `Sync` and owns everything built
/// inside it), and a signature that fits the hardest implementor is cheaper
/// than one the hardest implementor has to work around.
#[derive(Default)]
pub struct Js;

impl Backend for Js {
    fn name(&self) -> &'static str {
        "js"
    }

    /// A constant, and this is the one backend for which that is honest. The
    /// bytes depend on the emitter, the minifier and `runtime.js`, all three of
    /// which are inside this executable and move only when its version does. An
    /// LLVM backend cannot say this: `llvm-sys` links against whatever
    /// `llvm-config` found at build time, so two `buri` binaries with identical
    /// Rust source can have different LLVM underneath.
    fn identity(&self) -> String {
        String::from("runtime+minifier in-tree")
    }

    /// Which intrinsics this backend has no body for, asked of the program
    /// rather than accumulated as a side effect of a failed emission.
    ///
    /// The distinction is what makes the answer useful: a program can be told
    /// what is missing *before* a backend spends time on it, rather than only
    /// after it has tried.
    fn missing_intrinsics(&self, program: &Program, tables: &Tables) -> Vec<String> {
        let mut missing = generate::check_intrinsics(&generate::unimplemented_intrinsics(
            program, tables,
        ));
        missing.sort();
        missing.dedup();
        missing
    }

    fn emit(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        self.emit_with(program, tables, opts.profile, true)
    }
}

impl Js {
    /// [`Backend::emit`] for a test bundle: the same program, printed without
    /// the minifier's passes. A test bundle is written, run once and thrown
    /// away, so folding, dead-code elimination and mangling buy it nothing and
    /// cost seconds on a large repository. Every artifact a build writes still
    /// goes through [`Backend::emit`].
    pub fn emit_unminified(
        &self,
        program: &Program,
        tables: &Tables,
        profile: Profile,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        self.emit_with(program, tables, profile, false)
    }

    fn emit_with(
        &self,
        program: &Program,
        tables: &Tables,
        profile: Profile,
        minify: bool,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        let missing = self.missing_intrinsics(program, tables);
        if !missing.is_empty() {
            let mut diags = Diagnostics::new();
            diags.push(
                Diagnostic::error(
                    Span::NONE,
                    format!("the runtime has no implementation of {}", missing.join(", ")),
                )
                .with_fix("report it: this is a toolchain bug, not a problem with your program"),
            );
            return Err(diags);
        }

        let release = profile == Profile::Release;
        let out = generate::generate(program, tables, profile);
        // Debug builds stay readable: the names are what make a stack trace
        // useful, and `--release` is where size matters.
        let render = |stmts: Vec<javascript::Stmt>, roots: &[String]| {
            if !minify {
                return javascript::print(&stmts, true).into_bytes();
            }
            let stmts = javascript::minify(stmts, roots, release);
            javascript::print(&stmts, !release).into_bytes()
        };
        // Unit zero is the artifact; the rest are its `core/lazy` chunks, in
        // the order `middle::chunks` numbered them — which is the order
        // `$lazy(n)` asks for them in and the order the build system names the
        // files by. Nearly every program has exactly the one.
        let generate::Output { stmts, roots, chunks, .. } = out;
        let mut emitted = Vec::with_capacity(chunks.len().saturating_add(1));
        let bytes = render(stmts, &roots);
        emitted.push(Emitted {
            name: String::from("main.mjs"),
            key: crate::build::cache::ActionKey::of(&bytes),
            bytes,
        });
        for (n, chunk) in chunks.into_iter().enumerate() {
            let bytes = render(chunk.stmts, &chunk.roots);
            emitted.push(Emitted {
                name: format!("chunk.{n}.mjs"),
                key: crate::build::cache::ActionKey::of(&bytes),
                bytes,
            });
        }
        Ok(emitted)
    }
}
