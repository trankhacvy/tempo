//! Marketable-fill / passive-park classification of a resting order against the
//! oracle-anchored tick window (DDR-3).
//!
//! Byte-for-byte mirror of the program's `state::classify_resting_fold` — the
//! program is the source of truth. Kept **overflow-safe** (no `num_ticks *
//! tick_size` product; the classification is done by `price < floor` and the same
//! `(price - floor) / tick_size` offset the program uses) and **golden-guarded**
//! against the program's own test vectors below, so an off-chain reader (the keeper's
//! completeness view) can never drift from the on-chain completeness gate.

use crate::error::MathError;

/// Where a resting order folds relative to the current (recentered) tick window.
/// Mirrors the program's `state::RestingFold`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestingFold {
    /// Price is inside `[floor, top)` — fold at its own tick (the normal case).
    InWindow(u32),
    /// Off-window but marketable (SELL below floor / BUY above top) — fold at the
    /// boundary tick so it clears this round at the uniform clearing price.
    Marketable(u32),
    /// Off-window and passive (SELL above top / BUY below floor) — do NOT fold:
    /// leave it `Resting` and exempt it from the completeness gate.
    Passive,
}

/// Classify a resting order against the current window (DDR-3). Pure and
/// deterministic in `(price, is_sell, floor, tick_size, num_ticks)`, matching the
/// program's `classify_resting_fold`. `is_sell` mirrors the program's `OrderSide`
/// (Sell = 1, Buy = 0).
#[inline]
pub fn classify_resting_fold(
    price: u64,
    is_sell: bool,
    floor: u64,
    tick_size: u64,
    num_ticks: u32,
) -> Result<RestingFold, MathError> {
    if price == 0 || tick_size == 0 || num_ticks == 0 || !price.is_multiple_of(tick_size) {
        return Err(MathError::InvalidPrice);
    }
    if price < floor {
        // Below the window: a SELL is marketable (fold at tick 0); a BUY is passive.
        return Ok(if is_sell {
            RestingFold::Marketable(0)
        } else {
            RestingFold::Passive
        });
    }
    let offset = (price - floor) / tick_size;
    if offset >= num_ticks as u64 {
        // Above the window: a BUY is marketable (fold at the top tick); a SELL is passive.
        return Ok(if is_sell {
            RestingFold::Passive
        } else {
            RestingFold::Marketable(num_ticks - 1)
        });
    }
    Ok(RestingFold::InWindow(offset as u32))
}

/// A resting order the window has moved AWAY from — it legitimately cannot fold this
/// round (the on-chain completeness gate exempts exactly these). A malformed price
/// (classification error) is treated as *not* passive (it must still fold), matching
/// the program, where such an order would block finalize rather than be exempted.
#[inline]
pub fn is_passive(price: u64, is_sell: bool, floor: u64, tick_size: u64, num_ticks: u32) -> bool {
    matches!(
        classify_resting_fold(price, is_sell, floor, tick_size, num_ticks),
        Ok(RestingFold::Passive)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tick::price_to_tick_raw;

    // Golden guard: these are the program's own `test_classify_resting_fold` vectors
    // (state/market.rs). is_sell: true = Sell, false = Buy.
    #[test]
    fn golden_matches_program_classifier() {
        // Window [100, 100 + 10*64) = [100, 740): floor 100, tick_size 10, 64 ticks.
        let (floor, ts, nt) = (100u64, 10u64, 64u32);

        // In-window: folds at its own tick regardless of side.
        assert_eq!(
            classify_resting_fold(150, true, floor, ts, nt).unwrap(),
            RestingFold::InWindow(5)
        );
        assert_eq!(
            classify_resting_fold(100, false, floor, ts, nt).unwrap(),
            RestingFold::InWindow(0)
        );
        assert_eq!(
            classify_resting_fold(730, false, floor, ts, nt).unwrap(),
            RestingFold::InWindow(63)
        );

        // Below the floor: SELL is marketable at tick 0, BUY is passive.
        assert_eq!(
            classify_resting_fold(90, true, floor, ts, nt).unwrap(),
            RestingFold::Marketable(0)
        );
        assert_eq!(
            classify_resting_fold(90, false, floor, ts, nt).unwrap(),
            RestingFold::Passive
        );

        // At/above the top: BUY marketable at top tick, SELL passive.
        assert_eq!(
            classify_resting_fold(740, false, floor, ts, nt).unwrap(),
            RestingFold::Marketable(63)
        );
        assert_eq!(
            classify_resting_fold(740, true, floor, ts, nt).unwrap(),
            RestingFold::Passive
        );
        assert_eq!(
            classify_resting_fold(10_000, false, floor, ts, nt).unwrap(),
            RestingFold::Marketable(63)
        );

        // Non-tick-aligned or zero price is rejected.
        assert_eq!(
            classify_resting_fold(95, true, floor, ts, nt),
            Err(MathError::InvalidPrice)
        );
        assert_eq!(
            classify_resting_fold(0, false, floor, ts, nt),
            Err(MathError::InvalidPrice)
        );

        // In-window verdict agrees with price_to_tick_raw (never drift).
        for mult in 10..=73u64 {
            let price = mult * 10;
            assert_eq!(
                classify_resting_fold(price, true, floor, ts, nt).unwrap(),
                RestingFold::InWindow(price_to_tick_raw(price, floor, ts, nt).unwrap())
            );
        }
    }

    // The keeper's old hand copy computed `top = floor + num_ticks * tick_size`, which
    // overflows u64 for a large window config. The mirror avoids that product entirely,
    // so a huge tick_size * num_ticks classifies without panicking (debug) or wrapping.
    #[test]
    fn large_window_does_not_overflow() {
        let tick_size = u64::MAX / 2;
        let num_ticks = u32::MAX;
        // num_ticks*tick_size (and floor+that) would overflow; classification must not.
        // floor 0, price = tick_size → offset 1 → in-window (no overflow in the divide).
        assert_eq!(
            classify_resting_fold(tick_size, true, 0, tick_size, num_ticks).unwrap(),
            RestingFold::InWindow(1)
        );
        // A very high floor with a price below it: SELL marketable, BUY passive.
        let floor = u64::MAX - (u64::MAX % tick_size); // floor is a tick multiple
        assert_eq!(
            classify_resting_fold(tick_size, true, floor, tick_size, num_ticks).unwrap(),
            RestingFold::Marketable(0)
        );
        assert_eq!(
            classify_resting_fold(tick_size, false, floor, tick_size, num_ticks).unwrap(),
            RestingFold::Passive
        );
    }

    #[test]
    fn is_passive_matches_classifier() {
        let (floor, ts, nt) = (100u64, 10u64, 64u32);
        assert!(is_passive(90, false, floor, ts, nt)); // BUY below floor
        assert!(is_passive(740, true, floor, ts, nt)); // SELL above top
        assert!(!is_passive(150, true, floor, ts, nt)); // in-window
        assert!(!is_passive(90, true, floor, ts, nt)); // SELL below floor = marketable
        assert!(!is_passive(95, true, floor, ts, nt)); // malformed → not passive
    }
}
