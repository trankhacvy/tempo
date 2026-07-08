//! Phase-3 risk depth (plan.md §4): partial liquidation (§6.1), the
//! keeper-reward floor (§6.2), the OI soft cap (§1.2), and the staged
//! insurance withdraw (§4.4).

use tempo_integration_tests::*;

/// Money market with partial liquidation enabled: tick 1 × 32 ticks (genesis
/// window top 32), maint 5% / initial 10%, penalty 1%, close buffer 2%.
/// Owner opens long 100 @ 30 via the quote book; ladder cleared after the fill
/// so liquidation math sees only position margin (the §7.1 standing lock).
fn open_long_100_at_30() -> (
    TestContext,
    MarketPdas,
    solana_sdk::pubkey::Pubkey,     // oracle
    solana_sdk::pubkey::Pubkey,     // vault_ta
    solana_sdk::pubkey::Pubkey,     // mint
    solana_sdk::signature::Keypair, // owner
    solana_sdk::signature::Keypair, // liquidator
    solana_sdk::signature::Keypair, // seller (counterparty)
) {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = 500;
    ctx.market_initial_margin_bps = Some(1000);
    ctx.market_close_buffer_bps = 200;
    let pdas = ctx.init_market_with_oracle(1, 32, 8, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 1_000);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 1_000);
    ctx.init_position(&pdas, &owner);

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    ctx.mint_to(&mint, &seller_ta, 1_000);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 1_000);
    ctx.init_position(&pdas, &seller);

    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    // Long 100 @ 30: maker-buy vs taker-sell, both settled; then roll + clear
    // the ladder so only the position margin (initial 10% of 3000 = 300) locks.
    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 100);
    let sell = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 100);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell, &seller.pubkey());
    ctx.start_auction(&pdas);
    ctx.try_clear_maker_quote(&pdas, &owner, 2)
        .expect("clear releases the ladder reservation");

    let p = ctx.position(&ctx.position_pda(&pdas, &owner.pubkey()).0);
    assert_eq!(p.size, 100);
    assert_eq!(p.collateral, 300, "initial margin 10% of 3000");

    (ctx, pdas, oracle, vault_ta, mint, owner, liquidator, seller)
}

/// A mildly-underwater whale loses only a SLICE: the close is minimal, the
/// remainder is healthy (a second liquidation attempt is NotLiquidatable), and
/// the freed margin flows back.
#[test]
fn partial_close_restores_health_and_leaves_the_remainder() {
    let (mut ctx, pdas, oracle, _vault_ta, _mint, owner, liquidator, _seller) =
        open_long_100_at_30();

    // Mark 28: equity = 300 + (28-30)·100 = 100 < maint 100·28·5% = 140 →
    // liquidatable, but comfortably solvent → PARTIAL.
    // c = ceil((140·10200·1e4 − 100·1e8)/(28·(500·10200 − 100·1e4))) = 38.
    ctx.set_oracle(&oracle, 28, -8);
    ctx.liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());

    let p = ctx.position(&ctx.position_pda(&pdas, &owner.pubkey()).0);
    assert_eq!(p.size, 62, "only the minimal slice (38) was closed");
    assert_eq!(p.entry_price, 30, "reduce keeps the VWAP entry");
    // The full collateral stays locked on the remainder (conservative — the
    // realized loss flushed to the ledger; shrinking to the initial target
    // would leave the position itself below maintenance).
    assert_eq!(p.collateral, 300, "collateral kept on the remainder");

    // Penalty on the CLOSED notional only: 38·28·1% = 10 (floored).
    assert_eq!(
        ctx.user_collateral(&liquidator.pubkey()).balance,
        10,
        "penalty priced on the closed slice, not the whole position"
    );

    // The remainder is HEALTHY: a second attempt must be NotLiquidatable.
    assert!(
        ctx.try_liquidate(&pdas, &oracle, &liquidator, &owner.pubkey())
            .is_err(),
        "the partially-closed position is healthy again"
    );

    // The market's OI tracks the remainder (62 long vs the seller's 100 short
    // — the closed slice is de-risked OI, mirroring liquidate's full-close
    // behaviour on the long side).
    assert_eq!(ctx.market(&pdas).oi_long, 62);
}

