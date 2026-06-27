//! Off-chain preflight checks via `tempo-math` — reject locally what the program
//! would reject on-chain, saving a failed transaction. Because the arithmetic is
//! the *same* code the program runs, the checks cannot drift.

use tempo_math::margin::initial_margin;
use tempo_math::tick::price_to_tick_raw;

use crate::error::SdkError;

/// What a passing [`check_submit`] computes: the tick the order maps to and the
/// worst-case initial margin the program will reserve at submit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmitCheck {
    pub tick: u32,
    pub reserved_margin: u64,
}

/// Validate a `submit_order` before sending: the price must map into the
/// oracle-anchored window, and the trader must be able to back the worst-case
/// reservation (`qty · worst_price · initial_bps`). `worst_price` is the order's
/// limit for a buy, the window top for a sell.
#[allow(clippy::too_many_arguments)]
pub fn check_submit(
    window_floor: u64,
    tick_size: u64,
    num_ticks: u32,
    price: u64,
    qty: u64,
    worst_price: u64,
    initial_bps: u16,
    free_collateral: u64,
) -> Result<SubmitCheck, SdkError> {
    let tick = price_to_tick_raw(price, window_floor, tick_size, num_ticks)
        .map_err(|_| SdkError::PriceOutOfWindow)?;
    let reserved_margin = initial_margin(qty, worst_price, initial_bps);
    if reserved_margin > free_collateral {
        return Err(SdkError::InsufficientCollateral {
            need: reserved_margin,
            have: free_collateral,
        });
    }
    Ok(SubmitCheck {
        tick,
        reserved_margin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_submit_ok_and_failures() {
        // window [10, 10+63*10], tick_size 10: price 100 → tick 9.
        let ok = check_submit(10, 10, 64, 100, 5, 100, 500, 1_000_000).unwrap();
        assert_eq!(ok.tick, 9);
        assert_eq!(ok.reserved_margin, initial_margin(5, 100, 500));

        // price out of window.
        assert!(matches!(
            check_submit(10, 10, 64, 5, 5, 100, 500, 1_000_000),
            Err(SdkError::PriceOutOfWindow)
        ));

        // not enough collateral to back the reservation.
        assert!(matches!(
            check_submit(10, 10, 64, 100, 5, 100, 500, 1),
            Err(SdkError::InsufficientCollateral { .. })
        ));
    }
}
