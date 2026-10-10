//! The runtime's `Mutex` and `Condvar`: `std`'s, which helgrind can't see,
//! because they are a futex rather than a pthread call.
//!
//! With the `valgrind` feature, each lock and unlock is reported to helgrind
//! as one (`valgrind.rs`), so it orders what threads do under a lock and sees
//! lock-order inversions. Without it these are `std`'s types themselves.

#[cfg(not(feature = "valgrind"))]
pub(crate) use std::sync::{Condvar, Mutex, MutexGuard};

#[cfg(feature = "valgrind")]
pub(crate) use reported::{Condvar, Mutex, MutexGuard};

#[cfg(feature = "valgrind")]
mod reported {
    use crate::allocator::valgrind;
    use std::ops::{Deref, DerefMut};
    use std::sync::{LockResult, PoisonError, TryLockError, TryLockResult, WaitTimeoutResult};
    use std::time::Duration;

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
                if valgrind::helgrind() {
                    valgrind::acquired(lock);
                }
                MutexGuard { inner: Some(inner), lock }
            };
            match self.0.lock() {
                Ok(inner) => Ok(taken(inner)),
                Err(poisoned) => Err(PoisonError::new(taken(poisoned.into_inner()))),
            }
        }

        pub(crate) fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
            let lock = self.address();
            let taken = |inner| {
                if valgrind::helgrind() {
                    valgrind::acquired(lock);
                }
                MutexGuard { inner: Some(inner), lock }
            };
            match self.0.try_lock() {
                Ok(inner) => Ok(taken(inner)),
                Err(TryLockError::Poisoned(poisoned)) => {
                    Err(TryLockError::Poisoned(PoisonError::new(taken(poisoned.into_inner()))))
                }
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
            if self.inner.is_some() && valgrind::helgrind() {
                valgrind::released(self.lock);
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

        pub(crate) fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> LockResult<MutexGuard<'a, T>> {
            let lock = guard.lock;
            let back = |inner| {
                if valgrind::helgrind() {
                    valgrind::acquired(lock);
                }
                MutexGuard { inner: Some(inner), lock }
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
                if valgrind::helgrind() {
                    valgrind::acquired(lock);
                }
                (MutexGuard { inner: Some(inner), lock }, result)
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
