use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::MigrateMarket,
    state::Market,
    traits::{AccountSize, Discriminator, Versioned},
    utils::resize_pda_account,
};

/// Bytes appended to `Market` after the stable v4 prefix, up to the *current*
/// layout: the v5 risk block (`oi_long`/`oi_short`, the two social-loss indices,
/// the effective price + its slot, the last-good-oracle slot, the brake +
/// soft-stale config = 98), the v7 oracle-anchored `window_floor_price` (8,
/// known-issues §2.7), and the v8 pre-trade risk config (`initial_margin_bps` 2 +
/// `max_position_notional` 16 = 18, missing-features §1.1/§1.2). All are pure
/// appends, so the old account is exactly this much shorter than the current layout.
const MARKET_APPENDED_LEN: usize = (16 * 2 + 16 * 2 + 8 + 8 + 8 + 2 + 8) + 8 + (2 + 16); // = 124
/// The prior layout version this migration upgrades from.
const MARKET_OLD_VERSION: u8 = 4;
/// Account-data offset of the stable `authority` field: 2-byte prefix + the
/// 64-byte clearing prefix (7×u64 + 2×u32) that precedes it.
const AUTHORITY_OFFSET: usize = 2 + (8 * 7 + 4 * 2);

/// Processes MigrateMarket — an authority-gated, in-place layout upgrade of a
/// VERSION-4 `Market` account to VERSION 5 (the risk block). Because every
/// v5 field was *appended*, the existing bytes keep their meaning; migration only
/// grows the account, zero-initializes the new tail, sets the two admin-chosen risk
/// config fields, and bumps the version byte so the v5 zero-copy reader accepts it.
///
/// SUPERSEDED (known-issues §3): once `sync_fee_multiplier` was removed from the
/// Market *prefix*, this append-only path can no longer reconstruct the current
/// layout from a genuine on-chain pre-removal account (the dropped byte is not in
/// the appended tail). The version bump now makes such an account fail the
/// zero-copy check loudly; the only supported upgrade is a fresh re-provision. This
/// instruction is retained only for the synthetic same-layout migration test and
/// should be retired.
///
/// `oi_long`/`oi_short` start at 0 here — they cannot be reconstructed from the
/// market alone — and are rebuilt exactly as each member position is migrated via
/// `migrate_position` (which adds its own size back). The saturating OI math means
/// the interim under-count can never panic; the only effect is that ADL uses a
/// partial denominator until the positions finish migrating, so operators should
/// prefer to migrate during a quiescent (flat) period.
pub fn process_migrate_market(
    _program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = MigrateMarket::try_from((instruction_data, accounts))?;
    let market = ix.accounts.market;

    let old_len = Market::LEN - MARKET_APPENDED_LEN;

    // --- validate: a v4 Market we own, and the caller is its stored authority ---
    {
        let data = market.try_borrow()?;
        if data.len() != old_len
            || data[0] != Market::DISCRIMINATOR
            || data[1] != MARKET_OLD_VERSION
        {
            return Err(TempoProgramError::NotMigratable.into());
        }
        // `authority` lives in the stable prefix, present in the v4 layout.
        if &data[AUTHORITY_OFFSET..AUTHORITY_OFFSET + 32]
            != ix.accounts.authority.address().as_ref()
        {
            return Err(TempoProgramError::InvalidAuthority.into());
        }
    }

    // --- grow to the v5 size (tops up rent-exemption from the payer) ---
    resize_pda_account(ix.accounts.payer, market, Market::LEN)?;

    // --- initialize the new tail, set the version + admin config ---
    {
        let mut acct = *market;
        let mut data = acct.try_borrow_mut()?;
        // Zero the whole appended region (oi / social indices / effective price /
        // config all default to 0); defensive even though resize zero-fills.
        for b in data[old_len..].iter_mut() {
            *b = 0;
        }
        // Bump the version so the current reader accepts the account.
        data[1] = Market::VERSION;
        // Trailing layout (current): ... max_price_move_bps(2), soft_stale_slots(8),
        // window_floor_price(8), initial_margin_bps(2), max_position_notional(16).
        // The v8 tail (initial_margin_bps + max_position_notional) is left zero by the
        // wipe above — `Market::initial_margin_bps` falls back to maintenance when
        // zero, and a zero notional cap is disabled (missing-features §1.2). The older
        // three sit 18 bytes earlier than the account end now: max_price_move + soft_stale
        // are admin config; the window floor (known-issues §2.7) can't be reconstructed
        // here (migrate has no oracle), so seed it to the genesis default (tick_size, in
        // the preserved prefix at bytes 18..26) — the next `start_auction` re-snaps it.
        let n = Market::LEN;
        data[n - 36..n - 34].copy_from_slice(&ix.data.max_price_move_bps_per_slot.to_le_bytes());
        data[n - 34..n - 26].copy_from_slice(&ix.data.soft_stale_slots.to_le_bytes());
        let tick_size: [u8; 8] = data[18..26].try_into().unwrap();
        data[n - 26..n - 18].copy_from_slice(&tick_size);
    }

    Ok(())
}