/// Adversarial-review regression: a position with a LARGE accrued positive
/// realized PnL (e.g. from funding) that is under maintenance must PARTIAL-close,
/// not revert. The prior code flushed ALL realized to the free ledger, leaving the
/// isolated remainder underwater even though the account was healthy → the
/// progress backstop reverted → the position was un-liquidatable until it fell far
/// enough for a full close. The fix keeps the pre-existing realized in the position.
#[test]
fn partial_liquidation_survives_large_accrued_realized() {
    let (mut ctx, pdas, oracle, _vault_ta, _mint, owner, liquidator, _seller) =
        open_long_100_at_30();

    // Seed a large positive realized cushion (as sustained favorable funding would).
    let (owner_pos, _) = ctx.position_pda(&pdas, &owner.pubkey());
    ctx.set_position_realized_pnl(&owner_pos, 2115);

    // Mark crashes to 6 (an 80% drop from entry 30): unrealized = (6−30)·100 =
    // −2400. equity = 300 + 2115 − 2400 = 15 < maint (100·6·5% = 30) → liquidatable,
    // but equity > 0 → PARTIAL. Under the old flush-all behaviour the backstop would
    // see the isolated remainder at 300 + 0 − (24·36) = −564 and REVERT.
    ctx.set_oracle(&oracle, 6, -8);
    ctx.liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());

    let p = ctx.position(&owner_pos);
    assert!(
        p.size > 0 && p.size < 100,
        "position partially closed, not reverted or fully closed (size {})",
        p.size
    );
    assert_eq!(
        p.realized_pnl, 2115,
        "the pre-existing realized cushion stayed IN the position"
    );

    // The remainder is healthy: a second attempt is NotLiquidatable.
    assert!(
        ctx.try_liquidate(&pdas, &oracle, &liquidator, &owner.pubkey())
            .is_err(),
        "the partially-closed position is healthy again"
    );
}

/// An INSOLVENT position still full-closes (the partial formula returns None on
/// equity ≤ 0) — the pre-partial behaviour is untouched where it matters.
#[test]
fn insolvent_position_still_full_closes() {
    let (mut ctx, pdas, oracle, _vault_ta, _mint, owner, liquidator, _seller) =
        open_long_100_at_30();

    // Mark 20: equity = 300 − 1000 = −700 → insolvent → full close + bad debt.
    ctx.set_oracle(&oracle, 20, -8);
    ctx.liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());

    let p = ctx.position(&ctx.position_pda(&pdas, &owner.pubkey()).0);
    assert_eq!(p.size, 0, "insolvent → the whole position closes");
    assert_eq!(p.collateral, 0);
    assert_eq!(ctx.market(&pdas).oi_long, 0);
}

/// The keeper-reward floor (§6.2): when the penalty is tiny, insurance tops the
/// liquidator up to the floor (capped at the pool) — conserving throughout.
#[test]
fn reward_floor_tops_up_a_tiny_penalty() {
    let (mut ctx, pdas, oracle, vault_ta, mint, owner, liquidator, seller) = open_long_100_at_30();

    // Seed the pool so the floor has something to draw from, then set the
    // floor to 50 via the HOT param path (§3.2).
    let donor = ctx.new_funded_signer();
    let donor_ta = ctx.create_token_account(&mint, &donor.pubkey());
    ctx.mint_to(&mint, &donor_ta, 500);
    ctx.seed_insurance(&donor, &vault_ta, &donor_ta, 500);
    let authority = ctx.market_authority_keypair(&pdas).expect("authority");
    ctx.try_update_market_params(&pdas, &authority, 0, 0, 0, 0, 50)
        .expect("set reward floor");

    // Partial liquidation pays penalty 10 → floor tops up +40 from insurance.
    ctx.set_oracle(&oracle, 28, -8);
    let insurance_before = ctx.vault().insurance_balance;
    ctx.liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());
    assert_eq!(
        ctx.user_collateral(&liquidator.pubkey()).balance,
        50,
        "penalty 10 topped up to the 50 floor"
    );
    // Conserving both ways: the owner's realized loss on the closed slice
    // (38·2 = 76) flows INTO the pool; the 40 top-up flows OUT.
    assert_eq!(
        ctx.vault().insurance_balance,
        insurance_before + 76 - 40,
        "pool = before + owner's covered loss − the floor top-up"
    );
    // The aggregate stayed exact through the whole flow.
    ctx.assert_aggregate(&[owner.pubkey(), liquidator.pubkey(), seller.pubkey()]);
}

