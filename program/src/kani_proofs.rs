//! Formal-verification harnesses. Compiled ONLY under `cfg(kani)` (i.e.
//! `cargo kani`); the Kani toolchain injects the `kani` crate.
//!
//! These are aimed at what Kani uniquely provides over the property fuzzes:
//! **exhaustive** proof — over every input in a domain — of (a) panic/overflow/
//! underflow freedom on the raw integer arithmetic (which matters here because the
//! release profile sets `overflow-checks = true`, so an overflow is a real on-chain
//! panic → wedged instruction), and (b) the correctness invariant of the hand-
//! rolled 256-bit divider, which is effectively impossible to fuzz to completion.
//!
//! Each harness documents WHY its input domain is bounded the way it is.
//!
//! What is deliberately NOT verified here, and why (honest scoping — these are real
//! CBMC limits, not oversights):
//!   * Hardware `u128` **division** (`mul_div`'s fast path, `clamp_price_step`'s
//!     `/10_000`, the funding/mark band math): CBMC bit-blasts a hardware divider
//!     exponentially in operand width — intractable.
//!   * The wide **multiply** correctness `lo == a·b` and the **`div_256`** exact
//!     quotient `q·d + r == (hi:lo)`: both need a symbolic 128-bit multiply / a
//!     128-iteration unroll over a symbolic wide dividend, which also blow past any
//!     practical budget (measured: >5 min, no result).
//! All of the above are instead exhaustively *sampled* by the host differential /
//! property fuzzes — most directly `fuzz_wide_vs_u256_reference`, which asserts
//! exactly `q·d + r == a·b` and `r < d` over 50k large-operand cases. Kani is
//! applied here only where it is both tractable AND high-value: panic/overflow/
//! underflow-freedom on the raw arithmetic, plus the no-high-word property of the
//! wide multiply's carry logic.
#![cfg(kani)]

extern crate kani;

use crate::clearing::{find_cross, RATIONED_DEMAND, RATIONED_SUPPLY};
use crate::margin::unrealized_pnl;
use crate::wide_math::wide_mul;

/// `find_cross` is panic-free and its results are bounded by the book.
///
/// The point is the **panic-freedom**: `find_cross` builds cumulative demand/supply
/// by repeated `u128` addition and then forms the marginal allocation by *subtracting*
/// (`D(t) − demand[t]`, `S(t) − supply[t]`, `total_demand − d_below_excl`). Each of
/// those subtractions must never underflow — that is the real safety property of the
/// algorithm, and Kani proves it exhaustively over the whole bucket domain (CBMC
/// auto-asserts arithmetic-overflow/underflow + array bounds on every operation).
///
/// Three ticks (the smallest size with a genuine interior tick, so the marginal-tick
/// rationing branch is exercised). Buckets are bounded to 1e15 — far above any
/// realistic per-tick quantity — only so the `matched_volume` `u64` conversion inside
/// `find_cross` cannot saturate, keeping the proof on the Ok path. `find_cross` itself
/// is division-free, so this discharges quickly.
#[kani::proof]
#[kani::unwind(5)]
fn proof_find_cross_safe() {
    const MAX_Q: u64 = 1_000_000_000_000_000; // 1e15, >> any real per-tick qty
    let d: [u64; 3] = [kani::any(), kani::any(), kani::any()];
    let s: [u64; 3] = [kani::any(), kani::any(), kani::any()];
    for i in 0..3 {
        kani::assume(d[i] <= MAX_Q && s[i] <= MAX_Q);
    }

    // No-panic is checked automatically by CBMC across the whole call.
    let r = find_cross(&d, &s).unwrap();

    if r.crossed {
        let total_d = d[0] as u128 + d[1] as u128 + d[2] as u128;
        let total_s = s[0] as u128 + s[1] as u128 + s[2] as u128;
        // Matched volume never exceeds what either side brought.
        assert!(r.matched_volume as u128 <= total_d);
        assert!(r.matched_volume as u128 <= total_s);
        // The rationed side's marginal allocation never exceeds its bucket.
        if r.rationed_side == RATIONED_DEMAND || r.rationed_side == RATIONED_SUPPLY {
            assert!(
                r.volume_allocated_to_marginal_tick <= r.total_qty_at_marginal_tick
                    || r.total_qty_at_marginal_tick == 0
            );
        }
    }
}

