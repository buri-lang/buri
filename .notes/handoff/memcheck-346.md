# Memcheck #346: acceptor thread leaked at a hard stop

- Cause: the 56-byte block is the boxed closure `std::thread` allocates to start the acceptor. `park` in the frame is inlining. A second SIGTERM ended the process while the acceptor was returning from `accept`. The box's only address was in that thread's registers, and Memcheck ignores other threads' registers at a fatal signal (a C program reproduces this).
- Fix (`cli/runtime/net.rs`):
  - `Listening::stop_acceptor` waits for `Gate::accepting` to clear, then joins the thread.
  - `close` and `drain_all` call it.
  - `drain_all(settled)` calls `shutdown::settle` once every acceptor has ended. Until then a second signal is only counted; `settle` restores SIG_DFL and raises it.
- Same class: `http::within` joins its lookup thread on an answer. `tcp.rs` and the WebSocket client now use it.
- Left alone:
  - `child.rs`'s stdin feeder finishes once its input is in the pipe, long before the program can end.
  - tokio blocking-pool threads exit after 10 s idle. That is a rare residual.
- Tests:
  - `net::tests::{a_drains_first_step_ends_with_the_acceptor_thread_gone, closing_a_listener_ends_its_acceptor_thread, a_signals_drain_settles_once_every_acceptor_thread_has_ended}`
  - `http::tests::a_step_that_answers_has_ended_its_thread`
- Valgrind (Lima aarch64, LLVM build): the hard-stop e2e test was clean 12 out of 12 times. In 5 manual runs with every leak kind shown, the acceptor's 56-byte block never appears. Before the fix it showed as still reachable whenever the acceptor was alive.
