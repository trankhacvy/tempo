use tempo_common::telemetry::init_tracing;
use tempo_sim::config::ProvisionConfig;
use tempo_sim::error::SimError;
use tempo_sim::provision::provision;

/// One-shot provisioner: stand up the simulated market + money path and write the
/// artifact. Idempotent — safe to re-run after a devnet reset.
fn main() -> Result<(), SimError> {
    init_tracing();
    let cfg = ProvisionConfig::load()?;
    tracing::info!(
        rpc = %cfg.rpc_url,
        money = cfg.is_money_market(),
        traders = cfg.num_traders,
        makers = cfg.num_mm,
        "tempo-sim-provision starting"
    );
    let artifact = provision(&cfg)?;
    tracing::info!(
        market = %artifact.market,
        mint = ?artifact.collateral_mint,
        "tempo-sim-provision complete"
    );
    Ok(())
}
