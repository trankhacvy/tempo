use alloc::vec;
use alloc::vec::Vec;
use pinocchio::{cpi::Seed, error::ProgramError, Address};

use crate::assert_no_padding;
use crate::errors::TempoProgramError;
use crate::traits::{
    AccountDeserialize, AccountSerialize, AccountSize, Discriminator, PdaAccount, PdaSeeds,
    TempoAccountDiscriminators, Versioned,
};

/// Max member positions a cross-margin account may hold. Bounded by the
/// transaction account budget — each member contributes a position + a market
/// account to extraction/liquidation instructions. Open a second group for more.
pub const MAX_CROSS_POSITIONS: usize = 8;

// `withdraw_cross`/`liquidate_cross` encode each member's live/flat shape in a u8
// `live_mask` (one bit per member; known-issues §2.4), so a group can hold at most 8
// members. If this constant is ever raised, widen the mask in lockstep — this guard
// fails the build rather than silently leaving members 8+ undecodable.
const _: () = assert!(
    MAX_CROSS_POSITIONS <= 8,
    "MAX_CROSS_POSITIONS must fit in the u8 live_mask used by the cross-margin instructions"
);

/// A cross-margin grouping for one owner. Holds the set of member `Position`
/// keys so any extraction or liquidation can require *every* member to be present
/// (the completeness rule — omitting a losing position must fail closed). The
/// shared collateral and netted realized PnL stay in the owner's global
/// `UserCollateral`; this account is purely the member set.
///
/// # PDA Seeds
/// `[b"margin", owner.as_ref()]`
///
/// # Zero-copy layout (`#[repr(C)]`, **alignment 1**)
/// Address (32) + u8 (1) + u8 (1) + [u8; 32·N] (256) = 290.
// NOTE: not a CodamaAccount — the fixed `[u8; 256]` member array does not map to
// a Codama struct-field node, so this account is excluded from the generated IDL
// (clients read its fixed layout directly, as the integration harness does).
#[derive(Clone, Debug, PartialEq)]
#[repr(C)]
pub struct MarginAccount {
    pub owner: Address,
    pub position_count: u8,
    pub bump: u8,
    pub members: [u8; 32 * MAX_CROSS_POSITIONS],
}

assert_no_padding!(MarginAccount, 32 + 1 + 1 + 32 * MAX_CROSS_POSITIONS);

impl Discriminator for MarginAccount {
    const DISCRIMINATOR: u8 = TempoAccountDiscriminators::MarginAccountDiscriminator as u8;
}

impl Versioned for MarginAccount {
    const VERSION: u8 = 1;
}

impl AccountSize for MarginAccount {
    const DATA_LEN: usize = 32 + 1 + 1 + 32 * MAX_CROSS_POSITIONS;
}

impl AccountDeserialize for MarginAccount {}

impl AccountSerialize for MarginAccount {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.owner.as_ref());
        data.push(self.position_count);
        data.push(self.bump);
        data.extend_from_slice(&self.members);
        data
    }
}

impl PdaSeeds for MarginAccount {
    const PREFIX: &'static [u8] = b"margin";

    #[inline(always)]
    fn seeds(&self) -> Vec<&[u8]> {
        vec![Self::PREFIX, self.owner.as_ref()]
    }

    #[inline(always)]
    fn seeds_with_bump<'a>(&'a self, bump: &'a [u8; 1]) -> Vec<Seed<'a>> {
        vec![
            Seed::from(Self::PREFIX),
            Seed::from(self.owner.as_ref()),
            Seed::from(bump.as_slice()),
        ]
    }
}

impl PdaAccount for MarginAccount {
    #[inline(always)]
    fn bump(&self) -> u8 {
        self.bump
    }
}

impl MarginAccount {
    #[inline(always)]
    pub fn new(bump: u8, owner: Address) -> Self {
        Self {
            owner,
            position_count: 0,
            bump,
            members: [0u8; 32 * MAX_CROSS_POSITIONS],
        }
    }

