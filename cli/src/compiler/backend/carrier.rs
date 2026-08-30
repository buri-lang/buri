//! The **carrier entry ABI**: the one C signature by which something outside a
//! Buri artifact enters Buri code.
//!
//! `design/native/CODEGEN-*.md` describe how a Buri function is *called by
//! another Buri function*: frame-threaded on the stencil backend, `fastcc` on
//! the LLVM one. Neither is a convention a caller outside the artifact can
//! spell. Until this slice nothing needed one — the only thing that entered
//! Buri code was `main`, and `main` is emitted by the same backend that emits
//! the body it calls.
//!
//! A **carrier** is not. `cli/runtime/rt.rs` runs Buri code on an OS thread
//! from a pool, and what a pool hands a thread is a C function pointer. So
//! there has to be a door, and this module is the one place its shape is
//! written down.
//!
//! # The shape
//!
//! ```text
//!   void entry(void *state, void *out);
//! ```
//!
//! Two pointers and no answer, at `ccc`, on every target. `state` is the
//! caller's own record, opaque to the runtime and to this module; `out` is
//! where the callee's return area is copied, or null when the callee returns
//! nothing.
//!
//! **It is deliberately the same two words as `cli/runtime/list.rs`'s
//! `StepEntry` minus its element**, because it is the same idea one level up:
//! a generated C function that closes over everything about the callee the
//! runtime cannot say. `list.rs`'s header states the constraint both answer —
//! *"a Buri closure is `{ code, env }` where `code` is a thunk whose signature
//! is the flattened one of its own element type, so calling one from C would
//! mean synthesizing a call whose parameter list depends on `T`"*.
//!
//! # Why the signature is fixed **before** the caller exists
//!
//! `cli/runtime/rt.rs` §1 argues the opposite for the task table: an exported
//! symbol is a contract, and writing one before a backend has a call to emit
//! is a guess. That argument is about a *runtime* symbol, whose shape only the
//! runtime knows. This one is the mirror image — the symbol is the
//! **backend's**, two of them emit it, and if they do not agree byte for byte
//! then whichever one the scheduler happens to call first decides what the
//! other one meant. Fixing it here, once, with a test that reads it back out
//! of both emitters, is what stops that.
//!
//! `state` is passed and not read today, and that is the half a later slice
//! fills in: what goes in the record is the *call site's* business (D2 already
//! has two of them, one per backend, and they are deliberately different
//! shapes), and the roots this slice generates a door for take no arguments.
//! The parameter is here rather than added later because adding a parameter
//! later moves every call site there will be by then.
//!
//! # What is behind the door, per backend
//!
//! Not the same thing, and it should not be:
//!
//!  * **stencil** — generated Buri code runs on a Buri data stack that no
//!    kernel guards, and there has been exactly one of them: `asm.rs`'s 64 MiB
//!    `__bss` block. A carrier cannot use it, so the thunk asks
//!    [`STACK_ACQUIRE`] for its own and puts the answer in the frame-pointer
//!    register before it calls the body;
//!  * **LLVM** — a frame there *is* the machine's, and the machine stack of a
//!    carrier thread is the OS's and already guarded. The thunk is a `ccc`
//!    wrapper in front of the `fastcc` body and asks for no stack at all.
//!
//! The signature does not know which, which is the point of having one.

/// One word of the C signature.
///
/// Every word of this ABI is a pointer; the enum exists so that
/// [`Signature::render`] is a match rather than a format string, and so that a
/// widening — an `i32` status, say — is a variant rather than a second
/// renderer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Word {
    /// An untyped address: `void *` in C, `ptr` in LLVM's opaque-pointer IR,
    /// and one integer argument register on both machines this compiles for.
    Ptr,
}

impl Word {
    /// The word's name in the rendered signature.
    pub fn name(self) -> &'static str {
        match self {
            Word::Ptr => "ptr",
        }
    }
}

/// A C signature, as much of one as this ABI needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Signature {
    /// The parameters, in order, one integer argument register each.
    pub params: &'static [Word],
    /// `None` is `void`. Nothing here answers a value: the callee's return
    /// area goes through `out`, because a Buri return can be wider than a
    /// register and a signature that was sometimes wide and sometimes not
    /// would be two signatures.
    pub ret: Option<Word>,
}

