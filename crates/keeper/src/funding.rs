use std::time::{Duration, SystemTime, UNIX_EPOCH};

use solana_sdk::signature::Signer;
use tokio::sync::watch;

use tempo_sdk::benign;
use tempo_sdk::ix;

use crate::snapshot::KeeperCtx;

/// Phase-independent funding accrual. Reads the market for its oracle +
/// `last_funding_ts` and sends `update_funding` once the configured interval has
/// elapsed. The program does the staleness/oracle-band logic on-chain; this task
/// only paces the cadence and is safe to run alongside the crank loop (and a second
/// replica — the program rate-limits accrual by `last_funding_ts`).
pub async fn run(ctx: KeeperCtx, interval_secs: u64, mut shutdown: watch::Receiver<bool>) {
    let interval_secs = interval_secs.max(1);
    let tick = Duration::from_secs(interval_secs);
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
            _ = tokio::time::sleep(tick) => {
                if let Err(e) = maybe_update(&ctx, interval_secs).await {
                    tracing::warn!(error = %e, "funding update failed");
                }
            }
        }
    }
}

async fn maybe_update(ctx: &KeeperCtx, interval_secs: u64) -> Result<(), tempo_sdk::SdkError> {
    let market = ctx.client.fetch_market(&ctx.pdas.market).await?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let age = now.saturating_sub(market.last_funding_ts as i64).max(0);
    metrics::gauge!("keeper_funding_age_seconds").set(age as f64);

    if (age as u64) < interval_secs {
        return Ok(());
    }

    let ix = ix::update_funding(&ctx.pdas, ctx.cranker.pubkey(), market.oracle);
    match ctx.client.send(&ctx.cranker, &[ix]).await {
        Ok(_sig) => {
            metrics::counter!("keeper_funding_total", "result" => "ok").increment(1);
        }
        Err(e) if benign(&e) => {
            metrics::counter!("keeper_funding_total", "result" => "benign").increment(1);
        }
        Err(e) => {
            metrics::counter!("keeper_funding_total", "result" => "error").increment(1);
            return Err(e);
        }
    }
    Ok(())
}
