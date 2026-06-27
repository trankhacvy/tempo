//! Pyth price-feed reader (system-design §10).
//!
//! Reads a Pyth **`PriceUpdateV2`** account — the format used by both the pull
//! receiver and the sponsored push feeds (docs.pyth.network → push-feeds/solana)
//! — by parsing its byte layout directly, so it works under Pinocchio `no_std`
//! without the Anchor-flavored `pyth-solana-receiver-sdk`.
//!
//! The account is owned by the Pyth receiver program; the caller must verify
//! that ownership before trusting the bytes (the program id below).
//!
//! `PriceUpdateV2` borsh layout:
//! ```text
//! 0   .. 8    anchor discriminator
//! 8   .. 40   write_authority: Pubkey
//! 40  .. B    verification_level  (Full = 1 byte tag; Partial = tag + u8 → B varies)
//! B   .. B+32 price_message.feed_id: [u8;32]
//! B+32.. B+40 price_message.price: i64 (LE)
//! B+40.. B+48 price_message.conf: u64 (LE)
//! B+48.. B+52 price_message.exponent: i32 (LE)
//! B+52.. B+60 price_message.publish_time: i64 (LE)
//! ...         prev_publish_time, ema_price, ema_conf, posted_slot
//! ```

use pinocchio::{address::Address, error::ProgramError};

use crate::errors::TempoProgramError;

/// Pyth receiver program id (`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`),
/// the owner of every `PriceUpdateV2` account on Solana mainnet + devnet.
pub const PYTH_RECEIVER_ID: Address = Address::new_from_array([
    12, 183, 250, 187, 82, 247, 166, 72, 187, 91, 49, 125, 154, 1, 139, 144, 87, 203, 2, 71, 116,
    250, 254, 1, 230, 196, 223, 152, 204, 56, 88, 129,
]);

/// SOL/USD feed id (`0xef0d8b…b56d`).
pub const SOL_USD_FEED_ID: [u8; 32] = [
    239, 13, 139, 111, 218, 44, 235, 164, 29, 161, 93, 64, 149, 209, 218, 57, 42, 13, 47, 142, 208,
    198, 199, 188, 15, 76, 250, 200, 194, 128, 181, 109,
];

/// Minimum `PriceUpdateV2` account size (Full verification level path).
const MIN_LEN: usize = 134;
/// Offset of the verification-level tag.
const VL_OFFSET: usize = 40;

/// Default maximum oracle confidence interval, in basis points of price
/// (system-design §10). A print whose `conf/price` exceeds this is rejected as
/// too uncertain for funding/liquidation. Conservative value (5%); normal SOL
/// prints sit in the single-digit bps range.
pub const DEFAULT_MAX_CONF_BPS: u16 = 500;

/// A validated Pyth price.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OraclePrice {
    pub feed_id: [u8; 32],
    pub price: i64,
    pub conf: u64,
    pub exponent: i32,
    pub publish_time: i64,
    /// `price` normalized to fixed 1e8 units (positive).
    pub price_1e8: u64,
}

impl OraclePrice {
    /// Confidence interval as a fraction of price, in basis points
    /// (`conf / price * 10_000`). `conf` and `price` share the exponent, so the
    /// raw units cancel; `price` is guaranteed positive by [`read_price`].
    #[inline(always)]
    pub fn confidence_bps(&self) -> u64 {
        if self.price <= 0 {
            return u64::MAX;
        }
        ((self.conf as u128 * 10_000) / self.price as u128) as u64
    }

    /// Reject a price whose confidence interval is wider than `max_bps` of price
    /// (system-design §10 — halt when the oracle is too uncertain).
    #[inline(always)]
    pub fn require_confidence(&self, max_bps: u16) -> Result<(), ProgramError> {
        if self.confidence_bps() > max_bps as u64 {
            return Err(TempoProgramError::OracleConfidenceTooWide.into());
        }
        Ok(())
    }
}

