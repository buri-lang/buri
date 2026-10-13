//! Names a compiler invented, which the stencil extractor refuses.
//!
//! The extractor's half of `backend/stencil`: `cli/build.rs` compiles it, and
//! the crate only under `cfg(test)`, where a test checks the shipped
//! libraries against it.

// ---------------------------------------------------------------------------
// Names a compiler invented
// ---------------------------------------------------------------------------
//
// A stencil is one contiguous run of bytes copied out of the code section and
// patched, and every hole in it names a symbol the *generated C* declared:
// `_JIT_A`, a `buri_rt_*` entry point, a compiler-rt helper such as `__divti3`.
// A compiler that splits a function in two breaks both halves of that sentence
// at once — the body copied out is no longer the whole function, and the branch
// left behind names a symbol the stencil library will never carry.
//
// Both Linux readers already refuse the *visible* form of the split, because
// ELF gives the outlined half its own section: `extract_elf_arm64` refuses any
// section beside `.text`, and `extract_elf_x86` refuses an executable one (it
// has to allow the others, which are where a spilled SSE constant lives).
// Mach-O has no section prefixes, so on `macos-arm64` the outlined half lands
// *inside* `__text` and there is no section to see — which is how clang's
// hot/cold splitter put a `BRANCH26` hole named `st_decref_drop.cold.1` into
// the shipped host library with no error at all. The rule below is the
// container-independent half of the same guard, and it is what makes the three
// targets refuse the same object.

/// The families of name a compiler synthesises for a piece it carved out of a
/// function, and what to call each one in a refusal.
///
/// The rule [`outlining_artifact`] actually enforces is **structural**, not a
/// lookup in this table: a name the source declared is a C identifier, and
/// every segment below is reached across a `.` for the precise reason that a C
/// identifier cannot contain one — a compiler picks a spelling the source could
/// not have written so that its own names can never collide. An unrecognised
/// synthetic name is therefore refused too, and this table only decides how the
/// refusal reads.
///
/// Enumerated from what the toolchains that build this library emit, rather
/// than guessed at:
///
/// | segment | what emits it |
/// |---|---|
/// | `cold` | LLVM's `HotColdSplitting`, which names the outlined half `<fn>.cold.<n>`. Apple's clang runs it at `-O2` and upstream's does not, which is the G2 case; GCC's `-freorder-blocks-and-partition` spells its cold half the same way |
/// | `unlikely` | the cold-section spelling — LLVM's machine-function splitter and GCC both name the section `.text.unlikely.`, and a renamed symbol carries the same word |
/// | `part` | GCC's partial inlining, `<fn>.part.<n>` |
/// | `isra` | GCC's interprocedural scalar replacement of aggregates |
/// | `constprop` | GCC's interprocedural constant-propagation clone |
/// | `specialized` | LLVM's `FunctionSpecialization` |
/// | `resume`, `destroy`, `cleanup` | the three funclets `CoroSplit` makes of a coroutine |
/// | `llvm` | LLVM's rename of an internalised or promoted symbol, `<fn>.llvm.<hash>` |
/// | `lto_priv` | GCC's LTO privatisation |
///
/// `OUTLINED_FUNCTION_<n>` — the MachineOutliner's, on by default at `-Oz` for
/// AArch64 — is the one that *is* a valid C identifier, so it is named
/// separately in [`outlining_artifact`] rather than here.
const ARTIFACT_SEGMENTS: &[(&str, &str)] = &[
    ("cold", "a hot/cold split of it"),
    ("unlikely", "a cold-section split of it"),
    ("part", "a partial inline of it"),
    ("isra", "an argument-shape clone of it"),
    ("constprop", "a constant-propagation clone of it"),
    ("specialized", "a specialisation of it"),
    ("resume", "a coroutine funclet of it"),
    ("destroy", "a coroutine funclet of it"),
    ("cleanup", "a coroutine funclet of it"),
    ("llvm", "an internalising rename of it"),
    ("lto_priv", "an LTO privatisation of it"),
];

