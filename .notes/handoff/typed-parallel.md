# Typed runtime-driven closures

Phase 46 landed `Tasks.parallel` as a typed instruction, and phase 48 landed the Compute, Walk and Press rows as `Inst::Handover`. No runtime row finds its closure by position any more.

## Landed

```rust
Inst::Parallel(Box<Parallel>)

pub struct Parallel {
    pub dest: ValueId,
    pub tasks: Tasks,       // Host | Testing
    pub sched: ValueId,
    pub ctx: ValueId,
    pub items: ValueId,
    pub step: ValueId,
    pub params: [Ty; 3],    // C, Int, A
    pub ret: Ty,            // B
}
```

- `lower/lists.rs` writes it at each call site and as the body of the two `parallel` functions. `lists::handles` covers the `Tasks` keys.
- `runtime_table::parallel(tasks)` returns the row, with no key lookup. `Extra::Step`, `StepCall` and `step_call` are gone.
- `shares_counts` looks for a `Parallel` on `Tasks::Host` in function code, because `HostTasks.parallel` is no longer in `Program::runtimes`.
- Stencil: `rt_call` takes a `tail` of extra words. `step_words` builds a step's four words from the instruction.
- LLVM: `Unit::parallel` emits the call. `Elements` became a `(stride, glue_ty)` pair, which `generic_element` or `bare_carrier` always answers.
- `glue::entry_thunk` walks the parameters once and treats the last one as the element.
- `rtcall.rs` `fits` checks only reach. `Jit::emit` checks a folded hole's scale itself.

## Next

- **`MakeClosure`'s arity.** `jit::fn_ty` is left only for the stencil thunk, which needs the closure type's arity: a capture-free lambda keeps a leading unit parameter. Record the arity on `MakeClosure`, or have `middle::closures` drop that parameter. Either one changes the output.
- **LLVM type-kind helpers** (`agg`, `enum_ty`, `list_ty`, `int`, `float`, `ptr`). They need kind-typed ids on the instructions that read them, so they're a larger item than this one.
- JS needs nothing. It reads the monomorphized program, and `ctx_step_key` answers from `Tasks::of`.

## Coverage

Only branch fractions are gated. Deleting a covered branch lowers its area's fraction, so before each step, check every area against `coverage/compiler-branches.txt`.

No test reaches these stencil branches, and phase 48 found none of them clearly impossible:

- `emit.rs` `ArrayGet` with the destination in the array's pointer slot. `pin_call_values` may pin both to one outgoing slot when the array dies at the get.
- `glue.rs` `load_through` at an unaligned offset. Layouts align tags and pointers, but nothing in the types says so.
- `object.rs` with an addend wider than 24 bits. A data section over 16 MiB would reach it.
