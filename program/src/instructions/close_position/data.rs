use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ClosePosition (no payload — the position to close is
/// the passed account; closing is all-or-nothing on a flat, drained position).
pub struct ClosePositionData;

impl<'a> TryFrom<&'a [u8]> for ClosePositionData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for ClosePositionData {
    const LEN: usize = 0;
}