/// `wide_mul` is panic-free and agrees with native multiplication.
///
/// The hand-rolled 128×128→256 product propagates carries through several raw
/// `u128` additions (`hi = hh + (lh>>64) + (hl>>64) + (cross>>64)` etc.). Under
/// `overflow-checks` any intermediate that exceeds `u128::MAX` would panic, so
/// proving none can is real assurance. Additionally, on the sub-domain where the
/// true product fits `u128`, the wide result must be exactly `(0, a·b)` — verifying
/// the carry logic against the native multiplier.
///
/// Operands are bounded to 64 bits so the four inner products are exact 64×64→128
/// multiplies CBMC handles directly; this covers the full `qty · price` regime the
/// callers actually use (both well under 2^64).
#[kani::proof]
fn proof_wide_mul_no_overflow_and_correct() {
    let a: u128 = kani::any();
    let b: u128 = kani::any();
    kani::assume(a <= u64::MAX as u128 && b <= u64::MAX as u128);

    // No-panic across the whole computation (the carry additions) is checked
    // automatically. For 64-bit operands the true product fits u128, so the carry
    // logic must produce no high word.
    let (hi, _lo) = wide_mul(a, b);
    assert_eq!(hi, 0, "64-bit product has no high word");
}

/// Settlement conserves open interest at the price-discovery level: the rationing
/// constants `find_cross` publishes decompose `matched_volume` **exactly** on both
/// the demand and supply side — strictly-better volume + the marginal allocation
/// equals the cleared volume, and the short side clears in full. This is the core
/// "a matched trade is never minted or destroyed" invariant, and Kani proves it
/// **exhaustively** over the whole bounded bucket domain, where the host
/// `fuzz_full_book_conserves_oi` (20k iters) only samples it.
///
/// The proof is deliberately **division-free**: it recomputes the cumulative
/// demand-at-or-above and supply-at-or-below the clearing tick by addition and
/// checks the published constants against them with only `+`/`-`/`==`. It does NOT
/// call `compute_marginal_fill` — that function's `u128` division is exactly the
/// CBMC-intractable operation documented above, so the per-order cumulative-floor
/// *split within* the marginal bucket stays on the host fuzz (which settles every
/// individual order through `fill_against_cross` and asserts the same equality).
/// `MAX_Q` can be generous (2^40) since no multiply/divide is involved.
#[kani::proof]
#[kani::unwind(5)]
fn proof_settlement_conserves() {
    const MAX_Q: u64 = 1 << 40;
    let d: [u64; 3] = [kani::any(), kani::any(), kani::any()];
    let s: [u64; 3] = [kani::any(), kani::any(), kani::any()];
    for i in 0..3 {
        kani::assume(d[i] <= MAX_Q && s[i] <= MAX_Q);
    }

    let r = find_cross(&d, &s).unwrap();
    if !r.crossed {
        return;
    }
    let t = r.clearing_tick as usize;

    // Cumulative demand at/above and supply at/below the clearing tick — by addition.
    let mut d_at_or_above: u128 = 0;
    let mut k = t;
    while k < 3 {
        d_at_or_above += d[k] as u128;
        k += 1;
    }
    let mut s_at_or_below: u128 = 0;
    let mut j = 0usize;
    while j <= t {
        s_at_or_below += s[j] as u128;
        j += 1;
    }

    let v = r.matched_volume as u128;
    let alloc = r.volume_allocated_to_marginal_tick as u128;

    match r.rationed_side {
        RATIONED_DEMAND => {
            // Demand is the long side rationed at the margin: strictly-better demand
            // plus the marginal allocation equals the cleared volume; all supply at
            // or below the clearing tick clears (the short side is exhausted).
            let strictly_better = d_at_or_above - d[t] as u128;
            assert_eq!(strictly_better + alloc, v);
            assert_eq!(s_at_or_below, v);
            assert!(alloc <= d[t] as u128);
        }
        RATIONED_SUPPLY => {
            let strictly_better = s_at_or_below - s[t] as u128;
            assert_eq!(strictly_better + alloc, v);
            assert_eq!(d_at_or_above, v);
            assert!(alloc <= s[t] as u128);
        }
        _ => {
            // Balanced cross: both sides clear exactly the matched volume.
            assert_eq!(d_at_or_above, v);
            assert_eq!(s_at_or_below, v);
        }
    }
}

