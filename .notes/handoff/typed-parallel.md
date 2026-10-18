# Typed runtime-driven closures

Phase 46 landed `Tasks.parallel` as a typed instruction. The other closure rows still find their closure by position.

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

## Next

- **Compute, Walk and Press rows.** `rtcall::last_closure` and its guard serve these. Each needs its own typed instruction, the way `Parallel` works: `Ui.memo`/`Ui.watch` (Compute), `Ui.mount`/`renderInto` (Walk), and the handlers and timers (Press). With all three typed, `last_closure`, the press-context guard in `rtcall.rs` and `jit::fn_ty` go away. LLVM's matching arms in `entry_args` use `fn_ty(ir_ty)` the same way.
- **LLVM type-kind helpers** (`agg`, `fn_ty`, `enum_ty`, `list_ty`, `int`, `float`, `ptr`). They need kind-typed ids on the instructions that read them, so they're a larger item than this one.
- JS needs nothing. It reads the monomorphized program, and `ctx_step_key` answers from `Tasks::of`.

## Not landed yet

Branch `worktree-agent-ad44371508c890bec` isn't on `staging`. Deleting covered code drops these fractions just below the CI baseline:

| Area | Measure | CI baseline | Estimate after this change |
| --- | --- | --- | --- |
| stencil | branches | 1090/1102 | 1088/1100 |
| stencil | lines | 8183/8648 | 8162/8627 |
| llvm | lines | 7483/7670 | 7441/7629 |

To land it, first add e2e tests in `cli/tests` that cover:

- one uncovered stencil branch;
- about 10 more stencil lines;
- about 20 more llvm lines.

Then rebase onto `staging` and rerun coverage.

A candidate for the stencil branch is `emit.rs` `ArrayGet`, where the destination shares the array's pointer slot. A loop such as `s = s.items[0]` should reach it, where `items` is a list of `s`'s own record type.

Removing a covered branch lowers an area's fraction, so check `crates/stencil` and the line fractions before landing the next steps as well.
