//! The Tempo reference market maker: a permissionless bot that keeps a two-sided
//! maker-quote ladder in the book each Collect window. It reuses `crates/sdk`
//! end-to-end (PDAs, decoders, instruction builders, the shared `benign`
//! classifier) and the keeper's stateless-tick discipline — every tick is
//! reconstructed from chain, so a crash or a second instance is safe (the
//! program enforces monotonic quote sequences; a lost race is benign).

pub mod config;
pub mod error;
pub mod health;
pub mod metrics_defs;
pub mod strategy;

pub use config::MmConfig;
pub use error::MmError;

use std::sync::Arc;
use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use tokio::sync::watch;

use tempo_common::Backoff;
use tempo_sdk::accounts::{MakerQuoteView, PositionView, UserCollateralView};
use tempo_sdk::{benign, ix, pda, MarketPdas, SdkError, TempoClient};

use crate::health::Health;
use crate::strategy::{build_quote, MmStrategyConfig};

const PHASE_COLLECT: u8 = 0;
/// Unconstrained collateral budget for a clearing-only market (no money path, so
/// quotes reserve nothing on-chain). Half of `u64::MAX` to leave headroom in the
/// margin arithmetic.
const UNMETERED_COLLATERAL: u64 = u64::MAX / 2;

/// Shared context for every tick. Clone is cheap (all `Arc`/`Copy`).
#[derive(Clone)]
pub struct MmCtx {
    pub client: Arc<TempoClient>,
    pub maker: Arc<Keypair>,
    pub pdas: MarketPdas,
    pub maker_quote: Pubkey,
    pub maker_quote_bump: u8,
    pub collateral_mint: Option<Pubkey>,
    pub strategy: MmStrategyConfig,
    pub expiry_slots: u64,
}

impl MmCtx {
    pub fn new(
        client: Arc<TempoClient>,
        maker: Arc<Keypair>,
        pdas: MarketPdas,
        collateral_mint: Option<Pubkey>,
        strategy: MmStrategyConfig,
        expiry_slots: u64,
    ) -> Self {
        // See known-issues §4.9: one quote PDA per maker per market. Interim workaround:
        // run multiple instances with different keypairs for wider depth.
        let (maker_quote, maker_quote_bump) = pda::maker_quote(&pdas.market, &maker.pubkey());
        Self {
            client,
            maker,
            pdas,
            maker_quote,
            maker_quote_bump,
            collateral_mint,
            strategy,
            expiry_slots,
        }
    }
}

/// Run the market-maker loop until `shutdown` flips true.
pub async fn run(ctx: MmCtx, health: Health, poll: Duration, mut shutdown: watch::Receiver<bool>) {
    ensure_accounts(&ctx).await;
    let mut backoff = Backoff::new();
    // The (auction_id, window_floor) the current ladder was posted against.
    let mut last_quoted: Option<(u64, u64)> = None;
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("market maker shutting down");
                    break;
                }
            }
            _ = tokio::time::sleep(poll) => {
                match tick(&ctx, &health, &mut last_quoted).await {
                    Ok(()) => backoff.reset(),
                    Err(e) => {
                        tracing::warn!(error = %e, "mm tick failed");
                        metrics::counter!("mm_quotes_posted_total", "result" => "error").increment(1);
                        health.rpc_down();
                        backoff.sleep().await;
                    }
                }
            }
        }
    }
}

/// Create the maker's `Position` and `MakerQuote` if absent (benign on "already
/// exists" — idempotent startup, safe for a second instance).
async fn ensure_accounts(ctx: &MmCtx) {
    let maker = ctx.maker.pubkey();
    let (position_pda, _) = tempo_sdk::pda::position(&ctx.pdas.market, &maker);
    let pos_exists = ctx
        .client
        .fetch_account_data_opt(&position_pda)
        .await
        .map(|opt| opt.is_some())
        .unwrap_or(false);
    if pos_exists {
        tracing::info!("ensure_accounts: position already exists, skipping init_position");
    } else {
        tracing::info!("ensure_accounts: init_position ...");
        let init_pos = ix::init_position(&ctx.pdas, maker, maker);
        match ctx.client.send(&ctx.maker, &[init_pos]).await {
            Ok(sig) => tracing::info!(%sig, "ensure_accounts: init_position ok"),
            Err(e) if benign(&e) => tracing::info!("ensure_accounts: init_position benign"),
            Err(e) => tracing::warn!(error = %e, "init_position failed (non-benign)"),
        }
    }
    // init_maker_quote requires Collect phase — deferred to tick() which only
    // runs in Collect. Nothing to do here if the maker_quote doesn't exist yet.
}