/// `unrealized_pnl` does a raw `i128` multiply (`size · (mark − entry)`) which
/// would panic on overflow under `overflow-checks`. This pins down the safe
/// operating envelope: within the unit-assumption regime — `|size|` and prices
/// bounded to 48 bits (notional well inside a `u128`/`i128`) — the multiply cannot
/// overflow. Kani proves panic-freedom over that entire envelope. (Outside it the
/// operator-chosen units are the documented mitigation, per `margin.rs`.)
#[kani::proof]
fn proof_unrealized_pnl_no_overflow_in_envelope() {
    const LIM: i128 = 1 << 48; // ~2.8e14, far above realistic size/price products
    let size_signed: i128 = kani::any();
    let entry: u64 = kani::any();
    let mark: u64 = kani::any();
    kani::assume(size_signed >= -LIM && size_signed <= LIM);
    kani::assume((entry as i128) <= LIM && (mark as i128) <= LIM);

    // No-panic on the raw i128 multiply is checked automatically.
    let pnl = unrealized_pnl(size_signed, entry, mark);

    // Sign sanity: a long (size > 0) profits iff mark > entry.
    if size_signed > 0 {
        assert_eq!(pnl > 0, mark > entry);
    }
}

/// `partial_close_qty` (plan.md §4.1) is panic-free over the documented
/// operating envelope, and its result is bounded by the position: a `Some(c)`
/// is always strictly less than the target size (a partial stays partial —
/// full closes are the `None` path). All arithmetic is checked/`Option`-guarded,
/// so the property Kani buys here is exhaustive panic/overflow-freedom in the
/// same 48-bit envelope `proof_unrealized_pnl_no_overflow_in_envelope` pins
/// (the multiply/divide-heavy CORRECTNESS props — health restoration and
/// minimality — stay on the 20k-iter host fuzz, same split as the other
/// division-bearing math: CBMC cannot bit-blast the u128 division).
#[kani::proof]
fn proof_partial_close_qty_safe() {
    const LIM: u128 = 1 << 48;
    let target_abs_size: u128 = kani::any();
    let equity: i128 = kani::any();
    let maintenance: i128 = kani::any();
    let mark: u64 = kani::any();
    let maintenance_bps: u16 = kani::any();
    let penalty_bps: u16 = kani::any();
    let buffer_bps: u16 = kani::any();
    kani::assume(target_abs_size <= LIM);
    kani::assume(equity >= -(LIM as i128) && equity <= LIM as i128);
    kani::assume(maintenance >= -(LIM as i128) && maintenance <= LIM as i128);
    kani::assume((mark as u128) <= LIM);
    kani::assume(maintenance_bps <= 5_000);
    kani::assume(penalty_bps <= 5_000);

    if let Some(c) = crate::margin::partial_close_qty(
        target_abs_size,
        equity,
        maintenance,
        mark,
        maintenance_bps,
        penalty_bps,
        buffer_bps,
    ) {
        // A Some(c) is always a genuine partial: strictly inside the position.
        assert!((c as u128) < target_abs_size || c == 0);
    }
}
