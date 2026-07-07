use crate::define_instruction;

use super::accept_authority_transfer::{
    AcceptAuthorityTransferAccounts, AcceptAuthorityTransferData,
};
use super::add_position_to_margin::{AddPositionToMarginAccounts, AddPositionToMarginData};
use super::apply_insurance_withdraw::{ApplyInsuranceWithdrawAccounts, ApplyInsuranceWithdrawData};
use super::apply_risk_update::{ApplyRiskUpdateAccounts, ApplyRiskUpdateData};
use super::apply_set_oracle::{ApplySetOracleAccounts, ApplySetOracleData};
use super::cancel_order::{CancelOrderAccounts, CancelOrderData};
use super::clear_maker_quote::{ClearMakerQuoteAccounts, ClearMakerQuoteData};
use super::close_maker_quote::{CloseMakerQuoteAccounts, CloseMakerQuoteData};
use super::deposit::{DepositAccounts, DepositData};
use super::finalize_clear::{FinalizeClearAccounts, FinalizeClearData};
use super::force_reset::{ForceResetAccounts, ForceResetData};
use super::init_collateral::{InitCollateralAccounts, InitCollateralData};
use super::init_maker_quote::{InitMakerQuoteAccounts, InitMakerQuoteData};
use super::init_margin_account::{InitMarginAccountAccounts, InitMarginAccountData};
use super::init_position::{InitPositionAccounts, InitPositionData};
use super::init_shard::{InitShardAccounts, InitShardData};
use super::init_vault::{InitVaultAccounts, InitVaultData};
use super::initialize_market::{InitializeMarketAccounts, InitializeMarketData};
use super::liquidate::{LiquidateAccounts, LiquidateData};
use super::liquidate_cross::{LiquidateCrossAccounts, LiquidateCrossData};
use super::migrate_market::{MigrateMarketAccounts, MigrateMarketData};
use super::migrate_position::{MigratePositionAccounts, MigratePositionData};
use super::process_chunk::{ProcessChunkAccounts, ProcessChunkData};
use super::process_maker_quote::{ProcessMakerQuoteAccounts, ProcessMakerQuoteData};
use super::propose_authority_transfer::{
    ProposeAuthorityTransferAccounts, ProposeAuthorityTransferData,
};
use super::propose_insurance_withdraw::{
    ProposeInsuranceWithdrawAccounts, ProposeInsuranceWithdrawData,
};
use super::propose_risk_update::{ProposeRiskUpdateAccounts, ProposeRiskUpdateData};
use super::propose_set_oracle::{ProposeSetOracleAccounts, ProposeSetOracleData};
use super::read_oracle::{ReadOracleAccounts, ReadOracleData};
use super::remove_position_from_margin::{
    RemovePositionFromMarginAccounts, RemovePositionFromMarginData,
};
use super::reset_shard::{ResetShardAccounts, ResetShardData};
use super::seed_insurance::{SeedInsuranceAccounts, SeedInsuranceData};
use super::set_pause::{SetPauseAccounts, SetPauseData};
use super::settle_fill::{SettleFillAccounts, SettleFillData};
use super::settle_maker_quote::{SettleMakerQuoteAccounts, SettleMakerQuoteData};
use super::start_auction::{StartAuctionAccounts, StartAuctionData};
use super::submit_order::{SubmitOrderAccounts, SubmitOrderData};
use super::update_funding::{UpdateFundingAccounts, UpdateFundingData};
use super::update_maker_quote_levels::{
    UpdateMakerQuoteLevelsAccounts, UpdateMakerQuoteLevelsData,
};
use super::update_maker_quote_mid::{UpdateMakerQuoteMidAccounts, UpdateMakerQuoteMidData};
use super::update_market_params::{UpdateMarketParamsAccounts, UpdateMarketParamsData};
use super::withdraw::{WithdrawAccounts, WithdrawData};
use super::withdraw_cross::{WithdrawCrossAccounts, WithdrawCrossData};

