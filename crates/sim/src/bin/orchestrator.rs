//! Local single-process orchestrator: runs the keeper, the market makers, the
//! liquidator (money markets only), and the trader fleet against an already
//! provisioned market, all on one tokio runtime sharing one shutdown channel. For
//! `cargo run` / `just sim` demos; the durable devnet deployment uses the compose
//! profile instead.

use std::sync::Arc;
use std::time::Duration;

use futures::future::{join_all, LocalBoxFuture};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;
use tokio::sync::watch;

use tempo_common::telemetry::{init_metrics, init_tracing};
use tempo_common::{env_parse, load_keypair_file, RpcPool};
use tempo_sdk::{pda, MarketPdas, TempoClient};

use tempo_keeper::funding;
use tempo_keeper::KeeperCtx;
use tempo_liquidator::source::ChainScan;
use tempo_liquidator::LiqCtx;
use tempo_mm_bot::strategy::MmStrategyConfig;
use tempo_mm_bot::MmCtx;

use tempo_sim::artifact::SimArtifact;
use tempo_sim::config::SimConfig;
use tempo_sim::error::SimError;
use tempo_sim::persona::Persona;
use tempo_sim::strategy::TraderConfig;
use tempo_sim::TraderCtx;

#[tokio::main]
async fn main() -> Result<(), SimError> {
    let cfg = SimConfig::load()?;
    init_tracing();
    let _metrics = init_metrics().map_err(SimError::Common)?;

    let artifact_path =
        std::env::var("TEMPO_SIM_ARTIFACT").unwrap_or_else(|_| "./sim-artifact.json".to_string());
    let art = SimArtifact::load(&artifact_path)?;

    let market: Pubkey = art
        .market
        .parse()
        .map_err(|_| SimError::Config("artifact market is not a valid pubkey".into()))?;
    let pdas = MarketPdas::derive(market);
    let oracle: Pubkey = art
        .oracle
        .parse()
        .map_err(|_| SimError::Config("artifact oracle is not a valid pubkey".into()))?;
    let collateral_mint: Option<Pubkey> = match art.collateral_mint.as_deref() {
        Some(s) => Some(
            s.parse()
                .map_err(|_| SimError::Config("artifact collateral_mint invalid".into()))?,
        ),
        None => None,
    };
    let vault = collateral_mint.map(|m| pda::vault(&m).0);

    let pool = RpcPool::from_urls(&cfg.common.rpc_url, cfg.common.commitment_config())
        .map_err(SimError::Common)?;
    let client = Arc::new(TempoClient::new(pool, cfg.common.priority_fee_micro_lamports));

    let poll = Duration::from_millis(cfg.poll_ms);
    let funding_secs: u64 = env_parse("TEMPO_FUNDING_INTERVAL_SECS", 60);
    let (shutdown_tx, rx) = watch::channel(false);

    let mut tasks: Vec<LocalBoxFuture<'static, ()>> = Vec::new();

    // --- master funder cron + agent auto-top-up. The cron tops the master wallet
    // up via devnet airdrop every TEMPO_SIM_MASTER_AIRDROP_SECS (default 1h); the
    // top-up loop refills each agent (keeper/MMs/traders/liquidator) from the master
    // when its SOL drops below a threshold, so the keeper never runs dry and wedges
    // the round. Both are best-effort; skipped if no master keypair is configured.
    if let Ok(master_path) = std::env::var("TEMPO_SIM_MASTER_KEYPAIR") {
        match load_keypair_file(&master_path) {
            Ok(kp) => {
                let master_kp = Arc::new(kp);
                let master = master_kp.pubkey();

                let airdrop_pool =
                    RpcPool::from_urls(&cfg.common.rpc_url, cfg.common.commitment_config())
                        .map_err(SimError::Common)?;
                let secs: u64 = env_parse("TEMPO_SIM_MASTER_AIRDROP_SECS", 3600);
                let sol: u64 = env_parse("TEMPO_SIM_MASTER_AIRDROP_SOL", 2);
                tasks.push(Box::pin(tempo_sim::master_funder::run(
                    airdrop_pool,
                    master,
                    sol * tempo_sim::master_funder::LAMPORTS_PER_SOL,
                    Duration::from_secs(secs),
                    rx.clone(),
                )));

                let mut agents: Vec<Pubkey> = Vec::new();
                for s in [&art.keeper.pubkey, &art.liquidator.pubkey] {
                    if let Ok(p) = s.parse::<Pubkey>() {
                        agents.push(p);
                    }
                }
                for a in &art.market_makers {
                    if let Ok(p) = a.pubkey.parse::<Pubkey>() {
                        agents.push(p);
                    }
                }
                for t in &art.traders {
                    if let Ok(p) = t.pubkey.parse::<Pubkey>() {
                        agents.push(p);
                    }
                }
                let topup_secs: u64 = env_parse("TEMPO_SIM_TOPUP_SECS", 120);
                let threshold: u64 = env_parse("TEMPO_SIM_TOPUP_THRESHOLD_LAMPORTS", 20_000_000);
                let target: u64 = env_parse("TEMPO_SIM_TOPUP_TARGET_LAMPORTS", 100_000_000);
                let n_agents = agents.len();
                tasks.push(Box::pin(tempo_sim::master_funder::topup_run(
                    client.clone(),
                    master_kp,
                    agents,
                    threshold,
                    target,
                    Duration::from_secs(topup_secs),
                    rx.clone(),
                )));

                tracing::info!(
                    %master,
                    airdrop_every_secs = secs,
                    airdrop_sol = sol,
                    topup_agents = n_agents,
                    topup_every_secs = topup_secs,
                    "master funder + agent auto-top-up enabled"
                );
            }
            Err(e) => tracing::warn!(error = %e, "master funder cron disabled (bad keypair)"),
        }
    }

    // --- keeper + funding ---
    let keeper_kp = Arc::new(load_keypair_file(&art.keeper.keypair_path).map_err(SimError::Common)?);
    let keeper_ctx = KeeperCtx {
        client: client.clone(),
        cranker: keeper_kp,
        pdas,
        collateral_mint,
        vault,
        chunk_size: env_parse("TEMPO_CHUNK_SIZE", 256),
        settle_concurrency: env_parse("TEMPO_SETTLE_CONCURRENCY", 8),
    };
    let kh = tempo_keeper::health::Health::new(env_parse("TEMPO_NO_PROGRESS_SLOTS", 300));
    tasks.push(Box::pin(tempo_keeper::run(
        keeper_ctx.clone(),
        kh,
        poll,
        rx.clone(),
    )));
    tasks.push(Box::pin(funding::run(keeper_ctx, funding_secs, rx.clone())));

    // --- market makers ---
    for mm in &art.market_makers {
        let maker = Arc::new(load_keypair_file(&mm.keypair_path).map_err(SimError::Common)?);
        let mm_ctx = MmCtx::new(
            client.clone(),
            maker,
            pdas,
            collateral_mint,
            default_mm_strategy(),
            0,
        );
        let mmh = tempo_mm_bot::health::Health::new(env_parse("TEMPO_MM_STALE_QUOTE_WINDOWS", 5));
        tasks.push(Box::pin(tempo_mm_bot::run(mm_ctx, mmh, poll, rx.clone())));
    }

    // --- liquidator (money markets only) ---
    if let Some(mint) = collateral_mint {
        let liq_kp =
            Arc::new(load_keypair_file(&art.liquidator.keypair_path).map_err(SimError::Common)?);
        let liquidator_collateral = pda::user_collateral(&liq_kp.pubkey(), &mint).0;
        let liq_ctx = LiqCtx {
            client: client.clone(),
            liquidator: liq_kp,
            liquidator_collateral,
            source: Arc::new(ChainScan::new(client.clone())),
            markets: vec![market],
            vault,
            collateral_mint: Some(mint),
            scan_concurrency: env_parse("TEMPO_LIQ_CONCURRENCY", 8),
        };
        let lh = tempo_liquidator::health::Health::new(env_parse("TEMPO_LIQ_STALE_SCAN_SECS", 30));
        let liq_poll = Duration::from_millis(env_parse("TEMPO_LIQ_POLL_MS", 2000));
        tasks.push(Box::pin(tempo_liquidator::run(liq_ctx, lh, liq_poll, rx.clone())));
    }

    // --- trader fleet ---
    let _ = oracle; // oracle is recorded on the market; the keeper supplies it on roll.
    for t in &art.traders {
        let trader = Arc::new(load_keypair_file(&t.keypair_path).map_err(SimError::Common)?);
        let trader_cfg = TraderConfig {
            persona: Persona::parse(&t.persona),
            max_orders: cfg.max_orders,
            base_size: cfg.base_size,
            aggression_ticks: cfg.aggression_ticks,
            inner_spread_ticks: cfg.inner_spread_ticks,
            force_side: cfg.force_side,
        };
        let ctx = TraderCtx {
            client: client.clone(),
            trader,
            pdas,
            collateral_mint,
            cfg: trader_cfg,
            seed: t.seed,
        };
        let th = tempo_sim::health::Health::new();
        tasks.push(Box::pin(tempo_sim::run(ctx, th, poll, rx.clone())));
    }

    tracing::info!(
        market = %market,
        makers = art.market_makers.len(),
        traders = art.traders.len(),
        money = collateral_mint.is_some(),
        "orchestrator: starting fleet"
    );

    let all = join_all(tasks);
    let signal = async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "ctrl-c handler failed");
        }
        tracing::info!("orchestrator: shutdown signal received");
        let _ = shutdown_tx.send(true);
    };
    tokio::join!(all, signal);
    Ok(())
}

fn default_mm_strategy() -> MmStrategyConfig {
    MmStrategyConfig {
        levels: env_parse::<u8>("TEMPO_MM_LEVELS", 3).clamp(1, 8),
        inner_spread_ticks: env_parse("TEMPO_MM_INNER_SPREAD_TICKS", 1),
        tick_step: env_parse("TEMPO_MM_TICK_STEP", 1),
        base_size: env_parse("TEMPO_MM_BASE_SIZE", 100),
        size_growth_num: env_parse("TEMPO_MM_SIZE_GROWTH_NUM", 1),
        size_growth_den: env_parse::<u32>("TEMPO_MM_SIZE_GROWTH_DEN", 1).max(1),
        max_inventory: env_parse("TEMPO_MM_MAX_INVENTORY", 10_000),
        skew_ticks_max: env_parse("TEMPO_MM_SKEW_TICKS_MAX", 2),
    }
}
