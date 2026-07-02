//! PDA derivations. Seeds taken from the state structs in `program/src/state`.

use solana_pubkey::Pubkey;

use crate::ids::TEMPO_PROGRAM_ID;

pub fn market(market_seed: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"market", market_seed.as_ref()], &TEMPO_PROGRAM_ID)
}

pub fn histogram(market: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"histogram", market.as_ref()], &TEMPO_PROGRAM_ID)
}

/// A market's OrderSlab **shard** PDA (Stage A sharding): seeds
/// `[b"order_slab", market, shard_id.to_le_bytes()]`. A market has `num_slab_shards`
/// shards; orders are routed across them so submit/settle run in parallel.
pub fn order_slab(market: &Pubkey, shard_id: u16) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"order_slab", market.as_ref(), &shard_id.to_le_bytes()],
        &TEMPO_PROGRAM_ID,
    )
}

/// The canonical shard a trader routes ALL its orders to (Stage B / DDR-3 /
/// Finding 4). The on-chain per-trader order cap (`MAX_ORDERS_PER_TRADER`) is
/// enforced per shard (submit stays `Market`-read-only for parallel intake, so it
/// can't scan every shard). Routing each trader deterministically to one shard —
/// `shard = hash(trader) % num_slab_shards` — makes that per-shard cap act as the
/// trader's *global* standing-order cap. Reference clients (the sim taker fleet)
/// use this; a client that spreads one trader across shards would sidestep the cap.
/// Returns `0` for a single-shard market.
pub fn shard_for_trader(trader: &Pubkey, num_slab_shards: u16) -> u16 {
    if num_slab_shards <= 1 {
        return 0;
    }
    // FNV-1a over the pubkey bytes — a stable, dependency-free hash (matches nothing
    // security-sensitive; it only needs to spread traders evenly across shards).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in trader.as_ref() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h % num_slab_shards as u64) as u16
}

pub fn clearing(market: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"clearing", market.as_ref()], &TEMPO_PROGRAM_ID)
}

pub fn position(market: &Pubkey, owner: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"position", market.as_ref(), owner.as_ref()],
        &TEMPO_PROGRAM_ID,
    )
}

/// The ledger is mint-scoped (CR-3): seeds are `[b"collateral", owner, mint]`, so a
/// balance deposited under one mint can never be derived/withdrawn against another.
pub fn user_collateral(owner: &Pubkey, collateral_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"collateral", owner.as_ref(), collateral_mint.as_ref()],
        &TEMPO_PROGRAM_ID,
    )
}

pub fn vault(collateral_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"vault", collateral_mint.as_ref()], &TEMPO_PROGRAM_ID)
}

pub fn vault_authority() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"vault_authority"], &TEMPO_PROGRAM_ID)
}

pub fn margin_account(owner: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"margin", owner.as_ref()], &TEMPO_PROGRAM_ID)
}

// TODO(known-issues §4.9): multi-quote — currently one PDA per maker per market.
// Supporting multiple concurrent ladders requires adding a `quote_id: u8` to the
// seeds: `[b"maker_quote", market, maker, &[quote_id]]`. This is a program-level
// change (new PDA seeds, new instruction variant, migration). Until then, run
// multiple mm-bot instances with different keypairs for wider depth coverage.
pub fn maker_quote(market: &Pubkey, maker: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"maker_quote", market.as_ref(), maker.as_ref()],
        &TEMPO_PROGRAM_ID,
    )
}

pub fn event_authority() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"event_authority"], &TEMPO_PROGRAM_ID)
}

/// The per-market clearing-engine PDAs, derived once and reused (mirrors the
/// bots' `deriveMarketPdas`).
#[derive(Clone, Copy, Debug)]
pub struct MarketPdas {
    pub market: Pubkey,
    pub histogram: Pubkey,
    /// Shard 0's OrderSlab PDA (Stage A sharding). Use [`MarketPdas::slab_shard`] for a
    /// specific shard; this convenience field is shard 0.
    pub order_slab: Pubkey,
    pub clearing: Pubkey,
    /// Canonical bump of the `clearing` PDA — `finalize_clear` takes it as an arg
    /// (it may create the account), so carry it to avoid re-deriving at call time.
    pub clearing_bump: u8,
    pub event_authority: Pubkey,
}

impl MarketPdas {
    pub fn derive(market: Pubkey) -> Self {
        let (clearing, clearing_bump) = clearing(&market);
        Self {
            market,
            histogram: histogram(&market).0,
            order_slab: order_slab(&market, 0).0,
            clearing,
            clearing_bump,
            event_authority: event_authority().0,
        }
    }

    /// The OrderSlab shard PDA for `shard_id` (Stage A sharding).
    pub fn slab_shard(&self, shard_id: u16) -> Pubkey {
        order_slab(&self.market, shard_id).0
    }

    /// All `num_slab_shards` shard PDAs, in shard-id order. Callers pass this to
    /// `finalize_clear` (Design Z scans every shard) and `force_reset`.
    pub fn all_shards(&self, num_slab_shards: u16) -> Vec<Pubkey> {
        (0..num_slab_shards).map(|i| self.slab_shard(i)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pdas_are_off_curve_and_distinct() {
        let seed = Pubkey::new_unique();
        let (m, _) = market(&seed);
        let pdas = MarketPdas::derive(m);
        assert_ne!(pdas.histogram, pdas.order_slab);
        assert_ne!(pdas.order_slab, pdas.clearing);
        // event_authority is global (seed-independent).
        assert_eq!(pdas.event_authority, event_authority().0);
    }
}
