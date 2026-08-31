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
//! ### 2.1 One frame per ABI, and both of them are even
//!
//! What the two blocks save is neither the same set nor the same size — AAPCS64
//! has twenty words of it, SysV six registers and a return address — so the
//! layout is **per architecture and independently correct** rather than one
//! number with a footnote. [`FRAMES`] is both of them, compiled into every
//! build so that a case on either machine checks the other's arithmetic and
//! reads the other's assembly, and each block states the same numbers in `.set`
//! constants its own instructions are written in terms of.
//!
//! Both frames are an **even** number of words, and on x86-64 that costs a
//! reserved word: six callee-saved registers plus the return address is seven,
//! and a seven-word frame has one aligned end and one misaligned one. Which end
//! is misaligned is not a free choice — the high end is the caller's stack
//! pointer and the low end is what a suspended task *is* — so the frame carries
//! a zero word at its base and both of its ends are sixteen-byte aligned.
//!
//! That is the shape of the mistake this file has already made once. The frame
//! was one constant, it was twenty on AArch64 and seven on x86-64, and the case
//! that says a frame is a whole number of pairs was written on the machine
//! where it happened to be true. The second machine was a stack pointer eight
//! bytes out under every frame a task ran, and nothing on the first one could
//! see it — which is why both frames are now in every build and every case
//! below runs over both.
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

/// The saved frame's shape, in words, for one of the two machine ABIs.
///
/// `words` is the **whole** frame: every callee-saved register the block
/// writes, the address its `ret` will take, and any word that exists only to
/// keep the count even. `arg` and `ret` are the two words [`prepare`] writes
/// and the assembly reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Frame {
    /// The frame's size in machine words. **Always even** — see §2.1.
    pub(crate) words: usize,
    /// Where the argument sits: the first callee-saved register, which
    /// [`buri_rt_task_launch`] moves into the first argument register.
    pub(crate) arg: usize,
    /// Where the address the switch's `ret` will take sits.
    pub(crate) ret: usize,
}

/// The frame each ABI's block builds, AArch64 first.
///
/// **Both are compiled into every build**, host or not, so that the cases below
/// check the arithmetic of a machine this one is not and read the assembly of a
/// block this one never compiles. `HOST` picks the row the rest of the module
/// is written against; nothing else indexes this.
pub(crate) const FRAMES: [Frame; 2] = [
    // AAPCS64: `x19`–`x28`, `x29`, `x30`, then `d8`–`d15`. Twenty words, a
    // 160-byte frame of ten pairs, the argument in `x19` at its base and the
    // link register the eleventh word.
    Frame { words: 20, arg: 0, ret: 11 },
    // x86-64 SysV: a reserved word, `r15`, `r14`, `r13`, `r12`, `rbx`, `rbp`
    // and the return address `ret` will take. The six registers and the return
    // address are seven, which is odd, and the reserved word at the base is
    // what makes the frame a whole number of pairs — `switch_linux_x86_64.s`'s
    // header draws the layout.
    Frame { words: 8, arg: 5, ret: 7 },
];

/// Which row of [`FRAMES`] this build's assembly is.
#[cfg(target_arch = "aarch64")]
const HOST: usize = 0;
#[cfg(target_arch = "x86_64")]
const HOST: usize = 1;

