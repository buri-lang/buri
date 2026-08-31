//! The machine-stack switch: `buri_rt_task_switch`, and the frame a task
//! starts life with.
//!
//! Design: `design/native` track B, slice B9 — *"replace the carrier thread
//! with a stack switch"*. This file is the three hand-written blocks the row
//! asks for and nothing else; who switches, and when, is `rt.rs`'s.
//!
//! ## 1. Two stacks, and only one of them is switched here
//!
//! `reports/survey-runtime.md` §1.2 states the fact this slice turns on:
//!
//! > A suspension design here has **two stacks to account for, not one**.
//!
//! * The **Buri data stack** is the one `middle::layout` addresses frames on.
//!   The frame-threaded backend threads a pointer to it through `x0` (`rdi`),
//!   it grows *upward*, and since B7 a carrier gets one of its own from
//!   `memory::buri_rt_stack_acquire`.
//! * The **machine stack** carries the return-address chain — sixteen bytes
//!   per Buri call on the frame-threaded backend, a whole frame per call under
//!   LLVM — plus every Rust frame of this runtime.
//!
//! **This file switches the second, and the first comes along for free.** The
//! Buri stack is reached through a register and through values spilled into
//! machine frames, so a task that is switched out keeps its `x0` exactly where
//! it kept every other live value: on the machine stack that was just set
//! aside. What the two stacks *do* need is that they are both the task's own,
//! which is why B7's free list moved off the thread and onto the task in this
//! slice — see `memory::buri_rt_stack_acquire`.
//!
//! ## 2. The ABI
//!
//! ```c
//! void buri_rt_task_switch(void **from, void *to);
//! ```
//!
//! Save every callee-saved register of the platform's C ABI on the current
//! machine stack, write the resulting stack pointer through `from`, take `to`
//! as the new stack pointer, restore the same registers from it and return.
//! The call therefore returns **on the other stack**, and returns *here* again
//! only when somebody switches back — which is `swapcontext`'s shape, minus
//! the signal mask nobody wants and the system call it costs.
//!
//! Everything else is a caller-saved register, and a `extern "C"` call is
//! exactly the point at which the compiler has already assumed those are gone.
//! That is the whole reason this is nineteen instructions rather than a
//! register file: **the C ABI has already done most of the work at the call
//! site.**
//!
//! ```c
//! void buri_rt_task_launch(void);   /* never called; only ever returned into */
//! ```
//!
//! A task that has never run has no frame to restore, so [`prepare`] writes
//! one: zeroes, the task's address in the first callee-saved register, and
//! `buri_rt_task_launch` where the return address goes. The first switch into
//! a new task therefore "returns" into the launch pad, which moves that
//! register into the first argument register and calls
//! [`buri_rt_task_main`][crate::rt::buri_rt_task_main]. **A new task and a
//! resumed one take the same path**, which is one code path fewer to be wrong
//! in and the reason `prepare` builds the frame the switch would have written.
//!
//! ## 3. Why hand-written, and why three of them
//!
//! `swapcontext` itself is refused for three reasons and each is on its own
//! sufficient: it is **not in this runtime's dependency set** (closed by an
//! exact list — `manifest.toml`), it is **deprecated and absent on Darwin's
//! supported surface**, and where it does exist it makes a `sigprocmask`
//! system call per switch, which is the entire cost of a switch several times
//! over.
//!
//! Three files rather than one because the callee-saved set is a property of
//! the ABI and the symbol spelling is a property of the object format, and
//! there are three live combinations: `StencilTarget::ALL` is `MacosArm64`,
//! `LinuxArm64` and `LinuxX86_64`, and the two AArch64 files are the same
//! instructions under ELF's directives and Darwin's underscore. Only the
//! host's is ever compiled here — `cli/build.rs` builds the archive for the
//! host triple alone — so the other two are held up by
//! `the_three_switch_blocks_assemble_for_their_targets` in
//! `cli/tests/native/runtime.rs`, which hands each of them to `cc -target` and
//! reads the symbols back out. That test is the cross-target half of this
//! slice and it is the reason the blocks are `.s` files rather than string
//! literals.

// The one file the host's `(arch, os)` names. A pair with no file is a
// compile error rather than a silent fallback: there is no portable stack
// switch to fall back *to*, and a runtime that quietly had no scheduler would
// present as a hang.
#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
core::arch::global_asm!(include_str!("switch_macos_arm64.s"));
#[cfg(all(target_arch = "aarch64", not(target_os = "macos")))]
core::arch::global_asm!(include_str!("switch_linux_arm64.s"));
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(include_str!("switch_linux_x86_64.s"));

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
compile_error!(
    "the buri runtime has no machine-stack switch for this architecture; \
     cli/runtime/switch.rs names the three it has"
);

