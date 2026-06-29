use std::sync::Arc;

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;

use tempo_sdk::accounts::{
    decode_slab_orders, ClearingResultView, MakerQuoteView, MarketView, SlabOrder,
    STATUS_ACCUMULATED, STATUS_RESTING,
};
use tempo_sdk::{MarketPdas, SdkError, TempoClient};

/// Everything the keeper reads from chain in one tick. Plain data — `decide` is a
/// pure function over it, so the whole state machine is unit-testable with no RPC.
pub struct MarketSnapshot {
    pub market: MarketView,
    pub slab: Vec<SlabOrder>,
    pub clearing: Option<ClearingResultView>,
    pub quotes: Vec<(Pubkey, MakerQuoteView)>,
}

impl MarketSnapshot {
    /// Read the market, slab, clearing result, and maker quotes for one tick.
    pub async fn load(client: &TempoClient, pdas: &MarketPdas) -> Result<Self, SdkError> {
        let market = client.fetch_market(&pdas.market).await?;
        let slab = decode_slab_orders(&client.fetch_account_data(&pdas.order_slab).await?)?;
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

    /// Completeness mirror of the on-chain `all_active_orders_accumulated`: no slab
    /// order is still `Resting`.
    pub fn all_resting_folded(&self) -> bool {
        self.slab.iter().all(|o| o.status != STATUS_RESTING)
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
