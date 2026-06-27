use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for LiquidateCross.
///
/// # Layout
/// * `live_mask` (u8) — per-supplied-member shape bitmap (known-issues §2.4):
///   bit `i` set ⇒ member `i` is a `(position, market, oracle)` *live* triple;
///   bit `i` clear ⇒ member `i` is a bare *flat* `position` account (size 0, so it
///   contributes no unrealized PnL / maintenance and needs no market or oracle). The
///   close target is the first *non-flat* supplied member; a member claimed flat
///   that is not actually flat fails closed.
pub struct LiquidateCrossData {
    pub live_mask: u8,
}

impl<'a> TryFrom<&'a [u8]> for LiquidateCrossData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self { live_mask: data[0] })
    }
}

impl<'a> InstructionData<'a> for LiquidateCrossData {
    const LEN: usize = 1;
}
