use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for SettleMakerQuote (none — the quote account identifies the
/// maker; one quote settled per call).
pub struct SettleMakerQuoteData;

impl<'a> TryFrom<&'a [u8]> for SettleMakerQuoteData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for SettleMakerQuoteData {
    const LEN: usize = 0;
}