/// Parse + validate a `PriceUpdateV2` account.
///
/// Verifies the feed id matches `expected_feed_id`, the price is positive, and
/// the update is no older than `max_age_secs` relative to `now_ts` (the Clock
/// `unix_timestamp`). Ownership of the account by [`PYTH_RECEIVER_ID`] must be
/// checked by the caller.
pub fn read_price(
    data: &[u8],
    expected_feed_id: &[u8; 32],
    now_ts: i64,
    max_age_secs: i64,
) -> Result<OraclePrice, ProgramError> {
    if data.len() < MIN_LEN {
        return Err(TempoProgramError::OracleInvalidAccount.into());
    }
    // verification_level: Full(tag=1) → 1 byte; Partial(tag=0) → tag + num_sigs.
    let base = match data[VL_OFFSET] {
        1 => VL_OFFSET + 1,
        0 => VL_OFFSET + 2,
        _ => return Err(TempoProgramError::OracleInvalidAccount.into()),
    };
    if data.len() < base + 60 {
        return Err(TempoProgramError::OracleInvalidAccount.into());
    }

    let mut feed_id = [0u8; 32];
    feed_id.copy_from_slice(&data[base..base + 32]);
    if &feed_id != expected_feed_id {
        return Err(TempoProgramError::OracleFeedMismatch.into());
    }

    let price = i64::from_le_bytes(data[base + 32..base + 40].try_into().unwrap());
    let conf = u64::from_le_bytes(data[base + 40..base + 48].try_into().unwrap());
    let exponent = i32::from_le_bytes(data[base + 48..base + 52].try_into().unwrap());
    let publish_time = i64::from_le_bytes(data[base + 52..base + 60].try_into().unwrap());

    if price <= 0 {
        return Err(TempoProgramError::OracleNegativePrice.into());
    }
    // Staleness. A future publish_time and an old one are NOT the same failure: a
    // merely-old update may degrade to the frozen soft-stale mark, but a *future*
    // timestamp cannot be a lagging feed, so it is hard-rejected with a distinct
    // error that `solvency_mark` never treats as soft-stale (known-issues §2.9a).
    if publish_time > now_ts.saturating_add(max_age_secs) {
        return Err(TempoProgramError::OracleFutureTimestamp.into());
    }
    if now_ts.saturating_sub(publish_time) > max_age_secs {
        return Err(TempoProgramError::OracleStale.into());
    }

    let price_1e8 = normalize_1e8(price, exponent)?;

    Ok(OraclePrice {
        feed_id,
        price,
        conf,
        exponent,
        publish_time,
        price_1e8,
    })
}

/// Normalize a Pyth `(price, exponent)` to fixed 1e8 units. Pyth prices are
/// `price * 10^exponent`; exponent is typically `-8`.
fn normalize_1e8(price: i64, exponent: i32) -> Result<u64, ProgramError> {
    let shift = exponent + 8; // target exponent is -8
    let p = price as i128;
    let scaled = if shift >= 0 {
        let factor = 10i128
            .checked_pow(shift as u32)
            .ok_or(TempoProgramError::MathOverflow)?;
        p.checked_mul(factor)
            .ok_or(TempoProgramError::MathOverflow)?
    } else {
        let factor = 10i128
            .checked_pow((-shift) as u32)
            .ok_or(TempoProgramError::MathOverflow)?;
        p / factor
    };
    u64::try_from(scaled).map_err(|_| TempoProgramError::MathOverflow.into())
}

/// Maximum age (seconds) of a Pyth update accepted for funding/solvency pricing.
/// Shared by every instruction that prices off the oracle so the staleness bound
/// cannot drift between them.
pub const MAX_AGE_SECS: i64 = 120;

/// A resolved solvency price plus the freshness source it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolvencyMark {
    /// Fresh, confidence-checked raw oracle — the caller MAY advance the brake.
    Fresh(u64),
    /// Oracle is only soft-stale; priced off the frozen effective price (the brake
    /// must NOT be advanced).
    Frozen(u64),
}

impl SolvencyMark {
    /// The price to use for solvency, regardless of source.
    #[inline(always)]
    pub fn price(self) -> u64 {
        match self {
            SolvencyMark::Fresh(p) | SolvencyMark::Frozen(p) => p,
        }
    }
}

