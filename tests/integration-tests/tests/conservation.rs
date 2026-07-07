//! Money-path conservation (Step 1 + Step 3).
//!
//! Proves the two correctness properties the settlement loop must hold:
//!   1. Realized PnL flushes to withdrawable balance and margin releases on
//!      close, with a winner's gain == a loser's loss (zero-sum at flat) and
//!      Σ balance + insurance == vault token holdings.
//!   2. The protocol fee + crank fee move only *inside* the vault (trader →
//!      insurance → cranker), so total claims still equal vault token holdings.

use tempo_integration_tests::*;

/// Open A-long / B-short at one price, then close both at a higher price in the
/// next round. A's gain must equal B's loss, and all collateral is conserved.
#[test]
fn conservation_full_lifecycle() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 32, 8); // tick 1, fee_bps 0 (pure PnL)

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // Two traders, each funding a 1000-unit ledger and a position.
    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    for t in [&a, &b] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 1000);
        ctx.deposit(t, &vault_ta, &ta, 1000);
        ctx.init_position(&pdas, t);
    }
    assert_eq!(ctx.token_balance(&vault_ta), 2000, "both deposits in vault");

    // Maker liquidity now comes from the persistent MakerQuote book (§1.3):
    // submit_order is taker-only. Each trader inits ONE quote up front (the active
    // count persists across rounds, so completeness — folded == active — requires
    // every active quote to be folded every round). The round's maker posts a real
    // one-level bid ladder; the round's taker re-posts an EMPTY ladder so its
    // persistent quote folds to nothing. Per-maker sequence is strictly increasing.
    ctx.init_maker_quote(&pdas, &a, None, 0);
    ctx.init_maker_quote(&pdas, &b, None, 0);
    let tick = ctx.market(&pdas).tick_size;

    // --- Round 1: A opens long 10 @ 20 (maker-buy), B opens short 10 @ 20 (taker) ---
    ctx.update_maker_quote_levels(&pdas, &a, 1, price_to_tick(tick, 20), &[(0, 10)], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 1, price_to_tick(tick, 20), &[], &[]);
    let b_sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10); // taker-sell
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.process_maker_quote(&pdas, &b.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_margin(&pdas, b_sell, &b.pubkey());
    ctx.settle_maker_quote(&pdas, &b.pubkey()); // zero-fill no-op (empty ladder)

    assert_eq!(
        ctx.position(&ctx.position_pda(&pdas, &a.pubkey()).0).size,
        10
    );
    assert_eq!(
        ctx.position(&ctx.position_pda(&pdas, &b.pubkey()).0).size,
        -10
    );
    // Quote-time margin (§7.1): A's 10-lot ladder STANDS after its fill (levels
    // are parametric — they re-fold at full size next round), so A's lock is
    // position margin (10·20·5% = 10) + the standing ladder reservation
    // (10 lots · window top 32 · 5% = 16). B's ladder is empty (reserve 0), so
    // B carries only its position margin.
    assert_eq!(
        ctx.user_collateral(&a.pubkey()).locked,
        10 + 16,
        "A margin 10 + standing ladder reservation 16"
    );
    assert_eq!(ctx.user_collateral(&b.pubkey()).locked, 10, "B margin 10");

    ctx.start_auction(&pdas);

    // --- Round 2: price 25. A sells to close (+50, taker), B buys to close (-50, maker) ---
    ctx.update_maker_quote_levels(&pdas, &a, 2, price_to_tick(tick, 25), &[], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 2, price_to_tick(tick, 25), &[(0, 10)], &[]);
    let a_sell = ctx.submit_order(&pdas, &a, SIDE_SELL, 25, 10); // taker-sell
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.process_maker_quote(&pdas, &b.pubkey());
    ctx.finalize_clear(&pdas);
    // Settle the loser (B) before the winner (A): the conserving money path floats
    // PnL through insurance, so B's loss funds the pool that A's gain draws from
    // (insurance returns to its starting level by the end of the round).
    ctx.settle_maker_quote(&pdas, &b.pubkey());
    ctx.settle_fill_with_margin(&pdas, a_sell, &a.pubkey());
    ctx.settle_maker_quote(&pdas, &a.pubkey()); // zero-fill no-op (empty ladder)

    // Round 2 ends with B's 10-lot ladder still standing (reserve 16). Roll and
    // post empty ladders so both makers' standing reservations release (§7.1) —
    // the flat-and-fully-released end state the conservation asserts below need.
    ctx.start_auction(&pdas);
    ctx.update_maker_quote_levels(&pdas, &a, 3, price_to_tick(tick, 25), &[], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 3, price_to_tick(tick, 25), &[], &[]);

    let ua = ctx.user_collateral(&a.pubkey());
    let ub = ctx.user_collateral(&b.pubkey());

    // Both flat, margin released.
    assert_eq!(
        ctx.position(&ctx.position_pda(&pdas, &a.pubkey()).0).size,
        0
    );
    assert_eq!(
        ctx.position(&ctx.position_pda(&pdas, &b.pubkey()).0).size,
        0
    );
    assert_eq!(ua.locked, 0, "A margin released");
    assert_eq!(ub.locked, 0, "B margin released");

    // A gained (25-20)*10 = +50, B lost the same.
    assert_eq!(ua.balance, 1050, "A realized +50");
    assert_eq!(ub.balance, 950, "B realized -50");

    // Conservation: claims + insurance == vault token holdings, unchanged.
    let insurance = ctx.vault().insurance_balance;
    assert_eq!(insurance, 0, "no fees in this market");
    assert_eq!(
        ua.balance + ub.balance + insurance,
        ctx.token_balance(&vault_ta),
        "Σ balance + insurance == vault tokens"
    );
    assert_eq!(ua.balance + ub.balance, 2000, "zero-sum at flat");
}

