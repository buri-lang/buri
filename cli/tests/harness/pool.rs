//! Running a corpus's cases at once, and answering in the order they are in.
//!
//! A repository case is a scratch copy and a handful of `buri` invocations, and
//! almost all of what one costs is waiting on those children. Run one at a time
//! a corpus leaves nine of ten cores idle, which is what took the `ui` corpus to
//! four minutes when the snapshot sweeps took it from nine cases to thirty-five.
//!
//! Two rules make that safe to do, and they are the whole of the design:
//!
//! * **The order is the corpus's, not the scheduler's.** [`map`] answers a
//!   `Vec` indexed like the slice it was given, so a run's failures are
//!   collected and printed in exactly the order a one-at-a-time run printed
//!   them. A case that panics is re-raised after every worker has stopped, and
//!   the one re-raised is the *first* in corpus order — again what a
//!   one-at-a-time run would have reported.
//! * **The width is the binary's, not the caller's.** Cargo already runs this
//!   binary's `#[test]`s on their own threads, so fifteen corpora each opening
//!   `available_parallelism` workers would be a hundred and fifty `buri`
//!   processes on a ten-core machine. Every case takes a permit from one gate
//!   shared by the whole process instead, so the number in flight is the
//!   machine's width however many corpora are running.
//!
//! Nothing else is shared. A case's scratch tree is named for the process and a
//! counter ([`super::Scratch::empty`]), its goldens live in its own directory,
//! and no case in a corpus that comes through here opens a socket.
use std::any::Any;
use std::sync::{Condvar, Mutex, OnceLock};

/// How many cases may be running anywhere in this test binary at once.
fn width() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// The permits, and somewhere to wait for one.
struct Gate {
    free: Mutex<usize>,
    room: Condvar,
}

fn gate() -> &'static Gate {
    static GATE: OnceLock<Gate> = OnceLock::new();
    GATE.get_or_init(|| Gate { free: Mutex::new(width()), room: Condvar::new() })
}

/// A seat at the machine, given back when it drops — including when the case
/// holding it panics, because unwinding runs this.
struct Permit;

fn permit() -> Permit {
    let gate = gate();
    let mut free = gate.free.lock().unwrap_or_else(|e| e.into_inner());
    while *free == 0 {
        free = gate.room.wait(free).unwrap_or_else(|e| e.into_inner());
    }
    *free -= 1;
    Permit
}

impl Drop for Permit {
    fn drop(&mut self) {
        let gate = gate();
        *gate.free.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        gate.room.notify_one();
    }
}

/// What one worker came back with: the item's answer, or the panic it took.
type Answer<R> = Result<R, Box<dyn Any + Send>>;

/// `f` over every item, on several threads, answered in the items' own order.
///
/// A panic inside `f` is caught, every other item is still attempted, and the
/// earliest one to panic is re-raised once they have all stopped. So a broken
/// case fails the run with its own message, the way it does when the corpus is
/// walked one case at a time.
pub fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = width().min(items.len()).max(1);
    let next = Mutex::new(0usize);
    let done: Mutex<Vec<(usize, Answer<R>)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let at = {
                    let mut next = next.lock().unwrap_or_else(|e| e.into_inner());
                    let at = *next;
                    *next += 1;
                    at
                };
                let Some(item) = items.get(at) else { return };
                let answer = {
                    let _permit = permit();
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(item)))
                };
                done.lock().unwrap_or_else(|e| e.into_inner()).push((at, answer));
            });
        }
    });

    let mut answers = done.into_inner().unwrap_or_else(|e| e.into_inner());
    answers.sort_by_key(|(at, _)| *at);
    let mut out = Vec::with_capacity(answers.len());
    for (_, answer) in answers {
        match answer {
            Ok(r) => out.push(r),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
    out
}

#[cfg(test)]
mod pool_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The answers are the items', in the items' order, however the work landed
    /// on the threads.
    #[test]
    fn the_answers_come_back_in_the_order_the_items_were_in() {
        let items: Vec<usize> = (0..200).collect();
        assert_eq!(map(&items, |n| n * 2), items.iter().map(|n| n * 2).collect::<Vec<_>>());
    }

    /// Never more cases in flight than the machine is wide, which is the claim
    /// the gate exists to make.
    #[test]
    fn no_more_cases_run_at_once_than_there_are_permits() {
        static LIVE: AtomicUsize = AtomicUsize::new(0);
        static PEAK: AtomicUsize = AtomicUsize::new(0);
        let items: Vec<usize> = (0..64).collect();
        map(&items, |_| {
            let live = LIVE.fetch_add(1, Ordering::SeqCst) + 1;
            PEAK.fetch_max(live, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(2));
            LIVE.fetch_sub(1, Ordering::SeqCst);
        });
        assert!(
            PEAK.load(Ordering::SeqCst) <= width(),
            "{} cases ran at once on a machine {} wide",
            PEAK.load(Ordering::SeqCst),
            width()
        );
    }

    /// The first case to panic *in the corpus's order* is the one the run
    /// reports, whichever thread reached its own panic first.
    #[test]
    fn the_earliest_panic_is_the_one_raised() {
        let items: Vec<usize> = (0..40).collect();
        let raised = std::panic::catch_unwind(|| {
            map(&items, |n| {
                if *n == 7 || *n == 23 {
                    panic!("case {n} is broken");
                }
            })
        })
        .expect_err("a panicking case fails the run");
        let said = raised.downcast_ref::<String>().map(String::as_str).unwrap_or_default();
        assert_eq!(said, "case 7 is broken");
    }
}