unsafe extern "C" {
    /// Save this stack, take that one. See §2.
    ///
    /// # Safety
    /// `from` is a writable word this thread owns, and `to` is either a stack
    /// pointer a previous call to this function wrote through *its* `from`, or
    /// a frame [`prepare`] built. The stack `to` names must not be running on
    /// another thread, and the memory under it must stay mapped for as long as
    /// the context may be resumed.
    pub(crate) fn buri_rt_task_switch(from: *mut *mut u8, to: *mut u8);

    /// The launch pad. **Never called** — its address is planted by [`prepare`]
    /// where a return address goes, and the switch's own `ret` reaches it.
    pub(crate) fn buri_rt_task_launch();
}

/// How many words of the saved frame belong to the callee-saved set.
///
/// AArch64: `x19`–`x28`, `x29`, `x30`, then `d8`–`d15`, which is twenty words
/// and a 16-byte-aligned 160-byte frame. x86-64 SysV: `r15`, `r14`, `r13`,
/// `r12`, `rbx`, `rbp` and the return address `ret` will take, which is seven.
#[cfg(target_arch = "aarch64")]
pub(crate) const FRAME_WORDS: usize = 20;
#[cfg(target_arch = "x86_64")]
pub(crate) const FRAME_WORDS: usize = 7;

/// Where the argument sits in the saved frame — the first callee-saved
/// register, which [`buri_rt_task_launch`] moves into the first argument
/// register.
#[cfg(target_arch = "aarch64")]
const ARG_WORD: usize = 0; // x19
#[cfg(target_arch = "x86_64")]
const ARG_WORD: usize = 4; // rbx

/// Where the address the switch's `ret` will take sits.
#[cfg(target_arch = "aarch64")]
const RETURN_WORD: usize = 11; // x30, the link register
#[cfg(target_arch = "x86_64")]
const RETURN_WORD: usize = 6; // the return address itself

/// Build the frame a never-yet-run task is resumed from, and answer the stack
/// pointer that names it.
///
/// `top` is the high end of the task's machine stack — the stack grows down
/// from there — and must be 16-byte aligned, which is what every mapping this
/// runtime makes is. `arg` is handed to
/// [`buri_rt_task_main`][crate::rt::buri_rt_task_main] as its only argument.
///
/// The frame is **zeroed apart from the two words that matter**, and the frame
/// pointer's word is one of the zeroes: a debugger or a profiler that walks
/// frames off a task stack stops at its base rather than wandering into
/// whatever the previous tenant left.
///
/// # Safety
/// `[top - FRAME_WORDS * 8, top)` is writable and is not the live part of any
/// other stack.
pub(crate) unsafe fn prepare(top: *mut u8, arg: *mut u8) -> *mut u8 {
    debug_assert!((top as usize).is_multiple_of(16), "a task stack top must be 16-byte aligned");
    let sp = top.wrapping_sub(FRAME_WORDS * size_of::<usize>()).cast::<usize>();
    // SAFETY: the caller promises the range is writable and unshared.
    unsafe {
        sp.write_bytes(0, FRAME_WORDS);
        sp.add(ARG_WORD).write(arg as usize);
        sp.add(RETURN_WORD).write(buri_rt_task_launch as *const () as usize);
    }
    sp.cast()
}

