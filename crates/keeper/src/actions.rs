use futures::stream::{self, StreamExt};
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use tempo_sdk::accounts::SlabOrder;
use tempo_sdk::benign;
use tempo_sdk::ix::{self, SettleMoney};
use tempo_sdk::pda;

use crate::snapshot::{is_reapable, KeeperCtx};

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

/// ACCUMULATE: fold the slab chunk range across every shard, then fold each unfolded
/// maker quote. Folding an empty shard is a cheap no-op, so cranking all shards over the
/// chunk range is correct; refining the per-shard ranges from a full multi-shard snapshot
/// is the deferred A12.3 optimization (the snapshot reads shard 0 today).
pub async fn accumulate(
    ctx: &KeeperCtx,
    chunks: Vec<(u32, u32)>,
    quotes: Vec<Pubkey>,
    num_slab_shards: u16,
) {
    let cranker = ctx.cranker.pubkey();
    for shard_id in 0..num_slab_shards {
        for &(start, count) in &chunks {
            let ix = ix::process_chunk(&ctx.pdas, cranker, shard_id, start, count);
            send_one(ctx, &[ix], "process_chunk").await;
        }
    }
    for quote in quotes {
        let ix = ix::process_maker_quote(&ctx.pdas, cranker, quote);
        send_one(ctx, &[ix], "process_maker_quote").await;
    }
}

/// DISCOVER: publish the cross. Design Z (DDR-1) — finalize scans every shard for
/// completeness, so all `num_slab_shards` shards are passed. The crank fee is left
/// uncollected (None); a fee-collecting deployment can wire the accounts here.
pub async fn discover(ctx: &KeeperCtx, num_slab_shards: u16) {
    let shards = ctx.pdas.all_shards(num_slab_shards);
    let ix = ix::finalize_clear(&ctx.pdas, ctx.cranker.pubkey(), None, &shards);
    send_one(ctx, &[ix], "finalize_clear").await;
}

/// The optional money-path accounts for one order's owner. `position` is attached only
/// when it EXISTS on-chain (`has_position`): the program requires it for a NON-zero fill,
/// but permits consuming a ZERO-fill order WITHOUT one — and attaching an uninitialized
/// (System-owned) position PDA reverts `settle_fill` with "Invalid account owner" on the
/// account check, wedging the round (known-issues §2.16). collateral/vault ride only on a
/// declared money market.
fn settle_money(ctx: &KeeperCtx, trader: &Pubkey, has_position: bool) -> SettleMoney {
    let position = has_position.then(|| pda::position(&ctx.pdas.market, trader).0);
    let (user_collateral, vault) = match ctx.collateral_mint {
        Some(mint) if has_position => (Some(pda::user_collateral(trader, &mint).0), ctx.vault),
        _ => (None, None),
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
            // Attach the owner's position only if it actually exists on-chain (known-issues
            // §2.16): a zero-fill order whose owner has no position settles position-free,
            // and passing an uninitialized position PDA would revert "Invalid account owner"
            // and wedge the round. One getAccount per order, bounded by settle_concurrency.
            let position_pda = pda::position(&ctx.pdas.market, &order.trader).0;
            let has_position = ctx
                .client
                .fetch_account_data_opt(&position_pda)
                .await
                .ok()
                .flatten()
                .is_some();
            let money = settle_money(ctx, &order.trader, has_position);
            // Stage A multi-shard: settle each order against its OWN shard. The snapshot
            // stamps `shard_id` on every order as it loads each shard's slab, so a
            // multi-shard market settles correctly (was hardcoded to shard 0).
            let ix = ix::settle_fill(
                &ctx.pdas,
                cranker,
                order.shard_id,
                order.order_id,
                order.slot,
                &money,
            );
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
            Some(mint) => (Some(pda::user_collateral(&view.maker, &mint).0), ctx.vault),
            None => (None, None),
        };
        let ix =
            ix::settle_maker_quote(&ctx.pdas, cranker, quote, position, user_collateral, vault);
        send_one(ctx, &[ix], "settle_maker_quote").await;
    }
}

/// REAP (DDR-3 correction #2 + Correction-2 item 5): permissionlessly `cancel_order`
/// each expired resting order so a passive order's slot + reserved margin isn't squatted
/// forever (a passive order is never folded, so settle never runs on it). The released
/// margin always returns to the ORDER OWNER's ledger — the program enforces this, so the
/// keeper is a neutral GC. `cancel_order` is only valid in the Collect phase.
///
/// Scans EVERY shard (`[0, num_slab_shards)`): the tick snapshot only loads shard 0, so
/// reap loads each shard's slab here and filters it with the shared `is_reapable` rule,
/// otherwise expired orders on shards `1..N` would leak margin forever on a multi-shard
/// market. Cancels are fired bounded-concurrent (matching `settle`), not N serial
/// round-trips. Reaping an already-freed slot is a benign race (`send_one` swallows it).
pub async fn reap(ctx: &KeeperCtx, round: u64, num_slab_shards: u16) {
    // Load each shard's slab and collect (shard_id, order) for every reapable order.
    // A missing/undecodable shard is skipped (it simply yields nothing to reap).
    let mut targets: Vec<(u16, SlabOrder)> = Vec::new();
    for shard_id in 0..num_slab_shards {
        let slab_key = ctx.pdas.slab_shard(shard_id);
        let data = match ctx.client.fetch_account_data(&slab_key).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(shard = shard_id, error = %e, "reap: failed to load shard slab");
                continue;
            }
        };
        let orders = match tempo_sdk::accounts::decode_slab_orders(&data) {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(shard = shard_id, error = %e, "reap: failed to decode shard slab");
                continue;
            }
        };
        for o in orders.into_iter().filter(|o| is_reapable(o, round)) {
            targets.push((shard_id, o));
        }
    }
    if targets.is_empty() {
        return;
    }

    let cranker = ctx.cranker.pubkey();
    stream::iter(targets)
        .for_each_concurrent(ctx.settle_concurrency, |(shard_id, order)| async move {
            // The OWNER's collateral ledger (money-path markets only); the program checks
            // it belongs to order.trader, so a reaper can't substitute its own.
            let user_collateral = ctx
                .collateral_mint
                .map(|mint| pda::user_collateral(&order.trader, &mint).0);
            let ix = ix::cancel_order(
                &ctx.pdas,
                cranker,
                shard_id,
                order.order_id,
                order.slot,
                user_collateral,
            );
            send_one(ctx, &[ix], "cancel_order").await;
        })
        .await;
}

/// ROLL: drain+re-arm EVERY shard, then open the next round (only reached when the slab
/// is empty + quotes settled). Stage A: `start_auction` gates on every shard being reset
/// (`shards_ready == num_slab_shards`), so every `reset_shard` must run before it. Resetting
/// an already-reset shard is a benign idempotent no-op (a dropped reset simply retries next
/// tick), so re-emitting all shards each roll is safe.
pub async fn roll(ctx: &KeeperCtx, oracle: Pubkey, num_slab_shards: u16) {
    let cranker = ctx.cranker.pubkey();
    for shard_id in 0..num_slab_shards {
        let reset = ix::reset_shard(&ctx.pdas, cranker, shard_id);
        send_one(ctx, &[reset], "reset_shard").await;
    }
    let ix = ix::start_auction(&ctx.pdas, cranker, oracle);
    send_one(ctx, &[ix], "start_auction").await;
}
