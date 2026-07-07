use pinocchio::{account::AccountView, error::ProgramError};

/// Discriminators for the Tempo Program instructions.
#[repr(u8)]
pub enum TempoInstructionDiscriminators {
    /// Admin: create a Market + empty AuctionHistogram + OrderSlab.
    InitializeMarket = 0,
    /// Trader: place a resting order into the slab (phase must be Collect).
    SubmitOrder = 1,
    /// Trader: remove a resting order before clearing begins.
    CancelOrder = 2,
    /// Phase 1 ACCUMULATE (permissionless): fold a bounded slice of orders.
    ProcessChunk = 3,
    /// Phase 2 DISCOVER (permissionless): find clearing price, write ClearingResult.
    FinalizeClear = 4,
    /// Phase 3 SETTLE (permissionless to trigger): compute one order's fill.
    SettleFill = 5,
    /// Roll the market into its next round (permissionless, post-settlement).
    StartAuction = 6,
    /// Create a trader's Position account for a market.
    InitPosition = 7,
    /// Read the bound Pyth oracle, derive mark price, emit it (system-design §10).
    ReadOracle = 8,
    /// Admin: create the global collateral `Vault` singleton.
    InitVault = 9,
    /// Trader: create their `UserCollateral` ledger.
    InitCollateral = 10,
    /// Trader: deposit collateral into the vault.
    Deposit = 11,
    /// Trader: withdraw free collateral from the vault.
    Withdraw = 12,
    /// Permissionless: advance the market's funding index.
    UpdateFunding = 13,
    /// Permissionless: liquidate a position below maintenance margin.
    Liquidate = 14,
    /// Authority-gated escape hatch: force-reset a wedged round (system-design §7).
    ForceReset = 15,
    /// Maker: create a persistent parametric quote account.
    InitMakerQuote = 16,
    /// Maker: re-anchor the quote ladder by moving its mid (hot path).
    UpdateMakerQuoteMid = 17,
    /// Maker: rewrite the quote ladder levels.
    UpdateMakerQuoteLevels = 18,
    /// Maker: zero the ladder and deactivate the quote.
    ClearMakerQuote = 19,
    /// Permissionless crank: fold one maker quote into the histogram (ACCUMULATE).
    ProcessMakerQuote = 20,
    /// Permissionless: settle one maker quote's fills into the maker's position.
    SettleMakerQuote = 21,
    /// Create a cross-margin group for an owner.
    InitMarginAccount = 22,
    /// Bind a flat position into the owner's cross-margin group.
    AddPositionToMargin = 23,
    /// Cross-margin extraction against the combined member health.
    WithdrawCross = 24,
    /// Account-level liquidation — close one member of a combined-unhealthy group.
    LiquidateCross = 25,
    /// Migrate a v4 Market account in place to the v5 layout (admin-gated).
    MigrateMarket = 26,
    /// Migrate a v1 Position account in place to the v2 layout (owner-gated).
    MigratePosition = 27,
    /// Unbind a flat member position from the owner's cross-margin group.
    RemovePositionFromMargin = 28,
    /// Maker: close a cleared quote PDA and reclaim its rent.
    CloseMakerQuote = 29,
    /// Stage A sharding: create one OrderSlab shard for a market.
    InitShard = 30,
    /// Stage A sharding: reset one drained shard for the next round.
    ResetShard = 31,
    /// Authority circuit breaker: set the market's pause bitflags (§3.2).
    SetPause = 32,
    /// Authority: update the HOT market params (fees/caps/brake — §3.2).
    UpdateMarketParams = 33,
    /// Authority: stage a risk-class param change behind the delay (§3.2).
    ProposeRiskUpdate = 34,
    /// Permissionless: apply a staged risk update once the delay elapses.
    ApplyRiskUpdate = 35,
    /// Authority: stage an authority transfer (two-step, §3.3).
    ProposeAuthorityTransfer = 36,
    /// The staged NEW authority signs to take over.
    AcceptAuthorityTransfer = 37,
    /// Authority: stage an oracle repoint (paused-only, delayed — §3.3).
    ProposeSetOracle = 38,
    /// Permissionless: apply a staged oracle repoint (quiescence-gated).
    ApplySetOracle = 39,
    /// Permissionless donation into the vault insurance pool (§4.1).
    SeedInsurance = 40,
    /// Vault authority: stage an insurance withdrawal behind the delay (§4.4).
    ProposeInsuranceWithdraw = 41,
    /// Permissionless: apply a staged insurance withdrawal (backing-gated).
    ApplyInsuranceWithdraw = 42,
    /// Self-CPI event emission (carries event data in instruction data).
    EmitEvent = 228,
}

