//! Wide (256-bit-intermediate) multiply-divide for notional math.
//!
//! `qty · price · bps / 10_000` can overflow a `u128` intermediate at extreme
//! operator configs (a huge `tick_size` × a huge order quantity), which would
//! abort the transaction under `overflow-checks`. `mul_div_floor`/`mul_div_ceil`
//! compute `floor|ceil(a · b / d)` exactly via a 256-bit product, with a fast
//! `u128` path for the common (non-overflowing) case. No floats; rounds as named.

/// Full 128×128 → 256-bit product as `(hi, lo)` where `a·b = hi·2^128 + lo`.
/// `pub(crate)` so the Kani harnesses (`kani_proofs`) can verify it directly.
#[inline(always)]
pub(crate) fn wide_mul(a: u128, b: u128) -> (u128, u128) {
    let a_lo = a as u64 as u128;
    let a_hi = a >> 64;
    let b_lo = b as u64 as u128;
    let b_hi = b >> 64;

    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;

    let mask = 0xFFFF_FFFF_FFFF_FFFFu128;
    let cross = (ll >> 64) + (lh & mask) + (hl & mask);
    let lo = (ll & mask) | (cross << 64);
    let hi = hh + (lh >> 64) + (hl >> 64) + (cross >> 64);
    (hi, lo)
}

/// Divide the 256-bit value `(hi:lo)` by `d`, returning `(quotient, remainder)`,
/// or `None` if the quotient would not fit in `u128` (i.e. `hi >= d`). Schoolbook
/// binary long division; the quotient is built MSB-first. Verified by
/// `fuzz_wide_vs_u256_reference` (the 128-iteration unroll over a symbolic 128-bit
/// dividend is beyond CBMC, so this is fuzz- not Kani-verified — see `kani_proofs`).
fn div_256(hi: u128, lo: u128, d: u128) -> Option<(u128, u128)> {
    if d == 0 || hi >= d {
        return None;
    }
    let mut rem = hi; // invariant: rem < d at the start of each step
    let mut quo: u128 = 0;
    let mut i: i32 = 127;
    while i >= 0 {
        let bit = (lo >> i) & 1;
        let carry = rem >> 127; // bit shifted out of the u128 on the left shift
        let shifted = (rem << 1) | bit; // low 128 bits of rem·2 + bit
        if carry == 1 || shifted >= d {
            // true value (carry·2^128 + shifted) − d fits u128 because rem' < 2d.
            rem = shifted.wrapping_sub(d);
            quo = (quo << 1) | 1;
        } else {
            rem = shifted;
            quo <<= 1;
        }
        i -= 1;
    }
    Some((quo, rem))
}

/// `floor(a · b / d)`, or `None` on a zero divisor or a quotient exceeding u128.
#[inline]
pub fn mul_div_floor(a: u128, b: u128, d: u128) -> Option<u128> {
    if d == 0 {
        return None;
    }
    if let Some(p) = a.checked_mul(b) {
        return Some(p / d);
    }
    let (hi, lo) = wide_mul(a, b);
    div_256(hi, lo, d).map(|(q, _)| q)
}

/// `ceil(a · b / d)`, or `None` on a zero divisor or a quotient exceeding u128.
#[inline]
pub fn mul_div_ceil(a: u128, b: u128, d: u128) -> Option<u128> {
    if d == 0 {
        return None;
    }
    if let Some(p) = a.checked_mul(b) {
        let q = p / d;
        return if p % d != 0 {
            q.checked_add(1)
        } else {
            Some(q)
        };
    }
    let (hi, lo) = wide_mul(a, b);
    let (q, r) = div_256(hi, lo, d)?;
    if r != 0 {
        q.checked_add(1)
    } else {
        Some(q)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_cases() {
        assert_eq!(mul_div_floor(10, 3, 4), Some(7)); // 30/4 = 7
        assert_eq!(mul_div_ceil(10, 3, 4), Some(8));
        assert_eq!(mul_div_floor(10, 3, 5), Some(6)); // exact
        assert_eq!(mul_div_ceil(10, 3, 5), Some(6));
        assert_eq!(mul_div_floor(0, 5, 7), Some(0));
        assert_eq!(mul_div_floor(5, 5, 0), None);
    }

    #[test]
    fn test_overflowing_intermediate() {
        // a·b overflows u128 but a·b/d fits: (2^127)·4 / 8 = 2^126.
        let a = 1u128 << 127;
        assert_eq!(mul_div_floor(a, 4, 8), Some(1u128 << 126));
        assert_eq!(mul_div_ceil(a, 4, 8), Some(1u128 << 126));
        // u128::MAX · u128::MAX / u128::MAX == u128::MAX.
        assert_eq!(
            mul_div_floor(u128::MAX, u128::MAX, u128::MAX),
            Some(u128::MAX)
        );
        // quotient overflows u128 → None.
        assert_eq!(mul_div_floor(u128::MAX, u128::MAX, 1), None);
    }

    #[test]
    fn fuzz_matches_u128_fast_path() {
        // For inputs whose product fits u128, the wide path must equal plain math.
        let mut seed: u64 = 0xA0761D64_78BD642F;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 33
        };
        for _ in 0..50_000 {
            let a = (next() as u128) % 1_000_000_000;
            let b = (next() as u128) % 1_000_000_000;
            let d = 1 + (next() as u128) % 1_000_000_000;
            let p = a * b;
            assert_eq!(mul_div_floor(a, b, d), Some(p / d));
            let ceil = p / d + if p.is_multiple_of(d) { 0 } else { 1 };
            assert_eq!(mul_div_ceil(a, b, d), Some(ceil));
        }
    }

    #[test]
    fn fuzz_wide_vs_u256_reference() {
        // Cross-check the wide path against a slow but obvious bit-reference using
        // i128-free big arithmetic emulated with the same wide_mul + a long divide.
        let mut seed: u64 = 0x1D8E_4E27_C47D_124F;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 33
        };
        for _ in 0..50_000 {
            // Large operands that frequently overflow u128 when multiplied.
            let a = (next() as u128) << 40 | next() as u128;
            let b = (next() as u128) << 40 | next() as u128;
            let d = 1 + ((next() as u128) << 20 | next() as u128);
            let (hi, lo) = wide_mul(a, b);
            match div_256(hi, lo, d) {
                Some((q, r)) => {
                    // Reconstruct q·d + r == a·b (256-bit) and r < d.
                    assert!(r < d);
                    let (qd_hi, qd_lo) = wide_mul(q, d);
                    let (sum_lo, carry) = qd_lo.overflowing_add(r);
                    let sum_hi = qd_hi + carry as u128;
                    assert_eq!((sum_hi, sum_lo), (hi, lo), "q·d + r != a·b");
                }
                None => assert!(hi >= d, "None only when quotient overflows u128"),
            }
        }
    }
}
