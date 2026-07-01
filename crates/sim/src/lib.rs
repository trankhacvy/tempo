//! The Tempo devnet simulation: a synthetic, stateless trader plus a one-shot
//! provisioner. The trader mirrors `mm_bot`'s discipline — every tick is
//! reconstructed from chain, it only acts in the `Collect` phase, and it submits at
//! most one set of orders per round (so it never exceeds the per-trader cap). A lost
//! race is benign; a crash or a second instance is safe.

pub mod artifact;
pub mod config;
pub mod error;
pub mod health;
pub mod master_funder;
pub mod metrics_defs;
pub mod persona;
pub mod provision;
pub mod rng;
pub mod spl;
pub mod strategy;

pub use config::SimConfig;
pub use error::SimError;

use std::sync::Arc;
use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use tokio::sync::watch;

use tempo_common::Backoff;
use tempo_sdk::accounts::{PositionView, UserCollateralView};
use tempo_sdk::{benign, ix, pda, MarketPdas, SdkError, TempoClient};

use crate::health::Health;
use crate::rng::SimRng;
use crate::strategy::{build_orders, TraderConfig, UNMETERED_COLLATERAL};

const PHASE_COLLECT: u8 = 0;
/// How many `submit_order` ixs to bundle per transaction (each is ~8-15k CU; a
/// small bundle keeps us well under the per-tx limit and cuts tx count).
const ORDERS_PER_TX: usize = 4;

/// Per-trader context. Clone is cheap (all `Arc`/`Copy`).
#[derive(Clone)]
pub struct TraderCtx {
    pub client: Arc<TempoClient>,
    pub trader: Arc<Keypair>,
    pub pdas: MarketPdas,
    /// `None` ⇒ clearing-only (Phase A): orders carry no money accounts.
    pub collateral_mint: Option<Pubkey>,
    pub cfg: TraderConfig,
    pub seed: u64,
}

/// Run one trader loop until `shutdown` flips true.
pub async fn run(
    ctx: TraderCtx,
    health: Health,
    poll: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    ensure_accounts(&ctx).await;
    let mut backoff = Backoff::new();
    let mut last_round: Option<u64> = None;
    let mut rng = SimRng::new(ctx.seed);
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("trader shutting down");
                    break;
                }
            }
            _ = tokio::time::sleep(poll) => {
                match tick(&ctx, &health, &mut last_round, &mut rng).await {
                    Ok(()) => backoff.reset(),
                    Err(e) => {
                        tracing::warn!(error = %e, "trader tick failed");
                        metrics::counter!("sim_orders_submitted_total", "result" => "error").increment(1);
                        health.rpc_down();
                        backoff.sleep().await;
                    }
                }
            }
        }
    }
}

/// Run a single trader tick (ensure accounts, then one submission pass) and return.
/// Used by the `--once` smoke mode.
pub async fn run_once(ctx: &TraderCtx, health: &Health) -> Result<(), SdkError> {
    ensure_accounts(ctx).await;
    let mut last_round: Option<u64> = None;
    let mut rng = SimRng::new(ctx.seed);
    tick(ctx, health, &mut last_round, &mut rng).await
}

/// Create the trader's `Position` if absent (money markets only — a clearing-only
/// market needs no position). Idempotent: benign on "already exists".
async fn ensure_accounts(ctx: &TraderCtx) {
    if ctx.collateral_mint.is_none() {
        return;
    }
    let trader = ctx.trader.pubkey();
    let (position_pda, _) = pda::position(&ctx.pdas.market, &trader);
    let exists = ctx
        .client
        .fetch_account_data_opt(&position_pda)
        .await
        .map(|opt| opt.is_some())
        .unwrap_or(false);
    if exists {
        return;
    }
    let init_pos = ix::init_position(&ctx.pdas, trader, trader);
    match ctx.client.send(&ctx.trader, &[init_pos]).await {
        Ok(sig) => tracing::info!(%sig, "ensure_accounts: init_position ok"),
        Err(e) if benign(&e) => tracing::info!("ensure_accounts: init_position benign"),
        Err(e) => tracing::warn!(error = %e, "init_position failed (non-benign)"),
    }
}

