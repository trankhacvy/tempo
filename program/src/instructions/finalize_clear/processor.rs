use pinocchio::{account::AccountView, cpi::Seed, error::ProgramError, Address, ProgramResult};

use crate::{
    clearing::{find_cross, CrossResult},
    errors::TempoProgramError,
    events::ClearingFinalizedEvent,
    instructions::FinalizeClear,
    state::{
        all_active_orders_accumulated, read_region_values, AuctionHistogramHeader, AuctionPhase,
        ClearingResult, Market, OrderSlabHeader, Region,
    },
    traits::{
        AccountDeserialize, AccountSerialize, AccountSize, EventSerialize, PdaAccount, PdaSeeds,
    },
    utils::{create_pda_account_idempotent, emit_event},
};

/// ClearingResult PDA seed prefix (kept in sync with the IDL definition:
/// `pda("clearing", [seed("market", account("market"))])`).
const CLEARING_SEED: &[u8] = b"clearing";

/// Processes the FinalizeClear instruction — Phase 2 DISCOVER
/// (clearing-protocol §3). Permissionless. Requires the completeness check
/// (every resting order folded — enforced by the `all_active_orders_accumulated`
/// slab scan — plus every maker quote folded, clearing-protocol §4.2), runs a
/// single O(ticks) pass to find the clearing price and marginal-tick allocation,
/// and publishes a `ClearingResult`.
///
/// Runs **both** uniform-price crosses (system-design §1): the bid auction
/// (maker-buys vs taker-sells) and the ask auction (taker-buys vs maker-sells),
/// writing both sides of the `ClearingResult`. The histogram method applies to
/// each independently (clearing-protocol §5).
///
/// Pays the cranker a flat `Market.crank_fee` from the vault fee/insurance pool
/// when the optional cranker-collateral + vault accounts are supplied
/// (system-design §8); a no-op otherwise.
pub fn process_finalize_clear(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = FinalizeClear::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // --- validate market phase; capture params ---
    let (
        num_ticks,
        tick_size,
        window_floor,
        auction_id,
        crank_fee,
        market_collateral_mint,
        num_slab_shards,
    ) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.require_phase(AuctionPhase::Accumulating)?;
        // Maker-quote completeness check (clearing-protocol §4.2): refuse to finalize
        // until every active maker quote has been folded exactly once (keeps our
        // censorship guarantee for maker liquidity).
        if market.folded_maker_quote_count() != market.active_maker_quote_count() {
            return Err(TempoProgramError::AuctionNotComplete.into());
        }
        (
            market.num_ticks(),
            market.tick_size(),
            market.window_floor_price(),
            market.current_auction_id(),
            market.crank_fee(),
            market.collateral_mint,
            market.num_slab_shards(),
        )
    };

    // --- Design Z (DDR-1) order-side completeness: authoritatively scan EVERY shard ---
    //
    // The caller must pass all `num_slab_shards` slab shards; finalize proves the censorship
    // guarantee directly — no aggregate counter to drift. For each shard: it belongs to this
    // market, is the canonical PDA for a distinct in-range `shard_id`, is at this auction id,
    // and holds NO still-`Resting` (unfolded) order (`all_active_orders_accumulated`). A short,
    // wrong, foreign, stale, or duplicated shard set is rejected, so a hostile cranker cannot
    // dodge the scan to finalize while an order sits unfolded.
    if ix.accounts.shards.len() != num_slab_shards as usize {
        return Err(TempoProgramError::AuctionNotComplete.into());
    }
    {
        let mut seen_mask: u64 = 0;
        for shard_ai in ix.accounts.shards.iter() {
            let slab_data = shard_ai.try_borrow()?;
            let slab = OrderSlabHeader::from_bytes(&slab_data)?;
            if slab.market != market_key {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            slab.validate_pda(shard_ai, program_id, slab.bump)?;
            if slab.auction_id() != auction_id {
                return Err(TempoProgramError::AuctionIdMismatch.into());
            }
            let shard_id = slab.shard_id();
            if shard_id >= num_slab_shards {
                return Err(TempoProgramError::ShardOutOfRange.into());
            }
            // De-dup: each shard index appears at most once, so the caller cannot satisfy the
            // count check by passing one folded shard N times while a real shard stays unfolded.
            // (Mask covers up to 64 shards; larger markets exceed the tx account limit anyway.)
            let bit = 1u64
                .checked_shl(shard_id as u32)
                .ok_or(TempoProgramError::ShardOutOfRange)?;
            if seen_mask & bit != 0 {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            seen_mask |= bit;

            if !all_active_orders_accumulated(&slab_data, slab.capacity())? {
                return Err(TempoProgramError::AuctionNotComplete.into());
            }
        }
    }

    // --- read the histogram buckets into the four region arrays ---
    let (bid_demand, bid_supply, ask_demand, ask_supply) = {
        let hist_data = ix.accounts.histogram.try_borrow()?;
        let hist = AuctionHistogramHeader::from_bytes(&hist_data)?;
        if hist.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        if hist.auction_id() != auction_id {
            return Err(TempoProgramError::AuctionIdMismatch.into());
        }
        if hist.num_ticks() != num_ticks {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }

        // Read each region's buckets in one contiguous pass (cheaper than
        // per-tick read_region calls across four regions — cu_optimizations §3).
        (
            read_region_values(&hist_data, num_ticks, Region::BidDemand)?,
            read_region_values(&hist_data, num_ticks, Region::BidSupply)?,
            read_region_values(&hist_data, num_ticks, Region::AskDemand)?,
            read_region_values(&hist_data, num_ticks, Region::AskSupply)?,
        )
    };

    // --- the clearing passes (the crown jewel): one per auction ---
    let bid = find_cross(&bid_demand, &bid_supply)?;
    let ask = find_cross(&ask_demand, &ask_supply)?;

    // map clearing tick -> price: the canonical inverse of `price_to_tick`
    // (Market::tick_to_price), `price = tick·tick_size + window_floor`. Using the
    // legacy zero-anchored `(tick+1)·tick_size` here ignored the oracle-centered
    // window floor and fabricated the clearing price on every recentered market (CR-1).
    let cross_price = |c: &CrossResult| -> Result<u64, ProgramError> {
        if c.crossed {
            (c.clearing_tick as u64)
                .checked_mul(tick_size)
                .and_then(|off| off.checked_add(window_floor))
                .ok_or(TempoProgramError::MathOverflow.into())
        } else {
            Ok(0)
        }
    };
    let bid_price = cross_price(&bid)?;
    let ask_price = cross_price(&ask)?;

    // Derive the canonical clearing PDA on-chain. We deliberately do NOT trust
    // `ix.data.clearing_bump`: a non-canonical bump would let a caller create the
    // ClearingResult at an off-canonical address that settle_fill's canonical-PDA
    // check (validate_self -> find_program_address) later rejects, wedging the
    // market in Discovered forever (a free, permanent DoS). `find_program_address`
    // returns the canonical (highest) bump; we use it for the address check, the
    // stored bump, and the create-signer seeds.
    let (derived, clearing_bump) =
        Address::find_program_address(&[CLEARING_SEED, market_key.as_ref()], program_id);
    if ix.accounts.clearing_result.address() != &derived {
        return Err(ProgramError::InvalidSeeds);
    }
    // Defense-in-depth: a correct client derives the canonical bump, so reject a
    // mismatched one outright rather than silently overriding it. The account is
    // created at `derived` regardless, so this can never brick the market.
    if ix.data.clearing_bump != clearing_bump {
        return Err(ProgramError::InvalidSeeds);
    }

    // --- build + persist the ClearingResult (both auctions) ---
    let mut result = ClearingResult::empty(clearing_bump, market_key, auction_id);
    result.set_bid_clearing_price(bid_price);
    result.set_bid_matched_volume(bid.matched_volume);
    result.set_bid_marginal_tick(bid.clearing_tick);
    result.set_bid_volume_allocated_to_marginal_tick(bid.volume_allocated_to_marginal_tick);
    result.set_bid_total_qty_at_marginal_tick(bid.total_qty_at_marginal_tick);
    result.set_ask_clearing_price(ask_price);
    result.set_ask_matched_volume(ask.matched_volume);
    result.set_ask_marginal_tick(ask.clearing_tick);
    result.set_ask_volume_allocated_to_marginal_tick(ask.volume_allocated_to_marginal_tick);
    result.set_ask_total_qty_at_marginal_tick(ask.total_qty_at_marginal_tick);
    result.bid_rationed_side = bid.rationed_side;
    result.ask_rationed_side = ask.rationed_side;

    let clearing_bump_seed = [clearing_bump];
    let clearing_seeds: [Seed; 3] = [
        Seed::from(CLEARING_SEED),
        Seed::from(market_key.as_ref()),
        Seed::from(clearing_bump_seed.as_slice()),
    ];
    // Idempotent: the ClearingResult PDA is persistent and reused every round
    // (its size is fixed, so this creates on the first auction and is a no-op on
    // later ones), then we overwrite it with this round's result.
    create_pda_account_idempotent(
        ix.accounts.cranker,
        ClearingResult::LEN,
        program_id,
        ix.accounts.clearing_result,
        clearing_seeds,
    )?;
    {
        let mut acct = *ix.accounts.clearing_result;
        let mut slice = acct.try_borrow_mut()?;
        result.write_to_slice(&mut slice)?;
    }

    // --- advance market: phase Discovered, record bid fill price ---
    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.phase = AuctionPhase::Discovered as u8;
        market.set_last_bid_fill_price(bid_price);
        market.set_last_ask_fill_price(ask_price);
    }

    // Pay the cranker a flat fee from the vault fee/insurance pool, capped at the
    // available balance (system-design §8). Optional: only when the cranker's
    // collateral ledger + vault are supplied. Conserving — insurance → cranker
    // ledger, both inside the vault's token holdings.
    if crank_fee > 0 {
        if let (Some(cc_acct), Some(vault_acct)) =
            (ix.accounts.cranker_collateral, ix.accounts.vault)
        {
            let paid = {
                let mut v = *vault_acct;
                let mut v_data = v.try_borrow_mut()?;
                let vault = crate::state::Vault::from_bytes_mut(&mut v_data)?;
                // Reject any program-owned account that is not the canonical vault
                // PDA for its own mint (so insurance can't be drained from a
                // foreign pool); and, when the market declares a mint, bind it.
                vault.validate_self(vault_acct, program_id)?;
                if market_collateral_mint != Address::new_from_array([0u8; 32])
                    && vault.collateral_mint != market_collateral_mint
                {
                    return Err(TempoProgramError::AccountMarketMismatch.into());
                }
                let pay = crank_fee.min(vault.insurance_balance());
                vault.set_insurance_balance(vault.insurance_balance() - pay);
                pay
            };
            if paid > 0 {
                let mut cc = *cc_acct;
                let mut cc_data = cc.try_borrow_mut()?;
                let cc_ledger = crate::state::UserCollateral::from_bytes_mut(&mut cc_data)?;
                cc_ledger.validate_self(cc_acct, program_id)?;
                if cc_ledger.owner != *ix.accounts.cranker.address() {
                    return Err(TempoProgramError::InvalidCollateralAccount.into());
                }
                cc_ledger.credit(paid)?;
            }
        }
    }

    // Emit event via CPI.
    let event = ClearingFinalizedEvent {
        market: market_key,
        auction_id,
        bid_clearing_price: bid_price,
        bid_matched_volume: bid.matched_volume,
        ask_clearing_price: ask_price,
        ask_matched_volume: ask.matched_volume,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
