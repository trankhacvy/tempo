use codama::CodamaErrors;
use pinocchio::error::ProgramError;
use thiserror::Error;

/// Errors that may be returned by the Tempo Program.
#[derive(Clone, Debug, Eq, PartialEq, Error, CodamaErrors)]
pub enum TempoProgramError {
    /// (0) Market account is invalid or does not match expected PDA
    #[error("Market account is invalid or does not match expected PDA")]
    InvalidMarket,

    /// (1) Authority invalid or does not match the market authority
    #[error("Authority invalid or does not match the market authority")]
    InvalidAuthority,

    /// (2) Market is paused. Reserved for the unbuilt pause/halt circuit-breaker
    /// (see `docs/missing-features.md` §3.2); no paused flag exists yet, so this is
    /// intentionally returned nowhere — keep it for when the feature lands.
    #[error("Market is paused")]
    MarketPaused,

    /// (3) The auction is not in the phase required for this instruction
    #[error("The auction is not in the phase required for this instruction")]
    AuctionWrongPhase,

    /// (4) The order slab is full (orders-per-auction cap reached)
    #[error("The order slab is full (orders-per-auction cap reached)")]
    OrderSlabFull,

    /// (5) Order not found in the slab
    #[error("Order not found in the slab")]
    OrderNotFound,

    /// (6) Order price is invalid (zero, or not aligned to the tick size)
    #[error("Order price is invalid (zero, or not aligned to the tick size)")]
    InvalidPrice,

    /// (7) Order quantity is invalid
    #[error("Order quantity is invalid")]
    InvalidQuantity,

    /// (8) Order quantity is zero
    #[error("Order quantity is zero")]
    ZeroQuantity,

    /// (9) Completeness check failed: not every active order has been accumulated
    #[error("Completeness check failed: not every active order has been accumulated")]
    AuctionNotComplete,

    /// (10) Order has already been accumulated into the histogram
    #[error("Order has already been accumulated into the histogram")]
    OrderAlreadyAccumulated,

    /// (11) Clearing has not been finalized yet (no ClearingResult available)
    #[error("Clearing has not been finalized yet (no ClearingResult available)")]
    ClearingNotFinalized,

    /// (12) Price does not fall on a valid tick within the histogram range
    #[error("Price does not fall on a valid tick within the histogram range")]
    InvalidTick,

    /// (13) Event authority PDA is invalid
    #[error("Event authority PDA is invalid")]
    InvalidEventAuthority,

    /// (14) Order side byte is invalid (must be 0 = buy or 1 = sell)
    #[error("Order side byte is invalid (must be 0 = buy or 1 = sell)")]
    InvalidOrderSide,

    /// (15) Order does not belong to the provided trader
    #[error("Order does not belong to the provided trader")]
    InvalidOrderOwner,

    /// (16) Histogram / market / clearing-result auction id mismatch
    #[error("Auction id mismatch between accounts")]
    AuctionIdMismatch,

    /// (17) Order is in a state that cannot transition as requested
    #[error("Order is in an invalid state for this operation")]
    InvalidOrderStatus,

    /// (18) Arithmetic overflow during clearing math
    #[error("Arithmetic overflow during clearing math")]
    MathOverflow,

    /// (19) Provided account is the wrong type / does not match the market
    #[error("Provided account does not belong to this market")]
    AccountMarketMismatch,

    /// (20) Oracle account is not a valid Pyth PriceUpdateV2 / wrong owner
    #[error("Oracle account is not a valid Pyth price update")]
    OracleInvalidAccount,

    /// (21) Oracle feed id does not match the market's expected feed
    #[error("Oracle feed id does not match the expected feed")]
    OracleFeedMismatch,

    /// (22) Oracle price update is too old
    #[error("Oracle price update is stale")]
    OracleStale,

    /// (23) Oracle reported a non-positive price
    #[error("Oracle reported a non-positive price")]
    OracleNegativePrice,

    /// (24) Not enough free collateral for this operation
    #[error("Not enough free collateral")]
    InsufficientCollateral,

    /// (25) Position is not liquidatable (still above maintenance margin)
    #[error("Position is not liquidatable")]
    NotLiquidatable,

    /// (26) Token account / mint does not match the vault configuration
    #[error("Token account or mint does not match the vault")]
    InvalidCollateralAccount,

    /// (27) A settle with a non-zero fill must carry the trader's Position and
    /// collateral ledger (and the vault when a protocol fee applies) so the
    /// matched trade is always recorded — never silently discarded.
    #[error("Settle with a non-zero fill is missing required position/collateral accounts")]
    MissingSettleAccounts,

    /// (28) Oracle confidence interval is too wide relative to the price; the
    /// price is too uncertain to use for funding/liquidation (system-design §10).
    #[error("Oracle confidence interval is too wide")]
    OracleConfidenceTooWide,

    /// (29) Order quantity is below the market minimum (anti-dust).
    #[error("Order quantity is below the minimum order size")]
    OrderBelowMinimum,

    /// (30) Per-trader order cap for this auction reached.
    #[error("Per-trader order cap reached for this auction")]
    TraderOrderCapReached,

    /// (31) Market configuration parameter is out of the allowed range.
    #[error("Market configuration parameter is out of range")]
    MarketConfigOutOfRange,