/// Protocol fee accrues to insurance on each settled fill; the crank fee is paid
/// to the finalize cranker out of that pool. All movements stay inside the vault.
#[test]
fn protocol_fee_and_crank_fee_conserve() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    ctx.market_fee_bps = 100; // taker: 1% of notional per fill
    ctx.market_maker_fee_bps = 100; // maker too, so both sides pay (this test is role-agnostic)
    ctx.market_crank_fee = 3;
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    for t in [&a, &b] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 1000);
        ctx.deposit(t, &vault_ta, &ta, 1000);
        ctx.init_position(&pdas, t);
    }
    // The cranker only needs a ledger to receive the fee.
    let cranker = ctx.new_funded_signer();
    ctx.init_collateral(&cranker);

    // Persistent maker quotes (§1.3): one per trader, real ladder when acting as a
    // maker that round, empty ladder when acting as a taker.
    ctx.init_maker_quote(&pdas, &a, None, 0);
    ctx.init_maker_quote(&pdas, &b, None, 0);
    let tick = ctx.market(&pdas).tick_size;

    // --- Round 1: open at 20. fee = 10*20*1% = 2 per fill → insurance 4. ---
    ctx.update_maker_quote_levels(&pdas, &a, 1, price_to_tick(tick, 20), &[(0, 10)], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 1, price_to_tick(tick, 20), &[], &[]);
    let b_sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.process_maker_quote(&pdas, &b.pubkey());
    ctx.finalize_clear_with_fee(&pdas, &cranker); // insurance 0 → cranker paid 0
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_margin(&pdas, b_sell, &b.pubkey());
    ctx.settle_maker_quote(&pdas, &b.pubkey()); // zero-fill no-op
    assert_eq!(ctx.vault().insurance_balance, 4, "two 2-unit fees");
    assert_eq!(
        ctx.user_collateral(&cranker.pubkey()).balance,
        0,
        "nothing to pay the cranker yet"
    );

    ctx.start_auction(&pdas);

    // --- Round 2: close at 20 (no PnL). Crank fee paid from the pool. ---
    ctx.update_maker_quote_levels(&pdas, &a, 2, price_to_tick(tick, 20), &[], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 2, price_to_tick(tick, 20), &[(0, 10)], &[]);
    let a_sell = ctx.submit_order(&pdas, &a, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.process_maker_quote(&pdas, &b.pubkey());
    ctx.finalize_clear_with_fee(&pdas, &cranker); // insurance 4 → pay 3, left 1
    ctx.settle_fill_with_margin(&pdas, a_sell, &a.pubkey());
    ctx.settle_maker_quote(&pdas, &b.pubkey());
    ctx.settle_maker_quote(&pdas, &a.pubkey()); // zero-fill no-op

    let ua = ctx.user_collateral(&a.pubkey());
    let ub = ctx.user_collateral(&b.pubkey());
    let uc = ctx.user_collateral(&cranker.pubkey());
    let insurance = ctx.vault().insurance_balance;

    assert_eq!(uc.balance, 3, "cranker paid the flat crank fee");
    assert_eq!(insurance, 5, "4 - 3 paid + 4 new fees = 5");
    assert_eq!(ua.balance, 996, "A paid 2 fees of 2");
    assert_eq!(ub.balance, 996, "B paid 2 fees of 2");

    // Everything still backed by the same 2000 deposited tokens.
    assert_eq!(
        ua.balance + ub.balance + uc.balance + insurance,
        ctx.token_balance(&vault_ta),
        "claims + insurance == vault tokens"
    );
    assert_eq!(ua.balance + ub.balance + uc.balance + insurance, 2000);
}

