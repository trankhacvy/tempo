//! The clearing arithmetic — the crown jewel (clearing-protocol §2–§3).
//!
//! Pure, account-free, no floating point (system-design §11): all math is on
//! `u64`/`u128` with checked/saturating ops and rounds **against** the user.
//! Keeping it here makes it directly unit-testable and lets `finalize_clear` /
//! `settle_fill` stay thin.
//!
//! `find_cross` computes **one** uniform-price cross over a demand/supply
//! histogram and is side-agnostic. The full DFBA runs two such crosses per
//! round — a **bid auction** (maker-buys vs taker-sells) and an **ask auction**
//! (taker-buys vs maker-sells) — by calling `find_cross` once per pool with
//! maker/taker segregation (system-design §1 / clearing-protocol §5). Both
//! crosses are wired end-to-end: `process_chunk` routes orders into the four
//! histogram regions, `finalize_clear` runs both passes and publishes both
//! sides of the `ClearingResult`, and `settle_fill` settles each order against
//! its own auction. See `test_dual_auction_independent_crosses` and the
//! `happy_path` LiteSVM test. (The original clearing *simulations* were run on
//! the single cross first; they have not been re-run for the dual structure.)

use pinocchio::error::ProgramError;

use crate::errors::TempoProgramError;

/// Which side is rationed at the marginal tick. Only the rationed (excess) side
/// is pro-rated; the scarce side fills in full (clearing-protocol §3, §7).
pub const RATIONED_DEMAND: u8 = 0; // buy side rationed
pub const RATIONED_SUPPLY: u8 = 1; // sell side rationed
pub const RATIONED_NONE: u8 = 2; // balanced at the cross, or no cross

/// Output of one uniform-price cross over a price histogram (called once per
/// auction — bid and ask).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrossResult {
    /// Whether a non-zero crossing exists (`false` => no trade this round).
    pub crossed: bool,
    /// Tick index of the clearing (marginal) price.
    pub clearing_tick: u32,
    /// Total matched volume `V = max_t min(D(t), S(t))`.
    pub matched_volume: u64,
    /// Volume allocated to the marginal tick on the **rationed** side.
    pub volume_allocated_to_marginal_tick: u64,
    /// Total resting quantity at the marginal tick on the **rationed** side
    /// (rationing denominator).
    pub total_qty_at_marginal_tick: u64,
    /// Which side is rationed (`RATIONED_DEMAND` / `RATIONED_SUPPLY` /
    /// `RATIONED_NONE`). The marginal-tick constants above describe ONLY this
    /// side; the other (scarce) side fills its marginal orders in full.
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

