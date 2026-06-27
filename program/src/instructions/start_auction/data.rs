use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for StartAuction. No parameters — the next round's
/// parameters are inherited from the `Market`.
pub struct StartAuctionData;

impl<'a> TryFrom<&'a [u8]> for StartAuctionData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for StartAuctionData {
    const LEN: usize = 0;
}
