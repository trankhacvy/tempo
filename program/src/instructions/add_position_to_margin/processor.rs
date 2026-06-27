use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::AddPositionToMargin,
    state::{count_trader_live_orders, MarginAccount, OrderSlabHeader, Position},
    traits::{AccountDeserialize, PdaAccount, PdaSeeds},
};

/// Processes AddPositionToMargin: binds a flat, ungrouped `Position` owned by
/// the signer into their cross-margin group's member set. Requiring the position
/// be flat keeps the combined-equity accounting unambiguous at bind time.
pub fn process_add_position_to_margin(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = AddPositionToMargin::try_from((instruction_data, accounts))?;
    let owner = *ix.accounts.owner.address();
    let position_key = *ix.accounts.position.address();

    // The position must belong to the signer, be flat, and hold no isolated
    // margin.
    let position_market = {
        let pos_data = ix.accounts.position.try_borrow()?;
        let position = Position::from_bytes(&pos_data)?;
        position.validate_self(ix.accounts.position, program_id)?;
        if position.owner != owner {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        if position.size() != 0 || position.collateral() != 0 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }
        position.market
    };

    // Reject binding while an in-flight order exists this round: such an order
    // could settle as isolated after the flip, locking no margin (known-issues §2.5).
    {
        if *ix.accounts.market.address() != position_market {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        let slab_data = ix.accounts.order_slab.try_borrow()?;
        let header = OrderSlabHeader::from_bytes(&slab_data)?;
        if header.market != position_market {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        header.validate_pda(ix.accounts.order_slab, program_id, header.bump)?;
        if count_trader_live_orders(&slab_data, header.capacity(), &owner)? != 0 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }
    }

    // Bind it into cross mode.
    {
        let mut acct = *ix.accounts.position;
        let mut pos_data = acct.try_borrow_mut()?;
        Position::from_bytes_mut(&mut pos_data)?.margin_mode = 1;
    }

    // Append to the owner's group.
    {
        let mut acct = *ix.accounts.margin_account;
        let mut data = acct.try_borrow_mut()?;
        let margin = MarginAccount::from_bytes_mut(&mut data)?;
        margin.validate_self(ix.accounts.margin_account, program_id)?;
        if margin.owner != owner {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        margin.push_member(&position_key)?;
    }

    Ok(())
}
