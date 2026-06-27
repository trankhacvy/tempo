//! The clearing arithmetic — `find_cross`, `fill_against_cross`,
//! `compute_marginal_fill`. Mirror of `program/src/clearing.rs`.

use crate::error::MathError;

/// Which side is rationed at the marginal tick.
pub const RATIONED_DEMAND: u8 = 0;
pub const RATIONED_SUPPLY: u8 = 1;
pub const RATIONED_NONE: u8 = 2;

/// Output of one uniform-price cross over a price histogram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrossResult {
    pub crossed: bool,
    pub clearing_tick: u32,
    pub matched_volume: u64,
    pub volume_allocated_to_marginal_tick: u64,
    pub total_qty_at_marginal_tick: u64,
    pub rationed_side: u8,
}

impl Default for CrossResult {
    #[inline(always)]
    fn default() -> Self {
        Self {
            crossed: false,
            clearing_tick: 0,
            matched_volume: 0,
            volume_allocated_to_marginal_tick: 0,
            total_qty_at_marginal_tick: 0,
            rationed_side: RATIONED_NONE,
        }
    }
}

/// Find the uniform clearing cross from demand/supply buckets. The clearing tick
/// maximizes `min(D, S)`; ties pick the lowest tick (deterministic).
pub fn find_cross(demand: &[u64], supply: &[u64]) -> Result<CrossResult, MathError> {
    let n = demand.len();
    if n != supply.len() {
        return Err(MathError::InvalidTick);
    }
    if n == 0 {
        return Ok(CrossResult::default());
    }

    let total_demand: u128 = demand.iter().map(|&d| d as u128).sum();

    let mut best_matched: u128 = 0;
    let mut s_cum: u128 = 0;
    let mut d_below_excl: u128 = 0;
    for t in 0..n {
        s_cum += supply[t] as u128;
        let d_at_or_above = total_demand - d_below_excl;
        let matched = core::cmp::min(d_at_or_above, s_cum);
        if matched > best_matched {
            best_matched = matched;
        }
        d_below_excl += demand[t] as u128;
    }

    if best_matched == 0 {
        return Ok(CrossResult::default());
    }

    let v: u64 = best_matched.try_into().map_err(|_| MathError::Overflow)?;

    let mut chosen: Option<(u32, u128, u128)> = None;
    let mut fallback: Option<(u32, u128, u128)> = None;
    s_cum = 0;
    d_below_excl = 0;
    for t in 0..n {
        s_cum += supply[t] as u128;
        let d_at_or_above = total_demand - d_below_excl;
        let matched = core::cmp::min(d_at_or_above, s_cum);
        if matched == best_matched {
            if fallback.is_none() {
                fallback = Some((t as u32, s_cum, d_at_or_above));
            }
            let d_strict_above = d_at_or_above - demand[t] as u128;
            let s_strict_below = s_cum - supply[t] as u128;
            if d_strict_above <= best_matched && s_strict_below <= best_matched {
                chosen = Some((t as u32, s_cum, d_at_or_above));
                break;
            }
        }
        d_below_excl += demand[t] as u128;
    }

    let (clearing_tick, s_at_or_below, d_at_or_above) = chosen.or(fallback).unwrap_or((0, 0, 0));
    let t = clearing_tick as usize;

    let (vol_alloc, total_qty, rationed_side) = if d_at_or_above > s_at_or_below {
        let demand_strictly_better = d_at_or_above - demand[t] as u128;
        let alloc = best_matched.saturating_sub(demand_strictly_better);
        (alloc, demand[t] as u128, RATIONED_DEMAND)
    } else if s_at_or_below > d_at_or_above {
        let supply_strictly_better = s_at_or_below - supply[t] as u128;
        let alloc = best_matched.saturating_sub(supply_strictly_better);
        (alloc, supply[t] as u128, RATIONED_SUPPLY)
    } else {
        (demand[t] as u128, demand[t] as u128, RATIONED_NONE)
    };

    Ok(CrossResult {
        crossed: true,
        clearing_tick,
        matched_volume: v,
        volume_allocated_to_marginal_tick: vol_alloc.try_into().map_err(|_| MathError::Overflow)?,
        total_qty_at_marginal_tick: total_qty.try_into().map_err(|_| MathError::Overflow)?,
        rationed_side,
    })
}

/// The published per-side rationing constants from one auction cross.
#[derive(Clone, Copy, Debug)]
pub struct AuctionCross {
    pub marginal_tick: u32,
    pub matched_volume: u64,
    pub volume_allocated_to_marginal_tick: u64,
    pub total_qty_at_marginal_tick: u64,
    pub rationed_side: u8,
}

/// Settle ONE order/level against a single auction cross. The single source of
/// truth for fill classification; rounds against the user.
pub fn fill_against_cross(
    cross: &AuctionCross,
    is_buy: bool,
    order_tick: u32,
    qty: u64,
    cum_before: u64,
) -> Result<u64, MathError> {
    if cross.matched_volume == 0 {
        return Ok(0);
    }
    let (strictly_better, at_marginal, can_fill) = if is_buy {
        (
            order_tick > cross.marginal_tick,
            order_tick == cross.marginal_tick,
            order_tick >= cross.marginal_tick,
        )
    } else {
        (
            order_tick < cross.marginal_tick,
            order_tick == cross.marginal_tick,
            order_tick <= cross.marginal_tick,
        )
    };
    if !can_fill {
        return Ok(0);
    }
    let on_rationed_side = if is_buy {
        cross.rationed_side == RATIONED_DEMAND
    } else {
        cross.rationed_side == RATIONED_SUPPLY
    };
    if at_marginal && on_rationed_side {
        compute_marginal_fill(
            cum_before,
            qty,
            cross.volume_allocated_to_marginal_tick,
            cross.total_qty_at_marginal_tick,
        )
    } else {
        debug_assert!(strictly_better || at_marginal);
        Ok(qty)
    }
}