impl Signature {
    /// The signature as bytes, in one canonical spelling.
    ///
    /// This is what the two backends are compared on. Bytes rather than a
    /// structural `==` because the comparison is the *test's* whole content: a
    /// rendering each backend derives from what it actually emitted, read back
    /// and diffed, says more than two copies of the same constant being equal
    /// to themselves.
    pub fn render(&self) -> Vec<u8> {
        let mut out = String::from(match self.ret {
            None => "void",
            Some(w) => w.name(),
        });
        out.push('(');
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(p.name());
        }
        out.push(')');
        out.into_bytes()
    }
}

/// **The carrier entry signature.** `void(ptr, ptr)`: the caller's record, and
/// where to put the answer.
pub const ENTRY: Signature = Signature { params: &[Word::Ptr, Word::Ptr], ret: None };

/// Where the caller's record arrives: argument register 0.
pub const STATE: usize = 0;

/// Where the return area is to be copied: argument register 1.
pub const OUT: usize = 1;

/// The runtime entry a stencil thunk takes its Buri data stack from.
///
/// `cli/runtime/memory.rs`. Answers a 64 MiB block with its own 1 MiB
/// `PROT_NONE` guard, belonging to the calling carrier alone.
pub const STACK_ACQUIRE: &str = "buri_rt_stack_acquire";

/// The runtime entry that gives one back. `cli/runtime/memory.rs`.
pub const STACK_RELEASE: &str = "buri_rt_stack_release";

/// The symbol the door to a program's root is emitted under.
///
/// A fixed name rather than one derived from the root's own mangled symbol,
/// for the reason `asm.rs`'s shims have fixed names: a program has exactly one
/// root, something outside the artifact is what looks this up, and a name that
/// depended on the module the root happened to be written in would be a name
/// nothing outside could predict. The `$` is `asm::STACK_SYMBOL`'s guarantee —
/// no Buri path contains one, so no mangled symbol can collide.
pub const MAIN_ENTRY: &str = "buri$carrier$main";

/// The symbol the door to test block `i` is emitted under.
///
/// Test binaries have no root, so the index is the name — the same index
/// `buri_rt_test_enter` already identifies a block by.
pub fn test_entry(i: usize) -> String {
    format!("buri$carrier$test{i}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signature is two pointers and no answer, and this is the byte
    /// string both backends are held to.
    #[test]
    fn the_entry_signature_is_two_pointers_and_no_answer() {
        assert_eq!(ENTRY.params.len(), 2);
        assert_eq!(ENTRY.ret, None);
        assert_eq!(ENTRY.render(), b"void(ptr,ptr)".to_vec());
        // The two names are positions, and a backend reads its argument
        // registers by them.
        assert_eq!(STATE, 0);
        assert_eq!(OUT, 1);
        assert!(STATE < ENTRY.params.len() && OUT < ENTRY.params.len());
    }

    /// The renderer distinguishes what it is meant to distinguish: a different
    /// arity and a different return are different bytes.
    #[test]
    fn a_different_signature_renders_differently() {
        let one = Signature { params: &[Word::Ptr], ret: None };
        let answering = Signature { params: &[Word::Ptr, Word::Ptr], ret: Some(Word::Ptr) };
        assert_ne!(one.render(), ENTRY.render());
        assert_ne!(answering.render(), ENTRY.render());
        assert_eq!(one.render(), b"void(ptr)".to_vec());
        assert_eq!(answering.render(), b"ptr(ptr,ptr)".to_vec());
    }

    /// Every symbol this ABI names is one no Buri path can spell.
    #[test]
    fn the_door_symbols_cannot_collide_with_a_mangled_one() {
        assert!(MAIN_ENTRY.contains('$'));
        assert!(test_entry(0).contains('$'));
        assert_eq!(test_entry(7), "buri$carrier$test7");
        assert_ne!(test_entry(0), test_entry(1));
        assert_ne!(MAIN_ENTRY, test_entry(0));
        // The two runtime entries are `buri_rt_`, the one prefix every runtime
        // symbol has (`DECISIONS.md`).
        assert!(STACK_ACQUIRE.starts_with("buri_rt_"));
        assert!(STACK_RELEASE.starts_with("buri_rt_"));
    }
}
