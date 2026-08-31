// The machine-stack switch, AArch64 on Linux.
//
// Instruction for instruction the Darwin file beside it; the differences are
// ELF's and not the machine's — no leading underscore on a C symbol, `.type`
// and `.size` so the linker knows a function when it sees one, and the
// non-executable-stack note every object on this platform is expected to
// carry. `the_two_aarch64_blocks_are_one_body` asserts the bodies match.

.text
.p2align 2
.globl buri_rt_task_switch
.type buri_rt_task_switch, %function
buri_rt_task_switch:
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