/// Locked margin is never withdrawable: only `free = balance - locked` can leave.
#[test]
fn cannot_withdraw_locked_margin() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 128, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // A deposits 200: its long's margin locks 50, and (§7.1) its 10-lot bid
    // ladder standing-reserves another 10·128·5% = 64 at the window top.
    let a = ctx.new_funded_signer();
    ctx.init_collateral(&a);
    let a_ta = ctx.create_token_account(&mint, &a.pubkey());
    ctx.mint_to(&mint, &a_ta, 200);
    ctx.deposit(&a, &vault_ta, &a_ta, 200);
    ctx.init_position(&pdas, &a);

    // The counterparty seller must also post margin on a margin-enabled market.
    // A taker SELL reserves its WORST-CASE initial margin at submit (missing-features
    // §1.1): the bid auction could clear as high as the window top (tick 127 ≈ 128 in
    // this wide genesis window), so the reservation is 10·128·5% = 64 — fund it.
    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let s_ta = ctx.create_token_account(&mint, &seller.pubkey());
    ctx.mint_to(&mint, &s_ta, 100);
    ctx.deposit(&seller, &vault_ta, &s_ta, 100);
    ctx.init_position(&pdas, &seller);

    ctx.post_maker_order(&pdas, &a, SIDE_BUY, 100, 10); // 10 @ 100
    let s_sell = ctx.submit_order(&pdas, &seller, SIDE_SELL, 100, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &a.pubkey()); // margin = 10*100*5% = 50
    ctx.settle_fill_with_margin(&pdas, s_sell, &seller.pubkey());

    // Position margin 50 + the standing ladder reservation 64 (§7.1).
    let ua = ctx.user_collateral(&a.pubkey());
    assert_eq!(ua.locked, 50 + 64, "position margin + standing ladder");
    assert_eq!(ua.free(), 200 - 114, "the rest is free");

    // Roll and clear the quote: the ladder's standing reservation releases in
    // full, leaving exactly the position margin locked (§7.1 release path).
    ctx.start_auction(&pdas);
    ctx.try_clear_maker_quote(&pdas, &a, 2)
        .expect("clear releases the ladder reservation");
    let ua = ctx.user_collateral(&a.pubkey());
    assert_eq!(ua.locked, 50, "only the position margin remains");
    assert_eq!(ua.free(), 150);

    // Cannot pull the locked margin...
    assert!(
        ctx.try_withdraw(&a, &vault_ta, &a_ta, 160).is_err(),
        "withdrawing into locked margin must fail"
    );
    // ...but the free part withdraws fine.
    ctx.withdraw(&a, &vault_ta, &a_ta, 150);
    assert_eq!(ctx.token_balance(&a_ta), 150, "free collateral returned");
    assert_eq!(ctx.user_collateral(&a.pubkey()).free(), 0);
    assert!(
        ctx.try_withdraw(&a, &vault_ta, &a_ta, 1).is_err(),
        "nothing free left to withdraw"
    );
}

