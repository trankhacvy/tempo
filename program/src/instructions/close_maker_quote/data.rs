use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for CloseMakerQuote (no payload).
pub struct CloseMakerQuoteData;

impl<'a> TryFrom<&'a [u8]> for CloseMakerQuoteData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for CloseMakerQuoteData {
    const LEN: usize = 0;
}
