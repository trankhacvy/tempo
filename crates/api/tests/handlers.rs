//! Handler/WS/error tests against a hand-seeded `AppState` (no RPC): the watcher
//! is bypassed and `LiveState` is injected directly into the `ArcSwap`, so every
//! REST path is exercised over real serialization without a validator.

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use metrics_exporter_prometheus::PrometheusBuilder;
use tempo_sdk::Pubkey;
use tokio::sync::broadcast;
use tower::ServiceExt;

use tempo_api::history::NoHistory;
use tempo_api::routes;
use tempo_api::state::{AppState, LiveState};
use tempo_common::RpcPool;
use tempo_sdk::accounts::{
    ClearingResultView, HistogramView, MakerQuoteView, MarketView, PositionView, SlabOrder,
};
use tempo_sdk::{MarketPdas, TempoClient};

fn market_view() -> MarketView {
    MarketView {
        version: 8,
        current_auction_id: 42,
        phase_deadline_slot: 200,
        tick_size: 10,
        accumulated_order_count: 1,
        active_order_count: 2,
        orders_per_auction_cap: 64,
        num_ticks: 4,
        oracle: Pubkey::new_unique(),
        phase: 0,
        last_funding_ts: 1234,
        oracle_feed_id: [0u8; 32],
        maintenance_margin_bps: 500,
        collateral_mint: Pubkey::new_unique(),
        active_maker_quote_count: 1,
        folded_maker_quote_count: 0,
        window_floor_price: 1000,
        initial_margin_bps: 600,
        max_position_notional: 1_000_000,
    }
}

fn make_position(position_pda: Pubkey, market: Pubkey) -> (Pubkey, PositionView) {
    (
        position_pda,
        PositionView {
            owner: Pubkey::new_unique(),
            market,
            size: -25,
            entry_price: 1000,
            collateral: 5000,
            realized_pnl: -7,
            margin_mode: 0,
        },
    )
}

fn live_state(with_clearing: bool) -> LiveState {
    let market = Pubkey::new_unique();
    let histogram = HistogramView {
        auction_id: 42,
        num_ticks: 4,
        bid_demand: vec![0, 5, 10, 0],
        bid_supply: vec![0, 0, 3, 7],
        ask_demand: vec![1, 2, 0, 0],
        ask_supply: vec![0, 4, 6, 0],
    };
    let clearing = if with_clearing {
        Some(ClearingResultView {
            auction_id: 42,
            bid_clearing_price: 1020,
            ask_clearing_price: 1030,
            bid_matched_volume: 8,
            ask_matched_volume: 6,
            bid_marginal_tick: 2,
            ask_marginal_tick: 3,
        })
    } else {
        None
    };
    let order = SlabOrder {
        slot: 0,
        order_id: 7,
        trader: Pubkey::new_unique(),
        side: 0,
        status: 1,
        price: 1020,
        quantity: 5,
    };
    let quote_key = Pubkey::new_unique();
    let quote = MakerQuoteView {
        maker: Pubkey::new_unique(),
        market,
        sequence: 9,
        mid_tick: 2,
        status: 1,
        num_bids: 1,
        num_asks: 1,
        folded_auction_id: 0,
        settled_auction_id: 0,
    };
    LiveState {
        slot: 150,
        market: market_view(),
        histogram,
        clearing,
        orders: vec![order],
        quotes: vec![(quote_key, quote)],
        fetched_at_unix: 1234,
    }
}

