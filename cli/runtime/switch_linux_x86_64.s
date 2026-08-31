// The machine-stack switch, x86-64 SysV on Linux.
//
// Intel syntax, which is what `global_asm!` selects on this architecture; the
// directive is spelled out anyway so that the file assembles on its own — the
// cross-target test hands it to `cc -x assembler` and nothing wraps it there.
//
// ## The frame is eight words, and one of them is reserved
//
// SysV's callee-saved set is six general registers and no vector register, and
// the return address is already on the stack where `ret` wants it. Six plus one
// is **seven**, an odd number of words, and a frame of an odd number of words
// is a frame whose two ends cannot both be sixteen-byte aligned — which is the
// one thing both of this runtime's ABIs require of a stack pointer. AArch64's
// twenty happens to be even and this one does not, so the odd word is added
// here rather than papered over at the call site:
//
//     word 0   reserved, always zero        <- the saved stack pointer, aligned
//     word 1   r15
//     word 2   r14
//     word 3   r13
//     word 4   r12
//     word 5   rbx        the launch pad's argument
//     word 6   rbp
//     word 7   the address `ret` takes
//     word 8   ...                          <- the pointer `ret` leaves behind
//
// `switch::prepare` builds the same eight from nothing, and reads words 5 and 7
// out of the `.set` block below rather than out of a comment: the constants are
// the layout, the instructions are written in terms of them, and
// `the_blocks_build_the_frame_this_module_writes` holds the Rust side to the
// same three numbers. Two `.if` guards make the assembler itself refuse a frame
// that is not a whole number of pairs, or one whose last word is not the return
// address, so a wrong edit here fails at build time on any host rather than on
// the one machine that runs it.
//
// Stores at named offsets rather than `push` and `pop`: the offsets are then
// the constants above instead of a count of pushes a reader has to keep track
// of, and it costs nothing — six stores and one stack-pointer move for six
// pushes.
//
// `endbr64` at both entry points: a no-op on a machine without CET and the
// landing pad on one with it. `buri_rt_task_launch` is reached by `ret` rather
// than by an indirect call, which indirect-branch tracking does not police, but
// a function whose address is taken and never marked is the kind of asymmetry
// that costs an afternoon later.

.intel_syntax noprefix

.set BURI_RT_FRAME_WORDS,  8
.set BURI_RT_PAD_WORD,     0
.set BURI_RT_R15_WORD,     1
.set BURI_RT_R14_WORD,     2
.set BURI_RT_R13_WORD,     3
.set BURI_RT_R12_WORD,     4
.set BURI_RT_ARG_WORD,     5
.set BURI_RT_RBP_WORD,     6
.set BURI_RT_RETURN_WORD,  7

// A stack pointer that is not sixteen-byte aligned is undefined behaviour in
// this ABI, and the frame is the distance between two of them.
.if (BURI_RT_FRAME_WORDS * 8) % 16
.error "the x86-64 saved frame is not a whole number of 16-byte pairs"
.endif
// `ret` takes the word the stack pointer is left on, so the return address is
// the frame's last word by construction and not by choice.
.if BURI_RT_RETURN_WORD != BURI_RT_FRAME_WORDS - 1
.error "the x86-64 return address is not the last word of the saved frame"
.endif

.text
.p2align 4
.globl buri_rt_task_switch
.type buri_rt_task_switch, @function
buri_rt_task_switch:
    endbr64
    // The call left the return address at what is already the frame's last
    // word, so the pointer moves down by the rest of the frame and no further.
    sub  rsp, (BURI_RT_FRAME_WORDS - 1) * 8
    mov  qword ptr [rsp + BURI_RT_PAD_WORD * 8], 0
    mov  [rsp + BURI_RT_R15_WORD * 8], r15
    mov  [rsp + BURI_RT_R14_WORD * 8], r14
    mov  [rsp + BURI_RT_R13_WORD * 8], r13
    mov  [rsp + BURI_RT_R12_WORD * 8], r12
    mov  [rsp + BURI_RT_ARG_WORD * 8], rbx
    mov  [rsp + BURI_RT_RBP_WORD * 8], rbp
    mov  [rdi], rsp
    mov  rsp, rsi
    mov  r15, [rsp + BURI_RT_R15_WORD * 8]
    mov  r14, [rsp + BURI_RT_R14_WORD * 8]
    mov  r13, [rsp + BURI_RT_R13_WORD * 8]
    mov  r12, [rsp + BURI_RT_R12_WORD * 8]
    mov  rbx, [rsp + BURI_RT_ARG_WORD * 8]
    mov  rbp, [rsp + BURI_RT_RBP_WORD * 8]
    add  rsp, (BURI_RT_FRAME_WORDS - 1) * 8
    ret
.size buri_rt_task_switch, .-buri_rt_task_switch

.p2align 4
.globl buri_rt_task_launch
.type buri_rt_task_launch, @function
buri_rt_task_launch:
    endbr64
    mov  rdi, rbx
    xor  rbp, rbp
    // The frame above is a whole number of pairs and a task stack top is
    // sixteen-byte aligned, so the `ret` that reached here left an aligned
    // pointer and this is a no-op. It is kept because it is one instruction
    // once per task and it is the difference between a misaligned stack
    // *given* to this runtime and a crash inside somebody else's `movaps`.
    and  rsp, -16
    call buri_rt_task_main
    ud2
.size buri_rt_task_launch, .-buri_rt_task_launch

.section .note.GNU-stack,"",@progbits
