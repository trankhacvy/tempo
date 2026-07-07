//! Quote-time maker margin (missing-features §7.1, plan.md §2.4).
//!
//! The invariant under test: an UNBACKED ladder can never fold into the
//! histogram and steer the uniform clearing price. Every sized ladder carries a
//! STANDING worst-case reservation in the maker's ledger —
//! `initial_margin(Σ level sizes, window top)` — delta-locked at
//! `update_maker_quote_levels`, untouched by mid moves (mid-independent by
//! construction), and released in full by `clear_maker_quote`.

use tempo_integration_tests::*;

/// Money market: tick 1, 32 ticks (genesis window top = 32), maint 5% /
/// initial 10%. One funded maker; ladder reservations price at top·10%.
fn setup() -> (
    TestContext,
    MarketPdas,
    solana_sdk::pubkey::Pubkey,     // vault_ta
    solana_sdk::pubkey::Pubkey,     // mint
    solana_sdk::signature::Keypair, // maker
) {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    ctx.market_initial_margin_bps = Some(1000);
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let maker = ctx.new_funded_signer();
    ctx.init_collateral(&maker);
    let maker_ta = ctx.create_token_account(&mint, &maker.pubkey());
    ctx.mint_to(&mint, &maker_ta, 1_000);
    ctx.deposit(&maker, &vault_ta, &maker_ta, 1_000);
    ctx.init_position(&pdas, &maker);

    (ctx, pdas, vault_ta, mint, maker)
}

/// An unbacked ladder is rejected AT POST TIME (`InsufficientCollateral`) —
/// before it can ever fold and move the clearing price for everyone.
#[test]
fn unbacked_ladder_rejected_at_levels_write() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    ctx.market_initial_margin_bps = Some(1000);
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // Maker with a ledger but ZERO balance.
    let maker = ctx.new_funded_signer();
    ctx.init_collateral(&maker);
    ctx.init_maker_quote(&pdas, &maker, None, 0);

    // 100 lots × window top 32 × 10% = 320 needed; balance 0 → rejected.
    assert!(
        ctx.try_update_maker_quote_levels(&pdas, &maker, 1, 16, &[(0, 100)], &[])
            .is_err(),
        "an unbacked ladder must be rejected at the levels write"
    );
    // Nothing was locked and the ladder stayed empty.
    assert_eq!(ctx.user_collateral(&maker.pubkey()).locked, 0);
    let _ = vault_ta;
}

/// The reservation is exact and delta-locked: grow locks the delta, shrink
/// releases it, and an empty ladder releases everything.
#[test]
fn ladder_reservation_delta_locks_and_releases_exactly() {
    let (mut ctx, pdas, _vault_ta, _mint, maker) = setup();
    ctx.init_maker_quote(&pdas, &maker, None, 0);

    // 20 lots (both sides summed): reserve = 20 · 32 · 10% = 64.
    ctx.update_maker_quote_levels(&pdas, &maker, 1, 16, &[(0, 12)], &[(1, 8)]);
    assert_eq!(ctx.user_collateral(&maker.pubkey()).locked, 64, "12+8 lots");

    // Shrink to 10 lots: reserve = 10 · 32 · 10% = 32 → 32 released.
    ctx.update_maker_quote_levels(&pdas, &maker, 2, 16, &[(0, 10)], &[]);
    assert_eq!(ctx.user_collateral(&maker.pubkey()).locked, 32, "10 lots");

    // Empty ladder releases everything.
    ctx.update_maker_quote_levels(&pdas, &maker, 3, 16, &[], &[]);
    assert_eq!(
        ctx.user_collateral(&maker.pubkey()).locked,
        0,
        "empty ladder"
    );
}

/// Mid moves NEVER touch the reservation (mid-independence is what keeps the
/// O(1) re-quote path collateral-free).
#[test]
fn mid_moves_never_touch_the_reservation() {
    let (mut ctx, pdas, _vault_ta, _mint, maker) = setup();
    ctx.init_maker_quote(&pdas, &maker, None, 0);
    ctx.update_maker_quote_levels(&pdas, &maker, 1, 16, &[(0, 10)], &[]);
    let locked = ctx.user_collateral(&maker.pubkey()).locked;
    assert_eq!(locked, 32, "10 · 32 · 10%");

    for (i, mid) in [4u32, 20, 9, 28, 16].iter().enumerate() {
        ctx.update_maker_quote_mid(&pdas, &maker.pubkey(), &maker, 2 + i as u64, *mid);
        assert_eq!(
            ctx.user_collateral(&maker.pubkey()).locked,
            locked,
            "mid move #{i} must not change the reservation"
        );
    }
}

