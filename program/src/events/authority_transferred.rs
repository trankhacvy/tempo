use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

/// Emitted by `accept_authority_transfer` (plan.md §3.3): the market's admin
/// key rotated via the two-step propose/accept flow.
#[derive(CodamaType)]
pub struct AuthorityTransferredEvent {
    pub market: Address,
    pub old_authority: Address,
    pub new_authority: Address,
}

impl EventDiscriminator for AuthorityTransferredEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::AuthorityTransferred as u8;
}

impl EventSerialize for AuthorityTransferredEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(self.old_authority.as_ref());
        data.extend_from_slice(self.new_authority.as_ref());
        data
    }
}

impl AuthorityTransferredEvent {
    pub const DATA_LEN: usize = 32 * 3;
}
