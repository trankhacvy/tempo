use serde::Serialize;
use tempo_sdk::Pubkey;
use utoipa::ToSchema;

use tempo_sdk::accounts::{
    ClearingResultView, HistogramView, MakerQuoteView, MarketView, PositionView, SlabOrder,
};

use crate::state::LiveState;

/// Market control block + derived window bounds. `u64`/`u128` are stringified to
/// survive JS number precision (the UI is JS).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct MarketResponse {
    pub version: u8,
    pub current_auction_id: u64,
    pub phase: u8,
    pub phase_deadline_slot: u64,
    pub tick_size: String,
    pub num_ticks: u32,
    pub window_floor_price: String,
    pub window_top_price: String,
    pub orders_per_auction_cap: u32,
    pub active_order_count: u64,
    pub accumulated_order_count: u64,
    pub active_maker_quote_count: u64,
    pub folded_maker_quote_count: u64,
    pub maintenance_margin_bps: u16,
    pub initial_margin_bps: u16,
    pub max_position_notional: String,
    pub oracle: String,
    pub collateral_mint: String,
    pub last_funding_ts: u64,
}

impl From<&MarketView> for MarketResponse {
    fn from(m: &MarketView) -> Self {
        Self {
            version: m.version,
            current_auction_id: m.current_auction_id,
            phase: m.phase,
            phase_deadline_slot: m.phase_deadline_slot,
            tick_size: m.tick_size.to_string(),
            num_ticks: m.num_ticks,
            window_floor_price: m.window_floor_price.to_string(),
            window_top_price: m.window_top_price().to_string(),
            orders_per_auction_cap: m.orders_per_auction_cap,
            active_order_count: m.active_order_count,
            accumulated_order_count: m.accumulated_order_count,
            active_maker_quote_count: m.active_maker_quote_count,
            folded_maker_quote_count: m.folded_maker_quote_count,
            maintenance_margin_bps: m.maintenance_margin_bps,
            initial_margin_bps: m.effective_initial_margin_bps(),
            max_position_notional: m.max_position_notional.to_string(),
            oracle: m.oracle.to_string(),
            collateral_mint: m.collateral_mint.to_string(),
            last_funding_ts: m.last_funding_ts,
        }
    }
}

/// Live auction phase + countdown, for the phase-timeline widget.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AuctionResponse {
    pub auction_id: u64,
    pub phase: u8,
    pub phase_name: &'static str,
    pub phase_deadline_slot: u64,
    pub current_slot: u64,
    pub slots_remaining: u64,
    pub active_order_count: u64,
    pub accumulated_order_count: u64,
    pub active_maker_quote_count: u64,
    pub folded_maker_quote_count: u64,
}

fn phase_name(phase: u8) -> &'static str {
    match phase {
        0 => "Collect",
        1 => "Accumulating",
        2 => "Discovered",
        3 => "Settling",
        _ => "Unknown",
    }
}

impl AuctionResponse {
    pub fn from_live(live: &LiveState) -> Self {
        let m = &live.market;
        Self {
            auction_id: m.current_auction_id,
            phase: m.phase,
            phase_name: phase_name(m.phase),
            phase_deadline_slot: m.phase_deadline_slot,
            current_slot: live.slot,
            slots_remaining: m.phase_deadline_slot.saturating_sub(live.slot),
            active_order_count: m.active_order_count,
            accumulated_order_count: m.accumulated_order_count,
            active_maker_quote_count: m.active_maker_quote_count,
            folded_maker_quote_count: m.folded_maker_quote_count,
        }
    }
}

/// The published cross (per-side clearing price + marginal tick + matched volume).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CrossResponse {
    pub auction_id: u64,
    pub bid_clearing_price: String,
    pub ask_clearing_price: String,
    pub bid_marginal_tick: u32,
    pub ask_marginal_tick: u32,
    pub bid_matched_volume: String,
    pub ask_matched_volume: String,
}

