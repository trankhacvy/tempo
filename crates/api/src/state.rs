use std::sync::Arc;

use arc_swap::ArcSwapOption;
use tempo_sdk::Pubkey;
use tokio::sync::broadcast;

use tempo_sdk::accounts::{
    ClearingResultView, HistogramView, MakerQuoteView, MarketView, PositionView, SlabOrder,
};
use tempo_sdk::{MarketPdas, SdkError, TempoClient};

use crate::history::HistorySource;

/// Fast-changing auction state rebuilt every watcher tick (~400ms). Positions
/// are kept separate because their GPA scan is expensive and runs on its own
/// slower cadence (`run_positions`, default every 5s).
#[derive(Clone, Debug)]
pub struct LiveState {
    pub slot: u64,
    pub market: MarketView,
    pub histogram: HistogramView,
    pub clearing: Option<ClearingResultView>,
    pub orders: Vec<SlabOrder>,
    pub quotes: Vec<(Pubkey, MakerQuoteView)>,
    pub fetched_at_unix: i64,
}

impl LiveState {
    /// Read market, histogram, clearing result, slab, and maker quotes for one
    /// watcher tick. Positions are NOT fetched here — see `run_positions`.
    pub async fn load(
        client: &TempoClient,
        pdas: &MarketPdas,
        slot: u64,
        fetched_at_unix: i64,
    ) -> Result<Self, SdkError> {
        let (market, histogram, clearing, slab_data, quotes) = tokio::try_join!(
            client.fetch_market(&pdas.market),
            client.fetch_histogram(&pdas.histogram),
            client.fetch_clearing(&pdas.clearing),
            client.fetch_account_data(&pdas.order_slab),
            client.fetch_maker_quotes(&pdas.market),
        )?;
        let orders = tempo_sdk::accounts::decode_slab_orders(&slab_data)?;
        Ok(Self {
            slot,
            market,
            histogram,
            clearing,
            orders,
            quotes,
            fetched_at_unix,
        })
    }

    /// FNV-1a hash of the auction-progress fields. The watcher broadcasts a WS
    /// update only when this changes, so idle polls do not spam subscribers.
    pub fn fingerprint(&self) -> u64 {
        let m = &self.market;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let clearing_vol = self
            .clearing
            .map(|c| c.bid_matched_volume ^ c.ask_matched_volume.rotate_left(1))
            .unwrap_or(0);
        for v in [
            m.current_auction_id,
            m.phase as u64,
            m.accumulated_order_count,
            m.active_order_count,
            m.folded_maker_quote_count,
            self.orders.len() as u64,
            self.quotes.len() as u64,
            clearing_vol,
        ] {
            h ^= v;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

/// Shared application state. Cheap to clone (all `Arc`). Handlers never call
/// RPC — they read `live` and `positions`; only the watcher tasks touch the client.
#[derive(Clone)]
pub struct AppState {
    pub market: Pubkey,
    pub pdas: MarketPdas,
    pub client: Arc<TempoClient>,
    pub live: Arc<ArcSwapOption<LiveState>>,
    /// Populated by `run_positions` on a slower cadence than `live`. May be
    /// empty (`None`) before the first position scan completes.
    pub positions: Arc<ArcSwapOption<Vec<(Pubkey, PositionView)>>>,
    pub updates: broadcast::Sender<Arc<LiveState>>,
    pub history: Arc<dyn HistorySource>,
}

impl AppState {
    /// The current live snapshot, or `NotReady` before the first successful poll.
    pub fn snapshot(&self) -> Result<Arc<LiveState>, crate::error::ApiError> {
        self.live
            .load_full()
            .ok_or(crate::error::ApiError::NotReady)
    }

    /// Current position list, or an empty vec before the first position scan.
    pub fn positions_snapshot(&self) -> Vec<(Pubkey, PositionView)> {
        self.positions
            .load_full()
            .map(|arc| (*arc).clone())
            .unwrap_or_default()
    }
}
