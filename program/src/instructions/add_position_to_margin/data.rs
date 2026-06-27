use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for AddPositionToMargin (no payload).
pub struct AddPositionToMarginData;

impl<'a> TryFrom<&'a [u8]> for AddPositionToMarginData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for AddPositionToMarginData {
    const LEN: usize = 0;
}
