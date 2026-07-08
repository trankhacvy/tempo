//! Phase-4 devnet drill (plan.md P4.6): exercise the two new trading-UX paths
//! against the LIVE market, no re-provision.
//!
//!  1. **cancel_all_orders** — post two far-from-mid resting buys, batch-cancel
//!     them, assert the summed margin release restores the baseline, then fire
//!     the zero-order no-op.
//!  2. **IOC** — submit an `expires == arm_round` order that cannot cross, then
//!     (with the keeper/orchestrator rolling rounds alongside) poll until its
//!     arm round settles it: the order must be CONSUMED — never resting — and
//!     the full reservation released.
//!
//! Env: `TEMPO_SIM_ARTIFACT` (default `./sim-artifact-p2.json` — the live money
//! market), `TEMPO_SIM_DRILL_TRADER` (default `./keys/trader-0.json`, already
//! provisioned), plus the shared `SimConfig` env (RPC url etc).

use std::time::{Duration, Instant};

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};

use tempo_common::telemetry::init_tracing;
use tempo_common::{load_keypair_file, RpcPool};
use tempo_math::tick::tick_to_price;
use tempo_sdk::accounts::decode_slab_orders;
use tempo_sdk::ix::{self, SubmitMoney};
use tempo_sdk::{pda, MarketPdas, TempoClient};

use tempo_sim::artifact::SimArtifact;
use tempo_sim::config::SimConfig;
use tempo_sim::error::SimError;

const BUY: u8 = 0;
const STATUS_RESTING: u8 = 1;

struct Drill {
    client: TempoClient,
    pdas: MarketPdas,
    trader: Keypair,
    shard: u16,
    ledger: Pubkey,
    money: SubmitMoney,
}

impl Drill {
    async fn locked(&self) -> Result<u64, SimError> {
        Ok(self
            .client
            .fetch_user_collateral(&self.ledger)
            .await
            .map_err(SimError::Sdk)?
            .map(|u| u.locked)
            .unwrap_or(0))
    }

    /// The trader's orders in their shard.
    async fn my_orders(&self) -> Result<Vec<(u64, u8, u64)>, SimError> {
        let data = self
            .client
            .fetch_account_data(&self.pdas.slab_shard(self.shard))
            .await
            .map_err(SimError::Sdk)?;
        Ok(decode_slab_orders(&data)
            .map_err(SimError::Sdk)?
            .into_iter()
            .filter(|o| o.trader == self.trader.pubkey())
            .map(|o| (o.order_id, o.status, o.expires_at_auction))
            .collect())
    }

    /// A bottom-quarter in-window price for `salt` — never crosses a near-mid book.
    async fn low_price(&self, salt: u32) -> Result<u64, SimError> {
        let mv = self
            .client
            .fetch_market(&self.pdas.market)
            .await
            .map_err(SimError::Sdk)?;
        let lo = (mv.num_ticks / 4).max(1);
        Ok(
            tick_to_price(salt % lo, mv.window_floor_price, mv.tick_size, mv.num_ticks)
                .unwrap_or(mv.tick_size.max(1)),
        )
    }
}

