//! Phase 1: drive the keeper's pure decision engine (`tempo_keeper::engine::decide`)
//! against the REAL program in LiteSVM, reconstructing each tick's snapshot from
//! on-chain bytes via the SDK decoders. Proves (a) `decide` walks a full round
//! Collect → Accumulate → Discover → Settle → Roll to completion, and (b) the crank
//! actions are idempotent, which is what makes a second replica safe (D3).

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use tempo_integration_tests::*;
use tempo_keeper::engine::{decide, Plan};
use tempo_keeper::MarketSnapshot;
use tempo_sdk::accounts::{decode_slab_orders, ClearingResultView, MakerQuoteView, MarketView};

/// Rebuild the keeper's `MarketSnapshot` from LiteSVM account bytes via the SDK
/// decoders, including any maker quotes for the given makers.
fn snapshot(ctx: &TestContext, pdas: &MarketPdas, makers: &[Pubkey]) -> MarketSnapshot {
    let market =
        MarketView::decode(&ctx.raw_account(&pdas.market).expect("market")).expect("market");
    let slab = decode_slab_orders(&ctx.raw_account(&pdas.order_slab).expect("slab")).expect("slab");
    let clearing = ctx
        .raw_account(&pdas.clearing)
        .and_then(|d| ClearingResultView::decode(&d).ok());
    let mut quotes = Vec::new();
    for m in makers {
        let (q, _) = ctx.maker_quote_pda(pdas, m);
        if let Some(data) = ctx.raw_account(&q) {
            if let Ok(view) = MakerQuoteView::decode(&data) {
                quotes.push((q, view));
            }
        }
    }
    MarketSnapshot {
        market,
        slab,
        clearing,
        quotes,
    }
}

/// The maker pubkey behind a maker-quote account (the keeper derives this from the
/// decoded quote; the harness crank helpers are keyed by maker).
fn maker_of(ctx: &TestContext, quote: &Pubkey) -> Pubkey {
    let data = ctx.raw_account(quote).expect("quote account");
    MakerQuoteView::decode(&data).expect("decode quote").maker
}

#[test]
fn keeper_drives_full_round() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);

    // A maker SELL and a taker BUY at the same price land in the ask auction and
    // cross (takers trade against makers, not each other).
    let maker = ctx.new_funded_signer();
    ctx.post_maker_order(&pdas, &maker, SIDE_SELL, 100, 10);
    let taker = ctx.new_funded_signer();
    let taker_oid = ctx.submit_order(&pdas, &taker, SIDE_BUY, 100, 10);

    ctx.ensure_collect_window_closed(&pdas);
    let makers = [maker.pubkey()];
    let start_auction_id = ctx.market(&pdas).current_auction_id;

    let mut max_matched = 0u64;
    let mut saw_settle = false;
    let mut guard = 0;
    loop {
        guard += 1;
        assert!(guard < 40, "keeper loop did not converge");
        let snap = snapshot(&ctx, &pdas, &makers);
        if let Some(c) = &snap.clearing {
            max_matched = max_matched.max(c.bid_matched_volume + c.ask_matched_volume);
        }
        let now_slot = ctx.current_slot();
        match decide(&snap, now_slot, 256) {
            Plan::Idle => ctx.warp_slot(snap.market.phase_deadline_slot.max(now_slot + 1)),
            Plan::Accumulate { chunks, quotes } => {
                ctx.ensure_collect_window_closed(&pdas);
                for (start, count) in chunks {
                    ctx.process_chunk(&pdas, start, count);
                }
                for q in quotes {
                    let m = maker_of(&ctx, &q);
                    ctx.process_maker_quote(&pdas, &m);
                }
            }
            Plan::Discover => {
                ctx.finalize_clear(&pdas);
            }
            Plan::Settle { orders, quotes } => {
                saw_settle = true;
                for o in orders {
                    ctx.settle_fill(&pdas, o.order_id);
                }
                for q in quotes {
                    let m = maker_of(&ctx, &q);
                    ctx.settle_maker_quote_clearing(&pdas, &m);
                }
            }
            Plan::Roll { .. } => {
                ctx.start_auction(&pdas);
                break;
            }
        }
    }

    // The keeper cleared a real match and rolled to the next round.
    assert!(saw_settle, "the keeper should have entered a Settle phase");
    assert_eq!(max_matched, 10, "ask auction should match 10 base units");
    let after = ctx.market(&pdas);
    assert_eq!(after.current_auction_id, start_auction_id + 1);
    assert_eq!(after.phase, PHASE_COLLECT);
    let _ = taker_oid;
}

#[test]
fn keeper_actions_are_idempotent_for_replicas() {
    // The three properties a second replica relies on: double-fold is a no-op,
    // double-finalize is rejected, double-settle is rejected. Together with the
    // commutativity unit tests this is the D3 guarantee, demonstrated on-chain.
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);

    let maker = ctx.new_funded_signer();
    ctx.post_maker_order(&pdas, &maker, SIDE_SELL, 100, 10);
    let taker = ctx.new_funded_signer();
    let oid = ctx.submit_order(&pdas, &taker, SIDE_BUY, 100, 10);
    ctx.ensure_collect_window_closed(&pdas);

    // Fold the slab + the maker quote, then fold AGAIN — folding is commutative
    // integer addition over already-accumulated slots, so the second is a no-op.
    ctx.process_chunk(&pdas, 0, 16);
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    ctx.process_chunk(&pdas, 0, 16);
    // PERF-1: completeness is the authoritative histogram folded count == slab live count.
    assert_eq!(
        ctx.histogram(&pdas).accumulated_count,
        ctx.order_slab(&pdas).count as u64
    );

    // Discover once; a replica's second finalize must be rejected (already Discovered).
    ctx.finalize_clear(&pdas);
    assert!(
        ctx.try_finalize_clear(&pdas).is_err(),
        "a second finalize must be rejected"
    );

    // Settle the taker; a second settle of the same order must be rejected (consumed).
    let (_, fill) = ctx.settle_fill(&pdas, oid);
    assert_eq!(fill, 10, "taker fills the full 10 against the maker");
    assert!(
        ctx.try_settle_fill_no_position(&pdas, oid).is_err(),
        "re-settling a consumed order must be rejected"
    );

    // Settle the maker and roll — exactly one round advanced despite the double cranks.
    ctx.settle_maker_quote_clearing(&pdas, &maker.pubkey());
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).current_auction_id, 1);
}
