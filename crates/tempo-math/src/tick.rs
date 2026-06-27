//! Price↔tick mapping for the oracle-anchored histogram window.

use crate::error::MathError;

/// Map a price to its histogram tick index. The window floor is the price at
/// tick 0 (oracle-anchored), so `tick = (price − floor)/tick_size`. The price
/// must be non-zero, tick-aligned, at or above the floor, and inside
/// `[floor, floor + (num_ticks-1)·tick_size]`.
#[inline]
pub fn price_to_tick_raw(
    price: u64,
    floor: u64,
    tick_size: u64,
    num_ticks: u32,
) -> Result<u32, MathError> {
    if price == 0 || tick_size == 0 || !price.is_multiple_of(tick_size) || price < floor {
        return Err(MathError::InvalidPrice);
    }
    let offset = (price - floor) / tick_size;
    if offset >= num_ticks as u64 {
        return Err(MathError::InvalidTick);
    }
    Ok(offset as u32)
}

/// Inverse of [`price_to_tick_raw`]: the price represented by a tick index.
#[inline]
pub fn tick_to_price(
    tick: u32,
    floor: u64,
    tick_size: u64,
    num_ticks: u32,
) -> Result<u64, MathError> {
    if tick >= num_ticks {
        return Err(MathError::InvalidTick);
    }
    (tick as u64)
        .checked_mul(tick_size)
        .and_then(|off| off.checked_add(floor))
        .ok_or(MathError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_window_roundtrip() {
        let (floor, tick_size, num_ticks) = (10u64, 10u64, 64u32);
        assert_eq!(
            price_to_tick_raw(10, floor, tick_size, num_ticks).unwrap(),
            0
        );
        assert_eq!(
            price_to_tick_raw(20, floor, tick_size, num_ticks).unwrap(),
            1
        );
        assert_eq!(
            price_to_tick_raw(640, floor, tick_size, num_ticks).unwrap(),
            63
        );
        assert_eq!(tick_to_price(0, floor, tick_size, num_ticks).unwrap(), 10);
        assert_eq!(tick_to_price(63, floor, tick_size, num_ticks).unwrap(), 640);
        assert_eq!(
            price_to_tick_raw(15, floor, tick_size, num_ticks),
            Err(MathError::InvalidPrice)
        );
        assert_eq!(
            price_to_tick_raw(650, floor, tick_size, num_ticks),
            Err(MathError::InvalidTick)
        );
    }

    #[test]
    fn test_recentered_window() {
        let (floor, tick_size, num_ticks) = (9_680u64, 10u64, 64u32);
        assert_eq!(
            price_to_tick_raw(9_680, floor, tick_size, num_ticks).unwrap(),
            0
        );
        assert_eq!(
            price_to_tick_raw(10_000, floor, tick_size, num_ticks).unwrap(),
            32
        );
        assert_eq!(
            price_to_tick_raw(9_670, floor, tick_size, num_ticks),
            Err(MathError::InvalidPrice)
        );
    }
}