    /// (32) process_chunk attempted before the collection window closed.
    #[error("Collection window is still open; accumulation cannot start yet")]
    AuctionWindowOpen,

    /// (33) A winner's realized gain cannot be paid: the insurance pool is short
    /// and no open interest can absorb the residual. Fail closed (delay, not loss):
    /// the settle reverts and can be retried once losses are collected or insurance
    /// is topped up (hard solvency gate).
    #[error("Insurance pool is insolvent for this payout")]
    InsuranceInsolvent,

    /// (34) A liquidation step made no progress (the maintenance deficit did not
    /// shrink and the position is not flat); reverts cleanly.
    #[error("Liquidation made no progress")]
    LiquidationNoProgress,

    /// (35) The oracle is soft-stale: too old to take a fresh price, but within the
    /// hard window. Extraction-sensitive actions reject; conservative maintenance
    /// may proceed on the frozen effective price.
    #[error("Oracle is soft-stale; extraction is blocked")]
    OracleSoftStale,

    /// (36) A cross-margin group already holds `MAX_CROSS_POSITIONS` members.
    #[error("Cross-margin group is full")]
    MarginGroupFull,

    /// (37) A position is already a member of the cross-margin group.
    #[error("Position is already in the cross-margin group")]
    MarginMemberDuplicate,

    /// (38) A cross-margin extraction/health check was not supplied every member
    /// position — omitting one (e.g. a losing leg) must fail closed.
    #[error("Cross-margin instruction is missing member positions")]
    IncompletePortfolio,

    /// (39) RESERVED — formerly "cross-margin member market is stale". After the
    /// §2.2 `risk_price` removal, member-market staleness surfaces through the shared
    /// oracle path as `OracleSoftStale`, so nothing produces this anymore. Kept (not
    /// renumbered) to preserve the stable error-code mapping for clients, mirroring
    /// the reserved `MarketPaused` (known-issues §2.9e); re-use it only if a
    /// cross-margin-specific staleness error is ever genuinely needed.
    #[error("Cross-margin member market is stale (reserved)")]
    MarginMarketStale,

    /// (40) The account presented to a migration is not at the expected prior
    /// layout version/size (already migrated, or never the old version).
    #[error("Account is not at the expected pre-migration layout")]
    NotMigratable,

    /// (41) The position is not a member of the cross-margin group.
    #[error("Position is not a member of the cross-margin group")]
    MarginMemberNotFound,

    /// (42) A Pyth update's `publish_time` is in the future beyond tolerance. A
    /// future timestamp cannot be a merely-lagging feed, so it is treated as invalid
    /// and hard-rejected — never allowed to fall through to the soft-stale frozen
    /// mark the way an honestly-old update does (known-issues §2.9a).
    #[error("Oracle publish time is in the future")]
    OracleFutureTimestamp,

    /// (43) Cross-margin invariant violation: the summed collateral of a group's
    /// supplied members exceeds the shared ledger's `locked` total, i.e. the
    /// `MarginAccount` member set and the `UserCollateral` ledger have drifted. The
    /// foreign-locked floor surfaces this loud instead of clamping it to zero (which
    /// would let the owner withdraw margin reserved for non-group positions —
    /// known-issues §2.8).
    #[error("Cross-margin collateral ledger drift")]
    CollateralLedgerDrift,

    /// (44) A submitted order's worst-case resulting position would exceed the
    /// market's `max_position_notional` cap (missing-features §1.2). Rejected at
    /// submit (pre-trade), so the cap can never be breached by an already-matched
    /// trade that must settle.
    #[error("Order would exceed the market's max position notional")]
    PositionLimitExceeded,

    /// (45) A requested `shard_id` is outside `[0, num_slab_shards)` (Stage A
    /// sharding). Names the actual shard-index violation instead of the previously
    /// misleading `InvalidTick` (which points at price/tick logic).
    #[error("Shard id is out of range for this market")]
    ShardOutOfRange,

    /// (46) A submitted order's `expires_at_auction` is already reached/passed at
    /// submit time (`!= 0 && <= current_auction_id`) — it could never fold or fill,
    /// so it is rejected up front (DDR-3 Correction-2 item 4) rather than resting as
    /// dead margin the reaper must later collect.
    #[error("Order expiry is already reached at submit time")]
    OrderAlreadyExpired,
}

impl From<TempoProgramError> for ProgramError {
    fn from(e: TempoProgramError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_conversion() {
        let error: ProgramError = TempoProgramError::InvalidMarket.into();
        assert_eq!(error, ProgramError::Custom(0));

        let error: ProgramError = TempoProgramError::InvalidAuthority.into();
        assert_eq!(error, ProgramError::Custom(1));

        let error: ProgramError = TempoProgramError::AuctionWrongPhase.into();
        assert_eq!(error, ProgramError::Custom(3));

        let error: ProgramError = TempoProgramError::AuctionNotComplete.into();
        assert_eq!(error, ProgramError::Custom(9));

        let error: ProgramError = TempoProgramError::ClearingNotFinalized.into();
        assert_eq!(error, ProgramError::Custom(11));

        let error: ProgramError = TempoProgramError::AccountMarketMismatch.into();
        assert_eq!(error, ProgramError::Custom(19));
    }
}