impl From<&ClearingResultView> for CrossResponse {
    fn from(c: &ClearingResultView) -> Self {
        Self {
            auction_id: c.auction_id,
            bid_clearing_price: c.bid_clearing_price.to_string(),
            ask_clearing_price: c.ask_clearing_price.to_string(),
            bid_marginal_tick: c.bid_marginal_tick,
            ask_marginal_tick: c.ask_marginal_tick,
            bid_matched_volume: c.bid_matched_volume.to_string(),
            ask_matched_volume: c.ask_matched_volume.to_string(),
        }
    }
}

/// The dual-auction histogram + the cross drawn on it — the visual centerpiece.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct HistogramResponse {
    pub auction_id: u64,
    pub num_ticks: u32,
    pub tick_size: String,
    pub window_floor_price: String,
    pub bid_demand: Vec<String>,
    pub bid_supply: Vec<String>,
    pub ask_demand: Vec<String>,
    pub ask_supply: Vec<String>,
    pub cross: Option<CrossResponse>,
}

fn u64_vec(v: &[u64]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

impl HistogramResponse {
    pub fn from_live(live: &LiveState) -> Self {
        let h: &HistogramView = &live.histogram;
        Self {
            auction_id: h.auction_id,
            num_ticks: h.num_ticks,
            tick_size: live.market.tick_size.to_string(),
            window_floor_price: live.market.window_floor_price.to_string(),
            bid_demand: u64_vec(&h.bid_demand),
            bid_supply: u64_vec(&h.bid_supply),
            ask_demand: u64_vec(&h.ask_demand),
            ask_supply: u64_vec(&h.ask_supply),
            cross: live.clearing.as_ref().map(CrossResponse::from),
        }
    }
}

/// One open order in the current round's slab.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct OrderResponse {
    pub slot: u32,
    pub order_id: u64,
    pub trader: String,
    pub side: u8,
    pub status: u8,
    pub price: String,
    pub quantity: String,
}

impl From<&SlabOrder> for OrderResponse {
    fn from(o: &SlabOrder) -> Self {
        Self {
            slot: o.slot,
            order_id: o.order_id,
            trader: o.trader.to_string(),
            side: o.side,
            status: o.status,
            price: o.price.to_string(),
            quantity: o.quantity.to_string(),
        }
    }
}

/// One active maker quote's lifecycle state.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct QuoteResponse {
    pub pubkey: String,
    pub maker: String,
    pub sequence: u64,
    pub mid_tick: u32,
    pub status: u8,
    pub num_bids: u8,
    pub num_asks: u8,
    pub folded_auction_id: u64,
    pub settled_auction_id: u64,
}

impl QuoteResponse {
    pub fn from_pair(pubkey: &Pubkey, q: &MakerQuoteView) -> Self {
        Self {
            pubkey: pubkey.to_string(),
            maker: q.maker.to_string(),
            sequence: q.sequence,
            mid_tick: q.mid_tick,
            status: q.status,
            num_bids: q.num_bids,
            num_asks: q.num_asks,
            folded_auction_id: q.folded_auction_id,
            settled_auction_id: q.settled_auction_id,
        }
    }
}

/// One trader's position in the market.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PositionResponse {
    pub pubkey: String,
    pub owner: String,
    pub size: String,
    pub entry_price: String,
    pub collateral: String,
    pub realized_pnl: String,
    pub margin_mode: u8,
}

impl PositionResponse {
    pub fn from_pair(pubkey: &Pubkey, p: &PositionView) -> Self {
        Self {
            pubkey: pubkey.to_string(),
            owner: p.owner.to_string(),
            size: p.size.to_string(),
            entry_price: p.entry_price.to_string(),
            collateral: p.collateral.to_string(),
            realized_pnl: p.realized_pnl.to_string(),
            margin_mode: p.margin_mode,
        }
    }
}
