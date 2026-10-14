//! The runtime's `Mutex` and `Condvar`: `std`'s, which helgrind can't see,
//! because they are a futex rather than a pthread call.
//!
//! With the `valgrind` feature, each lock and unlock is reported to helgrind
//! as one (`valgrind.rs`), so it orders what threads do under a lock and sees
//! lock-order inversions. Without it these are `std`'s types themselves.

#[cfg(not(feature = "valgrind"))]
pub(crate) use std::sync::{Condvar, Mutex, MutexGuard};

#[cfg(feature = "valgrind")]
pub(crate) use reported::{Condvar, Mutex, MutexGuard, close};

/// Nothing to wait for: only helgrind minds a lock held at exit.
#[cfg(not(feature = "valgrind"))]
#[inline]
pub(crate) fn close() {}

#[cfg(feature = "valgrind")]
mod reported {
    use crate::allocator::valgrind;
    use std::ops::{Deref, DerefMut};
    use std::sync::{LockResult, PoisonError, TryLockError, TryLockResult, WaitTimeoutResult};
    use std::time::Duration;

    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

    /// Whether a thread has started to exit the process.
    static CLOSING: AtomicBool = AtomicBool::new(false);
    /// Guards out, on every thread.
    static HELD: AtomicUsize = AtomicUsize::new(0);

    thread_local! {
        static HELD_HERE: Cell<usize> = const { Cell::new(0) };
        static CLOSER: Cell<bool> = const { Cell::new(false) };
    }

    /// Wait for every other thread's critical sections to end, then keep any
    /// thread holding no lock from taking one, so the exit after this kills
    /// no thread that holds one. Helgrind reports each such thread.
    ///
    /// A second thread to exit leaves it to the first.
    ///
    /// This and [`counted`] and [`leave`] are `#[inline(never)]` for
    /// `rt::poll_in_reactor`'s reason: inlined into a caller that a task
    /// switch moves between threads, they'd use the old thread's
    /// thread-locals.
    #[inline(never)]
    pub(crate) fn close() {
        if CLOSER.get() {
            return;
        }
        if CLOSING.swap(true, SeqCst) {
            HELD.fetch_sub(HELD_HERE.get(), SeqCst);
            HELD_HERE.set(0);
            wait_for_the_exit();
        }
        CLOSER.set(true);
        while HELD.load(SeqCst) != HELD_HERE.get() {
            std::thread::yield_now();
        }
    }

    #[cfg(test)]
    pub(crate) fn closing() -> bool {
        CLOSING.load(SeqCst)
    }

    fn wait_for_the_exit() -> ! {
        loop {
            std::thread::park();
        }
    }

    /// `inner`, counted as held. Once the process is closing, a thread that
    /// held nothing gives it back and waits for the exit instead.
    fn enter<G>(inner: G) -> G {
        if !counted() {
            drop(inner);
            wait_for_the_exit();
        }
        inner
    }

    /// Counts one more guard as held, or answers false where this thread
    /// must not take it.
    #[inline(never)]
    fn counted() -> bool {
        HELD.fetch_add(1, SeqCst);
        if CLOSING.load(SeqCst) && !CLOSER.get() && HELD_HERE.get() == 0 {
            HELD.fetch_sub(1, SeqCst);
            return false;
        }
        HELD_HERE.set(HELD_HERE.get().saturating_add(1));
        true
    }

    #[inline(never)]
    fn leave() {
        HELD_HERE.set(HELD_HERE.get().saturating_sub(1));
        HELD.fetch_sub(1, SeqCst);
    }

    /// `std::sync::Mutex`, whose locking helgrind is told about.
    pub(crate) struct Mutex<T: ?Sized>(std::sync::Mutex<T>);

