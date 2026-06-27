//! Pyth `PriceUpdateV2` reader. Byte-for-byte mirror of `program/src/oracle.rs`
//! so an off-chain solvency check runs the *same* arithmetic the program enforces
//! (the golden guard — the program's own tests are copied verbatim below). The
//! only edits vs the program are `ProgramError` → [`MathError`] and dropping the
//! Pinocchio `Address` ownership constant: the SDK verifies Pyth-receiver
//! ownership at fetch time before trusting these bytes.

use crate::MathError;

/// SOL/USD feed id (`0xef0d8b…b56d`).
pub const SOL_USD_FEED_ID: [u8; 32] = [
    239, 13, 139, 111, 218, 44, 235, 164, 29, 161, 93, 64, 149, 209, 218, 57, 42, 13, 47, 142, 208,
    198, 199, 188, 15, 76, 250, 200, 194, 128, 181, 109,
];

/// Minimum `PriceUpdateV2` account size (Full verification level path).
const MIN_LEN: usize = 134;
/// Offset of the verification-level tag.
const VL_OFFSET: usize = 40;

/// Default maximum oracle confidence interval, in basis points of price.
pub const DEFAULT_MAX_CONF_BPS: u16 = 500;

/// Maximum age (seconds) of a Pyth update accepted for funding/solvency pricing.
pub const MAX_AGE_SECS: i64 = 120;

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
    /// Confidence interval as a fraction of price, in basis points.
    #[inline(always)]
    pub fn confidence_bps(&self) -> u64 {
        if self.price <= 0 {
            return u64::MAX;
        }
        ((self.conf as u128 * 10_000) / self.price as u128) as u64
    }

    /// Reject a price whose confidence interval is wider than `max_bps` of price.
    #[inline(always)]
    pub fn require_confidence(&self, max_bps: u16) -> Result<(), MathError> {
        if self.confidence_bps() > max_bps as u64 {
            return Err(MathError::OracleConfidenceTooWide);
        }
        Ok(())
    }
}

