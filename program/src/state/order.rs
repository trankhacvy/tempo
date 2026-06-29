use alloc::vec;
use alloc::vec::Vec;
use codama::{CodamaAccount, CodamaType};
use pinocchio::{cpi::Seed, error::ProgramError, Address};

use crate::errors::TempoProgramError;
use crate::traits::{
    AccountDeserialize, AccountSerialize, AccountSize, Discriminator, PdaAccount, PdaSeeds,
    TempoAccountDiscriminators, Versioned,
};
use crate::{assert_no_padding, le_field};

/// Order side.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderSide {
    Buy = 0,
    Sell = 1,
}

impl OrderSide {
    #[inline(always)]
    pub fn from_u8(value: u8) -> Result<Self, ProgramError> {
        match value {
            0 => Ok(Self::Buy),
            1 => Ok(Self::Sell),
            _ => Err(TempoProgramError::InvalidOrderSide.into()),
        }
    }
}

/// Lifecycle of a slot in the order slab.
///
/// `Empty` → free slot. `Resting` → live, eligible to be accumulated.
/// `Accumulated` → folded into the histogram exactly once (clearing-protocol
/// §4.2 completeness). `Consumed` → settled (fill computed in Phase 3).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderStatus {
    Empty = 0,
    Resting = 1,
    Accumulated = 2,
    Consumed = 3,
}

impl OrderStatus {
    #[inline(always)]
    pub fn from_u8(value: u8) -> Result<Self, ProgramError> {
        match value {
            0 => Ok(Self::Empty),
            1 => Ok(Self::Resting),
            2 => Ok(Self::Accumulated),
            3 => Ok(Self::Consumed),
            _ => Err(TempoProgramError::InvalidOrderStatus.into()),
        }
    }
}

/// A single order slot in the slab.
///
/// # Layout (`#[repr(C)]`, no padding)
/// 4 × u64 (32) + Address (32) + 3 × u8 (3) + 5 pad + u64 `cum_before` (8) +
/// u64 `reserved_margin` (8) = 88.
#[derive(Clone, Copy, Debug, PartialEq, CodamaType)]
#[repr(C)]
pub struct Order {
    // --- u64 block ---
    pub price: u64,
    pub quantity: u64,
    pub remaining: u64,
    pub order_id: u64,

    // --- Address block ---
    pub trader: Address,

    // --- u8 block ---
    /// `OrderSide` (0 = buy, 1 = sell).
    pub side: u8,
    /// **Always 0** for slab orders: `submit_order` is taker-only (known-issues
    /// §1.3 / Option A). Retained for layout/event parity with `FillSettled`
    /// (whose `is_maker` is still set to 1 by the `MakerQuote` settle path); the
    /// slab itself only ever carries taker flow, so it is never written non-zero.
    pub is_maker: u8,
    /// `OrderStatus`.
    pub status: u8,
    /// Explicit padding so `cum_before` lands 8-aligned.
    pub _padding: [u8; 5],
    /// Fold-time prefix snapshot (known-issues §2.7): the region/tick histogram
    /// bucket value captured immediately *before* this order folded — i.e. the
    /// Σ `remaining` of same-bucket orders folded earlier, in fold order.
    /// `settle_fill` reads this for O(1) marginal-tick rationing instead of
    /// re-scanning the whole slab. `0` until folded
    /// (`process_chunk` sets it when it marks the order `Accumulated`). Appended
    /// last so the earlier field offsets stay stable.
    pub cum_before: u64,
    /// Worst-case initial margin **reserved (locked) at submit** for this order
    /// (missing-features §1.1). Guarantees the order can always settle: a matched
    /// trade locks at most this much, so settle only ever *releases* — it never
    /// reverts for lack of collateral, which would wedge the round. `cancel_order`
    /// and `settle_fill` release exactly this amount back to the owner's ledger.
    /// `0` for a no-money-path (clearing-benchmark) market. Appended last so the
    /// earlier field offsets stay stable.
    pub reserved_margin: u64,
}

assert_no_padding!(Order, 8 * 4 + 32 + 1 + 1 + 1 + 5 + 8 + 8);

/// Byte length of one `Order` slot.
pub const ORDER_LEN: usize = 8 * 4 + 32 + 3 + 5 + 8 + 8;