    /// Its guard, which tells helgrind of the unlock before it happens.
    pub(crate) struct MutexGuard<'a, T: ?Sized> {
        inner: Option<std::sync::MutexGuard<'a, T>>,
        lock: usize,
    }

    impl<T> Mutex<T> {
        pub(crate) const fn new(value: T) -> Mutex<T> {
            Mutex(std::sync::Mutex::new(value))
        }
    }

    impl<T: ?Sized> Mutex<T> {
        fn address(&self) -> usize {
            std::ptr::from_ref(self).cast::<u8>().addr()
        }

        pub(crate) fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
            let lock = self.address();
            let taken = |inner| {
                let inner = enter(inner);
                if valgrind::helgrind() {
                    valgrind::acquired(lock);
                }
                MutexGuard {
                    inner: Some(inner),
                    lock,
                }
            };
            match self.0.lock() {
                Ok(inner) => Ok(taken(inner)),
                Err(poisoned) => Err(PoisonError::new(taken(poisoned.into_inner()))),
            }
        }

        pub(crate) fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
            let lock = self.address();
            let taken = |inner| {
                let inner = enter(inner);
                if valgrind::helgrind() {
                    valgrind::acquired(lock);
                }
                MutexGuard {
                    inner: Some(inner),
                    lock,
                }
            };
            match self.0.try_lock() {
                Ok(inner) => Ok(taken(inner)),
                Err(TryLockError::Poisoned(poisoned)) => Err(TryLockError::Poisoned(
                    PoisonError::new(taken(poisoned.into_inner())),
                )),
                Err(TryLockError::WouldBlock) => Err(TryLockError::WouldBlock),
            }
        }
    }

    impl<'a, T: ?Sized> MutexGuard<'a, T> {
        /// The `std` guard, with helgrind told the lock is about to go.
        fn release(mut self) -> std::sync::MutexGuard<'a, T> {
            if valgrind::helgrind() {
                valgrind::released(self.lock);
            }
            leave();
            match self.inner.take() {
                Some(inner) => inner,
                // Taken only here and in `drop`, which this skips.
                None => unreachable_guard(),
            }
        }
    }

    #[cold]
    fn unreachable_guard() -> ! {
        crate::abort::die(&[b"a mutex guard was released twice"])
    }

    impl<T: ?Sized> Deref for MutexGuard<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            match &self.inner {
                Some(inner) => inner,
                None => unreachable_guard(),
            }
        }
    }

    impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            match &mut self.inner {
                Some(inner) => inner,
                None => unreachable_guard(),
            }
        }
    }

    impl<T: ?Sized> Drop for MutexGuard<'_, T> {
        fn drop(&mut self) {
            if self.inner.is_some() {
                if valgrind::helgrind() {
                    valgrind::released(self.lock);
                }
                leave();
            }
        }
    }

    /// `std::sync::Condvar`, whose waits release and take the lock as
    /// helgrind sees it.
    pub(crate) struct Condvar(std::sync::Condvar);

    impl Condvar {
        pub(crate) const fn new() -> Condvar {
            Condvar(std::sync::Condvar::new())
        }

        pub(crate) fn wait<'a, T>(
            &self,
            guard: MutexGuard<'a, T>,
        ) -> LockResult<MutexGuard<'a, T>> {
            let lock = guard.lock;
            let back = |inner| {
                let inner = enter(inner);
                if valgrind::helgrind() {
                    valgrind::acquired(lock);
                }
                MutexGuard {
                    inner: Some(inner),
                    lock,
                }
            };
            match self.0.wait(guard.release()) {
                Ok(inner) => Ok(back(inner)),
                Err(poisoned) => Err(PoisonError::new(back(poisoned.into_inner()))),
            }
        }

        pub(crate) fn wait_timeout<'a, T>(
            &self,
            guard: MutexGuard<'a, T>,
            timeout: Duration,
        ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
            let lock = guard.lock;
            let back = |(inner, result)| {
                let inner = enter(inner);
                if valgrind::helgrind() {
                    valgrind::acquired(lock);
                }
                (
                    MutexGuard {
                        inner: Some(inner),
                        lock,
                    },
                    result,
                )
            };
            match self.0.wait_timeout(guard.release(), timeout) {
                Ok(pair) => Ok(back(pair)),
                Err(poisoned) => Err(PoisonError::new(back(poisoned.into_inner()))),
            }
        }

        pub(crate) fn notify_one(&self) {
            self.0.notify_one();
        }

        pub(crate) fn notify_all(&self) {
            self.0.notify_all();
        }
    }
}

/// Set as a thread is about to block on I/O that may never end, so a test
/// knows when to exit under it.
#[cfg(all(test, feature = "valgrind"))]
pub(crate) static BLOCKING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
#[cfg(feature = "valgrind")]
pub(crate) mod tests {
    use super::Mutex;
    use std::sync::atomic::Ordering::SeqCst;

