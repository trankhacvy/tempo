use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ReadOracle. No parameters — the feed account is bound
/// to the market and the feed id / staleness window are fixed in the processor.
pub struct ReadOracleData;

impl<'a> TryFrom<&'a [u8]> for ReadOracleData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for ReadOracleData {
    const LEN: usize = 0;
}
