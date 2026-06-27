use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ProcessMakerQuote (none — the quote account carries
/// everything; one quote folded per call, chunk by calling repeatedly).
pub struct ProcessMakerQuoteData;

impl<'a> TryFrom<&'a [u8]> for ProcessMakerQuoteData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for ProcessMakerQuoteData {
    const LEN: usize = 0;
}
