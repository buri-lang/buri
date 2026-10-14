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
        HELD_HERE.set(HELD_HERE.get() + 1);
        true
    }

    #[inline(never)]
    fn leave() {
        HELD_HERE.set(HELD_HERE.get() - 1);
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

#[cfg(all(test, feature = "valgrind"))]
mod tests {
    use super::Mutex;

    /// **An abort waits for a lock another thread holds**, so the exit never
    /// kills a thread inside a critical section, which helgrind reports as
    /// "Exiting thread still holds 1 lock" (#311).
    #[test]
    fn an_abort_waits_for_a_lock_another_thread_holds() {
        if std::env::var_os("BURI_RT_TEST_ABORT_CHILD").is_some() {
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
        let name = "sync::tests::an_abort_waits_for_a_lock_another_thread_holds";
        let out = std::process::Command::new(std::env::current_exe().expect("this test binary"))
            .args(["--exact", name, "--test-threads=1", "--nocapture"])
            .env("BURI_RT_TEST_ABORT_CHILD", "1")
            .output()
            .expect("the test binary runs");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert!(
            stderr.contains("stopped\nreleased\n"),
            "the abort didn't wait for the lock:\n{stderr}"
        );
    }
}
