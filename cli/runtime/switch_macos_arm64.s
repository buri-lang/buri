// The machine-stack switch, AArch64 on Darwin.
//
// Included by `cli/runtime/switch.rs` through `global_asm!`. Its header is the
// contract; what is here is the AAPCS64 half of it, which is: the ten
// callee-saved general registers, the frame pointer, the link register and the
// low halves of the eight callee-saved vector registers, in one 160-byte frame
// that `switch::prepare` also knows how to build from nothing.
//
// Darwin names a C symbol with a leading underscore and has no `.type` or
// `.size` directives, which is the whole of the difference from the Linux
// AArch64 file beside it. The two bodies are otherwise instruction for
// instruction the same, and `the_two_aarch64_blocks_are_one_body` asserts it.

.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _buri_rt_task_switch
_buri_rt_task_switch:
    sub  sp, sp, #160
    stp  x19, x20, [sp, #0]
    stp  x21, x22, [sp, #16]
    stp  x23, x24, [sp, #32]
    stp  x25, x26, [sp, #48]
    stp  x27, x28, [sp, #64]
    stp  x29, x30, [sp, #80]
    stp  d8,  d9,  [sp, #96]
    stp  d10, d11, [sp, #112]
    stp  d12, d13, [sp, #128]
    stp  d14, d15, [sp, #144]
    mov  x9, sp
    str  x9, [x0]
    mov  sp, x1
    ldp  x19, x20, [sp, #0]
    ldp  x21, x22, [sp, #16]
    ldp  x23, x24, [sp, #32]
    ldp  x25, x26, [sp, #48]
    ldp  x27, x28, [sp, #64]
    ldp  x29, x30, [sp, #80]
    ldp  d8,  d9,  [sp, #96]
    ldp  d10, d11, [sp, #112]
    ldp  d12, d13, [sp, #128]
    ldp  d14, d15, [sp, #144]
    add  sp, sp, #160
    ret

.p2align 2
.globl _buri_rt_task_launch
_buri_rt_task_launch:
    mov  x0, x19
    mov  x29, xzr
    bl   _buri_rt_task_main
    brk  #1