/// Find the uniform clearing cross from demand/supply buckets.
///
/// - `demand[t]` = buy quantity resting at tick `t` (price increases with `t`).
/// - `supply[t]` = sell quantity resting at tick `t`.
///
/// Builds cumulative demand `D(t) = Σ_{j>=t} demand[j]` (falls as `t` rises)
/// and cumulative supply `S(t) = Σ_{j<=t} supply[j]` (rises as `t` rises). The
/// clearing tick maximizes `min(D, S)` (single-peaked, clearing-protocol §2).
/// Ties pick the **lowest** tick to be deterministic (commutativity §4.1 means
/// the buckets are identical for every caller, so a fixed tie-break makes the
/// whole result caller-independent).
pub fn find_cross(demand: &[u64], supply: &[u64]) -> Result<CrossResult, ProgramError> {
    let n = demand.len();
    if n != supply.len() {
        return Err(TempoProgramError::InvalidTick.into());
    }
    if n == 0 {
        return Ok(CrossResult::default());
    }

    // Total supply at or below each tick (prefix), and total demand at or above
    // each tick (suffix). Use u128 accumulators to avoid overflow on summation.
    let total_demand: u128 = demand.iter().map(|&d| d as u128).sum();

    // Pass 1: the maximum matched volume V = max_t min(D(t), S(t)).
    let mut best_matched: u128 = 0;
    let mut s_cum: u128 = 0; // Σ_{j<=t} supply[j]
    let mut d_below_excl: u128 = 0; // Σ_{j<t} demand[j]
    for t in 0..n {
        s_cum += supply[t] as u128;
        let d_at_or_above = total_demand - d_below_excl; // D(t)
        let matched = core::cmp::min(d_at_or_above, s_cum);
        if matched > best_matched {
            best_matched = matched;
        }
        d_below_excl += demand[t] as u128;
    }

    if best_matched == 0 {
        return Ok(CrossResult::default());
    }

    let v: u64 = best_matched
        .try_into()
        .map_err(|_| TempoProgramError::MathOverflow)?;

    // Pass 2: choose the clearing tick on the max-matched plateau where neither
    // side's strictly-better quantity exceeds V — i.e. `D(t) - demand[t] <= V`
    // and `S(t) - supply[t] <= V`. That is the true economic clearing tick and is
    // exactly the condition settle_fill relies on (strictly-better orders fill in
    // full), so settlement conserves: Σ buy fills == Σ sell fills == V. The lowest
    // qualifying tick keeps the result deterministic. A single-peaked cross always
    // has such a tick; the lowest plateau tick is a total-function fallback.
    // Capture `(tick, S(t), D(t))` at the chosen tick during the walk so the
    // rationing below is O(1) — no extra slice-sum passes over the buckets.
    let mut chosen: Option<(u32, u128, u128)> = None;
    let mut fallback: Option<(u32, u128, u128)> = None; // lowest plateau tick
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

    // The clearing tick + its `S(t)`/`D(t)`: the chosen tick, else the lowest
    // plateau tick (a total-function fallback — unreachable for a real cross).
    let (clearing_tick, s_at_or_below, d_at_or_above) = chosen.or(fallback).unwrap_or((0, 0, 0));
    let t = clearing_tick as usize;

    // Marginal-tick allocation. The rationed side is the one whose cumulative
    // quantity exceeds V at the crossing tick (clearing-protocol §3 / §7 fix):
    // strictly-better levels fill fully, the marginal level is rationed to
    // exactly hit V. Strictly-better = `D(t) - demand[t]` / `S(t) - supply[t]`.
    let (vol_alloc, total_qty, rationed_side) = if d_at_or_above > s_at_or_below {
        let demand_strictly_better = d_at_or_above - demand[t] as u128;
        let alloc = best_matched.saturating_sub(demand_strictly_better);
        (alloc, demand[t] as u128, RATIONED_DEMAND)
    } else if s_at_or_below > d_at_or_above {
        let supply_strictly_better = s_at_or_below - supply[t] as u128;
        let alloc = best_matched.saturating_sub(supply_strictly_better);
        (alloc, supply[t] as u128, RATIONED_SUPPLY)
    } else {
        // Balanced: neither side rationed; both marginal buckets fill fully.
        (demand[t] as u128, demand[t] as u128, RATIONED_NONE)
    };

    Ok(CrossResult {
        crossed: true,
        clearing_tick,
        matched_volume: v,
        volume_allocated_to_marginal_tick: vol_alloc
            .try_into()
            .map_err(|_| TempoProgramError::MathOverflow)?,
        total_qty_at_marginal_tick: total_qty
            .try_into()
            .map_err(|_| TempoProgramError::MathOverflow)?,
        rationed_side,
    })
}

/// The published per-side rationing constants from one auction cross — the half
/// of `ClearingResult` an order (or maker-quote level) settles against. These
/// are exactly the `CrossResult` fields that survive into Phase 3 SETTLE; the
/// clearing *price* is a money-path value carried separately by the caller, not
/// part of the fill classification.
#[derive(Clone, Copy, Debug)]
pub struct AuctionCross {
    /// Marginal (clearing) tick of this auction.
    pub marginal_tick: u32,
    /// Total matched volume `V` (zero ⇒ the auction did not cross).
    pub matched_volume: u64,
    /// Volume allocated to the marginal tick on the rationed side.
    pub volume_allocated_to_marginal_tick: u64,
    /// Total resting quantity at the marginal tick on the rationed side.
    pub total_qty_at_marginal_tick: u64,
    /// Which side is rationed (`RATIONED_DEMAND`/`RATIONED_SUPPLY`/`RATIONED_NONE`).
    pub rationed_side: u8,
}