async fn tick(
    ctx: &MmCtx,
    health: &Health,
    last_quoted: &mut Option<(u64, u64)>,
) -> Result<(), SdkError> {
    let market = ctx.client.fetch_market(&ctx.pdas.market).await?;
    health.observe(market.current_auction_id);
    metrics::gauge!("mm_inventory").set(0.0);

    tracing::info!(
        auction_id = market.current_auction_id,
        phase = market.phase,
        "mm-bot tick"
    );
    // Quotes are only writable in the Collect phase.
    if market.phase != PHASE_COLLECT {
        return Ok(());
    }

    // Ensure the maker_quote account exists (requires Collect phase).
    let mq_exists = ctx
        .client
        .fetch_account_data_opt(&ctx.maker_quote)
        .await
        .map(|opt| opt.is_some())
        .unwrap_or(false);
    if !mq_exists {
        tracing::info!("tick: init_maker_quote (first time in Collect) ...");
        let init_quote = ix::init_maker_quote(
            &ctx.pdas,
            ctx.maker.pubkey(),
            ctx.maker_quote,
            ctx.maker_quote_bump,
            ctx.expiry_slots,
            Pubkey::default(),
        );
        match ctx.client.send(&ctx.maker, &[init_quote]).await {
            Ok(sig) => tracing::info!(%sig, "tick: init_maker_quote ok"),
            Err(e) if benign(&e) => tracing::info!("tick: init_maker_quote benign"),
            Err(e) => {
                tracing::warn!(error = %e, "tick: init_maker_quote failed, skipping this round");
                return Ok(());
            }
        }
    }

    // Re-quote when the round advanced or the oracle window moved (the prior
    // ladder is then stale); otherwise the existing ladder still stands.
    let key = (market.current_auction_id, market.window_floor_price);
    if *last_quoted == Some(key) {
        return Ok(());
    }

    let position = self_position(ctx).await?;
    let free_collateral = self_free_collateral(ctx).await?;
    let inventory = position.as_ref().map(|p| p.size).unwrap_or(0);
    metrics::gauge!("mm_inventory").set(inventory as f64);
    metrics::gauge!("mm_free_collateral").set(free_collateral as f64);

    let quote = match build_quote(&market, position.as_ref(), free_collateral, &ctx.strategy) {
        Some(q) => q,
        None => {
            metrics::counter!("mm_quotes_posted_total", "result" => "skip").increment(1);
            return Ok(());
        }
    };
    tracing::info!(
        mid_tick = quote.mid_tick,
        bids = quote.bids.len(),
        asks = quote.asks.len(),
        "mm-bot posting quote"
    );
    metrics::gauge!("mm_skew_ticks")
        .set((market.num_ticks as i64 / 2 - quote.mid_tick as i64) as f64);
    metrics::gauge!("mm_ladder_levels").set((quote.bids.len() + quote.asks.len()) as f64);

    if post_quote(ctx, health, &quote, key).await? {
        *last_quoted = Some(key);
    }
    Ok(())
}

/// The maker's own position, or `None` if uninitialized.
async fn self_position(ctx: &MmCtx) -> Result<Option<PositionView>, SdkError> {
    let (position_pda, _) = pda::position(&ctx.pdas.market, &ctx.maker.pubkey());
    match ctx.client.fetch_account_data_opt(&position_pda).await? {
        Some(data) => Ok(Some(PositionView::decode(&data)?)),
        None => Ok(None),
    }
}

/// Free collateral the bot risks: the maker's ledger on a money market, else an
/// unmetered budget (a clearing-only market reserves nothing at quote time).
async fn self_free_collateral(ctx: &MmCtx) -> Result<u64, SdkError> {
    match ctx.collateral_mint {
        Some(mint) => {
            let (uc, _) = pda::user_collateral(&ctx.maker.pubkey(), &mint);
            let view: Option<UserCollateralView> = ctx.client.fetch_user_collateral(&uc).await?;
            Ok(view.map(|v| v.free()).unwrap_or(0))
        }
        None => Ok(UNMETERED_COLLATERAL),
    }
}

/// Post the ladder on-chain. Returns `true` when the quote was successfully posted
/// (or lost a benign race — another instance won), `false` on a real send failure.
/// `last_quoted` is only updated by the caller when this returns `true`.
async fn post_quote(
    ctx: &MmCtx,
    health: &Health,
    quote: &crate::strategy::Quote,
    key: (u64, u64),
) -> Result<bool, SdkError> {
    let on_chain_seq = match ctx.client.fetch_account_data_opt(&ctx.maker_quote).await? {
        None => 0,
        Some(data) => MakerQuoteView::decode(&data)
            .map(|q| q.sequence)
            .unwrap_or(0),
    };
    let next_seq = on_chain_seq + 1;

    let started = std::time::Instant::now();
    let ix = ix::update_maker_quote_levels(
        &ctx.pdas,
        ctx.maker.pubkey(),
        ctx.maker_quote,
        next_seq,
        quote.mid_tick,
        &quote.bids,
        &quote.asks,
    );
    let posted = match ctx.client.send(&ctx.maker, &[ix]).await {
        Ok(_) => {
            metrics::counter!("mm_quotes_posted_total", "result" => "ok").increment(1);
            health.quoted(key.0);
            true
        }
        Err(e) if benign(&e) => {
            metrics::counter!("mm_quotes_posted_total", "result" => "benign").increment(1);
            health.quoted(key.0);
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "update_maker_quote_levels failed");
            metrics::counter!("mm_quotes_posted_total", "result" => "error").increment(1);
            false
        }
    };
    metrics::histogram!("mm_post_latency_seconds").record(started.elapsed().as_secs_f64());
    Ok(posted)
}
