//! Regression tests for the adversarial-review fixes.
//!
//! * Maker-quote settle completeness gate (v13): a folded-but-unsettled maker
//!   quote must block the roll (HIGH — conservation hole a hostile maker could
//!   force by cranking the roll while skipping its own settle).
//! * `PAUSE_ROLL` actually halts `start_auction` (was a no-op).
//! * `crank_fee` is bounded (can't be weaponized as an instant insurance drain).

use tempo_integration_tests::*;

/// A maker quote that folded this round but has NOT been settled must block
/// `start_auction` — otherwise the histogram is zeroed at roll and the maker's
/// counter-position is orphaned, leaving the takers' side unmatched.
#[test]
fn roll_blocked_until_folded_maker_quote_is_settled() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let mint = ctx.create_mint();
    ctx.market_collateral_mint = Some(mint);
    let pdas = ctx.init_market(1, 32, 8);

    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let maker = ctx.new_funded_signer();
    let taker = ctx.new_funded_signer();
    for t in [&maker, &taker] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 10_000);
        ctx.deposit(t, &vault_ta, &ta, 10_000);
        ctx.init_position(&pdas, t);
    }

    // Maker bids 10 @ tick 20; taker sells 10 → they cross in the bid auction.
    ctx.init_maker_quote(&pdas, &maker, None, 0);
    ctx.update_maker_quote_levels(&pdas, &maker, 1, 20, &[(0, 10)], &[]);
    let sell = ctx.submit_order(&pdas, &taker, SIDE_SELL, 21, 10);

    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    ctx.finalize_clear(&pdas);
    // Settle ONLY the taker, then drain + reset every shard (the taker slab is
    // now fully settled, so shards_ready reaches num_slab_shards).
    ctx.settle_fill_with_margin(&pdas, sell, &taker.pubkey());
    for shard in 0..pdas.num_slab_shards {
        ctx.reset_shard(&pdas, shard);
    }

    // The maker quote folded (steered the price, matched the taker) but was NOT
    // settled — the roll MUST be refused (AuctionNotComplete / Custom(9)).
    let err = ctx
        .try_start_auction(&pdas)
        .expect_err("roll must block on an unsettled folded maker quote");
    assert!(
        format!("{:?}", err.err).contains("Custom(9)"),
        "expected AuctionNotComplete(9), got {:?}",
        err.err
    );

    // Settle the maker quote → the roll now succeeds and the maker's long is booked.
    ctx.settle_maker_quote(&pdas, &maker.pubkey());
    ctx.try_start_auction(&pdas)
        .expect("roll succeeds once every folded quote settled");
    let (mpos, _) = ctx.position_pda(&pdas, &maker.pubkey());
    assert_eq!(
        ctx.position(&mpos).size,
        10,
        "maker's counter-position booked"
    );
}

/// `PAUSE_ROLL` halts `start_auction` (previously unenforced) so the authority
/// can wind a market down to quiescence; exits stay open.
#[test]
fn pause_roll_halts_the_roll() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    // Drive one empty round to a rollable state (Discovered, all shards reset).
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    for shard in 0..pdas.num_slab_shards {
        ctx.reset_shard(&pdas, shard);
    }

    // PAUSE_ROLL (bit 1) set → start_auction refused with MarketPaused(2).
    ctx.set_pause(&pdas, 2);
    let err = ctx
        .try_start_auction(&pdas)
        .expect_err("PAUSE_ROLL must block the roll");
    assert!(
        format!("{:?}", err.err).contains("Custom(2)"),
        "expected MarketPaused(2), got {:?}",
        err.err
    );

    // Unpause → the roll proceeds.
    ctx.set_pause(&pdas, 0);
    ctx.try_start_auction(&pdas)
        .expect("roll proceeds once PAUSE_ROLL is cleared");
}

/// `crank_fee` is bounded at init and via the hot update path, so it can't be
/// set to an insurance-draining value.
#[test]
fn crank_fee_is_bounded() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 32, 8);
    let authority = ctx.market_authority_keypair(&pdas).expect("authority");

    // An in-bounds crank fee updates fine; an over-the-cap one is rejected.
    // Arg order: (taker_fee, crank_fee, min_notional, max_oi, reward_floor).
    ctx.try_update_market_params(&pdas, &authority, 0, 1_000, 0, 0, 0)
        .expect("a modest crank fee is accepted");
    let err = ctx
        .try_update_market_params(&pdas, &authority, 0, u64::MAX, 0, 0, 0)
        .expect_err("an unbounded crank fee must be rejected");
    assert!(
        format!("{:?}", err.err).contains("Custom("),
        "expected a config-out-of-range custom error, got {:?}",
        err.err
    );
}