/// Exact marginal-tick allocation for a rationed-side order. Cumulative-floor /
/// largest-remainder: slices telescope to exactly `vol_alloc` over the bucket.
pub fn compute_marginal_fill(
    cum_before: u64,
    qty: u64,
    vol_alloc: u64,
    total_qty: u64,
) -> Result<u64, MathError> {
    if total_qty == 0 {
        return Ok(0);
    }
    let v = vol_alloc as u128;
    let q = total_qty as u128;
    let lo = (cum_before as u128)
        .checked_mul(v)
        .ok_or(MathError::Overflow)?
        / q;
    let hi = (cum_before as u128 + qty as u128)
        .checked_mul(v)
        .ok_or(MathError::Overflow)?
        / q;
    u64::try_from(hi - lo).map_err(|_| MathError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn cross_of(r: &CrossResult) -> AuctionCross {
        AuctionCross {
            marginal_tick: r.clearing_tick,
            matched_volume: r.matched_volume,
            volume_allocated_to_marginal_tick: r.volume_allocated_to_marginal_tick,
            total_qty_at_marginal_tick: r.total_qty_at_marginal_tick,
            rationed_side: r.rationed_side,
        }
    }

    #[test]
    fn test_known_book_clearing_price_and_rationing() {
        let demand = [0, 0, 30, 20];
        let supply = [0, 40, 0, 0];
        let r = find_cross(&demand, &supply).unwrap();
        assert!(r.crossed);
        assert_eq!(r.clearing_tick, 2);
        assert_eq!(r.matched_volume, 40);
        assert_eq!(r.rationed_side, RATIONED_DEMAND);
        assert_eq!(r.total_qty_at_marginal_tick, 30);
        assert_eq!(r.volume_allocated_to_marginal_tick, 20);
    }

    #[test]
    fn test_non_rationed_side_marginal_fills_fully() {
        let demand = [0, 0, 8, 9];
        let supply = [0, 0, 25, 0];
        let r = find_cross(&demand, &supply).unwrap();
        assert_eq!(r.rationed_side, RATIONED_SUPPLY);
        let cross = cross_of(&r);
        assert_eq!(fill_against_cross(&cross, true, 2, 8, 0).unwrap(), 8);
        assert_eq!(fill_against_cross(&cross, true, 3, 9, 0).unwrap(), 9);
        assert_eq!(fill_against_cross(&cross, false, 2, 25, 0).unwrap(), 17);
    }

    #[test]
    fn test_marginal_fill_rounds_against_user() {
        assert_eq!(compute_marginal_fill(0, 10, 10, 30).unwrap(), 3);
    }

    /// The 20k-iter whole-book OI-conservation fuzz, copied from the program
    /// (the golden guard): settle every order through `fill_against_cross` and
    /// assert `Σ buy fills == Σ sell fills == matched_volume`.
    #[test]
    fn fuzz_full_book_conserves_oi() {
        let mut seed: u64 = 0xD1B5_4A32_D192_ED03;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 33
        };

        for _ in 0..20_000 {
            let n = 1 + (next() % 12) as usize;
            let mut demand = std::vec![0u64; n];
            let mut supply = std::vec![0u64; n];
            let mut buys: Vec<(usize, u64)> = Vec::new();
            let mut sells: Vec<(usize, u64)> = Vec::new();
            for _ in 0..(1 + next() % 6) {
                let t = (next() as usize) % n;
                let q = 1 + next() % 30;
                demand[t] += q;
                buys.push((t, q));
            }
            for _ in 0..(1 + next() % 6) {
                let t = (next() as usize) % n;
                let q = 1 + next() % 30;
                supply[t] += q;
                sells.push((t, q));
            }

            let c = find_cross(&demand, &supply).unwrap();
            if !c.crossed {
                continue;
            }
            let cross = cross_of(&c);
            let mt = c.clearing_tick;
            let demand_rationed = c.rationed_side == RATIONED_DEMAND;
            let supply_rationed = c.rationed_side == RATIONED_SUPPLY;

            let mut buy_fill = 0u64;
            let mut cum_buy = 0u64;
            for &(t, q) in &buys {
                buy_fill += fill_against_cross(&cross, true, t as u32, q, cum_buy).unwrap();
                if t as u32 == mt && demand_rationed {
                    cum_buy += q;
                }
            }

            let mut sell_fill = 0u64;
            let mut cum_sell = 0u64;
            for &(t, q) in &sells {
                sell_fill += fill_against_cross(&cross, false, t as u32, q, cum_sell).unwrap();
                if t as u32 == mt && supply_rationed {
                    cum_sell += q;
                }
            }

            assert_eq!(buy_fill, sell_fill, "OI not conserved");
            assert_eq!(buy_fill, c.matched_volume, "fill != V");
        }
    }

    #[test]
    fn test_marginal_fill_conserves_exactly() {
        let cases: &[(&[u64], u64)] = &[
            (&[10, 20, 30, 40], 37),
            (&[10, 20, 30, 40], 100),
            (&[1, 1, 1, 1, 1], 3),
            (&[7, 13, 29, 51], 50),
            (&[100], 41),
            (&[3, 3, 3], 0),
        ];
        for (qtys, alloc) in cases {
            let total: u64 = qtys.iter().sum();
            let mut cum = 0u64;
            let mut sum = 0u64;
            for &q in qtys.iter() {
                let fill = compute_marginal_fill(cum, q, *alloc, total).unwrap();
                assert!(fill <= q);
                sum += fill;
                cum += q;
            }
            assert_eq!(sum, *alloc);
        }
    }
}
