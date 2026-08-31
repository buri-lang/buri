// The machine-stack switch, x86-64 SysV on Linux.
//
// Intel syntax, which is what `global_asm!` selects on this architecture; the
// directive is spelled out anyway so that the file assembles on its own — the
// cross-target test hands it to `cc -x assembler` and nothing wraps it there.
//
// SysV's callee-saved set is six general registers and no vector register, and
// the return address is already on the stack where `ret` wants it, so the frame
// is seven words rather than AArch64's twenty. `switch::prepare` builds the
// same seven from nothing.
//
// `endbr64` at both entry points: a no-op on a machine without CET and the
// landing pad on one with it. `buri_rt_task_launch` is reached by `ret` rather
// than by an indirect call, which indirect-branch tracking does not police, but
// a function whose address is taken and never marked is the kind of asymmetry
// that costs an afternoon later.

.intel_syntax noprefix
.text
.p2align 4
.globl buri_rt_task_switch
.type buri_rt_task_switch, @function
buri_rt_task_switch:
    endbr64
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov  [rdi], rsp
    mov  rsp, rsi
    pop  r15
    pop  r14
    pop  r13
    pop  r12
    pop  rbx
    pop  rbp
    ret
.size buri_rt_task_switch, .-buri_rt_task_switch

.p2align 4
.globl buri_rt_task_launch
.type buri_rt_task_launch, @function
buri_rt_task_launch:
    endbr64
    mov  rdi, rbx
    xor  rbp, rbp
    and  rsp, -16
    call buri_rt_task_main
    ud2
.size buri_rt_task_launch, .-buri_rt_task_launch

.section .note.GNU-stack,"",@progbits
