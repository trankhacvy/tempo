//! Pre-trade safety (missing-features §1.1/§1.2): a money-path `submit_order`
//! reserves the order's worst-case initial margin so a matched trade can always
//! settle. This proves the four user-visible guarantees:
//!   1. an under-collateralized order is rejected cleanly AT SUBMIT (never a wedged
//!      settlement);
//!   2. `cancel_order` releases the reservation;
//!   3. a settled fill nets the reservation down to the actual locked margin;
//!   4. `reduce_only` lets a fully-margined position close (reserves ~0), while a
//!      non-reduce sell at the same moment is rejected;
//!   5. the per-position `max_position_notional` cap is enforced at submit.

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use tempo_integration_tests::*;

const MAINT_BPS: u16 = 500; // 5% → initial margin defaults to the same

/// A money market (maint 5%) with a vault. tick_size 1, `num_ticks` window.
fn money_market(num_ticks: u32) -> (TestContext, MarketPdas, Pubkey, Pubkey) {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = MAINT_BPS;
    let pdas = ctx.init_market(1, num_ticks, 8);
    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);
    (ctx, pdas, mint, vault_ta)
}

/// A trader with a ledger, a position, and `deposit` collateral.
fn funded_trader(
    ctx: &mut TestContext,
    pdas: &MarketPdas,
    mint: &Pubkey,
    vault_ta: &Pubkey,
    deposit: u64,
) -> Keypair {
    let t = ctx.new_funded_signer();
    ctx.init_collateral(&t);
    let ta = ctx.create_token_account(mint, &t.pubkey());
    ctx.mint_to(mint, &ta, deposit);
    ctx.deposit(&t, vault_ta, &ta, deposit);
    ctx.init_position(pdas, &t);
    t
}

/// §1.1: a buy of 10 @ 30 reserves 10·30·5% = 15. A trader with only 10 free is
/// rejected at submit (clean — no order rests, nothing to wedge); a trader with 20
/// submits fine and locks exactly the 15 reservation.
#[test]
fn submit_rejected_when_collateral_below_reservation() {
    let (mut ctx, pdas, mint, vault_ta) = money_market(64);

    let poor = funded_trader(&mut ctx, &pdas, &mint, &vault_ta, 10);
    assert!(
        ctx.try_submit_order(&pdas, &poor, SIDE_BUY, 30, 10)
            .is_err(),
        "a buy reserving 15 must be rejected with only 10 free"
    );
    // Rejected at submit: no order rested.
    assert_eq!(
        ctx.order_slab(&pdas).count,
        0,
        "no order rested on rejection"
    );

    let rich = funded_trader(&mut ctx, &pdas, &mint, &vault_ta, 20);
    ctx.submit_order(&pdas, &rich, SIDE_BUY, 30, 10);
    assert_eq!(
        ctx.user_collateral(&rich.pubkey()).locked,
        15,
        "the worst-case reservation (10·30·5%) is locked at submit"
    );
}

/// §1.1: cancelling a resting order releases its reservation back to free balance.
#[test]
fn cancel_releases_reservation() {
    let (mut ctx, pdas, mint, vault_ta) = money_market(64);
    let trader = funded_trader(&mut ctx, &pdas, &mint, &vault_ta, 100);

    let oid = ctx.submit_order(&pdas, &trader, SIDE_BUY, 30, 10);
    assert_eq!(ctx.user_collateral(&trader.pubkey()).locked, 15, "reserved");

    ctx.cancel_order(&pdas, &trader, oid);
    assert_eq!(
        ctx.user_collateral(&trader.pubkey()).locked,
        0,
        "cancel releases the reservation"
    );
    assert_eq!(ctx.user_collateral(&trader.pubkey()).free(), 100);
}

/// §1.2: the per-position `max_position_notional` cap rejects an oversized order at
/// submit (worst-case resulting notional), while an order within the cap is fine.
#[test]
fn max_position_notional_caps_order() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = MAINT_BPS;
    ctx.market_max_position_notional = 200; // cap: |size|·price ≤ 200
    let pdas = ctx.init_market(1, 64, 8);
    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let trader = funded_trader(&mut ctx, &pdas, &mint, &vault_ta, 1000);
    // buy 10 @ 30 → resulting notional 300 > 200 → rejected.
    assert!(
        ctx.try_submit_order(&pdas, &trader, SIDE_BUY, 30, 10)
            .is_err(),
        "an order whose worst-case notional exceeds the cap is rejected"
    );
    // buy 5 @ 30 → notional 150 ≤ 200 → accepted.
    ctx.submit_order(&pdas, &trader, SIDE_BUY, 30, 5);
    assert_eq!(ctx.order_slab(&pdas).count, 1, "the within-cap order rests");
}

/// §1.1/§2.2: a fully-margined long can submit a reduce-only sell to close (it
/// reserves ~0 of new exposure), even though a normal sell at the same moment would
/// be rejected for want of free collateral.
#[test]
fn reduce_only_lets_a_maxed_position_close() {
    let (mut ctx, pdas, mint, vault_ta) = money_market(64);

    // The trader opens a long via a taker buy that exactly consumes its deposit as
    // margin (buy 10 @ 30 reserves 15; deposit 15 → free 0 after settle).
    let trader = funded_trader(&mut ctx, &pdas, &mint, &vault_ta, 15);
    // A maker sells 10 @ 30 for the buy to cross.
    let maker = funded_trader(&mut ctx, &pdas, &mint, &vault_ta, 1000);
    ctx.post_maker_order(&pdas, &maker, SIDE_SELL, 30, 10);
    let buy_id = ctx.submit_order(&pdas, &trader, SIDE_BUY, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &maker.pubkey());
    ctx.settle_fill_with_margin(&pdas, buy_id, &trader.pubkey());

    // The trader is long 10 with all of its collateral locked as margin.
    let pos = ctx.position_pda(&pdas, &trader.pubkey()).0;
    assert_eq!(ctx.position(&pos).size, 10);
    assert_eq!(
        ctx.user_collateral(&trader.pubkey()).free(),
        0,
        "fully margined"
    );

    // Roll to the next round so the slab reopens for collection.
    ctx.start_auction(&pdas);

    // A NORMAL sell to close is rejected — it would reserve worst-case margin the
    // fully-margined trader doesn't have free.
    assert!(
        ctx.try_submit_order(&pdas, &trader, SIDE_SELL, 30, 10)
            .is_err(),
        "a non-reduce close is blocked when fully margined"
    );
    // The reduce-only sell reserves ~0 (pure reduce) and is accepted.
    ctx.submit_order_reduce_only(&pdas, &trader, SIDE_SELL, 30, 10);
    assert_eq!(
        ctx.order_slab(&pdas).count,
        1,
        "the reduce-only close rests despite zero free collateral"
    );
}