async fn tick(
    ctx: &TraderCtx,
    health: &Health,
    last_round: &mut Option<u64>,
    rng: &mut SimRng,
) -> Result<(), SdkError> {
    let market = ctx.client.fetch_market(&ctx.pdas.market).await?;
    health.observe(market.current_auction_id);

    // Orders are only accepted in the Collect phase, and we submit one set per round.
    if market.phase != PHASE_COLLECT {
        return Ok(());
    }
    if *last_round == Some(market.current_auction_id) {
        return Ok(());
    }

    let position = self_position(ctx).await?;
    let free = self_free_collateral(ctx).await?;
    let inventory = position.as_ref().map(|p| p.size).unwrap_or(0);
    metrics::gauge!("sim_inventory").set(inventory as f64);
    metrics::gauge!("sim_free_collateral").set(free as f64);

    let intents = build_orders(&market, position.as_ref(), free, rng, &ctx.cfg);
    metrics::gauge!("sim_orders_per_round").set(intents.len() as f64);
    if intents.is_empty() {
        metrics::counter!("sim_orders_submitted_total", "result" => "skip").increment(1);
        *last_round = Some(market.current_auction_id);
        return Ok(());
    }

    let money = match ctx.collateral_mint {
        Some(mint) => ix::SubmitMoney::for_trader(&ctx.pdas, ctx.trader.pubkey(), mint),
        None => ix::SubmitMoney::default(),
    };

    let mut submitted_any = false;
    for chunk in intents.chunks(ORDERS_PER_TX) {
        let ixs: Vec<_> = chunk
            .iter()
            .map(|o| {
                ix::submit_order(
                    &ctx.pdas,
                    ctx.trader.pubkey(),
                    o.side,
                    o.price,
                    o.quantity,
                    o.reduce_only,
                    // Stage A: route to shard 0 only, to match the shard-0-only keeper
                    // (multi-shard end-to-end awaits the keeper fan-out, docs/plan.md A12.3).
                    0,
                    &money,
                )
            })
            .collect();
        match ctx.client.send(&ctx.trader, &ixs).await {
            Ok(sig) => {
                tracing::info!(%sig, n = chunk.len(), "submitted orders");
                metrics::counter!("sim_orders_submitted_total", "result" => "ok").increment(1);
                submitted_any = true;
            }
            Err(e) if benign(&e) => {
                metrics::counter!("sim_orders_submitted_total", "result" => "benign").increment(1);
                submitted_any = true;
            }
            Err(e) => {
                tracing::warn!(error = %e, "submit_order failed (non-benign)");
                metrics::counter!("sim_orders_submitted_total", "result" => "error").increment(1);
            }
        }
    }
    if submitted_any {
        *last_round = Some(market.current_auction_id);
    }
    Ok(())
}

async fn self_position(ctx: &TraderCtx) -> Result<Option<PositionView>, SdkError> {
    if ctx.collateral_mint.is_none() {
        return Ok(None);
    }
    let (position_pda, _) = pda::position(&ctx.pdas.market, &ctx.trader.pubkey());
    match ctx.client.fetch_account_data_opt(&position_pda).await? {
        Some(data) => Ok(Some(PositionView::decode(&data)?)),
        None => Ok(None),
    }
}

async fn self_free_collateral(ctx: &TraderCtx) -> Result<u64, SdkError> {
    match ctx.collateral_mint {
        Some(mint) => {
            let (uc, _) = pda::user_collateral(&ctx.trader.pubkey(), &mint);
            let view: Option<UserCollateralView> = ctx.client.fetch_user_collateral(&uc).await?;
            Ok(view.map(|v| v.free()).unwrap_or(0))
        }
        None => Ok(UNMETERED_COLLATERAL),
    }
}
