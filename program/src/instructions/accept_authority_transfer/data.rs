use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for AcceptAuthorityTransfer (none).
pub struct AcceptAuthorityTransferData {}

impl<'a> TryFrom<&'a [u8]> for AcceptAuthorityTransferData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

impl<'a> InstructionData<'a> for AcceptAuthorityTransferData {
    const LEN: usize = 0;
}
