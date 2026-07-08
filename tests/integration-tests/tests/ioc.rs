//! P4.1 — IOC orders (missing-features §2.3, plan.md §5.1): an order whose
//! `expires_at_auction` EQUALS its arm round participates in exactly one auction
//! and never rests — the fill happens normally, any remainder is `Consumed` at
//! that round's settle, and the full margin reservation is released.
//!
//! The submit boundary itself (`expiry < arm` rejected `Custom(46)`, `== arm`
//! accepted) is also covered in `resting_orders.rs::submit_of_already_expired_order_is_rejected`;
//! the reaper's strict-`<` boundary is unchanged and covered by
//! `resting_orders.rs` (non-owner reap only after the last active round).

use tempo_integration_tests::*;

/// Roll one empty round so `current_auction_id >= 1` (a genesis IOC would need
/// `expires = 0`, which means GTC).
fn roll_once(ctx: &mut TestContext, pdas: &MarketPdas) {
    ctx.process_chunk(pdas, 0, 64);
    ctx.finalize_clear(pdas);
    ctx.start_auction(pdas);
}

/// A Collect-phase IOC (expiry == current == arm round) fills what crosses and
/// CONSUMES the remainder in the same round — it never rests, unlike a GTC
/// partial fill which carries.
#[test]
fn ioc_fills_what_crosses_and_never_rests() {
    let mut ctx = TestContext::new();
    let tick = 10u64;
    let pdas = ctx.init_market(tick, 16, 64);
    roll_once(&mut ctx, &pdas);
    let cur = ctx.market(&pdas).current_auction_id;

    let mb = ctx.new_funded_signer(); // maker buyer (quote book)
    let ts = ctx.new_funded_signer(); // IOC taker seller
    ctx.init_position(&pdas, &mb);

    // Maker buys 12 @ 40; IOC sells 20 @ 40 → cross 12, remainder 8.
    ctx.post_maker_order(&pdas, &mb, SIDE_BUY, 4 * tick, 12);
    let o = ctx.submit_order_expiring(&pdas, &ts, SIDE_SELL, 4 * tick, 20, cur);

    // The record is armed for THIS round and expires with it (the IOC shape).
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.arm_auction_id, cur, "armed for the submit round");
    assert_eq!(rec.expires_at_auction, cur, "expires with the same round");

    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &mb.pubkey());
    ctx.finalize_clear(&pdas);
    let (_m, fill) = ctx.settle_fill(&pdas, o);
    assert_eq!(fill, 12, "the IOC filled the crossed 12");
    ctx.settle_maker_quote_clearing(&pdas, &mb.pubkey());

    // The 8-lot remainder is CONSUMED (one-round life), not re-armed Resting.
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_CONSUMED,
        "an IOC remainder is consumed, never rests"
    );
    assert_eq!(
        ctx.order_slab(&pdas).count,
        0,
        "the IOC left the book in its arm round"
    );
}

/// An IOC that MISSES the cross entirely (no counterparty) consumes with zero
/// fill and its full worst-case margin reservation is released back.
#[test]
fn ioc_that_misses_the_cross_releases_full_margin() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    ctx.market_initial_margin_bps = Some(1000);
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let t = ctx.new_funded_signer();
    ctx.init_collateral(&t);
    let t_ta = ctx.create_token_account(&mint, &t.pubkey());
    ctx.mint_to(&mint, &t_ta, 1_000);
    ctx.deposit(&t, &vault_ta, &t_ta, 1_000);
    ctx.init_position(&pdas, &t);

    roll_once(&mut ctx, &pdas);
    let cur = ctx.market(&pdas).current_auction_id;

    // IOC buy 10 @ 20 with no seller anywhere: margin locks at submit...
    let o = ctx.submit_order_expiring(&pdas, &t, SIDE_BUY, 20, 10, cur);
    let locked_after_submit = ctx.user_collateral(&t.pubkey()).locked;
    assert!(
        locked_after_submit > 0,
        "worst-case margin locked at submit"
    );

    // ...the round clears with no cross, settle consumes the IOC...
    ctx.process_chunk(&pdas, 0, 8);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill_with_margin(&pdas, o, &t.pubkey());

    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.status, STATUS_CONSUMED, "zero-fill IOC is consumed");
    // ...and the FULL reservation came back (nothing filled, nothing kept).
    assert_eq!(
        ctx.user_collateral(&t.pubkey()).locked,
        0,
        "the whole IOC reservation was released"
    );
}

/// A mid-round IOC (submitted after the market left Collect) arms for the NEXT
/// round (`current + 1`), so its IOC expiry is `current + 1` too: accepted,
/// skipped by the in-flight round, then lives exactly one round and consumes.
/// `expires == current` is `< arm` mid-round → rejected `Custom(46)`.
#[test]
fn mid_round_ioc_arms_and_expires_at_the_next_round() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);
    roll_once(&mut ctx, &pdas);
    let cur = ctx.market(&pdas).current_auction_id;

    // Put a round in flight (leave Collect).
    let a = ctx.new_funded_signer();
    let a_id = ctx.submit_order(&pdas, &a, SIDE_BUY, 30, 10);
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);

    let t = ctx.new_funded_signer();
    // Mid-round, arm = current + 1: an expiry AT the current round is `< arm` —
    // it could never fold — rejected with OrderAlreadyExpired (46).
    let err = ctx
        .try_submit_order_expiring(&pdas, &t, SIDE_BUY, 30, 5, cur)
        .expect_err("mid-round expiry at the current round can never fold");
    assert!(
        format!("{:?}", err.err).contains("Custom(46)"),
        "rejected with OrderAlreadyExpired (46), got {:?}",
        err.err
    );
    // Expiry == current + 1 == arm: the mid-round IOC. Same side as `a`, so its
    // arm round has no cross — the one-round life shows as a clean zero-fill
    // consume (the crossing case is `ioc_fills_what_crosses_and_never_rests`).
    let o = ctx.submit_order_expiring(&pdas, &t, SIDE_BUY, 30, 5, cur + 1);
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.arm_auction_id, cur + 1, "deferred to the next round");
    assert_eq!(
        rec.expires_at_auction,
        cur + 1,
        "IOC: expires with its arm round"
    );

    // The in-flight round ignores it (fold count stays at the one Collect order)...
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(ctx.histogram(&pdas).accumulated_count, 1);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, a_id);
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).current_auction_id, cur + 1);

    // ...its arm round folds and settles it: zero fill → CONSUMED, never rests.
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, o);
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_CONSUMED,
        "the mid-round IOC lived exactly its arm round"
    );
}
