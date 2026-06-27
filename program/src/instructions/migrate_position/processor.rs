use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::MigratePosition,
    state::{Market, OrderSlabHeader, Position},
    traits::{AccountDeserialize, AccountSize, Discriminator, PdaSeeds, Versioned},
    utils::resize_pda_account,
};

/// Tail appended across the VERSION 1 -> 3 upgrade (`last_social_index` + `margin_mode`).
const POSITION_V1_APPENDED: usize = 17;
/// Tail appended across the VERSION 2 -> 3 upgrade (`margin_mode`).
const POSITION_V2_APPENDED: usize = 1;
/// Account-data offsets of the stable `owner`, `market`, and `size` fields
/// (2-byte prefix + the struct offsets `owner@0`, `market@32`, `size@64`).
const OWNER_OFFSET: usize = 2;
const MARKET_OFFSET: usize = 2 + 32;
const SIZE_OFFSET: usize = 2 + 64;

/// Processes MigratePosition — an owner-gated, in-place upgrade of a `Position` to
/// the current VERSION 3, from either VERSION 1 (appending `last_social_index` +
/// `margin_mode`) or VERSION 2 (appending `margin_mode`). The new bytes are
/// zero-initialized (`last_social_index = 0`, `margin_mode = 0` = isolated). Only
/// the v1 upgrade rebuilds open interest: `migrate_market` resets OI to 0, so each
/// migrated v1 position re-adds its own size (v2 positions were already counted).
/// The exact source-version + length check makes the call idempotent.
pub fn process_migrate_position(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = MigratePosition::try_from((instruction_data, accounts))?;
    let position = ix.accounts.position;
    let market = ix.accounts.market;

    // --- detect the source version + validate owner/market binding ---
    let (old_len, old_version, size_signed) = {
        let data = position.try_borrow()?;
        if data.len() < 2 || data[0] != Position::DISCRIMINATOR {
            return Err(TempoProgramError::NotMigratable.into());
        }
        let old_len = match data[1] {
            1 => Position::LEN - POSITION_V1_APPENDED,
            2 => Position::LEN - POSITION_V2_APPENDED,
            _ => return Err(TempoProgramError::NotMigratable.into()),
        };
        if data.len() != old_len {
            return Err(TempoProgramError::NotMigratable.into());
        }
        if &data[OWNER_OFFSET..OWNER_OFFSET + 32] != ix.accounts.owner.address().as_ref() {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        if &data[MARKET_OFFSET..MARKET_OFFSET + 32] != market.address().as_ref() {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        let size =
            i64::from_le_bytes(data[SIZE_OFFSET..SIZE_OFFSET + 8].try_into().unwrap()) as i128;
        (old_len, data[1], size)
    };

    // The market must already be the current (v5) layout. The version check here
    // enforces market-then-position ordering (a position cannot migrate before its
    // market).
    let market_key = *market.address();
    {
        let mdata = market.try_borrow()?;
        Market::from_account(&mdata, market, program_id)?;
    }

    // For the v1 OI rebuild, require the slab to be fully settled (no resting or
    // accumulated order, `count == 0` — the `start_auction` quiescence gate) so no
    // in-flight settle can race the OI counters while we re-add this position's
    // size (known-issues §2.6).
    if old_version == 1 {
        let slab_data = ix.accounts.order_slab.try_borrow()?;
        let slab = OrderSlabHeader::from_bytes(&slab_data)?;
        if slab.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        slab.validate_pda(ix.accounts.order_slab, program_id, slab.bump)?;
        if slab.count() != 0 {
            return Err(TempoProgramError::AuctionNotComplete.into());
        }
    }

    // --- grow to the v3 size (rent from the owner) + init tail + bump version ---
    resize_pda_account(ix.accounts.owner, position, Position::LEN)?;
    {
        let mut acct = *position;
        let mut data = acct.try_borrow_mut()?;
        for b in data[old_len..].iter_mut() {
            *b = 0;
        }
        data[1] = Position::VERSION;
    }

    // --- v1 only: rebuild the market OI that migrate_market reset to 0 ---
    if old_version == 1 {
        let mut acct = *market;
        let mut mdata = acct.try_borrow_mut()?;
        Market::from_bytes_mut(&mut mdata)?.apply_oi_delta(0, size_signed);
    }

    Ok(())
}
