use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for UpdateFunding. No parameters — the feed account is bound
/// to the market and the funding interval is fixed in the processor.
pub struct UpdateFundingData;

impl<'a> TryFrom<&'a [u8]> for UpdateFundingData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for UpdateFundingData {
    const LEN: usize = 0;
}