impl Order {
    #[inline(always)]
    pub fn empty() -> Self {
        Self {
            price: 0,
            quantity: 0,
            remaining: 0,
            order_id: 0,
            trader: Address::new_from_array([0u8; 32]),
            side: OrderSide::Buy as u8,
            is_maker: 0,
            status: OrderStatus::Empty as u8,
            _padding: [0u8; 5],
            cum_before: 0,
            reserved_margin: 0,
        }
    }

    /// Build a resting (taker) order. There is no `is_maker` parameter:
    /// `submit_order` is taker-only (known-issues §1.3), so a slab order's
    /// `is_maker` is always 0 — maker liquidity lives in the `MakerQuote` book.
    #[inline(always)]
    pub fn new_resting(
        order_id: u64,
        trader: Address,
        side: OrderSide,
        price: u64,
        quantity: u64,
    ) -> Self {
        Self {
            price,
            quantity,
            remaining: quantity,
            order_id,
            trader,
            side: side as u8,
            is_maker: 0,
            status: OrderStatus::Resting as u8,
            _padding: [0u8; 5],
            cum_before: 0,
            // Set by `submit_order` after it computes the worst-case reservation.
            reserved_margin: 0,
        }
    }

    #[inline(always)]
    pub fn status(&self) -> Result<OrderStatus, ProgramError> {
        OrderStatus::from_u8(self.status)
    }

    #[inline(always)]
    pub fn side(&self) -> Result<OrderSide, ProgramError> {
        OrderSide::from_u8(self.side)
    }

    #[inline(always)]
    pub fn to_bytes(&self) -> [u8; ORDER_LEN] {
        let mut buf = [0u8; ORDER_LEN];
        buf[0..8].copy_from_slice(&self.price.to_le_bytes());
        buf[8..16].copy_from_slice(&self.quantity.to_le_bytes());
        buf[16..24].copy_from_slice(&self.remaining.to_le_bytes());
        buf[24..32].copy_from_slice(&self.order_id.to_le_bytes());
        buf[32..64].copy_from_slice(self.trader.as_ref());
        buf[64] = self.side;
        buf[65] = self.is_maker;
        buf[66] = self.status;
        // bytes 67..72 are padding, already zero
        buf[72..80].copy_from_slice(&self.cum_before.to_le_bytes());
        buf[80..88].copy_from_slice(&self.reserved_margin.to_le_bytes());
        buf
    }