/// clear releases the full standing reservation; close only works after clear
/// (an active quote is refused by status, and a cleared quote carries no
/// reservation by construction).
#[test]
fn clear_releases_and_close_reclaims() {
    let (mut ctx, pdas, _vault_ta, _mint, maker) = setup();
    ctx.init_maker_quote(&pdas, &maker, None, 0);
    ctx.update_maker_quote_levels(&pdas, &maker, 1, 16, &[(0, 10)], &[]);
    assert_eq!(ctx.user_collateral(&maker.pubkey()).locked, 32);

    // Close while ACTIVE is refused (status guard).
    assert!(
        ctx.try_close_maker_quote(&pdas, &maker).is_err(),
        "an active quote cannot be closed"
    );

    // Clear: ladder zeroed, reservation released in full.
    ctx.try_clear_maker_quote(&pdas, &maker, 2)
        .expect("clear releases");
    assert_eq!(ctx.user_collateral(&maker.pubkey()).locked, 0, "released");

    // Close now reclaims the rent (reservation is zero by construction).
    ctx.try_close_maker_quote(&pdas, &maker)
        .expect("close reclaims rent after clear");
}

/// A delegate (or anyone) can never point the reservation at a FOREIGN ledger:
/// the ledger owner must be the quote's maker.
#[test]
fn foreign_ledger_substitution_rejected() {
    let (mut ctx, pdas, vault_ta, mint, maker) = setup();
    ctx.init_maker_quote(&pdas, &maker, None, 0);

    // A second funded account whose ledger the maker tries to lock against.
    let victim = ctx.new_funded_signer();
    ctx.init_collateral(&victim);
    let victim_ta = ctx.create_token_account(&mint, &victim.pubkey());
    ctx.mint_to(&mint, &victim_ta, 1_000);
    ctx.deposit(&victim, &vault_ta, &victim_ta, 1_000);

    let victim_ledger = ctx.collateral_pda(&victim.pubkey()).0;
    assert!(
        ctx.try_update_maker_quote_levels_with_ledger(
            &pdas,
            &maker,
            1,
            16,
            &[(0, 10)],
            &[],
            Some(victim_ledger),
        )
        .is_err(),
        "locking the ladder against a foreign ledger must be rejected"
    );
    assert_eq!(
        ctx.user_collateral(&victim.pubkey()).locked,
        0,
        "the victim's ledger is untouched"
    );
}

/// The drained-maker settle no longer reverts (plan.md §2.4.6): a maker whose
/// free balance can't cover the settle re-lock still settles via `lock_up_to`,
/// with the position's stored collateral reporting what was ACTUALLY locked —
/// instead of wedging the quote settle and silently losing the round's fills
/// when the next fold overwrites the snapshots.
#[test]
fn drained_maker_settle_does_not_revert() {
    let (mut ctx, pdas, vault_ta, mint, maker) = setup();

    // A funded taker to cross against.
    let taker = ctx.new_funded_signer();
    ctx.init_collateral(&taker);
    let taker_ta = ctx.create_token_account(&mint, &taker.pubkey());
    ctx.mint_to(&mint, &taker_ta, 1_000);
    ctx.deposit(&taker, &vault_ta, &taker_ta, 1_000);
    ctx.init_position(&pdas, &taker);

    // Maker posts a 10-lot bid at 16 (reserve 32 locked of its 1000)…
    ctx.post_maker_order(&pdas, &maker, SIDE_BUY, 16, 10);
    // …then drains its FREE balance to zero (968 free after the 32 lock).
    let maker_ta = ctx.create_token_account(&mint, &maker.pubkey());
    ctx.withdraw(&maker, &vault_ta, &maker_ta, 968);
    assert_eq!(ctx.user_collateral(&maker.pubkey()).free(), 0, "drained");

    let sell = ctx.submit_order(&pdas, &taker, SIDE_SELL, 16, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    ctx.finalize_clear(&pdas);

    // The settle MUST NOT revert even though the maker has zero free balance:
    // lock_up_to locks what exists and the position under-reports accordingly.
    ctx.settle_maker_quote(&pdas, &maker.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell, &taker.pubkey());

    let mp = ctx.position(&ctx.position_pda(&pdas, &maker.pubkey()).0);
    assert_eq!(mp.size, 10, "the fill was booked, not lost");
    // Target was initial_margin(10, 16, 1000) = 16; free was 0, so the stored
    // collateral reports what lock_up_to actually got (0) — never the target.
    assert_eq!(
        mp.collateral, 0,
        "stored collateral == actually locked (never over-reported)"
    );
}
