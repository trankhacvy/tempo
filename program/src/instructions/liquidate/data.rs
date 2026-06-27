use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for Liquidate. No parameters — the mark is oracle-priced and
/// the risk params come from the vault.
pub struct LiquidateData;

impl<'a> TryFrom<&'a [u8]> for LiquidateData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for LiquidateData {
    const LEN: usize = 0;
}
