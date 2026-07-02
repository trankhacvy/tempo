//! High-volume batched submitter — a stress load generator for the sharded book.
//!
//! The point: simulate 100-1000 distinct "users" submitting orders on a SINGLE RPC by
//! packing one `submit_order` from many different ephemeral traders into each
//! transaction (the master keypair pays the fee, every trader signs). N orders then cost
//! ONE RPC send instead of N — the only way to push high order volume through one
//! endpoint. Orders route across shards via `shard_for_trader`, exercising the Stage-A
//! parallel-intake path.
//!
//! Phase-A only (no positions/collateral): every order is a BUY priced in the BOTTOM
//! quarter of the window so it never crosses (a maker's sells sit near mid), so the fill
//! is always zero and no `Position` account is needed. Run the orchestrator/keeper
//! alongside to clear the zero-fill rounds and free the slabs for sustained flooding;
//! run flood alone to measure raw peak intake into a fresh book (until the slabs fill).
//!
//! Config (env): `TEMPO_SIM_ARTIFACT`, `TEMPO_SIM_MASTER_KEYPAIR`, and
//! `TEMPO_SIM_FLOOD_FLEET` (distinct signers, default 120), `TEMPO_SIM_FLOOD_BATCH`
//! (submits per tx, default 6), `TEMPO_SIM_FLOOD_SECONDS` (duration, default 60),
//! `TEMPO_SIM_FLOOD_CONCURRENCY` (in-flight txs, default 8).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};

use tempo_common::telemetry::init_tracing;
use tempo_common::{env_parse, load_keypair_file, RpcPool};
use tempo_math::tick::tick_to_price;
use tempo_sdk::ix::{self, SubmitMoney};
use tempo_sdk::{pda, MarketPdas, TempoClient};

use tempo_sim::artifact::SimArtifact;
use tempo_sim::config::SimConfig;
use tempo_sim::error::SimError;

const BUY: u8 = 0;