    #[inline(always)]
    pub fn from_bytes(buf: &[u8]) -> Result<Self, ProgramError> {
        if buf.len() < ORDER_LEN {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let mut trader = [0u8; 32];
        trader.copy_from_slice(&buf[32..64]);
        Ok(Self {
            price: u64::from_le_bytes(buf[0..8].try_into().unwrap()),
            quantity: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
            remaining: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
            order_id: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
            trader: Address::new_from_array(trader),
            side: buf[64],
            is_maker: buf[65],
            status: buf[66],
            _padding: [0u8; 5],
            cum_before: u64::from_le_bytes(buf[72..80].try_into().unwrap()),
            reserved_margin: u64::from_le_bytes(buf[80..88].try_into().unwrap()),
        })
    }
}

/// `OrderSlab` header — a bounded slab of order slots for one market
/// (system-design §6.3). Orders rest here in the `Collect` phase; Phase 1 folds
/// them into the histogram and marks them accumulated.
///
/// # On-disk layout
/// ```text
/// [ disc | version | OrderSlabHeader | Order[0] | Order[1] | ... | Order[cap-1] ]
///   1      1         HEADER_LEN (64)   ORDER_LEN  ORDER_LEN        ORDER_LEN
/// ```
/// Total account size = 2 + HEADER_LEN + capacity*ORDER_LEN. Slots are accessed
/// via the helpers below on the full account data slice (capacity is dynamic
/// per market, so slots are not fields of the header).
///
/// # Header layout (`#[repr(C)]`, **alignment 1** — see `le_field!`)
/// 2 × [u8;8] (16) + 3 × [u8;4] (12) + Address (32) + u8 (1) = 61.
#[derive(Clone, Debug, PartialEq, CodamaAccount)]
#[codama(field("discriminator", number(u8), default_value = 4))]
#[codama(discriminator(field = "discriminator"))]
#[codama(seed(type = string(utf8), value = "order_slab"))]
#[codama(seed(name = "market", type = public_key))]
#[repr(C)]
pub struct OrderSlabHeader {
    /// Auction round the resting orders belong to.
    pub auction_id_le: [u8; 8],
    /// Monotonic id assigned to the next submitted order.
    pub next_order_id_le: [u8; 8],
    /// Maximum number of slots (orders-per-auction cap).
    pub capacity_le: [u8; 4],
    /// Number of currently-resting/active orders (not yet consumed).
    pub count_le: [u8; 4],
    /// Market this slab belongs to.
    pub market: Address,
    /// PDA bump.
    pub bump: u8,
    /// Forward allocation cursor (known-issues §2.7): the slot index
    /// `submit_order` tries first, making the common forward-fill allocation O(1)
    /// instead of a full Empty-slot scan. Reset to 0 at every round roll. Cancels
    /// leave holes below the cursor; `find_free_slot` wraps to reclaim them once
    /// the tail is full, so correctness never depends on the hint — it is only a
    /// starting point, and a stale hint costs at most one extra scan. Appended last
    /// so the `market`/`bump` offsets stay stable.
    pub next_free_hint_le: [u8; 4],
}

assert_no_padding!(OrderSlabHeader, 8 * 2 + 4 * 3 + 32 + 1);

impl Discriminator for OrderSlabHeader {
    const DISCRIMINATOR: u8 = TempoAccountDiscriminators::OrderSlabDiscriminator as u8;
}

impl Versioned for OrderSlabHeader {
    // v3: widened `Order` again to carry `reserved_margin` (ORDER_LEN 80 → 88), the
    // worst-case margin locked at submit (missing-features §1.1). The slot region size
    // changed, so a pre-v3 slab is incompatible and must be re-provisioned (dev-phase:
    // free); the version bump fails a stale slab loudly.
    //
    // v2: appended `next_free_hint` (O(1) alloc cursor) AND widened `Order` to carry
    // the fold-time `cum_before` snapshot (ORDER_LEN 72 → 80) — both known-issues §2.7.
    const VERSION: u8 = 3;
}

impl AccountSize for OrderSlabHeader {
    const DATA_LEN: usize = 8 * 2 + 4 * 3 + 32 + 1;
}

impl AccountDeserialize for OrderSlabHeader {}

impl AccountSerialize for OrderSlabHeader {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(&self.auction_id_le);
        data.extend_from_slice(&self.next_order_id_le);
        data.extend_from_slice(&self.capacity_le);
        data.extend_from_slice(&self.count_le);
        data.extend_from_slice(self.market.as_ref());
        data.push(self.bump);
        data.extend_from_slice(&self.next_free_hint_le);
        data
    }
}

impl PdaSeeds for OrderSlabHeader {
    const PREFIX: &'static [u8] = b"order_slab";

    #[inline(always)]
    fn seeds(&self) -> Vec<&[u8]> {
        vec![Self::PREFIX, self.market.as_ref()]
    }

    #[inline(always)]
    fn seeds_with_bump<'a>(&'a self, bump: &'a [u8; 1]) -> Vec<Seed<'a>> {
        vec![
            Seed::from(Self::PREFIX),
            Seed::from(self.market.as_ref()),
            Seed::from(bump.as_slice()),
        ]
    }
}

impl PdaAccount for OrderSlabHeader {
    #[inline(always)]
    fn bump(&self) -> u8 {
        self.bump
    }
}

impl OrderSlabHeader {
    le_field!(auction_id, set_auction_id, auction_id_le, u64);
    le_field!(next_order_id, set_next_order_id, next_order_id_le, u64);
    le_field!(capacity, set_capacity, capacity_le, u32);
    le_field!(count, set_count, count_le, u32);
    le_field!(next_free_hint, set_next_free_hint, next_free_hint_le, u32);

    #[inline(always)]
    pub fn new(bump: u8, market: Address, auction_id: u64, capacity: u32) -> Self {
        Self {
            auction_id_le: auction_id.to_le_bytes(),
            next_order_id_le: 0u64.to_le_bytes(),
            capacity_le: capacity.to_le_bytes(),
            count_le: 0u32.to_le_bytes(),
            market,
            bump,
            next_free_hint_le: 0u32.to_le_bytes(),
        }
    }

    #[inline(always)]
    pub fn account_size(capacity: u32) -> usize {
        Self::LEN + Self::slots_len(capacity)
    }

    #[inline(always)]
    pub fn slots_len(capacity: u32) -> usize {
        capacity as usize * ORDER_LEN
    }

    #[inline(always)]
    pub const fn slots_offset() -> usize {
        Self::LEN
    }
}

// ---------------------------------------------------------------------------
// Slot access helpers (operate on the full account data slice).
// ---------------------------------------------------------------------------

