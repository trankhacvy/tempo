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
