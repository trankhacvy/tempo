use pinocchio::{account::AccountView, entrypoint, error::ProgramError, Address, ProgramResult};

use crate::{
    instructions::{
        process_add_position_to_margin, process_cancel_order, process_clear_maker_quote,
        process_close_maker_quote, process_deposit, process_emit_event, process_finalize_clear,
        process_force_reset, process_init_collateral, process_init_maker_quote,
        process_init_margin_account, process_init_position, process_init_vault,
        process_initialize_market, process_liquidate, process_liquidate_cross,
        process_migrate_market, process_migrate_position, process_process_chunk,
        process_process_maker_quote, process_read_oracle, process_remove_position_from_margin,
        process_settle_fill, process_settle_maker_quote, process_start_auction,
        process_submit_order, process_update_funding, process_update_maker_quote_levels,
        process_update_maker_quote_mid, process_withdraw, process_withdraw_cross,
    },
    traits::TempoInstructionDiscriminators,
};

entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let (discriminator, instruction_data) = instruction_data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;

    let ix_discriminator = TempoInstructionDiscriminators::try_from(*discriminator)?;

    match ix_discriminator {
        TempoInstructionDiscriminators::InitializeMarket => {
            process_initialize_market(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::SubmitOrder => {
            process_submit_order(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::CancelOrder => {
            process_cancel_order(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::ProcessChunk => {
            process_process_chunk(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::FinalizeClear => {
            process_finalize_clear(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::SettleFill => {
            process_settle_fill(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::StartAuction => {
            process_start_auction(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::InitPosition => {
            process_init_position(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::ReadOracle => {
            process_read_oracle(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::InitVault => {
            process_init_vault(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::InitCollateral => {
            process_init_collateral(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::Deposit => {
            process_deposit(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::Withdraw => {
            process_withdraw(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::UpdateFunding => {
            process_update_funding(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::Liquidate => {
            process_liquidate(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::ForceReset => {
            process_force_reset(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::InitMakerQuote => {
            process_init_maker_quote(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::UpdateMakerQuoteMid => {
            process_update_maker_quote_mid(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::UpdateMakerQuoteLevels => {
            process_update_maker_quote_levels(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::ClearMakerQuote => {
            process_clear_maker_quote(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::ProcessMakerQuote => {
            process_process_maker_quote(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::SettleMakerQuote => {
            process_settle_maker_quote(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::InitMarginAccount => {
            process_init_margin_account(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::AddPositionToMargin => {
            process_add_position_to_margin(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::WithdrawCross => {
            process_withdraw_cross(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::LiquidateCross => {
            process_liquidate_cross(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::MigrateMarket => {
            process_migrate_market(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::MigratePosition => {
            process_migrate_position(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::RemovePositionFromMargin => {
            process_remove_position_from_margin(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::CloseMakerQuote => {
            process_close_maker_quote(program_id, accounts, instruction_data)
        }
        TempoInstructionDiscriminators::EmitEvent => process_emit_event(program_id, accounts),
    }
}
