use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ForceReset. No parameters.
pub struct ForceResetData;

impl<'a> TryFrom<&'a [u8]> for ForceResetData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for ForceResetData {
    const LEN: usize = 0;
}
