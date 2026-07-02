//! Drives the simulation's pure order strategy (`tempo_sim::strategy::build_orders`)
//! against the real program in LiteSVM: the orders it builds for a decoded market
//! must be tick-aligned, in-window, and accepted by `submit_order`, landing as
//! resting slab orders. This proves the trader → SDK → program path on the real
//! binary without devnet.

use tempo_integration_tests::*;

use solana_sdk::signature::Signer;

use tempo_sdk::accounts::{decode_slab_orders, MarketView};
use tempo_sdk::ix::{self, SubmitMoney};
use tempo_sdk::MarketPdas;

use tempo_sim::persona::Persona;
use tempo_sim::rng::SimRng;
use tempo_sim::strategy::{build_orders, TraderConfig, UNMETERED_COLLATERAL};

#[test]
fn build_orders_output_is_accepted_by_submit_order() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 32); // clearing-only (maintenance 0 by default)
    let dp = MarketPdas::derive(pdas.market);

    let market_data = ctx.raw_account(&dp.market).expect("market exists");
    let market = MarketView::decode(&market_data).expect("decode market");
    assert_eq!(market.phase, 0, "fresh market is in Collect");

    let trader = ctx.new_funded_signer();
    let cfg = TraderConfig {
        persona: Persona::Momentum,
        max_orders: 4,
        base_size: 5,
        aggression_ticks: 2,
        inner_spread_ticks: 1,
        force_side: None,
    };
    let mut rng = SimRng::new(7);
    let intents = build_orders(&market, None, UNMETERED_COLLATERAL, &mut rng, &cfg);
    assert!(!intents.is_empty(), "strategy should produce orders");

    // Every intent must be accepted by the real program (validates tick alignment,
    // in-window pricing, the new submit_order wrapper, and the clearing-only account set).
    for intent in &intents {
        let ixn = ix::submit_order(
            &dp,
            trader.pubkey(),
            intent.side,
            intent.price,
            intent.quantity,
            intent.reduce_only,
            0,
            0,
            &SubmitMoney::default(),
        );
        ctx.send_ix(ixn, &[&trader])
            .expect("submit_order accepted build_orders output");
    }

    // The slab now holds exactly the submitted orders, all resting.
    let slab = ctx.raw_account(&dp.order_slab).expect("slab exists");
    let resting = decode_slab_orders(&slab).expect("decode slab");
    assert_eq!(resting.len(), intents.len(), "all orders rest in the slab");
    for o in &resting {
        assert_eq!(o.status, 1, "order is Resting");
    }
}

#[test]
fn passive_orders_are_also_accepted() {
    // Passive orders rest inside the spread (do not cross) but must still be valid,
    // in-window prices the program accepts.
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 32);
    let dp = MarketPdas::derive(pdas.market);
    let market = MarketView::decode(&ctx.raw_account(&dp.market).unwrap()).unwrap();
    let trader = ctx.new_funded_signer();
    let cfg = TraderConfig {
        persona: Persona::Passive,
        max_orders: 2,
        base_size: 5,
        aggression_ticks: 2,
        inner_spread_ticks: 1,
        force_side: None,
    };
    let mut rng = SimRng::new(3);
    let intents = build_orders(&market, None, UNMETERED_COLLATERAL, &mut rng, &cfg);
    assert!(!intents.is_empty());
    for intent in &intents {
        let ixn = ix::submit_order(
            &dp,
            trader.pubkey(),
            intent.side,
            intent.price,
            intent.quantity,
            intent.reduce_only,
            0,
            0,
            &SubmitMoney::default(),
        );
        ctx.send_ix(ixn, &[&trader])
            .expect("passive order accepted");
    }
}