impl TryFrom<u8> for TempoInstructionDiscriminators {
    type Error = ProgramError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::InitializeMarket),
            1 => Ok(Self::SubmitOrder),
            2 => Ok(Self::CancelOrder),
            3 => Ok(Self::ProcessChunk),
            4 => Ok(Self::FinalizeClear),
            5 => Ok(Self::SettleFill),
            6 => Ok(Self::StartAuction),
            7 => Ok(Self::InitPosition),
            8 => Ok(Self::ReadOracle),
            9 => Ok(Self::InitVault),
            10 => Ok(Self::InitCollateral),
            11 => Ok(Self::Deposit),
            12 => Ok(Self::Withdraw),
            13 => Ok(Self::UpdateFunding),
            14 => Ok(Self::Liquidate),
            15 => Ok(Self::ForceReset),
            16 => Ok(Self::InitMakerQuote),
            17 => Ok(Self::UpdateMakerQuoteMid),
            18 => Ok(Self::UpdateMakerQuoteLevels),
            19 => Ok(Self::ClearMakerQuote),
            20 => Ok(Self::ProcessMakerQuote),
            21 => Ok(Self::SettleMakerQuote),
            22 => Ok(Self::InitMarginAccount),
            23 => Ok(Self::AddPositionToMargin),
            24 => Ok(Self::WithdrawCross),
            25 => Ok(Self::LiquidateCross),
            26 => Ok(Self::MigrateMarket),
            27 => Ok(Self::MigratePosition),
            28 => Ok(Self::RemovePositionFromMargin),
            29 => Ok(Self::CloseMakerQuote),
            30 => Ok(Self::InitShard),
            31 => Ok(Self::ResetShard),
            32 => Ok(Self::SetPause),
            33 => Ok(Self::UpdateMarketParams),
            34 => Ok(Self::ProposeRiskUpdate),
            35 => Ok(Self::ApplyRiskUpdate),
            36 => Ok(Self::ProposeAuthorityTransfer),
            37 => Ok(Self::AcceptAuthorityTransfer),
            38 => Ok(Self::ProposeSetOracle),
            39 => Ok(Self::ApplySetOracle),
            40 => Ok(Self::SeedInsurance),
            41 => Ok(Self::ProposeInsuranceWithdraw),
            42 => Ok(Self::ApplyInsuranceWithdraw),
            228 => Ok(Self::EmitEvent),
            _ => Err(ProgramError::InvalidInstructionData),
        }
    }
}

/// Marker trait for instruction account structs
///
/// Implementors should use TryFrom<&'a [AccountView]> for parsing
pub trait InstructionAccounts<'a>:
    Sized + TryFrom<&'a [AccountView], Error = ProgramError>
{
}

/// Marker trait for instruction data structs
///
/// Implementors should use TryFrom<&'a [u8]> for parsing
pub trait InstructionData<'a>: Sized + TryFrom<&'a [u8], Error = ProgramError> {
    /// Expected length of instruction data
    const LEN: usize;
}