/// The market's open-interest totals track settled fills and stay balanced
/// (oi_long == oi_short), returning to zero once all positions close.
#[test]
fn oi_tracking_balanced_and_returns_to_zero() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    for t in [&a, &b] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 1000);
        ctx.deposit(t, &vault_ta, &ta, 1000);
        ctx.init_position(&pdas, t);
    }

    assert_eq!(ctx.market(&pdas).oi_long, 0);
    assert_eq!(ctx.market(&pdas).oi_short, 0);

    // Persistent maker quotes (§1.3): one per trader, real ladder when a maker that
    // round, empty when a taker.
    ctx.init_maker_quote(&pdas, &a, None, 0);
    ctx.init_maker_quote(&pdas, &b, None, 0);
    let tick = ctx.market(&pdas).tick_size;

    // Round 1: A long 10 (maker-buy), B short 10 @ 20 (taker).
    ctx.update_maker_quote_levels(&pdas, &a, 1, price_to_tick(tick, 20), &[(0, 10)], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 1, price_to_tick(tick, 20), &[], &[]);
    let b_sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.process_maker_quote(&pdas, &b.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_margin(&pdas, b_sell, &b.pubkey());
    ctx.settle_maker_quote(&pdas, &b.pubkey()); // zero-fill no-op

    let m = ctx.market(&pdas);
    assert_eq!(m.oi_long, 10, "10 long open");
    assert_eq!(m.oi_short, 10, "10 short open");
    assert_eq!(m.oi_long, m.oi_short, "OI balanced");

    ctx.start_auction(&pdas);

    // Round 2: both close at 25. A sells (taker), B buys (maker-buy).
    ctx.update_maker_quote_levels(&pdas, &a, 2, price_to_tick(tick, 25), &[], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 2, price_to_tick(tick, 25), &[(0, 10)], &[]);
    let a_sell = ctx.submit_order(&pdas, &a, SIDE_SELL, 25, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.process_maker_quote(&pdas, &b.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &b.pubkey());
    ctx.settle_fill_with_margin(&pdas, a_sell, &a.pubkey());
    ctx.settle_maker_quote(&pdas, &a.pubkey()); // zero-fill no-op

    let m = ctx.market(&pdas);
    assert_eq!(m.oi_long, 0, "all longs closed");
    assert_eq!(m.oi_short, 0, "all shorts closed");
}

/// A winner cannot be paid before the loser funds the pool — settling the
/// winner first with an empty insurance pool fails closed (InsuranceInsolvent),
/// and the same settle succeeds after the loser settles (delay, not loss).
#[test]
fn settle_winner_before_loser_fails_closed_then_succeeds() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    for t in [&a, &b] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 1000);
        ctx.deposit(t, &vault_ta, &ta, 1000);
        ctx.init_position(&pdas, t);
    }

    // Persistent maker quotes (§1.3): one per trader, real/empty ladder per round.
    ctx.init_maker_quote(&pdas, &a, None, 0);
    ctx.init_maker_quote(&pdas, &b, None, 0);
    let tick = ctx.market(&pdas).tick_size;

    // Round 1: A long 10 (maker-buy), B short 10 @ 20 (taker).
    ctx.update_maker_quote_levels(&pdas, &a, 1, price_to_tick(tick, 20), &[(0, 10)], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 1, price_to_tick(tick, 20), &[], &[]);
    let b_sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.process_maker_quote(&pdas, &b.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_margin(&pdas, b_sell, &b.pubkey());
    ctx.settle_maker_quote(&pdas, &b.pubkey()); // zero-fill no-op
    ctx.start_auction(&pdas);

    // Round 2: close at 25. A (long) sells → +50 winner (taker); B (short) buys via
    // its maker quote → -50 loser.
    ctx.update_maker_quote_levels(&pdas, &a, 2, price_to_tick(tick, 25), &[], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 2, price_to_tick(tick, 25), &[(0, 10)], &[]);
    let a_sell = ctx.submit_order(&pdas, &a, SIDE_SELL, 25, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.process_maker_quote(&pdas, &b.pubkey());
    ctx.finalize_clear(&pdas);

    assert_eq!(
        ctx.vault().insurance_balance,
        0,
        "pool empty before any close"
    );
    // Winner first → fail closed.
    assert!(
        ctx.try_settle_fill_with_margin(&pdas, a_sell, &a.pubkey())
            .is_err(),
        "winner cannot be paid from an empty pool"
    );
    // Loser (B, the maker) funds the pool, then the winner settles fine.
    ctx.settle_maker_quote(&pdas, &b.pubkey());
    ctx.settle_fill_with_margin(&pdas, a_sell, &a.pubkey());
    ctx.settle_maker_quote(&pdas, &a.pubkey()); // zero-fill no-op

    assert_eq!(ctx.user_collateral(&a.pubkey()).balance, 1050, "A +50");
    assert_eq!(ctx.user_collateral(&b.pubkey()).balance, 950, "B -50");
    assert_eq!(ctx.vault().insurance_balance, 0, "pool back to 0");
}

/// Realized profit is withdrawable, but only up to real backing — a winner
/// can withdraw principal + gain (funded by the loser's settled loss) and not a
/// unit more. Combined with the winner/loser gate, balance is always vault-backed.
#[test]
fn profit_is_withdrawable_only_up_to_backing() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    let a_ta = ctx.create_token_account(&mint, &a.pubkey());
    let b_ta = ctx.create_token_account(&mint, &b.pubkey());
    for (t, ta) in [(&a, &a_ta), (&b, &b_ta)] {
        ctx.init_collateral(t);
        ctx.mint_to(&mint, ta, 1000);
        ctx.deposit(t, &vault_ta, ta, 1000);
        ctx.init_position(&pdas, t);
    }

    // Persistent maker quotes (§1.3): one per trader, real/empty ladder per round.
    ctx.init_maker_quote(&pdas, &a, None, 0);
    ctx.init_maker_quote(&pdas, &b, None, 0);
    let tick = ctx.market(&pdas).tick_size;

    // A long (maker-buy), B short @ 20 (taker); close @ 25 → A +50, B -50.
    ctx.update_maker_quote_levels(&pdas, &a, 1, price_to_tick(tick, 20), &[(0, 10)], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 1, price_to_tick(tick, 20), &[], &[]);
    let b_sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.process_maker_quote(&pdas, &b.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_margin(&pdas, b_sell, &b.pubkey());
    ctx.settle_maker_quote(&pdas, &b.pubkey()); // zero-fill no-op
    ctx.start_auction(&pdas);
    ctx.update_maker_quote_levels(&pdas, &a, 2, price_to_tick(tick, 25), &[], &[]);
    ctx.update_maker_quote_levels(&pdas, &b, 2, price_to_tick(tick, 25), &[(0, 10)], &[]);
    let a_sell = ctx.submit_order(&pdas, &a, SIDE_SELL, 25, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.process_maker_quote(&pdas, &b.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &b.pubkey());
    ctx.settle_fill_with_margin(&pdas, a_sell, &a.pubkey());
    ctx.settle_maker_quote(&pdas, &a.pubkey()); // zero-fill no-op

    // A holds principal 1000 + profit 50 = 1050, all backed by vault tokens.
    assert_eq!(ctx.user_collateral(&a.pubkey()).balance, 1050);
    ctx.withdraw(&a, &vault_ta, &a_ta, 1050);
    assert!(
        ctx.try_withdraw(&a, &vault_ta, &a_ta, 1).is_err(),
        "cannot withdraw beyond the backed balance"
    );
    assert_eq!(
        ctx.token_balance(&a_ta),
        1050,
        "A received principal + profit"
    );
}
