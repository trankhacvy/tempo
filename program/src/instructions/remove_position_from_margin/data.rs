use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for RemovePositionFromMargin (no payload).
pub struct RemovePositionFromMarginData;

impl<'a> TryFrom<&'a [u8]> for RemovePositionFromMarginData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for RemovePositionFromMarginData {
    const LEN: usize = 0;
}
