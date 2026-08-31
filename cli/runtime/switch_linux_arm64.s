// The machine-stack switch, AArch64 on Linux.
//
// Instruction for instruction the Darwin file beside it; the differences are
// ELF's and not the machine's — no leading underscore on a C symbol, `.type`
// and `.size` so the linker knows a function when it sees one, and the
// non-executable-stack note every object on this platform is expected to
// carry. `the_two_aarch64_blocks_are_one_body` asserts the bodies match.

// The frame, in words, and the two words `switch::prepare` writes into one it
// builds from nothing: the argument goes in the first callee-saved register and
// the switch's own `ret` takes the link register. They are `.set` constants
// rather than a comment because the instructions below are written in terms of
// them and because `the_blocks_build_the_frame_this_module_writes` reads them
// out of this file and holds `switch::FRAME` to the same three numbers.
.set BURI_RT_FRAME_WORDS, 20
.set BURI_RT_ARG_WORD,     0   // x19
.set BURI_RT_RETURN_WORD, 11   // x30, the link register

// A stack pointer that is not sixteen-byte aligned faults on this architecture
// rather than merely being undefined, and the frame is the distance between two
// of them. Twenty words is ten pairs; the guard is here so that a nineteenth or
// twenty-first would be refused by the assembler on every host that builds this
// file rather than by the one machine that runs it.
.if (BURI_RT_FRAME_WORDS * 8) % 16
.error "the AArch64 saved frame is not a whole number of 16-byte pairs"
.endif
// `x30` is the second half of the `x29`/`x30` pair, so the link register's word
// is one above the frame pointer's by construction and not by choice.
.if BURI_RT_RETURN_WORD % 2 != 1
.error "the AArch64 link register is not the odd half of its saved pair"
.endif

.text
.p2align 2
.globl buri_rt_task_switch
.type buri_rt_task_switch, %function
buri_rt_task_switch:
    sub  sp, sp, #(BURI_RT_FRAME_WORDS * 8)
    stp  x19, x20, [sp, #(BURI_RT_ARG_WORD * 8)]
    stp  x21, x22, [sp, #16]
    stp  x23, x24, [sp, #32]
    stp  x25, x26, [sp, #48]
    stp  x27, x28, [sp, #64]
    stp  x29, x30, [sp, #((BURI_RT_RETURN_WORD - 1) * 8)]
    stp  d8,  d9,  [sp, #96]
    stp  d10, d11, [sp, #112]
    stp  d12, d13, [sp, #128]
    stp  d14, d15, [sp, #144]
    mov  x9, sp
    str  x9, [x0]
    mov  sp, x1
    ldp  x19, x20, [sp, #(BURI_RT_ARG_WORD * 8)]
    ldp  x21, x22, [sp, #16]
    ldp  x23, x24, [sp, #32]
    ldp  x25, x26, [sp, #48]
    ldp  x27, x28, [sp, #64]
    ldp  x29, x30, [sp, #((BURI_RT_RETURN_WORD - 1) * 8)]
    ldp  d8,  d9,  [sp, #96]
    ldp  d10, d11, [sp, #112]
    ldp  d12, d13, [sp, #128]
    ldp  d14, d15, [sp, #144]
    add  sp, sp, #(BURI_RT_FRAME_WORDS * 8)
    ret
.size buri_rt_task_switch, .-buri_rt_task_switch

.p2align 2
.globl buri_rt_task_launch
.type buri_rt_task_launch, %function
buri_rt_task_launch:
    mov  x0, x19
    mov  x29, xzr
    bl   buri_rt_task_main
    brk  #1
.size buri_rt_task_launch, .-buri_rt_task_launch

.section .note.GNU-stack,"",%progbits