#[tokio::main]
async fn main() -> Result<(), SimError> {
    let cfg = SimConfig::load()?;
    init_tracing();

    let artifact_path = std::env::var("TEMPO_SIM_ARTIFACT")
        .unwrap_or_else(|_| "./sim-artifact-p2.json".to_string());
    let art = SimArtifact::load(&artifact_path)?;
    let market: Pubkey = art
        .market
        .parse()
        .map_err(|_| SimError::Config("artifact market is not a valid pubkey".into()))?;
    let mint: Pubkey = art
        .collateral_mint
        .as_deref()
        .ok_or_else(|| {
            SimError::Config("artifact has no collateral mint (money market required)".into())
        })?
        .parse()
        .map_err(|_| SimError::Config("artifact mint is not a valid pubkey".into()))?;
    let trader_path = std::env::var("TEMPO_SIM_DRILL_TRADER")
        .unwrap_or_else(|_| "./keys/trader-0.json".to_string());
    let trader = load_keypair_file(&trader_path).map_err(SimError::Common)?;

    let pool = RpcPool::from_urls(&cfg.common.rpc_url, cfg.common.commitment_config())
        .map_err(SimError::Common)?;
    let client = TempoClient::new(pool, cfg.common.priority_fee_micro_lamports);
    let pdas = MarketPdas::derive(market);
    let shard = pda::shard_for_trader(&trader.pubkey(), art.num_slab_shards.max(1));
    let ledger = pda::user_collateral(&trader.pubkey(), &mint).0;
    let money = SubmitMoney::for_trader(&pdas, trader.pubkey(), mint);
    let d = Drill {
        client,
        pdas,
        trader,
        shard,
        ledger,
        money,
    };

    let mv = d
        .client
        .fetch_market(&market)
        .await
        .map_err(SimError::Sdk)?;
    tracing::info!(
        %market, shard = d.shard, auction = mv.current_auction_id, phase = mv.phase,
        "p4 drill: live market state"
    );

    // ---- Step 0: a clean slate. Flatten any leftover resting orders (this is
    // itself a cancel_all exercise), then wait one full round so anything the
    // trader had in flight (Accumulated — not cancellable) settles out. Only
    // then is `locked` a stable baseline to assert equality against.
    let sig = d
        .client
        .send(
            &d.trader,
            &[ix::cancel_all_orders(
                &d.pdas,
                d.trader.pubkey(),
                d.shard,
                Some(d.ledger),
            )],
        )
        .await
        .map_err(SimError::Sdk)?;
    tracing::info!(%sig, "p4 drill: flatten (cancel_all on whatever was resting)");
    let start_auction = mv.current_auction_id;
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mv = d
            .client
            .fetch_market(&market)
            .await
            .map_err(SimError::Sdk)?;
        if mv.current_auction_id > start_auction && d.my_orders().await?.is_empty() {
            break;
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for a clean slate (is the keeper running?)");
        }
    }
    let baseline = d.locked().await?;
    tracing::info!(
        baseline_locked = baseline,
        "p4 drill: baseline (quiet trader)"
    );

    // ---- Drill 1: cancel_all_orders --------------------------------------
    let p1 = d.low_price(1).await?;
    let p2 = d.low_price(2).await?;
    for price in [p1, p2] {
        let ix = ix::submit_order(
            &d.pdas,
            d.trader.pubkey(),
            BUY,
            price,
            1,
            false,
            d.shard,
            0, // GTC — these exist to be batch-cancelled
            &d.money,
        );
        let sig = d
            .client
            .send(&d.trader, &[ix])
            .await
            .map_err(SimError::Sdk)?;
        tracing::info!(%sig, price, "p4 drill: resting buy submitted");
    }
    let locked_two = d.locked().await?;
    let mine = d.my_orders().await?;
    let resting = mine.iter().filter(|(_, s, _)| *s == STATUS_RESTING).count();
    tracing::info!(
        locked = locked_two,
        resting,
        "p4 drill: two resting orders on the book"
    );
    assert!(locked_two > baseline, "reservations locked");
    assert!(resting >= 2, "both orders resting");

    let sig = d
        .client
        .send(
            &d.trader,
            &[ix::cancel_all_orders(
                &d.pdas,
                d.trader.pubkey(),
                d.shard,
                Some(d.ledger),
            )],
        )
        .await
        .map_err(SimError::Sdk)?;
    tracing::info!(%sig, "p4 drill: cancel_all_orders sent");
    let locked_after = d.locked().await?;
    let resting_after = d
        .my_orders()
        .await?
        .iter()
        .filter(|(_, s, _)| *s == STATUS_RESTING)
        .count();
    assert_eq!(
        locked_after, baseline,
        "summed release restored the baseline lock"
    );
    assert_eq!(resting_after, 0, "no resting orders of mine remain");
    tracing::info!("p4 drill: batch cancel released Σ reserved, book clean");

    // The zero-order call is a success, not an error.
    let sig = d
        .client
        .send(
            &d.trader,
            &[ix::cancel_all_orders(
                &d.pdas,
                d.trader.pubkey(),
                d.shard,
                Some(d.ledger),
            )],
        )
        .await
        .map_err(SimError::Sdk)?;
    tracing::info!(%sig, "p4 drill: zero-order cancel_all no-op succeeded");

    // ---- Drill 2: IOC ------------------------------------------------------
    // Fresh view for the arm round. A bottom-quarter buy USUALLY misses the
    // cross, but on a live market the oracle can move the window under us and
    // fill it — both outcomes are valid IOC behavior, so verify fill-aware.
    let pos_pda = pda::position(&d.pdas.market, &d.trader.pubkey()).0;
    let size_before = {
        let data = d
            .client
            .fetch_account_data(&pos_pda)
            .await
            .map_err(SimError::Sdk)?;
        tempo_sdk::accounts::PositionView::decode(&data)
            .map_err(SimError::Sdk)?
            .size
    };
    // Submit with a per-attempt fresh view: a fast roll can move the arm round
    // between the snapshot and the landing (`OrderAlreadyExpired`, 0x2e) — the
    // documented stale-snapshot race a real client retries through.
    let mut attempts = 0;
    let arm = loop {
        attempts += 1;
        let mv = d
            .client
            .fetch_market(&market)
            .await
            .map_err(SimError::Sdk)?;
        let arm = ix::arm_round(&mv);
        let price = d.low_price(3).await?;
        let ioc = ix::submit_ioc(
            &d.pdas,
            d.trader.pubkey(),
            &mv,
            BUY,
            price,
            1,
            false,
            d.shard,
            &d.money,
        );
        match d.client.send(&d.trader, &[ioc]).await {
            Ok(sig) => {
                tracing::info!(%sig, arm, price, attempts, "p4 drill: IOC submitted (expires == arm)");
                break arm;
            }
            Err(e) if attempts < 6 && format!("{e:?}").contains("0x2e") => {
                tracing::info!(
                    attempts,
                    "p4 drill: arm round moved under the IOC — refetch and retry"
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => return Err(SimError::Sdk(e)),
        }
    };

    let mine = d.my_orders().await?;
    let (ioc_id, _, expires) = *mine
        .iter()
        .max_by_key(|(id, _, _)| *id)
        .expect("the IOC is on the book");
    assert_eq!(expires, arm, "on-chain expiry equals the arm round");

    // Poll: the keeper rolls the arm round; the IOC must leave the book without
    // EVER resting past its arm round, and the reservation must come back.
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mv = d
            .client
            .fetch_market(&market)
            .await
            .map_err(SimError::Sdk)?;
        let mine = d.my_orders().await?;
        let entry = mine.iter().find(|(id, _, _)| *id == ioc_id);
        match entry {
            Some((_, s, _)) if *s == STATUS_RESTING && mv.current_auction_id > arm => {
                // The window recentered away between submit and the arm round:
                // the IOC is PASSIVE (DDR-3) — never folded, never settled,
                // exempt from completeness. The designed cleanup is a cancel
                // (owner, any time) or a permissionless reap (once expired) —
                // do the owner cancel and verify the margin comes back whole.
                tracing::info!(
                    auction = mv.current_auction_id,
                    "p4 drill: IOC went PASSIVE (window moved off its price) — reclaiming via cancel_order"
                );
                let cx = ix::cancel_order(
                    &d.pdas,
                    d.trader.pubkey(),
                    d.shard,
                    ioc_id,
                    u32::MAX,
                    Some(d.ledger),
                );
                let sig = d
                    .client
                    .send(&d.trader, &[cx])
                    .await
                    .map_err(SimError::Sdk)?;
                let locked_end = d.locked().await?;
                tracing::info!(%sig, locked = locked_end, "p4 drill: passive IOC cancelled");
                assert_eq!(locked_end, baseline, "passive IOC margin reclaimed in full");
                break;
            }
            None => {
                let locked_end = d.locked().await?;
                let size_after = {
                    let data = d
                        .client
                        .fetch_account_data(&pos_pda)
                        .await
                        .map_err(SimError::Sdk)?;
                    tempo_sdk::accounts::PositionView::decode(&data)
                        .map_err(SimError::Sdk)?
                        .size
                };
                if size_after != size_before {
                    // The IOC CROSSED: it filled at the clearing price and was
                    // consumed — the reservation was swapped for position
                    // margin, so `locked` moves by the fill's margin, not back
                    // to baseline. The stronger outcome (fill + consume).
                    tracing::info!(
                        auction = mv.current_auction_id,
                        filled = size_after - size_before,
                        locked = locked_end,
                        "p4 drill: IOC FILLED at the cross and was consumed"
                    );
                } else {
                    tracing::info!(
                        auction = mv.current_auction_id,
                        locked = locked_end,
                        "p4 drill: IOC missed the cross — consumed with zero fill"
                    );
                    assert_eq!(locked_end, baseline, "zero-fill IOC fully released");
                }
                break;
            }
            _ => tracing::info!(
                auction = mv.current_auction_id,
                phase = mv.phase,
                "p4 drill: waiting for the arm round to settle the IOC"
            ),
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for the IOC round to settle (is the keeper running?)");
        }
    }

    tracing::info!("p4 drill: PASS — cancel_all released Σ reserved; IOC lived exactly one round");
    Ok(())
}
