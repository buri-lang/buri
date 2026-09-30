//! A JSON number as the decimal its text spells, so that `1.0` is an integer,
//! `0.3` is a multiple of `0.1`, and a comparison never rounds.

use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Number {
    Finite(Decimal),
    /// JSON5's `Infinity` and `-Infinity`. `true` is negative.
    Infinity(bool),
    /// JSON5's `NaN`.
    NaN,
}

/// `digits × 10^exp`, normalized: no leading or trailing zero digits, and zero
/// is no digits at all, with no sign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decimal {
    negative: bool,
    digits: Vec<u8>,
    exp: i64,
}

impl Decimal {
    /// From the parts of a decimal literal: the digits before the point, the
    /// digits after it, and the exponent.
    pub fn from_parts(negative: bool, whole: &str, fraction: &str, exp: i64) -> Decimal {
        let digits: Vec<u8> =
            whole.bytes().chain(fraction.bytes()).map(|b| b.saturating_sub(b'0')).collect();
        let fraction_len = i64::try_from(fraction.len()).unwrap_or(i64::MAX);
        Decimal::normalized(negative, digits, exp.saturating_sub(fraction_len))
    }

    /// From JSON5's `0x` digits.
    pub fn from_hex(negative: bool, hex: &str) -> Decimal {
        // Little-endian decimal digits, multiplied by sixteen per hex digit.
        let mut digits: Vec<u8> = Vec::new();
        for c in hex.chars() {
            let mut carry = c.to_digit(16).unwrap_or(0);
            for d in digits.iter_mut() {
                let v = u32::from(*d).saturating_mul(16).saturating_add(carry);
                *d = u8::try_from(v % 10).unwrap_or(0);
                carry = v / 10;
            }
            while carry > 0 {
                digits.push(u8::try_from(carry % 10).unwrap_or(0));
                carry /= 10;
            }
        }
        digits.reverse();
        Decimal::normalized(negative, digits, 0)
    }

    fn normalized(negative: bool, mut digits: Vec<u8>, mut exp: i64) -> Decimal {
        let lead = digits.iter().take_while(|d| **d == 0).count();
        digits.drain(..lead);
        while digits.last() == Some(&0) {
            digits.pop();
            exp = exp.saturating_add(1);
        }
        if digits.is_empty() {
            return Decimal { negative: false, digits, exp: 0 };
        }
        Decimal { negative, digits, exp }
    }

    pub fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    pub fn is_integer(&self) -> bool {
        self.exp >= 0
    }

    /// Where the leading digit sits, which orders two magnitudes before any
    /// digit is compared.
    fn magnitude(&self) -> i64 {
        i64::try_from(self.digits.len()).unwrap_or(i64::MAX).saturating_add(self.exp)
    }

    fn cmp_magnitude(&self, other: &Decimal) -> Ordering {
        match (self.is_zero(), other.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => {}
        }
        match self.magnitude().cmp(&other.magnitude()) {
            Ordering::Equal => {}
            unequal => return unequal,
        }
        let len = self.digits.len().max(other.digits.len());
        for i in 0..len {
            let a = self.digits.get(i).copied().unwrap_or(0);
            let b = other.digits.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                Ordering::Equal => {}
                unequal => return unequal,
            }
        }
        Ordering::Equal
    }

    /// Whether `self / divisor` is an integer. `divisor` is positive.
    pub fn is_multiple_of(&self, divisor: &Decimal) -> bool {
        if self.is_zero() {
            return true;
        }
        // `self` has no trailing zero, so `divisor × 10^k` with k > 0 cannot
        // divide it.
        if self.exp < divisor.exp {
            return false;
        }
        // The divisor's digits, when they fit with room for one more.
        let modulus = match divisor.digits.len() {
            0 => return false,
            1..=37 => divisor.digits.iter().fold(0u128, |n, d| {
                n.saturating_mul(10).saturating_add(u128::from(*d))
            }),
            _ => return self.f64_multiple_of(divisor),
        };
        let mut rest = self
            .digits
            .iter()
            .fold(0u128, |r, d| {
                r.saturating_mul(10).saturating_add(u128::from(*d)).checked_rem(modulus).unwrap_or(0)
            });
        // Past the 127th zero every power of two and of five a 128-bit divisor
        // can hold has been supplied, so more zeros change nothing.
        let zeros = self.exp.saturating_sub(divisor.exp).min(130);
        for _ in 0..zeros {
            rest = rest.saturating_mul(10).checked_rem(modulus).unwrap_or(0);
        }
        rest == 0
    }

    fn f64_multiple_of(&self, divisor: &Decimal) -> bool {
        let q = self.to_f64() / divisor.to_f64();
        q.is_finite() && q.fract() == 0.0
    }

    /// The integer, when it is one an `i64` holds.
    pub fn to_i64(&self) -> Option<i64> {
        if !self.is_integer() {
            return None;
        }
        let mut n: i64 = 0;
        let zeros = usize::try_from(self.exp).ok()?;
        for d in self.digits.iter().copied().chain(std::iter::repeat_n(0, zeros)) {
            n = n.checked_mul(10)?.checked_add(i64::from(d))?;
        }
        Some(if self.negative { n.checked_neg()? } else { n })
    }

    pub fn to_f64(&self) -> f64 {
        if self.is_zero() {
            return 0.0;
        }
        let digits: String = self.digits.iter().map(|d| char::from(b'0'.saturating_add(*d))).collect();
        let sign = if self.negative { "-" } else { "" };
        format!("{sign}{digits}e{}", self.exp).parse().unwrap_or(f64::NAN)
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Decimal) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => self.cmp_magnitude(other),
            (true, true) => other.cmp_magnitude(self),
        }
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Decimal) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(text: &str) -> Decimal {
        let (negative, text) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (mantissa, exp) = match text.split_once('e') {
            Some((m, e)) => (m, e.parse().unwrap()),
            None => (text, 0),
        };
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        Decimal::from_parts(negative, whole, fraction, exp)
    }

    #[test]
    fn equal_values_are_equal_however_they_are_spelled() {
        assert_eq!(d("1.0"), d("1"));
        assert_eq!(d("100"), d("1e2"));
        assert_eq!(d("-0"), d("0.000"));
        assert!(d("1.0").is_integer());
        assert!(!d("1.5").is_integer());
        assert!(d("1e2").is_integer());
    }

    #[test]
    fn comparison_is_exact() {
        assert!(d("0.1") < d("0.10000000000000000001"));
        assert!(d("-5") < d("-4.9"));
        assert!(d("-0.5") < d("0"));
        assert!(d("99") < d("100"));
        assert!(d("1e400") > d("9e399"));
    }

    #[test]
    fn multiples_are_exact() {
        assert!(d("0.3").is_multiple_of(&d("0.1")));
        assert!(d("0.0075").is_multiple_of(&d("0.0001")));
        assert!(!d("0.00751").is_multiple_of(&d("0.0001")));
        assert!(d("10").is_multiple_of(&d("2")));
        assert!(!d("7").is_multiple_of(&d("2")));
        assert!(!d("1e308").is_multiple_of(&d("0.123456789")));
        assert!(d("4.5").is_multiple_of(&d("1.5")));
        assert!(!d("35").is_multiple_of(&d("1.5")));
    }

    #[test]
    fn hex_reads_as_its_decimal_value() {
        assert_eq!(Decimal::from_hex(false, "ff"), d("255"));
        assert_eq!(Decimal::from_hex(true, "10"), d("-16"));
    }
}
