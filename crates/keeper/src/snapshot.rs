use std::sync::Arc;

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;

use tempo_sdk::accounts::{
    decode_slab_orders, ClearingResultView, MakerQuoteView, MarketView, SlabOrder,
    STATUS_ACCUMULATED, STATUS_RESTING,
};
use tempo_sdk::{MarketPdas, SdkError, TempoClient};

/// The shared "is this resting order reapable by a permissionless (non-owner) crank
/// THIS round?" rule (DDR-3 Correction-2 item 4). The on-chain `cancel_order` reap
/// boundary is STRICT `<` — an order is reapable by a non-owner only AFTER its last
/// active round — so the keeper (a non-owner reaper) must match it exactly, or it
/// would fire `cancel_order` txs the program rejects for an order still in its active
/// round. `0` = good-till-cancelled (never reapable). Applied per shard in
/// `actions::reap` and in `expired_resting_orders`.
pub fn is_reapable(o: &SlabOrder, round: u64) -> bool {
    o.status == STATUS_RESTING && o.expires_at_auction != 0 && o.expires_at_auction < round
}

/// Everything the keeper reads from chain in one tick. Plain data — `decide` is a
/// pure function over it, so the whole state machine is unit-testable with no RPC.
pub struct MarketSnapshot {
    pub market: MarketView,
    pub slab: Vec<SlabOrder>,
    pub clearing: Option<ClearingResultView>,
    pub quotes: Vec<(Pubkey, MakerQuoteView)>,
}

impl MarketSnapshot {
    /// Read the market, ALL shard slabs, clearing result, and maker quotes for one tick.
    ///
    /// Stage A multi-shard: the slab is `num_slab_shards` independent accounts. We load
    /// every shard and stamp each decoded order with its `shard_id`, so `all_resting_folded`
    /// (completeness), `accumulated_orders` (settle), and `expired_resting_orders` (reap) see
    /// the WHOLE book, and `settle_fill` can target the order's own shard. The shard count is
    /// read from the market itself (fetched first), so no caller needs to know it.
    pub async fn load(client: &TempoClient, pdas: &MarketPdas) -> Result<Self, SdkError> {
        let market = client.fetch_market(&pdas.market).await?;
        let mut slab: Vec<SlabOrder> = Vec::new();
        for shard_id in 0..market.num_slab_shards {
            let data = client
                .fetch_account_data(&pdas.slab_shard(shard_id))
                .await?;
            let mut orders = decode_slab_orders(&data)?;
            for o in &mut orders {
                o.shard_id = shard_id;
            }
            slab.extend(orders);
        }
        let clearing = match client.fetch_account_data_opt(&pdas.clearing).await? {
            Some(data) => ClearingResultView::decode(&data).ok(),
            None => None,
        };
        let quotes = client.fetch_maker_quotes(&pdas.market).await?;
        Ok(Self {
            market,
            slab,
            clearing,
            quotes,
        })
    }

    /// Completeness mirror of the on-chain `all_active_orders_accumulated` (DDR-3):
    /// every resting order that *can* fold this round has been folded. A resting
    /// order whose fixed price left the recentered window is exempt **iff it is
    /// passive** (SELL above the top / BUY below the floor) — the window moved away
    /// from it, so it legitimately can't fold now (on-chain, `process_chunk` skips it
    /// and finalize's gate exempts it). Without this the keeper would loop in
    /// ACCUMULATING forever, re-cranking a shard whose only unfolded order is passive
    /// and can never reach `Accumulated`.
    ///
    /// FIX C (DDR-3): the passive test is the SHARED, overflow-safe
    /// `tempo_math::resting::is_passive` mirror of the program's `classify_resting_fold`
    /// — never a hand copy (the old inline `num_ticks * tick_size` could overflow and
    /// drift from the on-chain rule).
    pub fn all_resting_folded(&self) -> bool {
        let floor = self.market.window_floor_price;
        let tick_size = self.market.tick_size;
        let num_ticks = self.market.num_ticks;
        let current = self.market.current_auction_id;
        self.slab.iter().all(|o| {
            if o.status != STATUS_RESTING {
                return true;
            }
            // DDR-4 (always-open submission): an order armed for a later round was
            // submitted mid-round and can't fold until then, so it does NOT block this
            // round's completeness (mirrors the on-chain gate's `arm > current` exemption).
            // Without this the keeper would loop in ACCUMULATING forever on a deferred order.
            if o.arm_auction_id > current {
                return true;
            }
            // side: 1 = Sell, 0 = Buy (mirrors OrderSide). A passive order can't fold
            // this round, so it does NOT block completeness; anything else must fold.
            tempo_math::resting::is_passive(o.price, o.side == 1, floor, tick_size, num_ticks)
        })
    }