    /// Whether this process is the copy [`exit_of`] started.
    pub(crate) fn in_the_child() -> bool {
        std::env::var_os("BURI_RT_TEST_ABORT_CHILD").is_some()
    }

    /// Waits for another thread to block on I/O, then aborts.
    pub(crate) fn abort_once_blocked() -> ! {
        while !super::BLOCKING.load(SeqCst) {
            std::thread::yield_now();
        }
        crate::abort::die(&[b"stopped"])
    }

    /// The exit code and standard error of this test binary running the test
    /// `name` alone as [`in_the_child`], whose input never ends and whose
    /// output is never read. A child still running after 30 seconds hung.
    pub(crate) fn exit_of(name: &str) -> (Option<i32>, String) {
        use std::io::Read as _;
        use std::process::{Command, Stdio};
        let mut child = Command::new(std::env::current_exe().expect("this test binary"))
            .args(["--exact", name, "--test-threads=1", "--nocapture"])
            .env("BURI_RT_TEST_ABORT_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the test binary runs");
        let (_input, _output) = (child.stdin.take(), child.stdout.take());
        let mut stderr = child.stderr.take().expect("a piped stderr");
        let (done, read) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            let _ = done.send(text);
        });
        let Ok(text) = read.recv_timeout(std::time::Duration::from_secs(30)) else {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child never exited");
        };
        (child.wait().expect("the child exits").code(), text)
    }

    /// **An abort waits for a lock another thread holds**, so the exit never
    /// kills a thread inside a critical section, which helgrind reports as
    /// "Exiting thread still holds 1 lock" (#311).
    #[test]
    fn an_abort_waits_for_a_lock_another_thread_holds() {
        if in_the_child() {
            static HELD: Mutex<()> = Mutex::new(());
            let (ready, wait) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let guard = HELD.lock();
                ready.send(()).unwrap();
                while !super::reported::closing() {
                    std::thread::yield_now();
                }
                eprintln!("released");
                drop(guard);
            });
            wait.recv().unwrap();
            crate::abort::die(&[b"stopped"]);
        }
        let (code, stderr) = exit_of("sync::tests::an_abort_waits_for_a_lock_another_thread_holds");
        assert_eq!(code, Some(1), "{stderr}");
        assert!(
            stderr.contains("stopped\nreleased\n"),
            "the abort didn't wait for the lock:\n{stderr}"
        );
    }

    /// **An abort ends a program another thread of which waits for input**
    /// that never comes.
    #[test]
    fn an_abort_ends_a_read_of_input_that_never_ends() {
        if in_the_child() {
            std::thread::spawn(|| {
                let mut line = crate::value::BuriStr::empty();
                // SAFETY: a live, aligned destination.
                unsafe { crate::host::buri_rt_host_stdin_read_line(&raw mut line) };
            });
            abort_once_blocked();
        }
        let (code, stderr) = exit_of("sync::tests::an_abort_ends_a_read_of_input_that_never_ends");
        assert_eq!(code, Some(1), "{stderr}");
        assert!(stderr.contains("stopped\n"), "{stderr}");
    }

    /// **An exit ends a program another thread of which writes to output
    /// nobody reads.** This exit skips the flush an abort does first, which
    /// would wait its turn behind that write: the writer started after it.
    #[test]
    fn an_exit_ends_a_write_nobody_reads() {
        if in_the_child() {
            std::thread::spawn(|| {
                let text = vec![b'x'; 1 << 20];
                // SAFETY: a live view of `text`.
                unsafe {
                    crate::host::buri_rt_host_stdout_print(
                        std::ptr::null_mut(),
                        text.as_ptr(),
                        text.len() as u64,
                    )
                };
            });
            while !super::BLOCKING.load(SeqCst) {
                std::thread::yield_now();
            }
            eprintln!("stopped");
            super::close();
            std::process::exit(1);
        }
        let (code, stderr) = exit_of("sync::tests::an_exit_ends_a_write_nobody_reads");
        assert_eq!(code, Some(1), "{stderr}");
        assert!(stderr.contains("stopped\n"), "{stderr}");
    }
}