/// Replace the address a prepared frame will `ret` into.
///
/// **For measurements and for this file's own cases only**, which is why it is
/// `#[cfg(test)]`: a real task is reached through [`buri_rt_task_launch`],
/// which is what moves the argument into place, and a caller that planted its
/// own entry would be testing a path no task takes. What it buys is that a
/// case about the *switch* need not also drag in `rt.rs`'s task table.
///
/// # Safety
/// `sp` came from [`prepare`] and no context is running on that frame.
#[cfg(test)]
pub(crate) unsafe fn plant_return(sp: *mut u8, target: *const ()) {
    // SAFETY: the caller's promise; the word is inside the frame `prepare`
    // wrote.
    unsafe { sp.cast::<usize>().add(RETURN_WORD).write(target as usize) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// The probe below is one `extern "C" fn` with no argument, so everything
    /// it needs is a static and only one case may use it at a time.
    static ONE: Mutex<()> = Mutex::new(());
    /// The address of the word the caller's context was saved through.
    static HOME: AtomicUsize = AtomicUsize::new(0);
    /// The word the probe saves its own context through, so the switch back
    /// has somewhere to write.
    static AWAY: AtomicUsize = AtomicUsize::new(0);
    /// What the probe computed on the far stack.
    static SUM: AtomicU64 = AtomicU64::new(0);
    /// Where the probe's own frame was, read back to prove it was the block.
    static WAS: AtomicUsize = AtomicUsize::new(0);

    /// Reached by the switch's own `ret`, on a stack this thread has never
    /// run on. It aborts rather than panicking on any surprise: an unwind out
    /// of a frame with no landing pad above it is not a test failure, it is a
    /// second fault on top of the first.
    extern "C" fn probe() {
        // Four kilobytes of frame, written and read, so that a switch which
        // had left `sp` where it found it would corrupt the caller instead of
        // quietly passing.
        let mut scratch = [0u64; 512];
        for (i, slot) in scratch.iter_mut().enumerate() {
            *slot = i as u64;
        }
        SUM.store(scratch.iter().sum(), Ordering::SeqCst);
        WAS.store(scratch.as_ptr() as usize, Ordering::SeqCst);

        let home = HOME.load(Ordering::SeqCst) as *const *mut u8;
        let away = AWAY.load(Ordering::SeqCst) as *mut *mut u8;
        if home.is_null() || away.is_null() {
            std::process::abort();
        }
        // SAFETY: two words the case below owns and keeps alive across the
        // switch; `*home` is the context the switch into here wrote.
        unsafe { buri_rt_task_switch(away, *home) };
        // Nothing switches back into a finished probe.
        std::process::abort();
    }

    /// **The switch comes back**, which is the smallest complete statement
    /// about this file.
    ///
    /// A frame is built from nothing on a plain heap block, entered, run deep
    /// enough to be sure the frame really is on that block, and switched out
    /// of; this thread then carries on from the call it made. A save and a
    /// restore that disagreed about an offset would be a wild `ret` and a
    /// signal rather than a wrong answer, which is why the case asserts *where*
    /// the far frame was as well as what it computed.
    ///
    /// The launch pad is deliberately **not** in this path: `prepare`'s return
    /// word is overwritten with the probe, because
    /// [`buri_rt_task_launch`]'s own next instruction is a call to
    /// `buri_rt_task_main`, which is `rt.rs`'s and needs a task. The pad is
    /// what every case in `rt.rs` enters a task through, so it is covered
    /// there and the two halves are separable here.
    #[test]
    fn a_prepared_frame_runs_and_switches_back() {
        let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // An ordinary heap block: this probe does not recurse, so a guard is
        // `memory.rs`'s business and not this file's.
        let block = vec![0u8; 256 * 1024];
        let low = block.as_ptr() as usize;
        let top = (low + block.len()) & !15usize;
        // SAFETY: the top of a live 256 KiB block, aligned down, and no other
        // stack is anywhere near it.
        let there = unsafe { prepare(top as *mut u8, std::ptr::null_mut()) };
        // SAFETY: the return word of a frame nothing is running.
        unsafe { plant_return(there, probe as *const ()) };

        let mut home: *mut u8 = std::ptr::null_mut();
        let mut away: *mut u8 = std::ptr::null_mut();
        HOME.store((&raw mut home) as usize, Ordering::SeqCst);
        AWAY.store((&raw mut away) as usize, Ordering::SeqCst);
        SUM.store(0, Ordering::SeqCst);
        WAS.store(0, Ordering::SeqCst);

        // SAFETY: `home` is this thread's own word, and `there` is the frame
        // just built, which nothing else is running.
        unsafe { buri_rt_task_switch(&raw mut home, there) };

        assert_eq!(SUM.load(Ordering::SeqCst), (0..512u64).sum(), "the far frame did not run");
        let was = WAS.load(Ordering::SeqCst);
        assert!(
            was >= low && was < top,
            "the probe's frame was at {was:#x}, outside the block it was given \
             ({low:#x}..{top:#x}): the switch did not take the new stack",
        );
        assert!(!away.is_null(), "the probe's own context was not saved on the way back");
        assert!(!home.is_null(), "this thread's context was not saved on the way in");
    }

    /// The frame is a whole number of 16-byte pairs, which both ABIs require
    /// of a stack pointer at every instruction boundary.
    #[test]
    fn the_saved_frame_keeps_the_stack_aligned() {
        assert!((FRAME_WORDS * size_of::<usize>()).is_multiple_of(16));
        assert!(ARG_WORD < FRAME_WORDS);
        assert!(RETURN_WORD < FRAME_WORDS);
        assert_ne!(ARG_WORD, RETURN_WORD);
    }

    /// `prepare` writes two words and zeroes the rest, and the two are where
    /// the assembly reads them.
    #[test]
    fn a_prepared_frame_is_two_words_and_zeroes() {
        let block = vec![0xabu8; 64 * 1024];
        let top = ((block.as_ptr() as usize + block.len()) & !15usize) as *mut u8;
        // SAFETY: the top of a live block.
        let sp = unsafe { prepare(top, 0x1234_5678 as *mut u8) };
        // SAFETY: `FRAME_WORDS` words were just written there.
        let words: Vec<usize> = (0..FRAME_WORDS).map(|i| unsafe { sp.cast::<usize>().add(i).read() }).collect();
        assert_eq!(words[ARG_WORD], 0x1234_5678);
        assert_eq!(words[RETURN_WORD], buri_rt_task_launch as *const () as usize);
        for (i, w) in words.iter().enumerate() {
            if i != ARG_WORD && i != RETURN_WORD {
                assert_eq!(*w, 0, "word {i} of a prepared frame was not zeroed");
            }
        }
        assert_eq!(top as usize - sp as usize, FRAME_WORDS * size_of::<usize>());
    }
}