/// Full instruction combining accounts and data
///
/// Implementors get automatic TryFrom<(&'a [u8], &'a [AccountView])>
pub trait Instruction<'a>: Sized {
    type Accounts: InstructionAccounts<'a>;
    type Data: InstructionData<'a>;

    /// Parse instruction from data and accounts tuple
    #[inline(always)]
    fn parse(data: &'a [u8], accounts: &'a [AccountView]) -> Result<Self, ProgramError>
    where
        Self: From<(Self::Accounts, Self::Data)>,
    {
        let accounts = Self::Accounts::try_from(accounts)?;
        let data = Self::Data::try_from(data)?;
        Ok(Self::from((accounts, data)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discriminator_try_from_initialize_market() {
        let result = TempoInstructionDiscriminators::try_from(0u8);
        assert!(result.is_ok());
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::InitializeMarket
        ));
    }

    #[test]
    fn test_discriminator_try_from_submit_order() {
        let result = TempoInstructionDiscriminators::try_from(1u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::SubmitOrder
        ));
    }

    #[test]
    fn test_discriminator_try_from_cancel_order() {
        let result = TempoInstructionDiscriminators::try_from(2u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::CancelOrder
        ));
    }

    #[test]
    fn test_discriminator_try_from_maker_quote_set() {
        assert!(matches!(
            TempoInstructionDiscriminators::try_from(16u8).unwrap(),
            TempoInstructionDiscriminators::InitMakerQuote
        ));
        assert!(matches!(
            TempoInstructionDiscriminators::try_from(17u8).unwrap(),
            TempoInstructionDiscriminators::UpdateMakerQuoteMid
        ));
        assert!(matches!(
            TempoInstructionDiscriminators::try_from(18u8).unwrap(),
            TempoInstructionDiscriminators::UpdateMakerQuoteLevels
        ));
        assert!(matches!(
            TempoInstructionDiscriminators::try_from(19u8).unwrap(),
            TempoInstructionDiscriminators::ClearMakerQuote
        ));
    }

    #[test]
    fn test_discriminator_try_from_process_chunk() {
        let result = TempoInstructionDiscriminators::try_from(3u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::ProcessChunk
        ));
    }

    #[test]
    fn test_discriminator_try_from_finalize_clear() {
        let result = TempoInstructionDiscriminators::try_from(4u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::FinalizeClear
        ));
    }

    #[test]
    fn test_discriminator_try_from_settle_fill() {
        let result = TempoInstructionDiscriminators::try_from(5u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::SettleFill
        ));
    }

    #[test]
    fn test_discriminator_try_from_start_auction() {
        let result = TempoInstructionDiscriminators::try_from(6u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::StartAuction
        ));
    }

    #[test]
    fn test_discriminator_try_from_emit_event() {
        let result = TempoInstructionDiscriminators::try_from(228u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::EmitEvent
        ));
    }

    #[test]
    fn test_discriminator_try_from_init_position() {
        let result = TempoInstructionDiscriminators::try_from(7u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::InitPosition
        ));
    }

    #[test]
    fn test_discriminator_try_from_read_oracle() {
        let result = TempoInstructionDiscriminators::try_from(8u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::ReadOracle
        ));
    }

    #[test]
    fn test_discriminator_try_from_init_vault() {
        let result = TempoInstructionDiscriminators::try_from(9u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::InitVault
        ));
    }

    #[test]
    fn test_discriminator_try_from_init_collateral() {
        let result = TempoInstructionDiscriminators::try_from(10u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::InitCollateral
        ));
    }

    #[test]
    fn test_discriminator_try_from_deposit() {
        let result = TempoInstructionDiscriminators::try_from(11u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::Deposit
        ));
    }

    #[test]
    fn test_discriminator_try_from_withdraw() {
        let result = TempoInstructionDiscriminators::try_from(12u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::Withdraw
        ));
    }

    #[test]
    fn test_discriminator_try_from_update_funding() {
        let result = TempoInstructionDiscriminators::try_from(13u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::UpdateFunding
        ));
    }

    #[test]
    fn test_discriminator_try_from_liquidate() {
        let result = TempoInstructionDiscriminators::try_from(14u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::Liquidate
        ));
    }

    #[test]
    fn test_discriminator_try_from_force_reset() {
        let result = TempoInstructionDiscriminators::try_from(15u8);
        assert!(matches!(
            result.unwrap(),
            TempoInstructionDiscriminators::ForceReset
        ));
    }

    #[test]
    fn test_discriminator_try_from_invalid() {
        assert!(matches!(
            TempoInstructionDiscriminators::try_from(99u8),
            Err(ProgramError::InvalidInstructionData)
        ));
        assert!(matches!(
            TempoInstructionDiscriminators::try_from(255u8),
            Err(ProgramError::InvalidInstructionData)
        ));
    }
}