/// Resolve a market's **solvency** mark (known-issues §2.2).
///
/// Solvency is priced off the RAW, confidence-checked oracle — never the braked
/// `effective_price`. The per-slot price brake is an anti-manipulation rail for
/// funding and order-acceptance only; pricing solvency off it lets the lagged mark
/// double as an *anti-liquidation* brake during a crash (a position the real price
/// has put underwater stays un-liquidatable, and over-withdrawal is permitted). So:
///
/// - **fresh** raw oracle (confidence-checked) → [`SolvencyMark::Fresh`]; the caller
///   may advance + persist the braked mark off this raw price (for funding).
/// - **soft-stale** (no fresh update, but within `soft_stale_slots` of the last good
///   one) → [`SolvencyMark::Frozen`] off the frozen `effective_price`; do not advance.
/// - **hard-stale** (beyond the window, or no effective price to fall back on) →
///   `OracleSoftStale`, so only wind-down may proceed.
///
/// Ownership of `oracle_data`'s account by [`PYTH_RECEIVER_ID`] must be verified by
/// the caller (this only parses bytes).
pub fn solvency_mark(
    oracle_data: &[u8],
    feed_id: &[u8; 32],
    now_ts: i64,
    now_slot: u64,
    effective_price: u64,
    last_good_oracle_slot: u64,
    soft_stale_slots: u64,
) -> Result<SolvencyMark, ProgramError> {
    match read_price(oracle_data, feed_id, now_ts, MAX_AGE_SECS) {
        Ok(price) => {
            price.require_confidence(DEFAULT_MAX_CONF_BPS)?;
            Ok(SolvencyMark::Fresh(price.price_1e8))
        }
        Err(e) => {
            if e != ProgramError::from(TempoProgramError::OracleStale) {
                return Err(e);
            }
            let age_slots = now_slot.saturating_sub(last_good_oracle_slot);
            if effective_price == 0 || soft_stale_slots == 0 || age_slots > soft_stale_slots {
                return Err(TempoProgramError::OracleSoftStale.into());
            }
            Ok(SolvencyMark::Frozen(effective_price))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Build a synthetic Full-verification `PriceUpdateV2` buffer.
    fn synthetic(
        feed_id: &[u8; 32],
        price: i64,
        exponent: i32,
        publish_time: i64,
    ) -> alloc::vec::Vec<u8> {
        let mut d = vec![0u8; MIN_LEN];
        d[VL_OFFSET] = 1; // Full
        let base = VL_OFFSET + 1;
        d[base..base + 32].copy_from_slice(feed_id);
        d[base + 32..base + 40].copy_from_slice(&price.to_le_bytes());
        d[base + 40..base + 48].copy_from_slice(&7_500_000u64.to_le_bytes()); // conf
        d[base + 48..base + 52].copy_from_slice(&exponent.to_le_bytes());
        d[base + 52..base + 60].copy_from_slice(&publish_time.to_le_bytes());
        d
    }

    #[test]
    fn test_reads_sol_usd_price() {
        // SOL ~ $171.769..., expo -8 → price 17176900000.
        let d = synthetic(&SOL_USD_FEED_ID, 17_176_900_000, -8, 1_000_000);
        let p = read_price(&d, &SOL_USD_FEED_ID, 1_000_030, 60).unwrap();
        assert_eq!(p.price, 17_176_900_000);
        assert_eq!(p.exponent, -8);
        assert_eq!(p.price_1e8, 17_176_900_000); // expo -8 → already 1e8
    }

    #[test]
    fn test_normalize_other_exponent() {
        // expo -6 → multiply by 100 to reach 1e8.
        let d = synthetic(&SOL_USD_FEED_ID, 171_769, -6, 1_000_000);
        let p = read_price(&d, &SOL_USD_FEED_ID, 1_000_000, 60).unwrap();
        assert_eq!(p.price_1e8, 17_176_900);
    }

    #[test]
    fn test_feed_mismatch() {
        let d = synthetic(&[9u8; 32], 1, -8, 1_000_000);
        assert_eq!(
            read_price(&d, &SOL_USD_FEED_ID, 1_000_000, 60),
            Err(TempoProgramError::OracleFeedMismatch.into())
        );
    }

    #[test]
    fn test_stale_rejected() {
        let d = synthetic(&SOL_USD_FEED_ID, 100, -8, 1_000_000);
        assert_eq!(
            read_price(&d, &SOL_USD_FEED_ID, 1_000_999, 60),
            Err(TempoProgramError::OracleStale.into())
        );
    }

    #[test]
    fn test_future_timestamp_rejected_distinctly() {
        // publish_time 1_000_999 while now is 1_000_000 (999s in the future, > 120s)
        // → a distinct hard error, NOT the soft-stale `OracleStale` an old update gets.
        let d = synthetic(&SOL_USD_FEED_ID, 100, -8, 1_000_999);
        assert_eq!(
            read_price(&d, &SOL_USD_FEED_ID, 1_000_000, 60),
            Err(TempoProgramError::OracleFutureTimestamp.into())
        );
    }

    #[test]
    fn test_solvency_mark_future_timestamp_does_not_soft_stale() {
        // A future-stamped oracle must hard-reject even when a frozen effective price
        // and a wide soft-stale window are available — a future time can't be a
        // lagging feed, so it never falls back to the frozen mark (§2.9a).
        let d = synthetic(&SOL_USD_FEED_ID, 100_00000000, -8, 1_001_000);
        assert_eq!(
            solvency_mark(
                &d,
                &SOL_USD_FEED_ID,
                1_000_000,
                50,
                120_00000000,
                49,
                10_000
            ),
            Err(TempoProgramError::OracleFutureTimestamp.into())
        );
    }

    #[test]
    fn test_negative_price_rejected() {
        let d = synthetic(&SOL_USD_FEED_ID, -5, -8, 1_000_000);
        assert_eq!(
            read_price(&d, &SOL_USD_FEED_ID, 1_000_000, 60),
            Err(TempoProgramError::OracleNegativePrice.into())
        );
    }

    #[test]
    fn test_confidence_gate() {
        // Realistic SOL print: conf ~7.5M on a ~17.2B price ≈ 4 bps → accepted.
        let tight = OraclePrice {
            feed_id: SOL_USD_FEED_ID,
            price: 17_176_900_000,
            conf: 7_500_000,
            exponent: -8,
            publish_time: 0,
            price_1e8: 17_176_900_000,
        };
        assert!(tight.confidence_bps() < 10);
        assert!(tight.require_confidence(DEFAULT_MAX_CONF_BPS).is_ok());

        // Wide print: conf = 10% of price → 1000 bps > 500 → rejected.
        let wide = OraclePrice {
            conf: 1_717_690_000,
            ..tight
        };
        assert_eq!(wide.confidence_bps(), 1000);
        assert_eq!(
            wide.require_confidence(DEFAULT_MAX_CONF_BPS),
            Err(TempoProgramError::OracleConfidenceTooWide.into())
        );
    }

    #[test]
    fn test_partial_verification_offset() {
        // Partial: tag 0 + num_sigs byte → price_message shifts by 1.
        let mut d = vec![0u8; MIN_LEN + 1];
        d[VL_OFFSET] = 0;
        d[VL_OFFSET + 1] = 5; // num_signatures
        let base = VL_OFFSET + 2;
        d[base..base + 32].copy_from_slice(&SOL_USD_FEED_ID);
        d[base + 32..base + 40].copy_from_slice(&50i64.to_le_bytes());
        d[base + 48..base + 52].copy_from_slice(&(-8i32).to_le_bytes());
        d[base + 52..base + 60].copy_from_slice(&1_000_000i64.to_le_bytes());
        let p = read_price(&d, &SOL_USD_FEED_ID, 1_000_010, 60).unwrap();
        assert_eq!(p.price, 50);
    }

    // -- solvency_mark (§2.2): raw oracle, soft-stale fallback, hard-stale halt --

    #[test]
    fn test_solvency_mark_prefers_fresh_raw_oracle() {
        // Fresh print at 100 (expo -8) while the frozen effective price lags at 120.
        let d = synthetic(&SOL_USD_FEED_ID, 100_00000000, -8, 1_000_000);
        let m = solvency_mark(&d, &SOL_USD_FEED_ID, 1_000_010, 50, 120_00000000, 49, 0).unwrap();
        // Solvency follows the RAW oracle (100), not the lagged effective price (120).
        assert_eq!(m, SolvencyMark::Fresh(100_00000000));
        assert_eq!(m.price(), 100_00000000);
    }

    #[test]
    fn test_solvency_mark_soft_stale_uses_frozen_effective() {
        // Stale print (publish 1_000_000, now 1_000_999 > 120s) but within the
        // soft-stale slot window → fall back to the frozen effective price.
        let d = synthetic(&SOL_USD_FEED_ID, 100_00000000, -8, 1_000_000);
        let m = solvency_mark(&d, &SOL_USD_FEED_ID, 1_000_999, 50, 120_00000000, 45, 10).unwrap();
        assert_eq!(m, SolvencyMark::Frozen(120_00000000));
    }

    #[test]
    fn test_solvency_mark_hard_stale_rejects() {
        let d = synthetic(&SOL_USD_FEED_ID, 100_00000000, -8, 1_000_000);
        // Beyond the soft-stale window (age 50 - 30 = 20 > 10) → hard-stale halt.
        assert_eq!(
            solvency_mark(&d, &SOL_USD_FEED_ID, 1_000_999, 50, 120_00000000, 30, 10),
            Err(TempoProgramError::OracleSoftStale.into())
        );
        // Soft-stale disabled (window 0) → any stale oracle is hard-stale.
        assert_eq!(
            solvency_mark(&d, &SOL_USD_FEED_ID, 1_000_999, 50, 120_00000000, 49, 0),
            Err(TempoProgramError::OracleSoftStale.into())
        );
        // No effective price to fall back on → hard-stale even inside the window.
        assert_eq!(
            solvency_mark(&d, &SOL_USD_FEED_ID, 1_000_999, 50, 0, 45, 10),
            Err(TempoProgramError::OracleSoftStale.into())
        );
    }
}