    /// The member position key at slot `i`, or `None` past the active count.
    #[inline(always)]
    pub fn member(&self, i: usize) -> Option<Address> {
        if i >= self.position_count as usize {
            return None;
        }
        let start = i * 32;
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&self.members[start..start + 32]);
        Some(Address::new_from_array(buf))
    }

    /// True iff `key` is already a member.
    #[inline(always)]
    pub fn contains(&self, key: &Address) -> bool {
        (0..self.position_count as usize).any(|i| self.member(i).as_ref() == Some(key))
    }

    /// Append a member position; rejects a duplicate or a full group.
    pub fn push_member(&mut self, key: &Address) -> Result<(), ProgramError> {
        if self.contains(key) {
            return Err(TempoProgramError::MarginMemberDuplicate.into());
        }
        let idx = self.position_count as usize;
        if idx >= MAX_CROSS_POSITIONS {
            return Err(TempoProgramError::MarginGroupFull.into());
        }
        let start = idx * 32;
        self.members[start..start + 32].copy_from_slice(key.as_ref());
        self.position_count += 1;
        Ok(())
    }

    /// Remove a member position, compacting the array so the freed slot is reusable
    /// (so a group that churns through positions is never permanently full,
    /// known-issues §2.4). Rejects a key that is not a member.
    pub fn remove_member(&mut self, key: &Address) -> Result<(), ProgramError> {
        let n = self.position_count as usize;
        let idx = (0..n)
            .find(|&i| self.member(i).as_ref() == Some(key))
            .ok_or(TempoProgramError::MarginMemberNotFound)?;
        // Shift each later member down one slot.
        for j in idx..n - 1 {
            let src = (j + 1) * 32;
            let dst = j * 32;
            let mut buf = [0u8; 32];
            buf.copy_from_slice(&self.members[src..src + 32]);
            self.members[dst..dst + 32].copy_from_slice(&buf);
        }
        // Clear the vacated last slot.
        let last = (n - 1) * 32;
        self.members[last..last + 32].copy_from_slice(&[0u8; 32]);
        self.position_count -= 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::Discriminator;

    #[test]
    fn test_roundtrip_and_members() {
        let owner = Address::new_from_array([7u8; 32]);
        let mut m = MarginAccount::new(254, owner);
        let p0 = Address::new_from_array([1u8; 32]);
        let p1 = Address::new_from_array([2u8; 32]);
        m.push_member(&p0).unwrap();
        m.push_member(&p1).unwrap();
        assert_eq!(m.position_count, 2);
        assert_eq!(m.member(0), Some(p0));
        assert_eq!(m.member(1), Some(p1));
        assert_eq!(m.member(2), None);
        assert!(m.contains(&p1));
        // duplicate rejected.
        assert!(m.push_member(&p0).is_err());

        let bytes = m.to_bytes();
        assert_eq!(bytes.len(), MarginAccount::LEN);
        assert_eq!(bytes[0], MarginAccount::DISCRIMINATOR);
        let de = MarginAccount::from_bytes(&bytes).unwrap();
        assert_eq!(de.owner, owner);
        assert_eq!(de.position_count, 2);
        assert_eq!(de.member(0), Some(p0));
    }

    #[test]
    fn test_group_full() {
        let mut m = MarginAccount::new(1, Address::new_from_array([0u8; 32]));
        for i in 0..MAX_CROSS_POSITIONS {
            m.push_member(&Address::new_from_array([(i as u8) + 1; 32]))
                .unwrap();
        }
        assert!(m
            .push_member(&Address::new_from_array([200u8; 32]))
            .is_err());
    }

    #[test]
    fn test_remove_member_compacts_and_frees_slot() {
        let mut m = MarginAccount::new(1, Address::new_from_array([0u8; 32]));
        let keys: Vec<Address> = (0..MAX_CROSS_POSITIONS)
            .map(|i| Address::new_from_array([(i as u8) + 1; 32]))
            .collect();
        for k in &keys {
            m.push_member(k).unwrap();
        }
        // Full group: a fresh add is rejected.
        let extra = Address::new_from_array([200u8; 32]);
        assert!(m.push_member(&extra).is_err());

        // Remove a middle member; the tail compacts down.
        m.remove_member(&keys[3]).unwrap();
        assert_eq!(m.position_count as usize, MAX_CROSS_POSITIONS - 1);
        assert!(!m.contains(&keys[3]));
        assert_eq!(m.member(3), Some(keys[4]), "later members shifted down");
        assert_eq!(m.member(MAX_CROSS_POSITIONS - 1), None, "last slot freed");

        // The freed slot is reusable.
        m.push_member(&extra).unwrap();
        assert!(m.contains(&extra));

        // Removing a non-member fails.
        assert_eq!(
            m.remove_member(&Address::new_from_array([250u8; 32])),
            Err(TempoProgramError::MarginMemberNotFound.into())
        );
    }
}
