//! The Tempo keeper: the stateless heartbeat that drives the three-phase clearing
//! protocol live. Each tick reconstructs the market from chain and derives the next
//! action (D3), so a crash or a second replica is always safe — correctness rests on
//! the program's commutativity + phase guards, never on the keeper's local view.

pub mod actions;
pub mod config;
pub mod engine;
pub mod error;
pub mod funding;
pub mod health;
pub mod metrics_defs;
pub mod snapshot;

pub use config::KeeperConfig;
pub use error::KeeperError;
pub use snapshot::{KeeperCtx, MarketSnapshot};

use std::time::Duration;

use tokio::sync::watch;

use tempo_common::Backoff;

use crate::engine::{decide, Plan};
use crate::health::Health;

/// The result of one tick: a fingerprint of advancing state + whether work is pending,
/// for the freeze watchdog.
struct TickReport {
    fingerprint: u64,
    pending: bool,
    now_slot: u64,
}

/// Run the keeper loop until `shutdown` flips true.
pub async fn run(
    ctx: KeeperCtx,
    health: Health,
    poll_interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut backoff = Backoff::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("keeper shutting down");
                    break;
                }
            }
            _ = tokio::time::sleep(poll_interval) => {
                match tick(&ctx).await {
                    Ok(report) => {
                        health.observe(report.fingerprint, report.pending, report.now_slot);
                        backoff.reset();
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "keeper tick failed");
                        metrics::counter!("keeper_tick_errors_total").increment(1);
                        health.rpc_down();
                        backoff.sleep().await;
                    }
                }
            }
        }
    }
}

async fn tick(ctx: &KeeperCtx) -> Result<TickReport, KeeperError> {
    let snapshot = MarketSnapshot::load(&ctx.client, &ctx.pdas).await?;
    let now_slot = ctx.client.current_slot().await?;
    metrics::gauge!("keeper_phase").set(snapshot.market.phase as f64);

    let fingerprint = snapshot.fingerprint();
    let plan = decide(&snapshot, now_slot, ctx.chunk_size);
    let pending = !matches!(plan, Plan::Idle);
    tracing::info!(
        auction_id = snapshot.market.current_auction_id,
        phase = snapshot.market.phase,
        slot = now_slot,
        plan = ?plan,
        "keeper tick"
    );

    // Operational duty (DDR-3 correction #2 + Correction-2 item 5): reap expired resting
    // orders so a passive expired order's slot + reserved margin isn't squatted forever.
    // `cancel_order` is permissionless when expired and only valid in the Collect phase;
    // it is orthogonal to the clearing phase machine, so it runs alongside the plan rather
    // than through it. `reap` scans EVERY shard itself (the snapshot only loads shard 0),
    // so expired orders on shards 1..N are not leaked; it early-returns when nothing is
    // reapable.
    if snapshot.market.phase == engine::PHASE_COLLECT {
        actions::reap(
            ctx,
            snapshot.market.current_auction_id,
            snapshot.market.num_slab_shards,
        )
        .await;
    }

    match plan {
        Plan::Idle => {}
        Plan::Accumulate { chunks, quotes } => {
            actions::accumulate(ctx, chunks, quotes, snapshot.market.num_slab_shards).await
        }
        Plan::Discover => actions::discover(ctx, snapshot.market.num_slab_shards).await,
        Plan::Settle { orders, quotes } => actions::settle(ctx, orders, quotes).await,
        Plan::Roll { oracle } => actions::roll(ctx, oracle).await,
    }

    Ok(TickReport {
        fingerprint,
        pending,
        now_slot,
    })
}
