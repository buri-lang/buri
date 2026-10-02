//! One corpus as several `#[test]`s, so the runner can spread it over cores.
//!
//! nextest runs each test in a process of its own, so a corpus walked by one
//! test is one process however wide the machine is, and the run waits for it.
//! [`shards!`] cuts the corpus into a fixed number of tests instead. Shard `k`
//! of `n` takes every `n`th case starting at `k` ([`of`]), so the shards
//! together are the corpus, each case exactly once. Every sharded corpus also
//! gets `every_case_is_in_exactly_one_shard`, which checks that against the
//! corpus's real length.
//!
//! A shard is a plain `#[test]`, so `cargo test` runs it too, on its threads.
#![allow(dead_code, unused_macros)]

/// Shard `at` of `count`: every `count`th item, starting at `at`.
pub fn of<T>(items: &[T], at: usize, count: usize) -> Vec<&T> {
    assert!(at < count, "shard {at} of {count} does not exist");
    items.iter().skip(at).step_by(count).collect()
}

/// Panics unless the `count` shards of `len` items hold every index exactly
/// once.
pub fn assert_partition(len: usize, count: usize) {
    let indices: Vec<usize> = (0..len).collect();
    let mut seen = vec![0usize; len];
    for at in 0..count {
        for &i in of(&indices, at, count) {
            seen[i] += 1;
        }
    }
    let wrong: Vec<usize> = (0..len).filter(|&i| seen[i] != 1).collect();
    assert!(
        wrong.is_empty(),
        "{count} shards of {len} cases do not cover each case exactly once: {wrong:?}"
    );
}

/// `name(run, len) = shard_0 shard_1 …;` makes a module `name` holding one
/// `#[test]` per shard identifier, each calling `run(at, count)`, and
/// `every_case_is_in_exactly_one_shard`, which calls `len()` for the corpus
/// size. `run` and `len` are looked up in the enclosing module.
macro_rules! shards {
    ($(#[$doc:meta])* $name:ident($run:ident, $len:ident) = $($shard:ident)+;) => {
        $(#[$doc])*
        mod $name {
            const COUNT: usize = [$(stringify!($shard)),+].len();
            shards!(@each $run, 0usize; $($shard)+);

            #[test]
            fn every_case_is_in_exactly_one_shard() {
                super::shard::assert_partition(super::$len(), COUNT);
            }
        }
    };
    (@each $run:ident, $at:expr; $first:ident $($rest:ident)*) => {
        #[test]
        fn $first() {
            super::$run($at, COUNT);
        }
        shards!(@each $run, $at + 1; $($rest)*);
    };
    (@each $run:ident, $at:expr;) => {};
}

#[cfg(test)]
mod shard_tests {
    use super::*;

    #[test]
    fn the_shards_are_the_items_in_order_each_once() {
        for len in 0..40 {
            for count in 1..12 {
                assert_partition(len, count);
            }
        }
        let items: Vec<usize> = (0..10).collect();
        assert_eq!(of(&items, 1, 4), [&1, &5, &9]);
    }
}
