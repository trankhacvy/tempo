use std::sync::Arc;
use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;
use tokio::sync::watch;

use tempo_common::telemetry::{init_metrics, init_tracing};
use tempo_common::{load_keypair_file, RpcPool};
use tempo_mm_bot::health::{self, Health};
use tempo_mm_bot::{metrics_defs, MmConfig, MmCtx, MmError};
use tempo_sdk::{ix, MarketPdas, TempoClient};

#[tokio::main]
async fn main() -> Result<(), MmError> {
    let cfg = MmConfig::load()?;
    init_tracing();
    let metrics_handle = init_metrics().map_err(MmError::Common)?;
    metrics_defs::register();

    let keypair_path = cfg
        .common
        .keypair
        .clone()
        .ok_or_else(|| MmError::Config("TEMPO_KEYPAIR is required".into()))?;
    let maker = Arc::new(load_keypair_file(&keypair_path).map_err(MmError::Common)?);

    let market: Pubkey = cfg
        .common
        .market
        .clone()
        .ok_or_else(|| MmError::Config("TEMPO_MARKET is required".into()))?
        .parse()
        .map_err(|_| MmError::Config("TEMPO_MARKET is not a valid pubkey".into()))?;
    let pdas = MarketPdas::derive(market);

    let collateral_mint =
        match cfg.common.collateral_mint.as_deref() {
            Some(s) => Some(s.parse::<Pubkey>().map_err(|_| {
                MmError::Config("TEMPO_COLLATERAL_MINT is not a valid pubkey".into())
            })?),
            None => None,
        };

    let pool = RpcPool::from_urls(&cfg.common.rpc_url, cfg.common.commitment_config())
        .map_err(MmError::Common)?;
    let client = Arc::new(TempoClient::new(
        pool,
        cfg.common.priority_fee_micro_lamports,
    ));

    // One-shot `--deposit <amount>`: fund the maker's collateral ledger, then exit.
    if let Some(amount) = deposit_arg() {
        return run_deposit(&client, &maker, collateral_mint, amount).await;
    }

    let ctx = MmCtx::new(
        client,
        maker,
        pdas,
        collateral_mint,
        cfg.strategy.clone(),
        cfg.expiry_slots(),
    );

    let health = Health::new(cfg.stale_quote_windows);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tracing::info!(market = %market, maker = %ctx.maker.pubkey(), "tempo-mm-bot starting");

    let poll = Duration::from_millis(cfg.poll_ms);
    let bot = tempo_mm_bot::run(ctx, health.clone(), poll, shutdown_rx.clone());
    let server = health::serve(
        cfg.health_addr.clone(),
        health,
        metrics_handle,
        shutdown_rx.clone(),
    );
    let signal = async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "ctrl-c handler failed");
        }
        tracing::info!("shutdown signal received");
        let _ = shutdown_tx.send(true);
    };

    let (_, server_res, _) = tokio::join!(bot, server, signal);
    if let Err(e) = server_res {
        tracing::warn!(error = %e, "health server exited with error");
    }
    Ok(())
}

/// Parse `--deposit <amount>` from the process args.
fn deposit_arg() -> Option<u64> {
    let mut args = std::env::args();
    while let Some(a) = args.next() {
        if a == "--deposit" {
            return args.next().and_then(|v| v.parse().ok());
        }
    }
    None
}

/// Ensure the maker's collateral ledger exists and deposit `amount`. Token
/// accounts come from `TEMPO_MM_USER_TOKEN_ACCOUNT` / `TEMPO_VAULT_TOKEN_ACCOUNT`
/// (the operator funds and knows these); the token program defaults to SPL.
async fn run_deposit(
    client: &TempoClient,
    maker: &solana_sdk::signature::Keypair,
    collateral_mint: Option<Pubkey>,
    amount: u64,
) -> Result<(), MmError> {
    let mint = collateral_mint
        .ok_or_else(|| MmError::Config("--deposit requires TEMPO_COLLATERAL_MINT".into()))?;
    let user_token = env_pubkey("TEMPO_MM_USER_TOKEN_ACCOUNT")?;
    let vault_token = env_pubkey("TEMPO_VAULT_TOKEN_ACCOUNT")?;
    let token_program = match std::env::var("TEMPO_TOKEN_PROGRAM").ok() {
        Some(s) => s
            .parse()
            .map_err(|_| MmError::Config("TEMPO_TOKEN_PROGRAM is not a valid pubkey".into()))?,
        None => ix::SPL_TOKEN_PROGRAM_ID,
    };

    let init = ix::init_collateral(maker.pubkey(), maker.pubkey());
    if let Err(e) = client.send(maker, &[init]).await {
        tracing::info!(error = %e, "init_collateral skipped (likely exists)");
    }
    let dep = ix::deposit(
        maker.pubkey(),
        mint,
        vault_token,
        user_token,
        token_program,
        amount,
    );
    let sig = client.send(maker, &[dep]).await?;
    tracing::info!(%sig, amount, "deposit complete");
    Ok(())
}

fn env_pubkey(key: &str) -> Result<Pubkey, MmError> {
    std::env::var(key)
        .map_err(|_| MmError::Config(format!("{key} is required for --deposit")))?
        .parse()
        .map_err(|_| MmError::Config(format!("{key} is not a valid pubkey")))
}