/// Settle ONE order or maker-quote level against a single auction cross
/// (clearing-protocol §3, Phase 3 SETTLE). This is the **single source of truth**
/// for fill classification, shared by `settle_fill` (taker orders) and
/// `settle_maker_quote` (maker ladder levels), so the marginal-tick boundary can
/// never drift between the two paths (a drift would let maker and taker fills
/// settle at different boundaries and stop netting to the matched volume — a
/// silent conservation break). Pure / no-float / rounds **against** the user.
///
/// - `is_buy` — the order is on the demand side (a buy). A sell is supply.
/// - `order_tick` — the order's price tick.
/// - `qty` — the quantity that was folded into the histogram for this order (its
///   `remaining`); this is the quantity the bucket totals tile over, so it is
///   also what the marginal allocation must ration.
/// - `cum_before` — the order's fold-order prefix of same-bucket quantity; used
///   **only** on the rationed marginal tick, where the telescoping floor
///   ([`compute_marginal_fill`]) makes the rationed side sum to exactly `V`.
///
/// Boundary semantics (the part that MUST stay identical for both callers):
///  - A buy fills if `tick >= marginal`; strictly-better = `tick > marginal`.
///  - A sell fills if `tick <= marginal`; strictly-better = `tick < marginal`.
///
/// Outcomes: a non-crossing auction (`matched_volume == 0`) fills zero; anything
/// worse than the marginal tick fills zero; strictly-better levels fill fully;
/// the scarce side fills fully at the marginal tick (under-filling it would break
/// conservation); only the rationed side is pro-rated at the marginal tick.
pub fn fill_against_cross(
    cross: &AuctionCross,
    is_buy: bool,
    order_tick: u32,
    qty: u64,
    cum_before: u64,
) -> Result<u64, ProgramError> {
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
        // Worse than the marginal price.
        return Ok(0);
    }
    let on_rationed_side = if is_buy {
        cross.rationed_side == RATIONED_DEMAND
    } else {
        cross.rationed_side == RATIONED_SUPPLY
    };
    if at_marginal && on_rationed_side {
        // Rationed side at the marginal tick: exact telescoping-floor allocation
        // (no OI-leaking dust; sums to exactly `V` for any fold/settle order).
        compute_marginal_fill(
            cum_before,
            qty,
            cross.volume_allocated_to_marginal_tick,
            cross.total_qty_at_marginal_tick,
        )
    } else {
        // The only remaining cases under `can_fill` are strictly-better (fills
        // fully — Σ strictly-better ≤ V by find_cross / §7) and the scarce side
        // at the marginal tick (its whole quantity is matched). Both fill fully.
        debug_assert!(strictly_better || at_marginal);
        Ok(qty)
    }
}