/// Whether `name` is something the generated C could have written down.
///
/// A C identifier, in the only sense that matters here: ASCII letters, digits
/// and underscores, not starting with a digit. Every name this library binds a
/// hole to is one — the `_JIT_*` holes, the `buri_rt_*` entry points, the
/// handful of compiler-rt helpers — and no name a compiler synthesises for
/// itself is.
fn is_c_identifier(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What `name` is, when it is a name a compiler invented rather than one the
/// source declared — and `None` when the source could have declared it.
///
/// Two names are deliberately *not* artifacts:
///
///  * the **empty** name, which is an ELF section symbol. A relocation against
///    one is how a spilled constant is reached on x86-64, and `x86.rs` handles
///    it as a constant rather than as a hole.
///  * a name beginning `.L`, an assembler-local label. Those are the labels on
///    the spilled constants themselves — `elfobj.rs` keeps them in the table
///    for exactly that reason — and they are data, never code.
pub fn outlining_artifact(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.starts_with(".L") {
        return None;
    }
    if name.starts_with("OUTLINED_FUNCTION_") {
        return Some("a machine-outliner fragment");
    }
    if is_c_identifier(name) {
        return None;
    }
    // The first segment is the function the compiler carved this out of, and
    // the *suffix* segments are what say what the carving was. An unrecognised
    // one is still refused: the `.` alone is proof the name was not written in
    // the source.
    let known = |seg: &str| ARTIFACT_SEGMENTS.iter().find(|(s, _)| *s == seg);
    Some(
        name.split('.')
            .skip(1)
            .find_map(|seg| known(seg).map(|(_, w)| *w))
            .unwrap_or("a name the compiler invented"),
    )
}

/// The refusal a hole whose target the compiler invented earns, or `None` when
/// the name is one the source declared.
///
/// Shared by both finishers — `extract.rs`'s `finish_arm64` for the two AArch64
/// containers and `x86.rs`'s `one` — so that the three targets refuse the same
/// object in the same words. One sentence in one place is the point: the wound
/// this guards against was one target erroring on the section a split produced
/// while another accepted the very same split in silence.
pub fn refuse_hole_name(stencil: &str, hole: &str) -> Option<String> {
    outlining_artifact(hole).map(|what| {
        format!(
            "{stencil}: a stencil reaches {hole}, which is {what}; every hole must be a \
             symbol the source declared"
        )
    })
}

/// The same refusal for a stencil's *own* name.
///
/// The outlined half is a function too, and on Mach-O it is a function in
/// `__text` whose name begins `_st_`, so it walks straight into the extractor's
/// `st_*` filter and becomes a stencil of its own. That is the branch of the
/// wound that survives a compiler which resolved the branch without leaving a
/// relocation behind: no hole to refuse, and a key in the library that is half
/// of a function.
pub fn refuse_stencil_name(stencil: &str) -> Option<String> {
    outlining_artifact(stencil).map(|what| {
        format!(
            "{stencil} is not a stencil, it is {what}; every stencil must be a function \
             the source declared"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::super::runtime;
    use super::*;

    /// The name that was actually in the shipped `macos-arm64` library, and the
    /// one the ELF targets errored on. A synthetic hole rather than a compiled
    /// fixture: inducing the split needs Apple's clang specifically — upstream
    /// clang 21 does not run `HotColdSplitting` at `-O2` at all — so a fixture
    /// would compile to nothing on the machines that do not reproduce it and
    /// would assert nothing there. What the extractor sees of that split is
    /// exactly this string, and this is the check it now fails.
    #[test]
    fn the_hot_cold_split_that_shipped_is_refused() {
        let e = refuse_hole_name("st_decref_drop", "st_decref_drop.cold.1")
            .expect("a hole named after a hot/cold split must be refused");
        assert!(e.contains("st_decref_drop.cold.1"), "{e}");
        assert!(e.contains("a hot/cold split of it"), "{e}");
        let rule = "every hole must be a symbol the source declared";
        assert!(e.contains(rule), "{e}");
        // And the same function reached as a stencil of its own, which is the
        // shape a compiler that left no relocation behind would produce.
        let e = refuse_stencil_name("st_decref_drop.cold.1")
            .expect("a stencil that is half of one must be refused");
        let rule = "every stencil must be a function the source declared";
        assert!(e.contains(rule), "{e}");
    }

    /// One assertion per family the table names, plus the two spellings that
    /// carry no `.` — the outliner's, and an ordinary name, which must pass.
    #[test]
    fn every_outlining_family_is_named_rather_than_lumped_together() {
        let cases = [
            ("st_x.cold", "a hot/cold split of it"),
            ("st_x.cold.3", "a hot/cold split of it"),
            ("st_x.unlikely.0", "a cold-section split of it"),
            ("st_x.part.0", "a partial inline of it"),
            ("st_x.isra.0", "an argument-shape clone of it"),
            ("st_x.constprop.0", "a constant-propagation clone of it"),
            ("st_x.specialized.1", "a specialisation of it"),
            ("st_x.resume", "a coroutine funclet of it"),
            ("st_x.destroy", "a coroutine funclet of it"),
            ("st_x.cleanup", "a coroutine funclet of it"),
            ("st_x.llvm.98765", "an internalising rename of it"),
            ("st_x.lto_priv.0", "an LTO privatisation of it"),
            ("OUTLINED_FUNCTION_0", "a machine-outliner fragment"),
            // Not in the table, and still refused: the `.` is the proof.
            ("st_x.whatever_comes_next.2", "a name the compiler invented"),
        ];
        for (name, what) in cases {
            assert_eq!(outlining_artifact(name), Some(what), "{name}");
        }
    }

    /// The other half of the rule, which is the half that would break the
    /// build if it were wrong: every name a hole is legitimately bound to must
    /// pass. The four kinds are the emitter's holes, the runtime entry points,
    /// compiler-rt's helpers, and the two ELF spellings `x86.rs` relocates
    /// against for a spilled constant.
    #[test]
    fn the_names_holes_are_really_bound_to_all_pass() {
        for name in [
            "JIT_A",
            "JIT_CONT",
            "JIT_CONT0",
            "_JIT_A",
            runtime::ALLOC,
            runtime::I128_DIVMOD,
            runtime::TEST_FAIL_COMPARED,
            "memcpy",
            "__divti3",
            "__udivti3",
            // An ELF section symbol, and an assembler-local label on a spilled
            // constant pool: both reach `x86.rs`'s spilled path and neither is
            // an outlined anything.
            "",
            ".LCPI0_0",
            ".Lswitch.table.st_x",
        ] {
            assert_eq!(outlining_artifact(name), None, "{name}");
            assert!(refuse_hole_name("st_x", name).is_none(), "{name}");
        }
    }
}
