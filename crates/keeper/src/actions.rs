use futures::stream::{self, StreamExt};
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use tempo_sdk::accounts::SlabOrder;
use tempo_sdk::benign;
use tempo_sdk::ix::{self, SettleMoney};
use tempo_sdk::pda;

use crate::snapshot::KeeperCtx;

/// The fate of one instruction send. Per-ix failures never abort the tick — the
/// freeze watchdog catches persistent no-progress; a single benign race is normal.
enum Outcome {
    Sent,
    Benign,
    Failed,
}

async fn send_one(ctx: &KeeperCtx, ixs: &[Instruction], label: &'static str) -> Outcome {
    match ctx.client.send(&ctx.cranker, ixs).await {
        Ok(_sig) => {
            metrics::counter!("keeper_tx_total", "ix" => label, "result" => "ok").increment(1);
            Outcome::Sent
        }
        Err(e) if benign(&e) => {
            metrics::counter!("keeper_tx_total", "ix" => label, "result" => "benign").increment(1);
            Outcome::Benign
        }
        Err(e) => {
            tracing::warn!(ix = label, error = %e, "instruction send failed");
            metrics::counter!("keeper_tx_total", "ix" => label, "result" => "error").increment(1);
            Outcome::Failed
        }
    }
}

/// ACCUMULATE: fold the slab chunk range, then fold each unfolded maker quote.
pub async fn accumulate(ctx: &KeeperCtx, chunks: Vec<(u32, u32)>, quotes: Vec<Pubkey>) {
    let cranker = ctx.cranker.pubkey();
    for (start, count) in chunks {
        let ix = ix::process_chunk(&ctx.pdas, cranker, start, count);
        send_one(ctx, &[ix], "process_chunk").await;
    }
    for quote in quotes {
        let ix = ix::process_maker_quote(&ctx.pdas, cranker, quote);
        send_one(ctx, &[ix], "process_maker_quote").await;
    }
}

/// DISCOVER: publish the cross. The crank fee is left uncollected (None) — the
/// program no-ops it; a fee-collecting deployment can wire the accounts here.
pub async fn discover(ctx: &KeeperCtx) {
    let ix = ix::finalize_clear(&ctx.pdas, ctx.cranker.pubkey(), None);
    send_one(ctx, &[ix], "finalize_clear").await;
}

/// The optional money-path accounts for one order's owner. `position` is always
/// supplied (the program requires it for a non-zero fill); collateral/vault only
/// when the market has a declared money path.
fn settle_money(ctx: &KeeperCtx, trader: &Pubkey) -> SettleMoney {
    let position = Some(pda::position(&ctx.pdas.market, trader).0);
    let (user_collateral, vault) = match ctx.collateral_mint {
        Some(_) => (Some(pda::user_collateral(trader).0), ctx.vault),
        None => (None, None),
    };
    SettleMoney {
        position,
        user_collateral,
        vault,
        integrator_collateral: None,
    }
}

/// SETTLE: pull each accumulated order's fill (bounded concurrency), then settle
/// each folded-not-settled maker quote (serial; small N).
pub async fn settle(ctx: &KeeperCtx, orders: Vec<SlabOrder>, quotes: Vec<Pubkey>) {
    let cranker = ctx.cranker.pubkey();
    let started = std::time::Instant::now();
    stream::iter(orders)
        .for_each_concurrent(ctx.settle_concurrency, |order| async move {
            let money = settle_money(ctx, &order.trader);
            let ix = ix::settle_fill(&ctx.pdas, cranker, order.order_id, order.slot, &money);
            send_one(ctx, &[ix], "settle_fill").await;
        })
        .await;
    metrics::histogram!("keeper_settle_latency_seconds").record(started.elapsed().as_secs_f64());

    for quote in quotes {
        // The maker's Position is required; collateral/vault only on a money market.
        let view = ctx
            .client
            .fetch_account_data(&quote)
            .await
            .ok()
            .and_then(|d| tempo_sdk::accounts::MakerQuoteView::decode(&d).ok());
        let Some(view) = view else { continue };
        let position = pda::position(&ctx.pdas.market, &view.maker).0;
        let (user_collateral, vault) = match ctx.collateral_mint {
            Some(_) => (Some(pda::user_collateral(&view.maker).0), ctx.vault),
            None => (None, None),
        };
        let ix =
            ix::settle_maker_quote(&ctx.pdas, cranker, quote, position, user_collateral, vault);
        send_one(ctx, &[ix], "settle_maker_quote").await;
    }
}

/// ROLL: open the next round (only reached when slab empty + quotes settled).
pub async fn roll(ctx: &KeeperCtx, oracle: Pubkey) {
    let ix = ix::start_auction(&ctx.pdas, ctx.cranker.pubkey(), oracle);
    send_one(ctx, &[ix], "start_auction").await;
}
