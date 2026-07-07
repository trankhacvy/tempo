use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ApplySetOracle (none).
pub struct ApplySetOracleData {}

impl<'a> TryFrom<&'a [u8]> for ApplySetOracleData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

impl<'a> InstructionData<'a> for ApplySetOracleData {
    const LEN: usize = 0;
}