/// Exact marginal-tick allocation for a rationed-side order.
///
/// Replaces the plain floor pro-rata (which loses up-to-dust per order and
/// breaks open-interest conservation in perps) with the cumulative-floor /
/// largest-remainder-equivalent method: an order's fill is
/// `floor((cum_before + qty)·V / Q) − floor(cum_before·V / Q)`, where `cum_before`
/// is the total quantity of same-bucket orders with a strictly lower `order_id`,
/// `qty` this order's quantity, `V = vol_alloc`, `Q = total_qty`.
///
/// Summed over the whole bucket in `order_id` order the slices telescope to
/// exactly `floor(Q·V / Q) = V`, so the rationed side fills exactly `V` and OI is
/// conserved. Each order's value depends only on `(cum_before, qty, V, Q)` — all
/// derived from immutable order fields + the published result — so it is
/// independent of settle order (commutative). Requires `cum_before + qty ≤ Q`.
pub fn compute_marginal_fill(
    cum_before: u64,
    qty: u64,
    vol_alloc: u64,
    total_qty: u64,
) -> Result<u64, ProgramError> {
    if total_qty == 0 {
        return Ok(0);
    }
    let v = vol_alloc as u128;
    let q = total_qty as u128;
    let lo = (cum_before as u128)
        .checked_mul(v)
        .ok_or(TempoProgramError::MathOverflow)?
        / q;
    let hi = (cum_before as u128 + qty as u128)
        .checked_mul(v)
        .ok_or(TempoProgramError::MathOverflow)?
        / q;
    // hi >= lo because qty >= 0; the difference fits u64 (≤ qty).
    u64::try_from(hi - lo).map_err(|_| TempoProgramError::MathOverflow.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// Lift a `find_cross` result into the rationing half `fill_against_cross`
    /// settles against (price is a money-path value, irrelevant to the fill).
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
    fn test_no_orders() {
        let r = find_cross(&[], &[]).unwrap();
        assert!(!r.crossed);
        let r = find_cross(&[0, 0, 0], &[0, 0, 0]).unwrap();
        assert!(!r.crossed);
    }

    #[test]
    fn test_no_cross_disjoint() {
        // Buyers only at low ticks, sellers only at high ticks => with our tick
        // semantics (buy fills if price>=P*, sell if price<=P*), a buy at tick 0
        // and sell at tick 3 cannot match at any single price.
        // demand at tick 0, supply at tick 3.
        let demand = [10, 0, 0, 0];
        let supply = [0, 0, 0, 10];
        let r = find_cross(&demand, &supply).unwrap();
        // At tick 0: D=10, S=0 -> 0. tick3: D=0,S=10 ->0. No positive match.
        assert!(!r.crossed);
        assert_eq!(r.matched_volume, 0);
    }

    #[test]
    fn test_simple_full_cross() {
        // One buyer of 10 at tick 2 (price high enough), one seller of 10 at tick 1.
        // At tick 1: D = demand at>=1 = 10, S = supply at<=1 = 10 => matched 10.
        let demand = [0, 0, 10, 0];
        let supply = [0, 10, 0, 0];
        let r = find_cross(&demand, &supply).unwrap();
        assert!(r.crossed);
        assert_eq!(r.matched_volume, 10);
        // balanced at the crossing => clearing tick is where min is maximized
    }

    #[test]
    fn test_known_book_clearing_price_and_rationing() {
        // 4 ticks. Demand (buys) and supply (sells):
        //   tick:    0    1    2    3
        //   demand:  0    0   30   20   (buys priced at tick2=30, tick3=20)
        //   supply:  0   40    0    0   (sells priced at tick1=40)
        //
        // D(t)=Σ_{>=t} demand, S(t)=Σ_{<=t} supply:
        //   t0: D=50 S=0  -> 0
        //   t1: D=50 S=40 -> 40
        //   t2: D=50 S=40 -> 40
        //   t3: D=20 S=40 -> 20
        // best matched = 40 on the plateau {t1,t2}. The clearing tick is the one
        // where strictly-better quantity does not exceed V: at t1 the strictly-
        // better demand (50) exceeds V=40, so settlement there would over-fill;
        // t2 satisfies it (D(2)-demand[2]=20<=40, S(2)-supply[2]=40<=40), so the
        // economic clearing tick is t2 and the cross conserves.
        let demand = [0, 0, 30, 20];
        let supply = [0, 40, 0, 0];
        let r = find_cross(&demand, &supply).unwrap();
        assert!(r.crossed);
        assert_eq!(r.clearing_tick, 2);
        assert_eq!(r.matched_volume, 40);
        // D(2)=50 > S(2)=40 => buy side rationed at marginal tick 2.
        // strictly-better demand (tick 3) = 20, so allocated = 40 - 20 = 20 over a
        // marginal bucket of demand[2] = 30.
        assert_eq!(r.rationed_side, RATIONED_DEMAND);
        assert_eq!(r.total_qty_at_marginal_tick, 30);
        assert_eq!(r.volume_allocated_to_marginal_tick, 20);
    }

    #[test]
    fn test_marginal_rationing_pro_rata() {
        // Demand all sits at the marginal tick to exercise rationing.
        //   tick:    0    1    2
        //   demand:  0    0   100   (buys at tick2)
        //   supply:  0   60    0    (sells at tick1, total 60)
        // D(t): t0=100 t1=100 t2=100 ; S(t): t0=0 t1=60 t2=60
        // min: t0=0, t1=60, t2=60 -> best 60 at t1. V=60.
        // D(1)=100 > S(1)=60 -> buy rationed at tick1. demand[1]=0 so no buyers
        // exactly at the marginal tick. Instead test with a marginal buy bucket:
        //   demand:  0  70  40 ; supply: 0 60 0
        //   D: t0=110 t1=110 t2=40 ; S: t0=0 t1=60 t2=60
        //   min: 0,60,40 -> best 60 at t1. D(1)=110>S(1)=60 buy rationed.
        //   strictly-better demand (t>1) = 40. allocated = 60-40 = 20.
        //   total_qty_at_marginal = demand[1] = 70.
        let demand = [0, 70, 40];
        let supply = [0, 60, 0];
        let r = find_cross(&demand, &supply).unwrap();
        assert_eq!(r.clearing_tick, 1);
        assert_eq!(r.matched_volume, 60);
        assert_eq!(r.total_qty_at_marginal_tick, 70);
        assert_eq!(r.volume_allocated_to_marginal_tick, 20);

        assert_eq!(r.rationed_side, RATIONED_DEMAND);
        let cross = cross_of(&r);
        // A buyer of 35 at the marginal tick (rationed side) gets floor(35*20/70)=10.
        let fill = fill_against_cross(&cross, true, 1, 35, 0).unwrap();
        assert_eq!(fill, 10);
        // A buyer strictly better (tick 2) fills fully.
        let fill_full = fill_against_cross(&cross, true, 2, 40, 0).unwrap();
        assert_eq!(fill_full, 40);
    }

    #[test]
    fn test_marginal_fill_rounds_against_user() {
        // Floor rounding at the marginal tick: 10 alloc / 30 total, order 10 ->
        // floor(10*10/30) = floor(3.33) = 3 (cum_before 0 ⇒ first in the bucket).
        let fill = compute_marginal_fill(0, 10, 10, 30).unwrap();
        assert_eq!(fill, 3);
    }

    #[test]
    fn test_fill_zero_when_worse() {
        // Buy priced below the marginal tick → cannot fill.
        let cross = AuctionCross {
            marginal_tick: 2,
            matched_volume: 100,
            volume_allocated_to_marginal_tick: 100,
            total_qty_at_marginal_tick: 100,
            rationed_side: RATIONED_DEMAND,
        };
        let fill = fill_against_cross(&cross, true, 0, 50, 0).unwrap();
        assert_eq!(fill, 0);
        // And a non-crossing auction (V == 0) fills zero regardless of tick.
        let no_cross = AuctionCross {
            matched_volume: 0,
            ..cross
        };
        assert_eq!(fill_against_cross(&no_cross, true, 2, 50, 0).unwrap(), 0);
    }

    /// The shared classifier covers every (side × position-vs-marginal) case with
    /// the side-correct boundary — the single boundary both settle paths now use.
    #[test]
    fn test_fill_against_cross_classification() {
        // Demand rationed at marginal tick 5: alloc 30 of a 100-qty bucket.
        let demand_rationed = AuctionCross {
            marginal_tick: 5,
            matched_volume: 80,
            volume_allocated_to_marginal_tick: 30,
            total_qty_at_marginal_tick: 100,
            rationed_side: RATIONED_DEMAND,
        };
        // Buy strictly better (tick 6 > 5) fills fully; the qty passed is honored.
        assert_eq!(
            fill_against_cross(&demand_rationed, true, 6, 40, 0).unwrap(),
            40
        );
        // Buy worse (tick 4 < 5) fills zero.
        assert_eq!(
            fill_against_cross(&demand_rationed, true, 4, 40, 0).unwrap(),
            0
        );
        // Buy at the marginal tick on the rationed side → telescoping floor.
        // cum_before 0, qty 50: floor(50*30/100) = 15.
        assert_eq!(
            fill_against_cross(&demand_rationed, true, 5, 50, 0).unwrap(),
            15
        );
        // Its sibling at cum_before 50, qty 50: floor(100*30/100)-floor(50*30/100)
        // = 30 - 15 = 15 → the two tile the bucket and sum to exactly the alloc 30.
        assert_eq!(
            fill_against_cross(&demand_rationed, true, 5, 50, 50).unwrap(),
            15
        );
        // A SELL at the same marginal tick is the scarce side here → fills fully.
        assert_eq!(
            fill_against_cross(&demand_rationed, false, 5, 7, 0).unwrap(),
            7
        );
        // Sell strictly better (tick 4 < 5) fills fully; sell worse (tick 6) zero.
        assert_eq!(
            fill_against_cross(&demand_rationed, false, 4, 7, 0).unwrap(),
            7
        );
        assert_eq!(
            fill_against_cross(&demand_rationed, false, 6, 7, 0).unwrap(),
            0
        );
    }

    /// Regression (found by the multi-agent devnet sim): when the SUPPLY side is
    /// rationed, a DEMAND order resting exactly at the marginal tick is on the
    /// scarce side and must fill FULLY — not be pro-rated by the supply bucket.
    /// Previously it was under-filled, breaking conservation
    /// (filled-demand ≠ filled-supply). Mirrors the ask-auction case from the
    /// sim (oracle $73.59 → ask cleared with a marginal demand order at 0).
    #[test]
    fn test_non_rationed_side_marginal_fills_fully() {
        // demand (buys): 8 at the marginal tick 2, 9 strictly better at tick 3.
        // supply (sells): 25 at tick 2 (excess) — supply is rationed.
        let demand = [0, 0, 8, 9];
        let supply = [0, 0, 25, 0];
        let r = find_cross(&demand, &supply).unwrap();
        assert!(r.crossed);
        assert_eq!(r.clearing_tick, 2);
        assert_eq!(r.matched_volume, 17);
        assert_eq!(r.rationed_side, RATIONED_SUPPLY);
        assert_eq!(r.total_qty_at_marginal_tick, 25); // supply bucket
        assert_eq!(r.volume_allocated_to_marginal_tick, 17);

        let cross = cross_of(&r);
        // Demand marginal order (qty 8, tick 2), NOT on the rationed side → fills fully.
        let demand_marginal = fill_against_cross(&cross, true, 2, 8, 0).unwrap();
        assert_eq!(
            demand_marginal, 8,
            "scarce-side marginal order must fill fully"
        );
        // Demand strictly-better order (qty 9, tick 3) → fills fully.
        let demand_better = fill_against_cross(&cross, true, 3, 9, 0).unwrap();
        assert_eq!(demand_better, 9);
        // Supply marginal order (qty 25, tick 2), ON the rationed side → floor(25*17/25)=17.
        let supply_marginal = fill_against_cross(&cross, false, 2, 25, 0).unwrap();
        assert_eq!(supply_marginal, 17);

        // Conservation: filled demand == filled supply == matched volume.
        assert_eq!(demand_marginal + demand_better, r.matched_volume);
        assert_eq!(supply_marginal, r.matched_volume);
    }

    /// Dual auction (system-design §1): the bid auction (maker-buys vs
    /// taker-sells) and the ask auction (taker-buys vs maker-sells) clear
    /// independently, each via `find_cross`, and can settle at different prices.
    #[test]
    fn test_dual_auction_independent_crosses() {
        // Bid auction: maker-buy 10 at tick 2, taker-sell 10 at tick 1 -> cross V=10.
        let bid_demand = [0, 0, 10, 0];
        let bid_supply = [0, 10, 0, 0];
        // Ask auction: taker-buy 25 at tick 3, maker-sell 25 at tick 2 -> cross V=25.
        let ask_demand = [0, 0, 0, 25];
        let ask_supply = [0, 0, 25, 0];

        let bid = find_cross(&bid_demand, &bid_supply).unwrap();
        let ask = find_cross(&ask_demand, &ask_supply).unwrap();

        assert!(bid.crossed);
        assert_eq!(bid.matched_volume, 10);
        assert!(ask.crossed);
        assert_eq!(ask.matched_volume, 25);
        // The two auctions are fully independent: different volumes, different ticks.
        assert_ne!(bid.matched_volume, ask.matched_volume);
    }

    /// Invariant fuzz: over thousands of random books, the
    /// histogram cross must equal a brute-force reference, and the total of
    /// every order's self-computed fill at the marginal tick must never exceed
    /// the volume allocated to it (clearing-protocol §7 — the property whose
    /// violation the original simulation caught). Deterministic LCG, no deps.
    #[test]
    fn fuzz_cross_matches_bruteforce_and_conserves() {
        let mut seed: u64 = 0x9E3779B97F4A7C15;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 33
        };

        for _ in 0..4000 {
            let n = 1 + (next() % 12) as usize;
            let demand: Vec<u64> = (0..n).map(|_| next() % 50).collect();
            let supply: Vec<u64> = (0..n).map(|_| next() % 50).collect();

            // Brute-force max matched volume: max_p min(D(p), S(p)).
            let mut best = 0u128;
            for p in 0..n {
                let d: u128 = demand[p..].iter().map(|&x| x as u128).sum();
                let s: u128 = supply[..=p].iter().map(|&x| x as u128).sum();
                best = best.max(d.min(s));
            }

            let cross = find_cross(&demand, &supply).unwrap();
            assert_eq!(
                cross.matched_volume as u128, best,
                "cross != brute force for {demand:?}/{supply:?}"
            );

            if cross.crossed {
                // Conservation at the marginal tick: floor pro-rata of every unit
                // resting there cannot exceed the allocation.
                let total = cross.total_qty_at_marginal_tick;
                let alloc = cross.volume_allocated_to_marginal_tick;
                assert!(
                    alloc <= total || total == 0,
                    "allocation exceeds bucket qty"
                );
                // A single order holding the whole marginal bucket fills exactly
                // the allocation (floor of alloc*total/total).
                if total > 0 {
                    let fill = compute_marginal_fill(0, total, alloc, total).unwrap();
                    assert!(
                        fill <= alloc,
                        "marginal fill {fill} exceeds allocation {alloc}"
                    );
                }
            }
        }
    }

    /// Whole-book conservation: for thousands of random books, settle EVERY order
    /// through the shared `fill_against_cross` primitive (the exact production
    /// classifier used by both `settle_fill` and `settle_maker_quote`) and assert
    /// `Σ buy fills == Σ sell fills == matched_volume`. This both guards the
    /// property the gapped-book tie-break previously violated AND proves the
    /// shared classifier conserves OI — so the two settle paths cannot drift.
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
            let mut demand = alloc::vec![0u64; n];
            let mut supply = alloc::vec![0u64; n];
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

            // Track each rationed-marginal bucket's fold-order prefix (`cum_*`)
            // exactly as `process_chunk` snapshots it, and settle through the
            // shared classifier.
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

            assert_eq!(
                buy_fill, sell_fill,
                "OI not conserved: buys={buy_fill} sells={sell_fill} demand={demand:?} supply={supply:?}"
            );
            assert_eq!(
                buy_fill, c.matched_volume,
                "fill != V: demand={demand:?} supply={supply:?}"
            );
        }
    }

    /// The cumulative-floor marginal allocation sums to EXACTLY the allocated
    /// volume (no dust) for any bucket, regardless of evaluation order.
    #[test]
    fn test_marginal_fill_conserves_exactly() {
        // Each case: (order quantities at the marginal tick, vol allocated).
        let cases: &[(&[u64], u64)] = &[
            (&[10, 20, 30, 40], 37),
            (&[10, 20, 30, 40], 100), // full allocation
            (&[1, 1, 1, 1, 1], 3),    // tiny orders, scarce allocation
            (&[7, 13, 29, 51], 50),
            (&[100], 41),    // single order
            (&[3, 3, 3], 0), // nothing allocated
        ];
        for (qtys, alloc) in cases {
            let total: u64 = qtys.iter().sum();
            // Cumulative prefix in order_id order.
            let mut cum = 0u64;
            let mut sum = 0u64;
            for &q in qtys.iter() {
                let fill = compute_marginal_fill(cum, q, *alloc, total).unwrap();
                assert!(fill <= q, "fill {fill} exceeds order qty {q}");
                sum += fill;
                cum += q;
            }
            assert_eq!(
                sum, *alloc,
                "cumulative marginal fills must sum to exactly {alloc} (qtys {qtys:?})"
            );
        }
    }

    /// The allocation is independent of the order in which orders are evaluated
    /// (settlement is permissionless / unordered) — the multiset of fills, hence
    /// the sum, is invariant under reordering because each slice is determined by
    /// its order_id-prefix, not evaluation order.
    #[test]
    fn test_marginal_fill_order_independent() {
        // Bucket of 4 orders (ids 0..4 by their position here), alloc 37 of 100.
        let qtys = [40u64, 30, 20, 10];
        let total: u64 = qtys.iter().sum();
        let alloc = 37u64;
        // Compute each order's fill from its own prefix (prefix = sum of lower ids).
        let mut prefix = [0u64; 4];
        let mut acc = 0u64;
        for i in 0..4 {
            prefix[i] = acc;
            acc += qtys[i];
        }
        let fills: alloc::vec::Vec<u64> = (0..4)
            .map(|i| compute_marginal_fill(prefix[i], qtys[i], alloc, total).unwrap())
            .collect();
        let sum: u64 = fills.iter().sum();
        assert_eq!(
            sum, alloc,
            "exact conservation regardless of which order settles when"
        );
    }

    /// A maker quote's marginal-tick level shares the rationed
    /// allocation with resting orders at the same tick. The conserving prefix is
    /// "all orders at the tick, then quote levels by ascending quote_id", so a
    /// quote level's `cum_before = total_order_qty + Σ lower-id quote levels`.
    /// The same telescoping floor (`compute_marginal_fill`) then conserves exactly
    /// across the mixed pool, independent of contributor type — which is why
    /// `settle_maker_quote` can reuse it unchanged.
    #[test]
    fn test_maker_quote_marginal_conserves_mixed() {
        // (order qtys at the tick, quote-level qtys at the tick, vol allocated).
        let cases: &[(&[u64], &[u64], u64)] = &[
            (&[10, 20], &[15, 5], 30),
            (&[7], &[13, 29, 51], 40),
            (&[], &[10, 10, 10], 17), // quotes only
            (&[10, 10, 10], &[], 17), // orders only
            (&[5, 5], &[5, 5], 20),   // full allocation
            (&[3, 3, 3], &[3, 3], 0), // nothing allocated
        ];
        for (orders, quotes, alloc) in cases {
            let total: u64 = orders.iter().chain(quotes.iter()).sum();
            let mut cum = 0u64;
            let mut sum = 0u64;
            // Orders first (their prefix is the order-only one, unchanged from
            // settle_fill), then quotes (whose prefix includes all orders).
            for &q in orders.iter().chain(quotes.iter()) {
                let fill = compute_marginal_fill(cum, q, *alloc, total).unwrap();
                assert!(fill <= q);
                sum += fill;
                cum += q;
            }
            assert_eq!(
                sum, *alloc,
                "mixed order+quote pool conserves to exactly {alloc}"
            );
        }
    }
}
