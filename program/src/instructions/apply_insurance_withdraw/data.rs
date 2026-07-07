use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ApplyInsuranceWithdraw (none — everything staged).
pub struct ApplyInsuranceWithdrawData {}

impl<'a> TryFrom<&'a [u8]> for ApplyInsuranceWithdrawData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

impl<'a> InstructionData<'a> for ApplyInsuranceWithdrawData {
    const LEN: usize = 0;
}