#[inline(always)]
fn slot_range(capacity: u32, index: u32) -> Option<core::ops::Range<usize>> {
    if index >= capacity {
        return None;
    }
    let start = OrderSlabHeader::slots_offset() + index as usize * ORDER_LEN;
    Some(start..start + ORDER_LEN)
}

/// Read the order at `index` from the full account data slice.
#[inline(always)]
pub fn read_order(data: &[u8], capacity: u32, index: u32) -> Result<Order, ProgramError> {
    let range = slot_range(capacity, index).ok_or(ProgramError::AccountDataTooSmall)?;
    Order::from_bytes(&data[range])
}

/// Write the order at `index` into the full account data slice.
#[inline(always)]
pub fn write_order(
    data: &mut [u8],
    capacity: u32,
    index: u32,
    order: &Order,
) -> Result<(), ProgramError> {
    let range = slot_range(capacity, index).ok_or(ProgramError::AccountDataTooSmall)?;
    data[range].copy_from_slice(&order.to_bytes());
    Ok(())
}

/// Find a free (`Empty`) slot index, starting at `hint` and wrapping (known-issues
/// §2.7). The common forward-fill path returns `hint` immediately (O(1)); the wrap
/// to `[0, hint)` reclaims holes left by cancels once the tail is full, so a stale
/// or out-of-range hint only ever costs an extra scan — never a wrong result or a
/// spurious `OrderSlabFull`.
#[inline(always)]
pub fn find_free_slot(data: &[u8], capacity: u32, hint: u32) -> Result<u32, ProgramError> {
    if capacity == 0 {
        return Err(TempoProgramError::OrderSlabFull.into());
    }
    let start = hint.min(capacity - 1);
    for offset in 0..capacity {
        let i = {
            let raw = start + offset;
            if raw >= capacity {
                raw - capacity
            } else {
                raw
            }
        };
        let order = read_order(data, capacity, i)?;
        if order.status == OrderStatus::Empty as u8 {
            return Ok(i);
        }
    }
    Err(TempoProgramError::OrderSlabFull.into())
}

/// Find the slot index holding `order_id` among non-empty slots.
#[inline(always)]
pub fn find_order_by_id(data: &[u8], capacity: u32, order_id: u64) -> Result<u32, ProgramError> {
    for i in 0..capacity {
        let order = read_order(data, capacity, i)?;
        if order.status != OrderStatus::Empty as u8 && order.order_id == order_id {
            return Ok(i);
        }
    }
    Err(TempoProgramError::OrderNotFound.into())
}

/// Find the slot holding `order_id`, trying `hint` first (known-issues §2.7).
///
/// O(1) on the happy path: if the slot at `hint` is non-empty and carries the
/// requested `order_id`, it is returned without a scan. The hint is **validated,
/// never trusted** — a stale, wrong, or out-of-range hint simply falls back to the
/// full [`find_order_by_id`] scan, so a malicious or out-of-date client value can
/// never settle/cancel the wrong order; it only forfeits the speedup.
#[inline(always)]
pub fn find_order_by_id_hinted(
    data: &[u8],
    capacity: u32,
    order_id: u64,
    hint: u32,
) -> Result<u32, ProgramError> {
    if hint < capacity {
        let order = read_order(data, capacity, hint)?;
        if order.status != OrderStatus::Empty as u8 && order.order_id == order_id {
            return Ok(hint);
        }
    }
    find_order_by_id(data, capacity, order_id)
}

