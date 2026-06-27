use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::RemovePositionFromMargin,
    state::{MarginAccount, Position},
    traits::{AccountDeserialize, PdaAccount},
};

/// Processes RemovePositionFromMargin: unbinds a flat, owner-matched member
/// position from the cross-margin group and returns it to isolated mode, freeing
/// its slot so the group is never permanently full (known-issues §2.4). Requiring
/// the position be flat with no locked collateral keeps the combined-equity
/// accounting unambiguous at unbind time (a flat position has no unsettled funding
/// or socialized loss to carry).
pub fn process_remove_position_from_margin(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = RemovePositionFromMargin::try_from((instruction_data, accounts))?;
    let owner = *ix.accounts.owner.address();
    let position_key = *ix.accounts.position.address();

    // The position must belong to the signer, be flat, hold no isolated margin,
    // and be in cross mode; return it to isolated.
    {
        let mut acct = *ix.accounts.position;
        let mut pos_data = acct.try_borrow_mut()?;
        let position = Position::from_bytes_mut(&mut pos_data)?;
        position.validate_self(ix.accounts.position, program_id)?;
        if position.owner != owner {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        if position.size() != 0 || position.collateral() != 0 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }
        position.margin_mode = 0;
    }

    // Remove from the owner's group.
    {
        let mut acct = *ix.accounts.margin_account;
        let mut data = acct.try_borrow_mut()?;
        let margin = MarginAccount::from_bytes_mut(&mut data)?;
        margin.validate_self(ix.accounts.margin_account, program_id)?;
        if margin.owner != owner {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        margin.remove_member(&position_key)?;
    }

    Ok(())
}
