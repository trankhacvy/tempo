//! P4.3 — `CancelAllOrders` (disc 43, missing-features §2.7, plan.md §5.2): the
//! owner-only batch cancel. One shard scan removes every still-`Resting` order
//! the signer owns, releases ONE summed margin reservation, emits one
//! `OrderCancelled` event per order, and treats zero matches as a no-op success.
//! Reaping strangers' expired orders deliberately stays on single `cancel_order`.

use solana_sdk::pubkey::Pubkey;
use tempo_integration_tests::*;

/// Batch cancel removes ONLY the signer's Resting orders: another trader's
/// orders survive, and an already-folded (Accumulated) order is skipped — a
/// later cancel-all by its owner is a clean no-op that desyncs nothing.
#[test]
fn cancels_only_the_signers_resting_orders() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    for (price, qty) in [(30u64, 5u64), (40, 3), (50, 2)] {
        ctx.submit_order(&pdas, &a, SIDE_BUY, price, qty);
    }
    let b1 = ctx.submit_order(&pdas, &b, SIDE_SELL, 40, 4);
    let b2 = ctx.submit_order(&pdas, &b, SIDE_SELL, 50, 4);
    assert_eq!(ctx.order_slab(&pdas).count, 5);

    // A pulls their book: exactly A's three go, B's two stay Resting.
    ctx.cancel_all_orders(&pdas, &a);
    let remaining = ctx.orders(&pdas);
    assert_eq!(ctx.order_slab(&pdas).count, 2, "only A's 3 orders left");
    assert!(
        remaining.iter().all(|o| o.trader == b.pubkey()),
        "B's orders are untouched by A's batch cancel"
    );
    assert!(
        remaining
            .iter()
            .all(|o| o.status == STATUS_RESTING && (o.order_id == b1 || o.order_id == b2)),
        "B's orders still Resting"
    );

    // Fold B's orders, then B batch-cancels: Accumulated orders are SKIPPED
    // (histogram already counts them — removing them would desync clearing).
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(ctx.histogram(&pdas).accumulated_count, 2);
    ctx.cancel_all_orders(&pdas, &b);
    assert_eq!(
        ctx.order_slab(&pdas).count,
        2,
        "a folded order is not batch-cancellable (no-op, nothing desyncs)"
    );
}

/// Money path: the batch releases ONE summed reservation equal to Σ of the
/// cancelled orders' `reserved_margin` — free balance is whole again, locked
/// returns to zero, and the ledger balance itself never moves.
#[test]
fn one_summed_release_equals_total_reserved() {
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

    // Three buys: worst-case notionals 10·20, 5·24, 4·30 → 10% initial margin
    // locks 20 + 12 + 12 = 44.
    ctx.submit_order(&pdas, &t, SIDE_BUY, 20, 10);
    ctx.submit_order(&pdas, &t, SIDE_BUY, 24, 5);
    ctx.submit_order(&pdas, &t, SIDE_BUY, 30, 4);
    let uc = ctx.user_collateral(&t.pubkey());
    assert_eq!(uc.locked, 44, "three reservations locked");
    assert_eq!(uc.balance, 1_000);

    ctx.cancel_all_orders(&pdas, &t);
    let uc = ctx.user_collateral(&t.pubkey());
    assert_eq!(uc.locked, 0, "the summed release freed every reservation");
    assert_eq!(
        uc.balance, 1_000,
        "release moves locked only, never balance"
    );
    assert_eq!(ctx.order_slab(&pdas).count, 0, "the whole book left");
    ctx.assert_aggregate(&[t.pubkey()]);
}

/// A trader with no orders (or an unrelated market state) batch-cancels into
/// thin air: zero matches is a SUCCESS, not an error — fire-and-forget per shard.
#[test]
fn zero_order_cancel_all_is_a_noop_success() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    let t = ctx.new_funded_signer();
    ctx.try_cancel_all_orders(&pdas, &t)
        .expect("a zero-match batch cancel succeeds as a no-op");
    assert_eq!(ctx.order_slab(&pdas).count, 0);

    // Still a no-op when OTHER traders have resting orders.
    let other = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &other, SIDE_BUY, 30, 5);
    ctx.try_cancel_all_orders(&pdas, &t)
        .expect("no-op again with a stranger's order in the shard");
    assert_eq!(ctx.order_slab(&pdas).count, 1, "the stranger's order stays");
}

/// Owner path ONLY: a stranger's EXPIRED order — which single `cancel_order`
/// would let anyone reap — is untouched by the stranger's batch cancel. The
/// reap boundary stays in exactly one place (`cancel_order`'s strict `<`).
#[test]
fn strangers_expired_order_is_untouched() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500; // money path so the parked order locks margin
    let oracle = Pubkey::new_unique();
    ctx.set_oracle(&oracle, 100_000, -8);
    let pdas = ctx.init_market_with_oracle(10, 64, 16, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 1_000_000);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 1_000_000);
    ctx.init_position(&pdas, &owner);
    let id0 = ctx.market(&pdas).current_auction_id;

    // Park an expired passive order (the reap test's recipe): in-window sell
    // expiring at id0+1, folded once, then the window recenters DOWN so it is
    // passive (never folds → settle never consumes) and survives rolls Resting.
    let o = ctx.submit_order_expiring(&pdas, &owner, SIDE_SELL, 100_000, 5, id0 + 1);
    let locked = ctx.user_collateral(&owner.pubkey()).locked;
    assert!(locked > 0);
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, o);
    ctx.set_oracle(&oracle, 50_000, -8);
    ctx.start_auction(&pdas);
    // Roll once more so expiry (id0+1) < current (id0+2): REAPABLE via cancel_order.
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).current_auction_id, id0 + 2);

    // A stranger with their own resting order batch-cancels: their order goes,
    // the owner's expired-but-reapable order is NOT touched (owner path only).
    let stranger = ctx.new_funded_signer();
    ctx.init_collateral(&stranger);
    let stranger_ta = ctx.create_token_account(&mint, &stranger.pubkey());
    ctx.mint_to(&mint, &stranger_ta, 1_000_000);
    ctx.deposit(&stranger, &vault_ta, &stranger_ta, 1_000_000);
    ctx.init_position(&pdas, &stranger);
    let s_id = ctx.submit_order(&pdas, &stranger, SIDE_BUY, 50_000, 5);
    ctx.cancel_all_orders(&pdas, &stranger);
    let remaining = ctx.orders(&pdas);
    assert!(
        remaining.iter().all(|r| r.order_id != s_id),
        "the stranger's own order was cancelled"
    );
    let parked = remaining
        .iter()
        .find(|r| r.order_id == o)
        .expect("the owner's expired order is still in the book");
    assert_eq!(parked.status, STATUS_RESTING);
    assert_eq!(
        ctx.user_collateral(&owner.pubkey()).locked,
        locked,
        "the parked order's margin reservation was not released"
    );
}
