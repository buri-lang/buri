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

pub mod generate;
pub mod intrinsics;
pub mod javascript;

use crate::compiler::backend::{Backend, Emitted, Linker, LinkOptions, Options, Profile};
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

        let release = opts.profile == Profile::Release;
        let out = generate::generate(program, tables, opts.profile, opts.target.platform);
        // Debug builds stay readable: the names are what make a stack trace
        // useful, and `--release` is where size matters.
        let stmts = javascript::minify(out.stmts, &out.roots, release);
        let bytes = javascript::print(&stmts, !release).into_bytes();
        // One unit, always. `Vec<Emitted>` is the shape because the native
        // backends emit one object per codegen unit and relink only the ones
        // that moved; a special case for the backend that has one unit would
        // be a second code path through the build system for the only backend
        // currently covered by tests.
        Ok(vec![Emitted {
            name: String::from("main.mjs"),
            key: crate::build::cache::ActionKey::of(&bytes),
            bytes,
        }])
    }
}

/// "Take element zero."
///
/// A JavaScript artifact is one file, so linking it is a copy. It is a `Linker`
/// rather than a special case in the build system for the same reason `emit`
/// returns a vector: one path through the build, and the backend that has the
/// simplest answer gives the simplest answer rather than being exempt from the
/// question.
pub struct Concatenate;

impl Linker for Concatenate {
    fn name(&self) -> &'static str {
        "js-concat"
    }

    fn version(&self) -> String {
        String::from("1")
    }

    fn link(
        &self,
        units: &[Emitted],
        _unchanged: &[usize],
        out: &std::path::Path,
        _opts: &LinkOptions<'_>,
    ) -> Result<(), Diagnostics> {
        let mut diags = Diagnostics::new();
        let Some(unit) = units.first() else {
            diags.push(Diagnostic::error(
                Span::NONE,
                String::from("internal error: the JavaScript backend emitted no unit"),
            ));
            return Err(diags);
        };
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(out, &unit.bytes) {
            diags.push(
                Diagnostic::error(Span::NONE, format!("cannot write {}: {e}", out.display()))
                    .with_fix("check the directory exists and is writable"),
            );
            return Err(diags);
        }
        Ok(())
    }
}
