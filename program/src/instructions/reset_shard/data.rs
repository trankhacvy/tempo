use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ResetShard (Stage A sharding). No arguments — the shard is
/// identified by the passed `order_slab` account (its PDA is validated).
pub struct ResetShardData;

impl<'a> TryFrom<&'a [u8]> for ResetShardData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for ResetShardData {
    const LEN: usize = 0;
}