fn app_with(
    live: Option<LiveState>,
    positions: Vec<(Pubkey, PositionView)>,
) -> (axum::Router, Pubkey) {
    let market = Pubkey::new_unique();
    let pdas = MarketPdas::derive(market);
    let pool = RpcPool::from_urls(
        "http://127.0.0.1:8899",
        solana_commitment_config::CommitmentConfig::confirmed(),
    )
    .unwrap();
    let client = Arc::new(TempoClient::new(pool, 0));
    let (updates, _) = broadcast::channel(16);
    let live_slot = ArcSwapOption::empty();
    if let Some(l) = live {
        live_slot.store(Some(Arc::new(l)));
    }
    let pos_swap = ArcSwapOption::empty();
    if !positions.is_empty() {
        pos_swap.store(Some(Arc::new(positions)));
    }
    let state = AppState {
        market,
        pdas,
        client,
        live: Arc::new(live_slot),
        positions: Arc::new(pos_swap),
        updates,
        history: Arc::new(NoHistory),
    };
    let handle = PrometheusBuilder::new().build_recorder().handle();
    (routes::router(state, handle, &["*".to_string()]), market)
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn ready_reflects_snapshot_presence() {
    let (app, _) = app_with(None, vec![]);
    let (status, _) = get(&app, "/v1/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (app, _) = app_with(Some(live_state(false)), vec![]);
    let (status, _) = get(&app, "/v1/ready").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn market_endpoint_serves_configured_market() {
    let (app, market) = app_with(Some(live_state(false)), vec![]);
    let (status, body) = get(&app, &format!("/v1/markets/{market}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"current_auction_id\":42"));
    assert!(body.contains("\"window_top_price\":\"1030\"")); // 1000 + 3*10
}

#[tokio::test]
async fn unknown_market_is_404_and_bad_pubkey_is_400() {
    let (app, _) = app_with(Some(live_state(false)), vec![]);
    let other = Pubkey::new_unique();
    let (status, _) = get(&app, &format!("/v1/markets/{other}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&app, "/v1/markets/not-a-pubkey").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn histogram_includes_cross_only_when_finalized() {
    let (app, market) = app_with(Some(live_state(false)), vec![]);
    let (status, body) = get(&app, &format!("/v1/markets/{market}/histogram")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"cross\":null"));
    assert!(body.contains("\"bid_demand\":[\"0\",\"5\",\"10\",\"0\"]"));

    let (app, market) = app_with(Some(live_state(true)), vec![]);
    let (_, body) = get(&app, &format!("/v1/markets/{market}/histogram")).await;
    assert!(body.contains("\"bid_clearing_price\":\"1020\""));
    assert!(body.contains("\"ask_marginal_tick\":3"));
}

#[tokio::test]
async fn positions_list_serves_seeded_positions() {
    let pos_pda = Pubkey::new_unique();
    let pos = make_position(pos_pda, Pubkey::new_unique());
    let (app, market) = app_with(Some(live_state(true)), vec![pos]);
    let (status, body) = get(&app, &format!("/v1/markets/{market}/positions")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"size\":\"-25\""));
}

#[tokio::test]
async fn positions_list_empty_before_first_scan() {
    let (app, market) = app_with(Some(live_state(true)), vec![]);
    let (status, body) = get(&app, &format!("/v1/markets/{market}/positions")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "[]");
}

#[tokio::test]
async fn owner_position_found_and_missing() {
    let owner = Pubkey::new_unique();
    let market = Pubkey::new_unique();
    let pdas = MarketPdas::derive(market);
    let (position_pda, _) = tempo_sdk::pda::position(&market, &owner);
    let pos = make_position(position_pda, market);
    let pool = RpcPool::from_urls(
        "http://127.0.0.1:8899",
        solana_commitment_config::CommitmentConfig::confirmed(),
    )
    .unwrap();
    let (updates, _) = broadcast::channel(16);
    let live_swap = ArcSwapOption::empty();
    live_swap.store(Some(Arc::new(live_state(true))));
    let pos_swap = ArcSwapOption::empty();
    pos_swap.store(Some(Arc::new(vec![pos])));
    let state = AppState {
        market,
        pdas,
        client: Arc::new(TempoClient::new(pool, 0)),
        live: Arc::new(live_swap),
        positions: Arc::new(pos_swap),
        updates,
        history: Arc::new(NoHistory),
    };
    let handle = PrometheusBuilder::new().build_recorder().handle();
    let app = routes::router(state, handle, &["*".to_string()]);

    let (status, _) = get(&app, &format!("/v1/positions/{owner}")).await;
    assert_eq!(status, StatusCode::OK);

    let missing = Pubkey::new_unique();
    let (status, _) = get(&app, &format!("/v1/positions/{missing}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn history_endpoints_are_not_implemented() {
    let (app, market) = app_with(Some(live_state(true)), vec![]);
    let (status, body) = get(&app, &format!("/v1/markets/{market}/fills")).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(body.contains("indexer"));
    let (status, _) = get(&app, &format!("/v1/markets/{market}/funding")).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn openapi_doc_is_served() {
    let (app, _) = app_with(Some(live_state(false)), vec![]);
    let (status, body) = get(&app, "/v1/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"openapi\""));
    assert!(body.contains("/v1/markets/{market}/histogram"));
}

#[tokio::test]
async fn watcher_broadcast_reaches_subscribers() {
    let (updates, _) = broadcast::channel::<Arc<LiveState>>(16);
    let mut rx = updates.subscribe();
    let live = Arc::new(live_state(true));
    updates.send(live.clone()).unwrap();
    let got = rx.recv().await.unwrap();
    assert_eq!(got.market.current_auction_id, 42);
    assert!(got.clearing.is_some());
}