/// The frame the block compiled into *this* binary builds.
pub(crate) const FRAME: Frame = FRAMES[HOST];

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
/// whatever the previous tenant left. Where the layout has a reserved word
/// (§2.1, x86-64) that is a zero too, and the assembly writes it zero on the
/// way out for the same reason: a word nothing reads is a word nothing should
/// be able to read something *out of*.
///
/// # Safety
/// `[top - FRAME.words * 8, top)` is writable and is not the live part of any
/// other stack.
pub(crate) unsafe fn prepare(top: *mut u8, arg: *mut u8) -> *mut u8 {
    debug_assert!((top as usize).is_multiple_of(16), "a task stack top must be 16-byte aligned");
    let sp = top.wrapping_sub(FRAME.words * size_of::<usize>()).cast::<usize>();
    // SAFETY: the caller promises the range is writable and unshared.
    unsafe {
        sp.write_bytes(0, FRAME.words);
        sp.add(FRAME.arg).write(arg as usize);
        sp.add(FRAME.ret).write(buri_rt_task_launch as *const () as usize);
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
    unsafe { sp.cast::<usize>().add(FRAME.ret).write(target as usize) };
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

    /// The three blocks and the frame each of them builds.
    ///
    /// `include_str!` rather than a read at run time, for the reason
    /// `cli/tests/native/runtime.rs` gives where it does the same: the case is
    /// a statement about the files this runtime was **built** from, and a path
    /// read at run time would pass against a tree the archive does not carry.
    const BLOCKS: [(&str, &str, Frame, &[&str]); 3] = [
        ("switch_macos_arm64.s", include_str!("switch_macos_arm64.s"), FRAMES[0], AAPCS64),
        ("switch_linux_arm64.s", include_str!("switch_linux_arm64.s"), FRAMES[0], AAPCS64),
        ("switch_linux_x86_64.s", include_str!("switch_linux_x86_64.s"), FRAMES[1], SYSV64),
    ];

    /// AAPCS64 §6.1.1: the ten general registers, the frame pointer, the link
    /// register, and the **low 64 bits** of the eight vector registers.
    const AAPCS64: &[&str] = &[
        "x19", "x20", "x21", "x22", "x23", "x24", "x25", "x26", "x27", "x28", "x29", "x30", "d8",
        "d9", "d10", "d11", "d12", "d13", "d14", "d15",
    ];

    /// x86-64 SysV §3.2.1, figure 3.4: six general registers and not one vector
    /// register. `rsp` is the thing being switched and is not in the set.
    const SYSV64: &[&str] = &["rbx", "rbp", "r12", "r13", "r14", "r15"];

    /// One register and the frame slot a block moves it through, as the two
    /// appear in the file.
    type Move = (String, String);

    /// Every [`Move`] a block's `buri_rt_task_switch` writes to its own stack,
    /// and every one it reads back, in the order they appear.
    ///
    /// The slot is the operand's **text** rather than a number, because the
    /// text is what the two halves have to agree about: `.set`-derived on both
    /// architectures, so a save at one offset and a restore at another is two
    /// strings that differ. `mov [rdi], rsp` and `mov rsp, rsi` are the switch
    /// itself and address no slot of this frame; the pad's `mov qword ptr …, 0`
    /// stores an immediate rather than a register and is not one either.
    fn saves_and_restores(source: &str) -> (Vec<Move>, Vec<Move>) {
        let (mut saves, mut restores) = (Vec::new(), Vec::new());
        for line in source.lines().map(str::trim) {
            if line.starts_with("//") {
                continue;
            }
            let Some((op, rest)) = line.split_once(char::is_whitespace) else { continue };
            let rest = rest.trim();
            match op {
                // AArch64: `stp x19, x20, [sp, #(…)]`, and the halves of a pair
                // are one word apart whatever the expression says.
                "stp" | "ldp" => {
                    let mut parts = rest.splitn(3, ',');
                    let low = parts.next().unwrap_or_default().trim();
                    let high = parts.next().unwrap_or_default().trim();
                    let slot = parts.next().unwrap_or_default().trim();
                    let into = if op == "stp" { &mut saves } else { &mut restores };
                    into.push((low.to_string(), format!("{slot} low half")));
                    into.push((high.to_string(), format!("{slot} high half")));
                }
                // x86-64: `mov [rsp + …], r15` one way and `mov r15, [rsp + …]`
                // the other.
                "mov" => {
                    let Some((lhs, rhs)) = rest.split_once(',') else { continue };
                    let (lhs, rhs) = (lhs.trim(), rhs.trim());
                    if lhs.starts_with("[rsp") {
                        saves.push((rhs.to_string(), lhs.to_string()));
                    } else if rhs.starts_with("[rsp") {
                        restores.push((lhs.to_string(), rhs.to_string()));
                    }
                }
                _ => {}
            }
        }
        (saves, restores)
    }

    /// The block this build's `global_asm!` included.
    const fn host_block() -> &'static str {
        if cfg!(target_arch = "x86_64") {
            "switch_linux_x86_64.s"
        } else if cfg!(target_os = "macos") {
            "switch_macos_arm64.s"
        } else {
            "switch_linux_arm64.s"
        }
    }

    /// The value of one `.set BURI_RT_…` line of a block.
    fn setting(source: &str, file: &str, name: &str) -> usize {
        let head = format!(".set {name},");
        let line = source
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with(&head))
            .unwrap_or_else(|| panic!("{file} has no `{head}` line"));
        let rest = line.get(head.len()..).unwrap_or_default().trim();
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().unwrap_or_else(|_| panic!("{file}'s `{head}` is not a number: {rest}"))
    }

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

    /// **Every** saved frame is a whole number of 16-byte pairs, which both
    /// ABIs require of a stack pointer at every instruction boundary.
    ///
    /// Over both rows of [`FRAMES`] rather than over the host's, because the
    /// frame that was wrong was the one the machine running this case did not
    /// use: a seven-word x86-64 frame passed here on every AArch64 host in the
    /// world and was eight bytes out on the first machine that ran it.
    #[test]
    fn the_saved_frame_keeps_the_stack_aligned() {
        assert_eq!(size_of::<usize>(), 8, "every block here is written for a 64-bit word");
        for (file, _, frame, _) in BLOCKS {
            let bytes = frame.words * 8;
            assert!(
                bytes.is_multiple_of(16),
                "{file}: a {}-word frame is {bytes} bytes, which is not a whole number of \
                 16-byte pairs — one of its two ends cannot be aligned",
                frame.words,
            );
            assert!(frame.arg < frame.words, "{file}: the argument word is outside the frame");
            assert!(frame.ret < frame.words, "{file}: the return word is outside the frame");
            assert_ne!(
                frame.arg, frame.ret,
                "{file}: the argument and the return address are the same word",
            );
        }
    }

    /// **The assembly and [`FRAMES`] are one layout.**
    ///
    /// Each block states its frame in `.set` constants and writes its own
    /// instructions in terms of them; this reads those constants back out of
    /// all three files and holds the Rust side to them. Two of the three are
    /// never compiled by anything a given host runs — `cli/build.rs` builds the
    /// archive for the host triple alone — so on an AArch64 machine this case
    /// is the whole of what stands between the x86-64 block and a `prepare`
    /// that builds a frame that block does not restore.
    ///
    /// Three things per block: the constants agree with the row, the block
    /// **uses** each of them somewhere that is not the `.set` line itself (a
    /// constant the instructions never mention is a comment with a colon in
    /// it), and — for the one block this build actually included — the module's
    /// own [`FRAME`] is that block's.
    #[test]
    fn the_blocks_build_the_frame_this_module_writes() {
        for (file, source, frame, _) in BLOCKS {
            for (name, want) in [
                ("BURI_RT_FRAME_WORDS", frame.words),
                ("BURI_RT_ARG_WORD", frame.arg),
                ("BURI_RT_RETURN_WORD", frame.ret),
            ] {
                assert_eq!(
                    setting(source, file, name),
                    want,
                    "{file}'s `{name}` and `switch::FRAMES` disagree",
                );
                let uses = source
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.starts_with("//") && !l.starts_with(".set") && l.contains(name))
                    .count();
                assert!(uses > 0, "{file} states `{name}` and never uses it");
            }
        }

        let host = host_block();
        let (file, source, ..) = BLOCKS
            .into_iter()
            .find(|(f, ..)| *f == host)
            .unwrap_or_else(|| panic!("no row for {host}"));
        assert_eq!(setting(source, file, "BURI_RT_FRAME_WORDS"), FRAME.words);
        assert_eq!(setting(source, file, "BURI_RT_ARG_WORD"), FRAME.arg);
        assert_eq!(setting(source, file, "BURI_RT_RETURN_WORD"), FRAME.ret);
    }

    /// **Each block saves exactly its ABI's callee-saved set, and restores
    /// exactly what it saved.**
    ///
    /// The cases above are about the frame's *size*; this one is about what is
    /// in it. Six words of the right length holding the wrong six registers
    /// corrupt every caller and satisfy every other assertion in this file, and
    /// a block that saves `d12` and forgets to restore it is the same fault by
    /// half.
    ///
    /// It is a case about the text of the three files rather than a round trip
    /// through the running one, and deliberately: two of the three are never
    /// compiled on any given host, a round trip can only fail on the machine it
    /// is run on, and — the reason that decided it — a round trip cannot say
    /// *which* register was dropped without naming registers in a fourth block
    /// of assembly. What makes it readable is that an offset is *text* either
    /// way — a `.set` expression on the words the Rust side names, a literal on
    /// the rest — so a save at one offset and a restore from another are two
    /// strings that differ.
    #[test]
    fn every_block_saves_and_restores_its_abis_callee_saved_set() {
        for (file, source, _, callee_saved) in BLOCKS {
            let (saves, restores) = saves_and_restores(source);
            assert_eq!(
                saves, restores,
                "{file} does not restore what it saves: a register saved at one slot and \
                 restored from another, or not restored at all",
            );
            let mut registers: Vec<&str> = saves.iter().map(|(r, _)| r.as_str()).collect();
            registers.sort_unstable();
            let mut want: Vec<&str> = callee_saved.to_vec();
            want.sort_unstable();
            assert_eq!(
                registers, want,
                "{file} does not save its ABI's callee-saved set: a register missing from the \
                 left is one the switch loses, and one missing from the right is work nobody \
                 asked for",
            );
        }
    }

    /// **A saved context is a stack pointer the ABI would accept**, which is
    /// sixteen-byte aligned.
    ///
    /// This is the running statement of what the frame arithmetic is *for*, and
    /// it is the case the seven-word x86-64 frame failed: a frame of an odd
    /// number of words cannot have both of its ends aligned, and the end it
    /// gave up was the one a suspended task is named by. Every frame the
    /// runtime pushes above a context restored from it was then eight bytes
    /// out, which on x86-64 is a `movaps` away from a fault and on AArch64 is
    /// the fault itself.
    ///
    /// Three pointers, and each is a different way of making one: `there` is
    /// [`prepare`]'s, from nothing; `home` is the switch's own, saved on the way
    /// out of this thread; `away` is the switch's again, saved on a stack that
    /// had never been run on.
    #[test]
    fn a_saved_context_is_a_stack_pointer_the_abi_would_accept() {
        let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let block = vec![0u8; 256 * 1024];
        let top = ((block.as_ptr() as usize + block.len()) & !15usize) as *mut u8;
        // SAFETY: the top of a live 256 KiB block, aligned down, and no other
        // stack is anywhere near it.
        let there = unsafe { prepare(top, std::ptr::null_mut()) };
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

        for (what, sp) in [("prepare's", there), ("the switch's own", home), ("the far stack's", away)]
        {
            assert!(
                (sp as usize).is_multiple_of(16),
                "{what} saved stack pointer is {sp:?}, which is not 16-byte aligned: the frame \
                 above it is {} words and one of a frame's two ends is the other's alignment",
                FRAME.words,
            );
        }

        // The word the layout calls the return address is one: a context whose
        // `ret` would take a zero is a frame `prepare` built and the switch
        // never wrote through.
        // SAFETY: `home` is a frame the switch just saved, so its `FRAME.words`
        // words are written and this thread owns them.
        let returns_to = unsafe { home.cast::<usize>().add(FRAME.ret).read() };
        assert_ne!(returns_to, 0, "the saved frame's return word is empty");
    }

    /// `prepare` writes two words and zeroes the rest, and the two are where
    /// the assembly reads them.
    #[test]
    fn a_prepared_frame_is_two_words_and_zeroes() {
        let block = vec![0xabu8; 64 * 1024];
        let top = ((block.as_ptr() as usize + block.len()) & !15usize) as *mut u8;
        // SAFETY: the top of a live block.
        let sp = unsafe { prepare(top, 0x1234_5678 as *mut u8) };
        // SAFETY: `FRAME.words` words were just written there.
        let words: Vec<usize> =
            (0..FRAME.words).map(|i| unsafe { sp.cast::<usize>().add(i).read() }).collect();
        assert_eq!(words[FRAME.arg], 0x1234_5678);
        assert_eq!(words[FRAME.ret], buri_rt_task_launch as *const () as usize);
        for (i, w) in words.iter().enumerate() {
            if i != FRAME.arg && i != FRAME.ret {
                assert_eq!(*w, 0, "word {i} of a prepared frame was not zeroed");
            }
        }
        assert_eq!(top as usize - sp as usize, FRAME.words * size_of::<usize>());
    }
}