/// The completeness source of truth (clearing-protocol §4.2): every non-empty
/// slot has been folded — none is still `Resting`. `finalize_clear` requires this
/// directly off the slab, so the censorship guarantee never rests on the
/// hand-maintained order counters alone (known-issues §2.1).
#[inline(always)]
pub fn all_active_orders_accumulated(data: &[u8], capacity: u32) -> Result<bool, ProgramError> {
    for i in 0..capacity {
        let order = read_order(data, capacity, i)?;
        if order.status == OrderStatus::Resting as u8 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// One pass over the slab for `submit_order`: returns `(resting_count,
/// same_side_remaining)` for `trader` — the number of their still-`Resting` orders
/// (any side; the anti-spam cap) and the summed `remaining` of those on `side`
/// (the reduce-only headroom, missing-features §1.1). Fuses what used to be two
/// separate full-slab scans into a single traversal on the hot order-entry path.
///
/// The same-side sum bounds a reduce-only order's free (reserve-0) headroom: a
/// position can only be *reduced* by at most its own size, so the trader's
/// already-resting same-side quantity is charged against that headroom before a new
/// reduce-only order claims any. Counting ALL same-side resting qty (not only
/// reduce-only ones) is deliberately conservative — it can only make a new order
/// reserve MORE, never less, so the reservation can never under-cover a flip.
///
/// PERF-2: the same-side sum is only consumed by the reduce-only opening-qty path,
/// so it is gated behind `reduce_only`. The common *open* submit skips the per-slot
/// `remaining` addition (and the side compare) entirely; it returns `same_side = 0`,
/// which the caller never reads on the open path. `resting_count` (the anti-spam
/// cap) is always computed, so `MAX_ORDERS_PER_TRADER` enforcement is unchanged.
#[inline(always)]
pub fn trader_resting_stats(
    data: &[u8],
    capacity: u32,
    trader: &Address,
    side: u8,
    reduce_only: bool,
) -> Result<(u32, u64), ProgramError> {
    let mut count = 0u32;
    let mut same_side = 0u64;
    for i in 0..capacity {
        let o = read_order(data, capacity, i)?;
        if o.status == OrderStatus::Resting as u8 && &o.trader == trader {
            count += 1;
            if reduce_only && o.side == side {
                same_side = same_side
                    .checked_add(o.remaining)
                    .ok_or(TempoProgramError::MathOverflow)?;
            }
        }
    }
    Ok((count, same_side))
}

/// Count the orders belonging to `trader` that are live this round — `Resting`
/// (not yet folded) or `Accumulated` (folded, awaiting settle). Used to reject
/// binding a position to a cross-margin group while it has an in-flight order
/// (known-issues §2.5).
#[inline(always)]
pub fn count_trader_live_orders(
    data: &[u8],
    capacity: u32,
    trader: &Address,
) -> Result<u32, ProgramError> {
    let mut n = 0u32;
    for i in 0..capacity {
        let order = read_order(data, capacity, i)?;
        if (order.status == OrderStatus::Resting as u8
            || order.status == OrderStatus::Accumulated as u8)
            && &order.trader == trader
        {
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::Discriminator;

    #[test]
    fn test_order_roundtrip_bytes() {
        let o = Order::new_resting(
            7,
            Address::new_from_array([5u8; 32]),
            OrderSide::Sell,
            100,
            40,
        );
        let bytes = o.to_bytes();
        assert_eq!(bytes.len(), ORDER_LEN);
        let de = Order::from_bytes(&bytes).unwrap();
        assert_eq!(de, o);
        assert_eq!(de.side().unwrap(), OrderSide::Sell);
        assert_eq!(de.status().unwrap(), OrderStatus::Resting);
        assert_eq!(de.remaining, 40);
        // Slab orders are taker-only (§1.3): is_maker is always 0.
        assert_eq!(de.is_maker, 0);
    }

    #[test]
    fn test_order_side_status_parse() {
        assert_eq!(OrderSide::from_u8(0).unwrap(), OrderSide::Buy);
        assert_eq!(OrderSide::from_u8(1).unwrap(), OrderSide::Sell);
        assert!(OrderSide::from_u8(2).is_err());
        assert_eq!(OrderStatus::from_u8(2).unwrap(), OrderStatus::Accumulated);
        assert!(OrderStatus::from_u8(9).is_err());
    }

    fn fresh_slab(capacity: u32) -> Vec<u8> {
        let h = OrderSlabHeader::new(255, Address::new_from_array([1u8; 32]), 0, capacity);
        let mut data = vec![0u8; OrderSlabHeader::account_size(capacity)];
        h.write_to_slice(&mut data).unwrap();
        // initialize all slots to Empty (status byte already 0 == Empty)
        data
    }

    #[test]
    fn test_slab_header_roundtrip() {
        let h = OrderSlabHeader::new(254, Address::new_from_array([9u8; 32]), 3, 8);
        let bytes = h.to_bytes();
        assert_eq!(bytes[0], OrderSlabHeader::DISCRIMINATOR);
        assert_eq!(bytes[1], OrderSlabHeader::VERSION);
        let de = OrderSlabHeader::from_bytes(&bytes).unwrap();
        assert_eq!(de.auction_id(), 3);
        assert_eq!(de.capacity(), 8);
        assert_eq!(de.bump, 254);
        assert_eq!(de.count(), 0);
    }

    #[test]
    fn test_account_size() {
        assert_eq!(
            OrderSlabHeader::account_size(4),
            OrderSlabHeader::LEN + 4 * ORDER_LEN
        );
    }

    #[test]
    fn test_find_free_and_write_read() {
        let cap = 4;
        let mut data = fresh_slab(cap);

        let idx = find_free_slot(&data, cap, 0).unwrap();
        assert_eq!(idx, 0);

        let o = Order::new_resting(
            1,
            Address::new_from_array([2u8; 32]),
            OrderSide::Buy,
            30,
            12,
        );
        write_order(&mut data, cap, idx, &o).unwrap();

        // next free slot should advance (cursor past slot 0)
        assert_eq!(find_free_slot(&data, cap, 1).unwrap(), 1);
        // a hint past the end clamps to the last slot and scans from there (still
        // a valid empty slot); the wrap-reclaim path is covered separately below
        assert_eq!(find_free_slot(&data, cap, 99).unwrap(), 3);

        let read = read_order(&data, cap, 0).unwrap();
        assert_eq!(read.order_id, 1);
        assert_eq!(read.price, 30);

        // lookup by id (scan) + hinted lookup (O(1) happy path + scan fallback)
        assert_eq!(find_order_by_id(&data, cap, 1).unwrap(), 0);
        assert_eq!(find_order_by_id_hinted(&data, cap, 1, 0).unwrap(), 0);
        // a wrong hint falls back to the scan and still finds it
        assert_eq!(find_order_by_id_hinted(&data, cap, 1, 3).unwrap(), 0);
        assert_eq!(
            find_order_by_id(&data, cap, 99),
            Err(TempoProgramError::OrderNotFound.into())
        );
        // a hint pointing at the wrong order also falls back, then errors honestly
        assert_eq!(
            find_order_by_id_hinted(&data, cap, 99, 0),
            Err(TempoProgramError::OrderNotFound.into())
        );
    }

    #[test]
    fn test_find_free_slot_reclaims_holes_on_wrap() {
        // Fill all slots, free a hole below the cursor, and confirm the wrap
        // reclaims it even when the hint points at the (full) tail.
        let cap = 4;
        let mut data = fresh_slab(cap);
        for i in 0..cap {
            let o = Order::new_resting(
                i as u64,
                Address::new_from_array([2u8; 32]),
                OrderSide::Buy,
                10,
                1,
            );
            let idx = find_free_slot(&data, cap, i).unwrap();
            write_order(&mut data, cap, idx, &o).unwrap();
        }
        // Free slot 1 (a hole below a cursor that now sits at capacity).
        let mut hole = read_order(&data, cap, 1).unwrap();
        hole.status = OrderStatus::Empty as u8;
        write_order(&mut data, cap, 1, &hole).unwrap();
        assert_eq!(find_free_slot(&data, cap, cap).unwrap(), 1);
    }

    #[test]
    fn test_all_active_orders_accumulated() {
        let cap = 4;
        let mut data = fresh_slab(cap);
        // Empty slab → complete.
        assert!(all_active_orders_accumulated(&data, cap).unwrap());
        // A resting order → not complete.
        let mut o = Order::new_resting(
            1,
            Address::new_from_array([2u8; 32]),
            OrderSide::Buy,
            30,
            12,
        );
        write_order(&mut data, cap, 0, &o).unwrap();
        assert!(!all_active_orders_accumulated(&data, cap).unwrap());
        // Folded (Accumulated) → complete again.
        o.status = OrderStatus::Accumulated as u8;
        write_order(&mut data, cap, 0, &o).unwrap();
        assert!(all_active_orders_accumulated(&data, cap).unwrap());
        // Consumed also counts as folded.
        o.status = OrderStatus::Consumed as u8;
        write_order(&mut data, cap, 0, &o).unwrap();
        assert!(all_active_orders_accumulated(&data, cap).unwrap());
    }

    #[test]
    fn test_slab_full() {
        let cap = 2;
        let mut data = fresh_slab(cap);
        for i in 0..cap {
            let o = Order::new_resting(
                i as u64,
                Address::new_from_array([2u8; 32]),
                OrderSide::Buy,
                10,
                1,
            );
            let idx = find_free_slot(&data, cap, i).unwrap();
            write_order(&mut data, cap, idx, &o).unwrap();
        }
        assert_eq!(
            find_free_slot(&data, cap, 0),
            Err(TempoProgramError::OrderSlabFull.into())
        );
    }
}
