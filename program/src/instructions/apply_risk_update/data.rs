use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ApplyRiskUpdate (none).
pub struct ApplyRiskUpdateData {}

impl<'a> TryFrom<&'a [u8]> for ApplyRiskUpdateData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

impl<'a> InstructionData<'a> for ApplyRiskUpdateData {
    const LEN: usize = 0;
}