define_instruction!(
    InitializeMarket,
    InitializeMarketAccounts,
    InitializeMarketData
);
define_instruction!(SubmitOrder, SubmitOrderAccounts, SubmitOrderData);
define_instruction!(CancelOrder, CancelOrderAccounts, CancelOrderData);
define_instruction!(ProcessChunk, ProcessChunkAccounts, ProcessChunkData);
define_instruction!(
    ProcessMakerQuote,
    ProcessMakerQuoteAccounts,
    ProcessMakerQuoteData
);
define_instruction!(FinalizeClear, FinalizeClearAccounts, FinalizeClearData);
define_instruction!(SettleFill, SettleFillAccounts, SettleFillData);
define_instruction!(
    SettleMakerQuote,
    SettleMakerQuoteAccounts,
    SettleMakerQuoteData
);
define_instruction!(StartAuction, StartAuctionAccounts, StartAuctionData);
define_instruction!(InitPosition, InitPositionAccounts, InitPositionData);
define_instruction!(
    InitMarginAccount,
    InitMarginAccountAccounts,
    InitMarginAccountData
);
define_instruction!(
    AddPositionToMargin,
    AddPositionToMarginAccounts,
    AddPositionToMarginData
);
define_instruction!(
    RemovePositionFromMargin,
    RemovePositionFromMarginAccounts,
    RemovePositionFromMarginData
);
define_instruction!(ReadOracle, ReadOracleAccounts, ReadOracleData);
define_instruction!(InitVault, InitVaultAccounts, InitVaultData);
define_instruction!(InitCollateral, InitCollateralAccounts, InitCollateralData);
define_instruction!(Deposit, DepositAccounts, DepositData);
define_instruction!(SetPause, SetPauseAccounts, SetPauseData);
define_instruction!(SeedInsurance, SeedInsuranceAccounts, SeedInsuranceData);
define_instruction!(
    UpdateMarketParams,
    UpdateMarketParamsAccounts,
    UpdateMarketParamsData
);
define_instruction!(
    ProposeRiskUpdate,
    ProposeRiskUpdateAccounts,
    ProposeRiskUpdateData
);
define_instruction!(
    ApplyRiskUpdate,
    ApplyRiskUpdateAccounts,
    ApplyRiskUpdateData
);
define_instruction!(
    ProposeAuthorityTransfer,
    ProposeAuthorityTransferAccounts,
    ProposeAuthorityTransferData
);
define_instruction!(
    AcceptAuthorityTransfer,
    AcceptAuthorityTransferAccounts,
    AcceptAuthorityTransferData
);
define_instruction!(
    ProposeSetOracle,
    ProposeSetOracleAccounts,
    ProposeSetOracleData
);
define_instruction!(ApplySetOracle, ApplySetOracleAccounts, ApplySetOracleData);
define_instruction!(
    ProposeInsuranceWithdraw,
    ProposeInsuranceWithdrawAccounts,
    ProposeInsuranceWithdrawData
);
define_instruction!(
    ApplyInsuranceWithdraw,
    ApplyInsuranceWithdrawAccounts,
    ApplyInsuranceWithdrawData
);
define_instruction!(Withdraw, WithdrawAccounts, WithdrawData);
define_instruction!(LiquidateCross, LiquidateCrossAccounts, LiquidateCrossData);
define_instruction!(WithdrawCross, WithdrawCrossAccounts, WithdrawCrossData);
define_instruction!(UpdateFunding, UpdateFundingAccounts, UpdateFundingData);
define_instruction!(Liquidate, LiquidateAccounts, LiquidateData);
define_instruction!(ForceReset, ForceResetAccounts, ForceResetData);
define_instruction!(InitMakerQuote, InitMakerQuoteAccounts, InitMakerQuoteData);
define_instruction!(
    UpdateMakerQuoteMid,
    UpdateMakerQuoteMidAccounts,
    UpdateMakerQuoteMidData
);
define_instruction!(
    UpdateMakerQuoteLevels,
    UpdateMakerQuoteLevelsAccounts,
    UpdateMakerQuoteLevelsData
);
define_instruction!(
    ClearMakerQuote,
    ClearMakerQuoteAccounts,
    ClearMakerQuoteData
);
define_instruction!(
    CloseMakerQuote,
    CloseMakerQuoteAccounts,
    CloseMakerQuoteData
);
define_instruction!(MigrateMarket, MigrateMarketAccounts, MigrateMarketData);
define_instruction!(
    MigratePosition,
    MigratePositionAccounts,
    MigratePositionData
);
define_instruction!(InitShard, InitShardAccounts, InitShardData);
define_instruction!(ResetShard, ResetShardAccounts, ResetShardData);
