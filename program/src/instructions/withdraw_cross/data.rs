use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for WithdrawCross.
///
/// # Layout
/// * `amount` (u64) — base units to withdraw
/// * `live_mask` (u8) — per-supplied-member shape bitmap (known-issues §2.4):
///   bit `i` set ⇒ member `i` is a `(position, market, oracle)` *live* triple;
///   bit `i` clear ⇒ member `i` is a bare *flat* `position` account (size 0, so it
///   contributes no unrealized PnL / maintenance and needs no market or oracle). A
///   member claimed flat that is not actually flat fails closed.
pub struct WithdrawCrossData {
    pub amount: u64,
    pub live_mask: u8,
}

impl<'a> TryFrom<&'a [u8]> for WithdrawCrossData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self {
            amount: u64::from_le_bytes(data[0..8].try_into().unwrap()),
            live_mask: data[8],
        })
    }
}

impl<'a> InstructionData<'a> for WithdrawCrossData {
    const LEN: usize = 9;
}
