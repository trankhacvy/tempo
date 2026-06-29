use crate::ID as TEMPO_PROGRAM_ID;
use pinocchio::{account::AccountView, error::ProgramError};

/// Verify the account is the system program, returning an error if it is not.
#[inline(always)]
pub fn verify_system_program(account: &AccountView) -> Result<(), ProgramError> {
    if account.address() != &pinocchio_system::ID {
        return Err(ProgramError::IncorrectProgramId);
    }

    Ok(())
}

/// Verify the account is the current (Tempo) program, returning an error if it is not.
#[inline(always)]
pub fn verify_current_program(account: &AccountView) -> Result<(), ProgramError> {
    if account.address() != &TEMPO_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }

    Ok(())
}

/// Verify the account is the canonical SPL Token program (HS-12).
///
/// The collateral money path only supports **classic SPL Token** mints with **no
/// transfer fee**: the deposit/withdraw paths credit/debit the ledger by the
/// instruction's face `amount` and do not reconcile `post − pre` actual balances, so
/// a Token-2022 transfer-fee mint would break the `vault ≥ Σ balances + insurance`
/// invariant. Pinning the token program here rejects the Token-2022 program id
/// outright; supporting it would require `TransferChecked` + actual-received credit.
#[inline(always)]
pub fn verify_token_program(account: &AccountView) -> Result<(), ProgramError> {
    if account.address() != &pinocchio_token::ID {
        return Err(ProgramError::IncorrectProgramId);
    }

    Ok(())
}