    /// Resting orders whose expiry has passed (DDR-3 correction #2). A passive expired
    /// order is never folded, so `settle_fill` (the only place expiry is otherwise
    /// handled) never runs on it and its `reserved_margin` would be locked forever. The
    /// keeper reaps each via the permissionless `cancel_order` as an operational duty;
    /// the released margin always returns to the owner's ledger.
    ///
    /// The snapshot now loads ALL shards (each order stamped with its `shard_id`), so this
    /// sees the whole book. `actions::reap` still does its own independent per-shard load and
    /// fires the cancels (it needs each order's shard id to build the tx, and stays correct
    /// even without a snapshot); this method backs the fingerprint/tests.
    pub fn expired_resting_orders(&self) -> Vec<SlabOrder> {
        let round = self.market.current_auction_id;
        self.slab
            .iter()
            .filter(|o| is_reapable(o, round))
            .copied()
            .collect()
    }

    /// Maker quotes that still need a `process_maker_quote` this round.
    pub fn unfolded_quotes(&self) -> Vec<Pubkey> {
        let round = self.market.current_auction_id;
        self.quotes
            .iter()
            .filter(|(_, q)| q.needs_fold(round))
            .map(|(k, _)| *k)
            .collect()
    }

    /// Maker quotes folded this round but not yet settled — they must settle before
    /// the round may roll (rolling resets `folded_maker_quote_count` + zeroes the
    /// histogram, which would strand the fill).
    pub fn unsettled_folded_quotes(&self) -> Vec<Pubkey> {
        let round = self.market.current_auction_id;
        self.quotes
            .iter()
            .filter(|(_, q)| q.needs_settle(round))
            .map(|(k, _)| *k)
            .collect()
    }

    /// Slab orders folded into the histogram and awaiting their fill.
    pub fn accumulated_orders(&self) -> Vec<SlabOrder> {
        self.slab
            .iter()
            .filter(|o| o.status == STATUS_ACCUMULATED)
            .copied()
            .collect()
    }

    /// A hash of the fields that advance with clearing progress (round, phase,
    /// accumulated/folded counts, live slot count). Used by the freeze watchdog to
    /// detect "no progress in N slots". PERF-1 removed the market's accumulated-order
    /// mirror, so the accumulation-progress term is derived from the slab itself (the
    /// count of folded — non-`Resting` — slots advances exactly as folding proceeds).
    pub fn fingerprint(&self) -> u64 {
        let m = &self.market;
        let folded_orders = self
            .slab
            .iter()
            .filter(|o| o.status != STATUS_RESTING)
            .count() as u64;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a offset basis
        for v in [
            m.current_auction_id,
            m.phase as u64,
            folded_orders,
            m.folded_maker_quote_count,
            self.slab.len() as u64,
        ] {
            h ^= v;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

/// Shared context passed to every action. Clone is cheap (all `Arc`/`Copy`), so the
/// keeper loop, funding task, and settle fan-out share one context.
#[derive(Clone)]
pub struct KeeperCtx {
    pub client: Arc<TempoClient>,
    pub cranker: Arc<Keypair>,
    pub pdas: MarketPdas,
    pub collateral_mint: Option<Pubkey>,
    pub vault: Option<Pubkey>,
    pub chunk_size: u32,
    pub settle_concurrency: usize,
}
