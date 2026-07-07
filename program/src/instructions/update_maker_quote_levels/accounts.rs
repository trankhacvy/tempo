use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the UpdateMakerQuoteLevels instruction (full ladder rewrite).
///
/// # Account Layout
/// 0. `[signer]` writer - the maker or its delegate
/// 1. `[]` market - supplies `num_ticks` for the bound checks
/// 2. `[writable]` maker_quote
/// 3. `[writable]` user_collateral (OPTIONAL) - the MAKER's mint-scoped ledger;
///    the ladder's worst-case reservation is delta-locked here (missing-features
///    §7.1). REQUIRED on a money-path market (`maintenance_margin_bps > 0`) —
///    the processor rejects a reservation change without it; a clearing-only
///    market omits it (no vault/ledger exists there at all).
pub struct UpdateMakerQuoteLevelsAccounts<'a> {
    pub writer: &'a AccountView,
    pub market: &'a AccountView,
    pub maker_quote: &'a AccountView,
    pub user_collateral: Option<&'a AccountView>,
}

impl<'a> TryFrom<&'a [AccountView]> for UpdateMakerQuoteLevelsAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [writer, market, maker_quote, rest @ ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(writer, false)?;
        verify_writable(maker_quote, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(maker_quote)?;

        // Optional maker ledger (present on money-path markets only).
        let user_collateral = match rest {
            [] => None,
            // Codama emits the program id as the "omitted optional" sentinel.
            [uc] if uc.address() == &crate::ID => None,
            [uc] => {
                verify_writable(uc, true)?;
                verify_current_program_account(uc)?;
                Some(uc)
            }
            _ => return Err(ProgramError::NotEnoughAccountKeys),
        };

        Ok(Self {
            writer,
            market,
            maker_quote,
            user_collateral,
        })
    }
}

impl<'a> InstructionAccounts<'a> for UpdateMakerQuoteLevelsAccounts<'a> {}
