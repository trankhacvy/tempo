use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for CloseMarket (no payload — everything to close is passed
/// as accounts, and the quiescence gates read the market itself).
pub struct CloseMarketData;

impl<'a> TryFrom<&'a [u8]> for CloseMarketData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for CloseMarketData {
    const LEN: usize = 0;
}
