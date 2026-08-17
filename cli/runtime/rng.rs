//! The random source behind `Rand`.
//!
//! `Rand` is an effect, so a program that never binds it cannot reach this, and
//! a test that wants determinism binds a fake instead of reseeding a global —
//! which is why there is no `seed` entry point here and no way to ask for one.
//!
//! xoshiro256++ over a seed from the operating system. No dependency, about
//! forty lines, and the properties that matter for the two operations `core/cap`
//! actually declares (`nextInt`, `nextFloat`) are properties xoshiro has: a
//! 2^256 period, passes BigCrush, and is four instructions per word.
//!
//! It is deliberately **not** a cryptographic generator, and `core/cap` does not
//! claim one. A `Rand` that promised unpredictability would need to say so in
//! its own documentation and would be a different effect.

use std::sync::Mutex;

static STATE: Mutex<Option<[u64; 4]>> = Mutex::new(None);

/// SplitMix64, which is what xoshiro's own authors specify for expanding a seed:
/// four words from one, with no correlation between them.
fn split_mix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Eight bytes from the operating system, or a time-derived fallback.
///
/// `/dev/urandom` is present on both supported platforms and needs no crate to
/// read. The fallback exists because a program in a sandbox with no `/dev` is a
/// program that should still run: a weak seed is a worse random number, and a
/// hard failure here would be a worse outcome than a weak one for the uses this
/// effect is declared for.
fn os_seed() -> u64 {
    use std::io::Read;
    let mut buf = [0_u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            return u64::from_le_bytes(buf);
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x0123_4567_89AB_CDEF);
    now ^ (&buf as *const u8 as u64)
}

fn next_u64() -> u64 {
    let mut guard = match STATE.lock() {
        Ok(g) => g,
        // The language has no threads, so a poisoned lock means a panic already
        // happened inside this runtime; recovering the state is strictly better
        // than a second failure on top of the first.
        Err(poisoned) => poisoned.into_inner(),
    };
    let s = guard.get_or_insert_with(|| {
        let mut seed = os_seed();
        [split_mix(&mut seed), split_mix(&mut seed), split_mix(&mut seed), split_mix(&mut seed)]
    });

    let result = s[0].wrapping_add(s[3]).rotate_left(23).wrapping_add(s[0]);
    let t = s[1] << 17;
    s[2] ^= s[0];
    s[3] ^= s[1];
    s[1] ^= s[2];
    s[0] ^= s[3];
    s[2] ^= t;
    s[3] = s[3].rotate_left(45);
    result
}

/// A uniform value in `0 ..< range`, without modulo bias.
///
/// Lemire's multiply-and-reject: one 64x64 multiply in the common case, and a
/// rejection loop that runs with probability under `range / 2^64`.
fn below(range: u64) -> u64 {
    let mut m = u128::from(next_u64()).wrapping_mul(u128::from(range));
    let mut low = m as u64;
    if low < range {
        let threshold = range.wrapping_neg() % range;
        while low < threshold {
            m = u128::from(next_u64()).wrapping_mul(u128::from(range));
            low = m as u64;
        }
    }
    (m >> 64) as u64
}

/// A uniform integer in `lo ..< hi`. The caller has already rejected `hi <= lo`.
pub fn int_in(lo: i64, hi: i64) -> i64 {
    let range = (hi as i128 - lo as i128) as u128 as u64;
    lo.wrapping_add(below(range) as i64)
}

/// A uniform `f64` in `[0, 1)`, with 53 bits of mantissa.
///
/// The same interval `Math.random()` promises, so a program that scales it is
/// scaling the same shape on both backends even though the bits differ.
pub fn float() -> f64 {
    (next_u64() >> 11) as f64 * (1.0 / (1_u64 << 53) as f64)
}