#[tokio::main]
async fn main() -> Result<(), SimError> {
    let cfg = SimConfig::load()?;
    init_tracing();

    let artifact_path =
        std::env::var("TEMPO_SIM_ARTIFACT").unwrap_or_else(|_| "./sim-artifact.json".to_string());
    let art = SimArtifact::load(&artifact_path)?;
    let market: Pubkey = art
        .market
        .parse()
        .map_err(|_| SimError::Config("artifact market is not a valid pubkey".into()))?;
    let pdas = Arc::new(MarketPdas::derive(market));
    let shards = art.num_slab_shards.max(1);

    let master_path = std::env::var("TEMPO_SIM_MASTER_KEYPAIR")
        .map_err(|_| SimError::Config("TEMPO_SIM_MASTER_KEYPAIR is required".into()))?;
    let master = Arc::new(load_keypair_file(&master_path).map_err(SimError::Common)?);

    let fleet_n: usize = env_parse("TEMPO_SIM_FLOOD_FLEET", 120);
    let batch: usize = env_parse::<usize>("TEMPO_SIM_FLOOD_BATCH", 6).max(1);
    let seconds: u64 = env_parse("TEMPO_SIM_FLOOD_SECONDS", 60);
    let concurrency: usize = env_parse::<usize>("TEMPO_SIM_FLOOD_CONCURRENCY", 8).max(1);

    // Ephemeral signers — they never pay (master is the fee payer) and hold no positions,
    // so they need no SOL and no setup; they exist only to be distinct order owners.
    let fleet: Vec<Arc<Keypair>> = (0..fleet_n).map(|_| Arc::new(Keypair::new())).collect();

    let pool = RpcPool::from_urls(&cfg.common.rpc_url, cfg.common.commitment_config())
        .map_err(SimError::Common)?;
    let client = Arc::new(TempoClient::new(
        pool,
        cfg.common.priority_fee_micro_lamports,
    ));

    let mv = client.fetch_market(&market).await.map_err(SimError::Sdk)?;
    let num_ticks = mv.num_ticks;
    let floor = mv.window_floor_price;
    let tick_size = mv.tick_size;
    // Bottom quarter of the window → never crosses a near-mid maker sell (zero fill).
    let lo_ticks = (num_ticks / 4).max(1);

    tracing::info!(
        %market, shards, fleet_n, batch, concurrency, seconds,
        "flood: starting batched submit load"
    );

    let ok = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let txs = Arc::new(AtomicU64::new(0));

    let start = Instant::now();
    let deadline = start + Duration::from_secs(seconds);
    let mut wave: u64 = 0;

    while Instant::now() < deadline {
        // One submit per fleet member this wave, grouped into `batch`-sized txs.
        let groups: Vec<Vec<usize>> = (0..fleet_n)
            .collect::<Vec<_>>()
            .chunks(batch)
            .map(|c| c.to_vec())
            .collect();

        stream::iter(groups)
            .for_each_concurrent(concurrency, |idxs| {
                let (client, master, pdas) = (client.clone(), master.clone(), pdas.clone());
                let (ok, failed, txs) = (ok.clone(), failed.clone(), txs.clone());
                let group_kps: Vec<Arc<Keypair>> = idxs.iter().map(|&i| fleet[i].clone()).collect();
                async move {
                    let mut ixs = Vec::with_capacity(group_kps.len());
                    for (n, kp) in group_kps.iter().enumerate() {
                        let tick = ((wave + (idxs[n] as u64)) % lo_ticks as u64) as u32;
                        let price = tick_to_price(tick, floor, tick_size, num_ticks)
                            .unwrap_or(tick_size.max(1));
                        let shard = pda::shard_for_trader(&kp.pubkey(), shards);
                        ixs.push(ix::submit_order(
                            &pdas,
                            kp.pubkey(),
                            BUY,
                            price,
                            1,
                            false,
                            shard,
                            0,
                            &SubmitMoney {
                                position: None,
                                user_collateral: None,
                            },
                        ));
                    }
                    // signers[0] = fee payer (master), then every order's owner.
                    let mut signers: Vec<&Keypair> = Vec::with_capacity(group_kps.len() + 1);
                    signers.push(master.as_ref());
                    signers.extend(group_kps.iter().map(|a| a.as_ref()));

                    txs.fetch_add(1, Ordering::Relaxed);
                    match client.send_signed(&signers, &ixs).await {
                        Ok(_) => {
                            ok.fetch_add(group_kps.len() as u64, Ordering::Relaxed);
                        }
                        Err(e) => {
                            failed.fetch_add(group_kps.len() as u64, Ordering::Relaxed);
                            tracing::debug!(error = %e, "flood: batch failed (slab full / backpressure)");
                        }
                    }
                }
            })
            .await;

        wave += 1;
        let elapsed = start.elapsed().as_secs_f64().max(0.001);
        let o = ok.load(Ordering::Relaxed);
        let f = failed.load(Ordering::Relaxed);
        let t = txs.load(Ordering::Relaxed);
        tracing::info!(
            wave,
            ok = o,
            failed = f,
            txs = t,
            orders_per_sec = format!("{:.0}", o as f64 / elapsed),
            txs_per_sec = format!("{:.0}", t as f64 / elapsed),
            "flood: progress"
        );
    }

    let elapsed = start.elapsed().as_secs_f64().max(0.001);
    let o = ok.load(Ordering::Relaxed);
    let f = failed.load(Ordering::Relaxed);
    let t = txs.load(Ordering::Relaxed);
    tracing::info!(
        total_ok = o,
        total_failed = f,
        total_txs = t,
        elapsed_s = format!("{:.1}", elapsed),
        orders_per_sec = format!("{:.0}", o as f64 / elapsed),
        txs_per_sec = format!("{:.0}", t as f64 / elapsed),
        avg_orders_per_tx = format!("{:.1}", o as f64 / (t.max(1) as f64)),
        "flood: DONE — batched submit throughput"
    );
    Ok(())
}