/// Parse + validate a `PriceUpdateV2` account.
pub fn read_price(
    data: &[u8],
    expected_feed_id: &[u8; 32],
    now_ts: i64,
    max_age_secs: i64,
) -> Result<OraclePrice, MathError> {
    if data.len() < MIN_LEN {
        return Err(MathError::OracleInvalidAccount);
    }
    let base = match data[VL_OFFSET] {
        1 => VL_OFFSET + 1,
        0 => VL_OFFSET + 2,
        _ => return Err(MathError::OracleInvalidAccount),
    };
    if data.len() < base + 60 {
        return Err(MathError::OracleInvalidAccount);
    }

    let mut feed_id = [0u8; 32];
    feed_id.copy_from_slice(&data[base..base + 32]);
    if &feed_id != expected_feed_id {
        return Err(MathError::OracleFeedMismatch);
    }

    let price = i64::from_le_bytes(data[base + 32..base + 40].try_into().unwrap());
    let conf = u64::from_le_bytes(data[base + 40..base + 48].try_into().unwrap());
    let exponent = i32::from_le_bytes(data[base + 48..base + 52].try_into().unwrap());
    let publish_time = i64::from_le_bytes(data[base + 52..base + 60].try_into().unwrap());

    if price <= 0 {
        return Err(MathError::OracleNegativePrice);
    }
    if publish_time > now_ts.saturating_add(max_age_secs) {
        return Err(MathError::OracleFutureTimestamp);
    }
    if now_ts.saturating_sub(publish_time) > max_age_secs {
        return Err(MathError::OracleStale);
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

/// Normalize a Pyth `(price, exponent)` to fixed 1e8 units.
fn normalize_1e8(price: i64, exponent: i32) -> Result<u64, MathError> {
    let shift = exponent + 8;
    let p = price as i128;
    let scaled = if shift >= 0 {
        let factor = 10i128
            .checked_pow(shift as u32)
            .ok_or(MathError::Overflow)?;
        p.checked_mul(factor).ok_or(MathError::Overflow)?
    } else {
        let factor = 10i128
            .checked_pow((-shift) as u32)
            .ok_or(MathError::Overflow)?;
        p / factor
    };
    u64::try_from(scaled).map_err(|_| MathError::Overflow)
}

/// A resolved solvency price plus the freshness source it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolvencyMark {
    /// Fresh, confidence-checked raw oracle.
    Fresh(u64),
    /// Oracle is only soft-stale; priced off the frozen effective price.
    Frozen(u64),
}

impl SolvencyMark {
    #[inline(always)]
    pub fn price(self) -> u64 {
        match self {
            SolvencyMark::Fresh(p) | SolvencyMark::Frozen(p) => p,
        }
    }
}

/// Resolve a market's solvency mark off the RAW, confidence-checked oracle, with
/// the soft-stale → frozen-effective fall-back. Mirror of `oracle::solvency_mark`.
pub fn solvency_mark(
    oracle_data: &[u8],
    feed_id: &[u8; 32],
    now_ts: i64,
    now_slot: u64,
    effective_price: u64,
    last_good_oracle_slot: u64,
    soft_stale_slots: u64,
) -> Result<SolvencyMark, MathError> {
    match read_price(oracle_data, feed_id, now_ts, MAX_AGE_SECS) {
        Ok(price) => {
            price.require_confidence(DEFAULT_MAX_CONF_BPS)?;
            Ok(SolvencyMark::Fresh(price.price_1e8))
        }
        Err(e) => {
            if e != MathError::OracleStale {
                return Err(e);
            }
            let age_slots = now_slot.saturating_sub(last_good_oracle_slot);
            if effective_price == 0 || soft_stale_slots == 0 || age_slots > soft_stale_slots {
                return Err(MathError::OracleSoftStale);
            }
            Ok(SolvencyMark::Frozen(effective_price))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(feed_id: &[u8; 32], price: i64, exponent: i32, publish_time: i64) -> Vec<u8> {
        let mut d = vec![0u8; MIN_LEN];
        d[VL_OFFSET] = 1; // Full
        let base = VL_OFFSET + 1;
        d[base..base + 32].copy_from_slice(feed_id);
        d[base + 32..base + 40].copy_from_slice(&price.to_le_bytes());
        d[base + 40..base + 48].copy_from_slice(&7_500_000u64.to_le_bytes());
        d[base + 48..base + 52].copy_from_slice(&exponent.to_le_bytes());
        d[base + 52..base + 60].copy_from_slice(&publish_time.to_le_bytes());
        d
    }

    #[test]
    fn test_reads_sol_usd_price() {
        let d = synthetic(&SOL_USD_FEED_ID, 17_176_900_000, -8, 1_000_000);
        let p = read_price(&d, &SOL_USD_FEED_ID, 1_000_030, 60).unwrap();
        assert_eq!(p.price, 17_176_900_000);
        assert_eq!(p.exponent, -8);
        assert_eq!(p.price_1e8, 17_176_900_000);
    }

    #[test]
    fn test_normalize_other_exponent() {
        let d = synthetic(&SOL_USD_FEED_ID, 171_769, -6, 1_000_000);
        let p = read_price(&d, &SOL_USD_FEED_ID, 1_000_000, 60).unwrap();
        assert_eq!(p.price_1e8, 17_176_900);
    }

    #[test]
    fn test_feed_mismatch() {
        let d = synthetic(&[9u8; 32], 1, -8, 1_000_000);
        assert_eq!(
            read_price(&d, &SOL_USD_FEED_ID, 1_000_000, 60),
            Err(MathError::OracleFeedMismatch)
        );
    }

    #[test]
    fn test_stale_rejected() {
        let d = synthetic(&SOL_USD_FEED_ID, 100, -8, 1_000_000);
        assert_eq!(
            read_price(&d, &SOL_USD_FEED_ID, 1_000_999, 60),
            Err(MathError::OracleStale)
        );
    }

    #[test]
    fn test_future_timestamp_rejected_distinctly() {
        let d = synthetic(&SOL_USD_FEED_ID, 100, -8, 1_000_999);
        assert_eq!(
            read_price(&d, &SOL_USD_FEED_ID, 1_000_000, 60),
            Err(MathError::OracleFutureTimestamp)
        );
    }

    #[test]
    fn test_negative_price_rejected() {
        let d = synthetic(&SOL_USD_FEED_ID, -5, -8, 1_000_000);
        assert_eq!(
            read_price(&d, &SOL_USD_FEED_ID, 1_000_000, 60),
            Err(MathError::OracleNegativePrice)
        );
    }

    #[test]
    fn test_confidence_gate() {
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

        let wide = OraclePrice {
            conf: 1_717_690_000,
            ..tight
        };
        assert_eq!(wide.confidence_bps(), 1000);
        assert_eq!(
            wide.require_confidence(DEFAULT_MAX_CONF_BPS),
            Err(MathError::OracleConfidenceTooWide)
        );
    }

    #[test]
    fn test_partial_verification_offset() {
        let mut d = vec![0u8; MIN_LEN + 1];
        d[VL_OFFSET] = 0;
        d[VL_OFFSET + 1] = 5;
        let base = VL_OFFSET + 2;
        d[base..base + 32].copy_from_slice(&SOL_USD_FEED_ID);
        d[base + 32..base + 40].copy_from_slice(&50i64.to_le_bytes());
        d[base + 48..base + 52].copy_from_slice(&(-8i32).to_le_bytes());
        d[base + 52..base + 60].copy_from_slice(&1_000_000i64.to_le_bytes());
        let p = read_price(&d, &SOL_USD_FEED_ID, 1_000_010, 60).unwrap();
        assert_eq!(p.price, 50);
    }

    #[test]
    fn test_solvency_mark_future_timestamp_does_not_soft_stale() {
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
            Err(MathError::OracleFutureTimestamp)
        );
    }

    #[test]
    fn test_solvency_mark_prefers_fresh_raw_oracle() {
        let d = synthetic(&SOL_USD_FEED_ID, 100_00000000, -8, 1_000_000);
        let m = solvency_mark(&d, &SOL_USD_FEED_ID, 1_000_010, 50, 120_00000000, 49, 0).unwrap();
        assert_eq!(m, SolvencyMark::Fresh(100_00000000));
        assert_eq!(m.price(), 100_00000000);
    }

    #[test]
    fn test_solvency_mark_soft_stale_uses_frozen_effective() {
        let d = synthetic(&SOL_USD_FEED_ID, 100_00000000, -8, 1_000_000);
        let m = solvency_mark(&d, &SOL_USD_FEED_ID, 1_000_999, 50, 120_00000000, 45, 10).unwrap();
        assert_eq!(m, SolvencyMark::Frozen(120_00000000));
    }

    #[test]
    fn test_solvency_mark_hard_stale_rejects() {
        let d = synthetic(&SOL_USD_FEED_ID, 100_00000000, -8, 1_000_000);
        assert_eq!(
            solvency_mark(&d, &SOL_USD_FEED_ID, 1_000_999, 50, 120_00000000, 30, 10),
            Err(MathError::OracleSoftStale)
        );
        assert_eq!(
            solvency_mark(&d, &SOL_USD_FEED_ID, 1_000_999, 50, 120_00000000, 49, 0),
            Err(MathError::OracleSoftStale)
        );
        assert_eq!(
            solvency_mark(&d, &SOL_USD_FEED_ID, 1_000_999, 50, 0, 45, 10),
            Err(MathError::OracleSoftStale)
        );
    }
}
