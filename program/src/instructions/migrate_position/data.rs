use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// MigratePosition carries no data — the upgrade is fully determined by the
/// account's prior layout and its market.
pub struct MigratePositionData;

impl<'a> TryFrom<&'a [u8]> for MigratePositionData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for MigratePositionData {
    const LEN: usize = 0;
}