/// The OI soft cap (§1.2): an INCREASE past the cap is rejected at submit; a
/// pure reduce always passes, even over the cap.
#[test]
fn oi_cap_blocks_increase_never_derisking() {
    let (mut ctx, pdas, _oracle, _vault_ta, _mint, owner, _liq, _seller) = open_long_100_at_30();

    // Cap the per-side OI at 120 (current long OI is 100).
    let authority = ctx.market_authority_keypair(&pdas).expect("authority");
    ctx.try_update_market_params(&pdas, &authority, 0, 0, 0, 120, 0)
        .expect("set OI cap");

    // +30 exposure → 130 > 120 → rejected (Custom 50).
    assert!(
        ctx.try_submit_order(&pdas, &owner, SIDE_BUY, 30, 30)
            .is_err(),
        "an OI increase past the cap is rejected"
    );
    // +10 → 110 ≤ 120 → accepted.
    ctx.submit_order(&pdas, &owner, SIDE_BUY, 30, 10);
    // A pure REDUCE (sell against the long) passes even at/over the cap.
    ctx.submit_order(&pdas, &owner, SIDE_SELL, 30, 50);
}

/// The staged insurance withdraw (§4.4): authority-gated propose, delay-gated
/// permissionless apply, HS-12 recipient check, and the tokens actually move.
#[test]
fn insurance_withdraw_is_staged_delayed_and_backed() {
    let (mut ctx, _pdas, _oracle, vault_ta, mint, _owner, _liq, _seller) = open_long_100_at_30();

    // Seed a pool to withdraw from.
    let donor = ctx.new_funded_signer();
    let donor_ta = ctx.create_token_account(&mint, &donor.pubkey());
    ctx.mint_to(&mint, &donor_ta, 800);
    ctx.seed_insurance(&donor, &vault_ta, &donor_ta, 800);

    let vault_admin = ctx.vault_admin_keypair().expect("vault admin recorded");
    // The destination must be OWNED BY THE VAULT AUTHORITY (security fix): apply
    // is permissionless, so an attacker-owned recipient must be rejected.
    let recipient_ta = ctx.create_token_account(&mint, &vault_admin.pubkey());

    // Nothing staged → apply is NoPendingUpdate.
    assert!(
        ctx.try_apply_insurance_withdraw(&vault_ta, &recipient_ta)
            .is_err(),
        "apply with nothing pending must fail"
    );
    // A stranger cannot propose.
    let stranger = ctx.new_funded_signer();
    assert!(
        ctx.try_propose_insurance_withdraw(&stranger, 300).is_err(),
        "only the vault authority may propose"
    );
    // The vault authority (the init_vault admin) proposes 300.
    ctx.try_propose_insurance_withdraw(&vault_admin, 300)
        .expect("propose");
    // Early apply → PendingDelayNotElapsed.
    assert!(
        ctx.try_apply_insurance_withdraw(&vault_ta, &recipient_ta)
            .is_err(),
        "apply before the delay must fail"
    );
    let now = ctx.current_slot();
    ctx.warp_slot(now + 3_001);

    // SECURITY REGRESSION: a same-mint recipient NOT owned by the vault authority
    // must be rejected, even post-delay — otherwise any cranker could front-run
    // the apply and steal the staged pool withdrawal to their own account.
    let attacker = ctx.new_funded_signer();
    let attacker_ta = ctx.create_token_account(&mint, &attacker.pubkey());
    assert!(
        ctx.try_apply_insurance_withdraw(&vault_ta, &attacker_ta)
            .is_err(),
        "a recipient not owned by the vault authority must be rejected"
    );
    assert_eq!(
        ctx.token_balance(&attacker_ta),
        0,
        "no tokens leaked to the attacker"
    );
    assert_eq!(
        ctx.vault().pending_withdraw_amount,
        300,
        "the rejected apply left the staging intact"
    );

    // Post-delay, authority-owned recipient: the PERMISSIONLESS apply pays.
    ctx.try_apply_insurance_withdraw(&vault_ta, &recipient_ta)
        .expect("post-delay apply to the authority-owned recipient");
    assert_eq!(ctx.token_balance(&recipient_ta), 300, "tokens moved");
    assert_eq!(ctx.vault().insurance_balance, 500, "pool debited");
    assert_eq!(ctx.vault().pending_withdraw_amount, 0, "staging cleared");
    // Exactly once.
    assert!(
        ctx.try_apply_insurance_withdraw(&vault_ta, &recipient_ta)
            .is_err(),
        "a staged withdraw applies exactly once"
    );
}
