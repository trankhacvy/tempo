use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for CancelAllOrders (missing-features §2.7). No arguments —
/// the set to cancel is "every still-`Resting` order in the passed shard owned by
/// the signer", discovered by a slab scan (bounded by the ≤ 90-slot shard
/// capacity, and ≤ 8 matches under the per-trader-per-shard anti-spam cap).
/// Multi-shard cancel-all is a client-side loop over shards — each shard is an
/// independent account, so the transactions run in parallel.
pub struct CancelAllOrdersData;

impl<'a> TryFrom<&'a [u8]> for CancelAllOrdersData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(_data: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl<'a> InstructionData<'a> for CancelAllOrdersData {
    const LEN: usize = 0;
}
