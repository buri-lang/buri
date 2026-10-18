//! The timed goals' yardstick: a fixed workload, timed right before and right
//! after each phase, that says how fast this machine is at that moment
//! (`design/PERFORMANCE.md` §6.82).
//!
//! ```text
//! time on the reference core = median(phase / yardstick) * REFERENCE
//! ```
//!
//! A shared runner that's 2x slow makes both 2x slow, and the ratio doesn't
//! move. A compiler that's 2x slow moves only the phase. The workload uses no
//! compiler code, so no compiler change can speed it up or slow it down.

use std::collections::HashMap;
use std::time::Duration;

/// The yardstick's fastest run on one performance core of the M3 Pro the goals
/// were set on. Re-measure with `--goals=reference` whenever [`Yardstick`]
/// changes; `CHECKSUM` fails a test until you do.
pub const REFERENCE: Duration = Duration::from_micros(58_500);

/// What [`Yardstick::run`] returns. A different value is different work.
pub const CHECKSUM: u64 = 14_140_823_603_909_224_060;

/// Built once, outside any timing. A run then pays for the three things a
/// checker spends its time on: chasing pointers, hashing and branching.
pub struct Yardstick {
    /// One random cycle through 32 MB, bigger than any cache it runs on.
    next: Vec<u32>,
}

const SLOTS: usize = 1 << 23;
const CHASES: usize = 1 << 19;
const KEYS: u64 = 1 << 16;
const BRANCHES: usize = 1 << 22;

impl Yardstick {
    pub fn new() -> Self {
        // Sattolo's shuffle: one cycle through every slot.
        let mut next: Vec<u32> = (0..SLOTS as u32).collect();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for i in (1..SLOTS).rev() {
            let j = (rng.next() % i as u64) as usize;
            next.swap(i, j);
        }
        Yardstick { next }
    }

    pub fn run(&self) -> u64 {
        let mut at = 0u32;
        for _ in 0..CHASES {
            at = self.next[at as usize];
        }
        let mut sum = u64::from(at);

        let mut rng = Rng(sum | 1);
        let mut map: HashMap<u64, Vec<u64>> = HashMap::new();
        for _ in 0..KEYS {
            let k = rng.next() % (KEYS / 2);
            map.entry(k).or_default().push(k);
        }
        for _ in 0..KEYS {
            sum += map.get(&(rng.next() % KEYS)).map_or(1, |v| v.len() as u64);
        }

        for _ in 0..BRANCHES {
            sum = match rng.next() % 7 {
                0 => sum.wrapping_add(3),
                1 | 2 => sum ^ 0x55,
                3 => sum.rotate_left(5),
                4 => sum.wrapping_mul(31),
                _ => sum.wrapping_sub(1),
            };
        }
        sum
    }
}

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// One phase run, between two yardstick runs.
#[derive(Clone, Copy)]
pub struct Sample {
    pub before: Duration,
    pub phase: Duration,
    pub after: Duration,
}

/// What the phase would take on the reference core, given `reference`, the
/// yardstick's time there. The median ignores a sample where the load changed
/// mid-sample.
pub fn on_reference(samples: &[Sample], reference: Duration) -> Duration {
    reference.mul_f64(median(samples.iter().map(|s| {
        s.phase.as_secs_f64() * 2.0 / (s.before + s.after).as_secs_f64()
    })))
}

/// The phase's median time on this machine, unscaled.
pub fn median_phase(samples: &[Sample]) -> Duration {
    Duration::from_secs_f64(median(samples.iter().map(|s| s.phase.as_secs_f64())))
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = values.collect();
    v.sort_by(f64::total_cmp);
    match v.len() {
        0 => 0.0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}
