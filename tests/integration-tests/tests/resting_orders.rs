//! Stage B — resting orders (plan §3). An unfilled / partially-filled order is not
//! discarded at settle: it re-arms as `Resting` and carries to the next round, so a
//! trader places once and the order lives until it is fully filled, expires, or is
//! cancelled. These tests pin the Stage-B-specific invariants:
//!   * a carried Resting order must be re-folded before the next round can finalize
//!     (the DDR-1 re-review trigger: the authoritative per-shard finalize scan still
//!     gates completeness after orders re-fold each round);
//!   * the roll gate is "no order still `Accumulated`" (fully SETTLED), not the old
//!     drain-to-`count == 0` — a folded-but-unsettled order blocks the roll, but a
//!     re-armed Resting survivor does not, and it carries across the roll;
//!   * `expires_at_auction` consumes a leftover instead of re-arming it;
//!   * conservation — a partial fill that rests then completes next round fills EXACTLY
//!     its original quantity (Σ fills across rounds == original qty).

use solana_sdk::pubkey::Pubkey;
use tempo_integration_tests::*;

const STATUS_RESTING: u8 = 1;
const STATUS_ACCUMULATED: u8 = 2;
const STATUS_CONSUMED: u8 = 3;

/// DDR-1 re-review trigger. A non-crossing taker buy fills nothing, so at settle it is
/// re-armed `Resting` (not `Consumed`) and carries to the next round. In that next round
/// it is unfolded again, so `finalize_clear` MUST refuse until it is re-folded — proving
/// the authoritative per-shard completeness scan still holds with resting orders.
#[test]
fn carried_resting_order_blocks_finalize_next_round() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let id0 = ctx.market(&pdas).current_auction_id;

    // A lone taker buy — no maker sells, so it crosses nothing (zero fill).
    let t = ctx.new_funded_signer();
    let o = ctx.submit_order(&pdas, &t, SIDE_BUY, 40, 5);

    // Round 1: fold, finalize, settle. Zero fill + still live ⇒ re-armed Resting.
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    let (_m, fill) = ctx.settle_fill(&pdas, o);
    assert_eq!(fill, 0, "non-crossing buy fills nothing");
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_RESTING,
        "zero-fill live order re-armed Resting"
    );
    assert_eq!(rec.remaining, 5, "remaining unchanged");
    assert_eq!(
        ctx.order_slab(&pdas).count,
        1,
        "order stays in the book (count kept)"
    );

    // Roll — the Resting survivor carries (it is not Accumulated, so the roll gate passes).
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).current_auction_id, id0 + 1, "rolled");
    assert_eq!(
        ctx.order_slab(&pdas).count,
        1,
        "carried into the next round"
    );

    // Round 2: enter Accumulating by folding a slot range that SKIPS the carried order
    // (it rests in slot 0; start at slot 1 folds nothing). The carried order is thus still
    // Resting (unfolded), so finalize must be refused — the censorship guarantee.
    ctx.process_chunk(&pdas, 1, 64);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);
    assert!(
        ctx.try_finalize_clear(&pdas).is_err(),
        "a carried Resting order not re-folded must block finalize",
    );

    // Re-fold it → completeness satisfied → finalize succeeds.
    ctx.process_chunk(&pdas, 0, 64);
    assert!(
        ctx.try_finalize_clear(&pdas).is_ok(),
        "once re-folded, the round finalizes",
    );
}

/// The Stage-B roll gate is "no order still `Accumulated`" (fully settled), not the old
/// `count == 0`. A folded-but-unsettled order must block `reset_shard`; once settled (here
/// re-armed Resting on a zero fill), the shard resets and the survivor carries.
#[test]
fn roll_gate_rejects_unsettled_then_carries_resting() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let id0 = ctx.market(&pdas).current_auction_id;

    let t = ctx.new_funded_signer();
    let o = ctx.submit_order(&pdas, &t, SIDE_BUY, 40, 5);

    // Fold (→ Accumulated) and finalize, but do NOT settle yet.
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);

    // The order is Accumulated (folded, unsettled) → reset_shard must refuse, even though
    // it would satisfy the old count==0 gate only after settle. (count is still 1 here.)
    assert!(
        ctx.try_reset_shard(&pdas, 0).is_err(),
        "roll must refuse a shard with a folded-but-unsettled (Accumulated) order",
    );

    // Settle it (zero fill → re-armed Resting). Now no order is Accumulated.
    let (_m, fill) = ctx.settle_fill(&pdas, o);
    assert_eq!(fill, 0);

    // Roll now succeeds and the Resting survivor carries with count preserved.
    ctx.start_auction(&pdas);
    let m = ctx.market(&pdas);
    assert_eq!(m.current_auction_id, id0 + 1, "rolled once settled");
    assert_eq!(m.phase, PHASE_COLLECT, "reopened Collect");
    let s = ctx.order_slab_shard(&pdas, 0).unwrap();
    assert_eq!(s.count, 1, "survivor carried");
    assert_eq!(s.auction_id, id0 + 1, "shard re-armed to the new round");
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.status, STATUS_RESTING, "carried as Resting");
}

/// `expires_at_auction` bounds how long an order rests. An order live in the round it is
/// submitted but past its expiry a round later is `Consumed` at settle (leaves the book),
/// not re-armed — even though it filled nothing.
#[test]
fn expired_resting_order_is_consumed_not_rearmed() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let id0 = ctx.market(&pdas).current_auction_id; // 0

    // Expire at auction id0+1: still live in round id0, expired when settled in round id0+1.
    let t = ctx.new_funded_signer();
    let o = ctx.submit_order_expiring(&pdas, &t, SIDE_BUY, 40, 5, id0 + 1);

    // Round id0: zero fill, expiry (id0+1) > current (id0) ⇒ NOT expired ⇒ re-armed Resting.
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, o);
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.status, STATUS_RESTING, "not yet expired → carries");
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).current_auction_id, id0 + 1);

    // Round id0+1: expiry (id0+1) <= current (id0+1) ⇒ expired ⇒ Consumed, leaves the book.
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, o);
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_CONSUMED,
        "expired order is consumed, not re-armed"
    );
    assert_eq!(
        ctx.order_slab(&pdas).count,
        0,
        "expired order left the book (count decremented)"
    );
}

/// Conservation across rounds: a taker sell that only PARTIALLY fills rests with its
/// leftover, then completes against fresh liquidity next round — filling EXACTLY its
/// original quantity over the two rounds and no more (Σ fills == original qty).
#[test]
fn partial_fill_rests_then_completes_conserving() {
    let mut ctx = TestContext::new();
    let tick = 10u64;
    let pdas = ctx.init_market(tick, 16, 64);
    let mid_tick = 3u32; // price 40 = tick 3 (price/tick_size - 1)

    let mb = ctx.new_funded_signer(); // maker buyer (bid demand, quote book)
    let ts = ctx.new_funded_signer(); // taker seller (bid supply, slab)
    ctx.init_position(&pdas, &mb);

    // Round 1: maker buys 12, taker sells 20 at price 40 → cross 12. Taker partially fills.
    ctx.post_maker_order(&pdas, &mb, SIDE_BUY, 4 * tick, 12);
    let o = ctx.submit_order(&pdas, &ts, SIDE_SELL, 4 * tick, 20);

    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &mb.pubkey());
    ctx.finalize_clear(&pdas);
    assert_eq!(
        ctx.clearing(&pdas).unwrap().bid_matched_volume,
        12,
        "bid auction matched the maker's 12"
    );

    let (_m, cum1) = ctx.settle_fill(&pdas, o);
    assert_eq!(cum1, 12, "round-1 fill is the crossed 12");
    ctx.settle_maker_quote_clearing(&pdas, &mb.pubkey());
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.status, STATUS_RESTING, "partial fill re-arms Resting");
    assert_eq!(rec.remaining, 8, "leftover 20 - 12 = 8 carries");

    // Roll — the partially-filled taker carries into round 2.
    ctx.start_auction(&pdas);
    assert_eq!(ctx.order_slab(&pdas).count, 1, "leftover carried");

    // Round 2: re-arm the maker to buy the remaining 8, then clear.
    ctx.update_maker_quote_levels(&pdas, &mb, 2, mid_tick, &[(0, 8)], &[]);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &mb.pubkey());
    ctx.finalize_clear(&pdas);
    assert_eq!(
        ctx.clearing(&pdas).unwrap().bid_matched_volume,
        8,
        "round-2 cross is the remaining 8"
    );

    let (_m2, cum2) = ctx.settle_fill(&pdas, o);
    // `order_fill` is cumulative (quantity - remaining): after round 2 the order is fully
    // filled, so this reads the full original quantity — conservation.
    assert_eq!(
        cum2, 20,
        "Σ fills across rounds == original 20 (no over-fill)"
    );
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_CONSUMED,
        "fully filled → leaves the book"
    );
    assert_eq!(rec.remaining, 0, "nothing left to fill");
}

/// DDR-3 (the wedge fix, passive-park half): a resting order whose fixed price leaves the
/// recentered window because the market moved AWAY from it (a SELL now above the window top)
/// must PARK — `process_chunk` skips it (no hard error) and finalize's completeness gate
/// exempts it, so a single out-of-window order can never wedge the whole market. When the
/// window later slides back over it, it folds normally.
#[test]
fn passive_resting_order_parks_then_folds_when_window_returns() {
    let mut ctx = TestContext::new();
    let oracle = Pubkey::new_unique();
    // Window centers on 100_000: floor = 100_000 - (64/2)*10 = 99_680; top = 99_680 + 640 = 100_320.
    ctx.set_oracle(&oracle, 100_000, -8);
    let pdas = ctx.init_market_with_oracle(10, 64, 16, oracle);

    // A lone in-window resting SELL at 100_000. No counterparty → zero fill → re-arms Resting.
    let t = ctx.new_funded_signer();
    let o = ctx.submit_order(&pdas, &t, SIDE_SELL, 100_000, 5);
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, o);

    // Recenter DOWN to 50_000 → window [49_680, 50_320). The SELL at 100_000 is now ABOVE the
    // top → PASSIVE. (Set the oracle BEFORE the roll: start_auction recenters at roll time.)
    ctx.set_oracle(&oracle, 50_000, -8);
    ctx.start_auction(&pdas);
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);

    // Pre-fix this hard-errored in `price_to_tick_raw`. Now it is skipped: the order stays
    // Resting and does NOT block finalize.
    ctx.process_chunk(&pdas, 0, 64);
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_RESTING,
        "passive order parked (not folded)"
    );
    assert!(
        ctx.try_finalize_clear(&pdas).is_ok(),
        "a passive out-of-window order must NOT wedge finalize (DDR-3)",
    );

    // Recenter BACK to 100_000 and roll: the parked order is in-window again → it folds.
    ctx.set_oracle(&oracle, 100_000, -8);
    ctx.start_auction(&pdas);
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_ACCUMULATED,
        "once the window returns, the parked order folds normally"
    );
    assert!(
        ctx.try_finalize_clear(&pdas).is_ok(),
        "and the round finalizes"
    );
}

/// DDR-3 (the wedge fix, marketable half): a resting order the market moved THROUGH (a SELL
/// now below the recentered floor) is MARKETABLE — `process_chunk` folds it at the boundary
/// tick (it does NOT error), so it clears this round instead of wedging. With no counterparty
/// it fills 0 and re-arms, but the point is it folds and settles cleanly.
#[test]
fn marketable_resting_order_folds_after_recenter() {
    let mut ctx = TestContext::new();
    let oracle = Pubkey::new_unique();
    ctx.set_oracle(&oracle, 100_000, -8); // window [99_680, 100_320)
    let pdas = ctx.init_market_with_oracle(10, 64, 16, oracle);

    // Lone in-window SELL at the floor (99_680). Zero fill → rests.
    let t = ctx.new_funded_signer();
    let o = ctx.submit_order(&pdas, &t, SIDE_SELL, 99_680, 5);
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, o);

    // Recenter UP to 200_000 → window [199_680, 200_320). The SELL at 99_680 is now BELOW the
    // floor → MARKETABLE.
    ctx.set_oracle(&oracle, 200_000, -8);
    ctx.start_auction(&pdas);
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);

    // It must FOLD (status Accumulated), not error.
    ctx.process_chunk(&pdas, 0, 64);
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_ACCUMULATED,
        "marketable order folds at the boundary tick, not errors"
    );
    // finalize + settle proceed with no wedge/revert (no counterparty → fill 0, re-arms).
    ctx.finalize_clear(&pdas);
    let (_m, fill) = ctx.settle_fill(&pdas, o);
    assert_eq!(fill, 0, "no counterparty this round → zero fill, no revert");
}

/// DDR-3 correction #1: a reduce-only order NEVER rests. Fill quantity is fixed at fold
/// time, so clamping it at settle would drop already-matched volume and break conservation
/// (`vault_token ≥ Σ balances + insurance`). Instead a reduce-only order applies its FULL
/// computed fill and is forced `Consumed` this round — even on a partial fill — so a carried
/// leftover can never re-arm and open new exposure the market gapped it into.
#[test]
fn reduce_only_partial_fill_is_consumed_not_rearmed() {
    let mut ctx = TestContext::new();
    let tick = 10u64;
    let pdas = ctx.init_market(tick, 16, 64); // genesis window (maker ticks align)

    let mb = ctx.new_funded_signer(); // maker buyer (bid demand, partial counterparty)
    let ts = ctx.new_funded_signer(); // reduce-only taker seller (bid supply, slab)
    ctx.init_position(&pdas, &mb);

    // Maker buys 12 @ 40; reduce-only taker sells 20 @ 40 → cross 12, taker partially fills.
    // A plain resting order would re-arm the leftover 8; a reduce-only one is Consumed instead.
    ctx.post_maker_order(&pdas, &mb, SIDE_BUY, 4 * tick, 12);
    let o = ctx.submit_order_reduce_only(&pdas, &ts, SIDE_SELL, 4 * tick, 20);

    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &mb.pubkey());
    ctx.finalize_clear(&pdas);
    let (_m, fill) = ctx.settle_fill(&pdas, o);
    assert_eq!(
        fill, 12,
        "reduce-only applies its full computed fill (no clamp)"
    );

    // The reduce-only order is Consumed on a partial fill, NOT re-armed Resting — so its
    // leftover 8 can never carry into the next round and open new exposure.
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .expect("reduce-only order slot must still be present (not emptied)");
    assert_eq!(
        rec.status, STATUS_CONSUMED,
        "reduce-only order is Consumed on partial fill, not re-armed Resting"
    );
    assert_eq!(
        ctx.order_slab(&pdas).count,
        0,
        "reduce-only leftover left the book (count decremented)"
    );
}

/// DDR-3 Correction-2 item 4: the permissionless reap boundary is STRICT `<`. During the
/// order's active round (`expires_at_auction == current_auction_id`) a NON-OWNER may NOT
/// cancel it (that would be a denial-of-fill grief — the order is still entitled to fold and
/// fill this round). Only AFTER its last active round (`expires_at_auction < current`) can a
/// non-owner reap it, and the released margin returns to the OWNER's ledger, never the reaper.
/// A parked passive order is used so it survives rolls unfolded (settle never runs on it),
/// exercising both boundaries in one flow.
#[test]
fn permissionless_reap_boundary_is_strict_less_than() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500; // money-path market so a reservation is locked
    let oracle = Pubkey::new_unique();
    // Window centers on 100_000: floor 99_680, top 100_320 (tick 10, 64 ticks).
    ctx.set_oracle(&oracle, 100_000, -8);
    let pdas = ctx.init_market_with_oracle(10, 64, 16, oracle);

    // Vault + a funded owner with a position and collateral.
    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 1_000_000);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 1_000_000);
    ctx.init_position(&pdas, &owner);

    let id0 = ctx.market(&pdas).current_auction_id;

    // A resting SELL @ 100_000 (in-window) expiring at id0+1. It reserves worst-case margin.
    let o = ctx.submit_order_expiring(&pdas, &owner, SIDE_SELL, 100_000, 5, id0 + 1);
    let locked_at_submit = ctx.user_collateral(&owner.pubkey()).locked;
    assert!(
        locked_at_submit > 0,
        "the resting sell locks worst-case margin"
    );

    // Round id0: fold + settle → zero fill → re-armed Resting (expiry id0+1 > id0).
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, o);

    // Recenter DOWN so the SELL is now ABOVE the window top → PASSIVE (never folds again, so
    // settle never Consumes it — it survives rolls as a parked Resting order).
    ctx.set_oracle(&oracle, 50_000, -8);
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).current_auction_id, id0 + 1);

    // (a) Round id0+1: current == expiry (id0+1). Strict `<` ⇒ NOT reapable by a non-owner.
    let reaper = ctx.new_funded_signer();
    assert!(
        ctx.try_reap_order(&pdas, &reaper, &owner.pubkey(), o)
            .is_err(),
        "a non-owner may NOT reap during the order's active round (expires_at == current)"
    );
    // The margin is still locked (nothing was released by the rejected reap).
    assert_eq!(
        ctx.user_collateral(&owner.pubkey()).locked,
        locked_at_submit,
        "rejected reap released nothing"
    );

    // Park + roll again (passive order stays Resting): id0+1 → id0+2.
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas); // passive exempt, so this succeeds
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).current_auction_id, id0 + 2);

    // (b) Round id0+2: expiry (id0+1) < current (id0+2) ⇒ reapable. A NON-OWNER reap succeeds
    // and the released margin returns to the OWNER's ledger (not the reaper).
    assert!(
        ctx.try_reap_order(&pdas, &reaper, &owner.pubkey(), o)
            .is_ok(),
        "a non-owner may reap after the order's last active round (expires_at < current)"
    );
    assert_eq!(
        ctx.user_collateral(&owner.pubkey()).locked,
        0,
        "reaped order's margin returned to the OWNER, not the reaper"
    );
    assert_eq!(
        ctx.order_slab(&pdas).count,
        0,
        "the reaped order left the book"
    );
}

/// DDR-3 Correction-2 item 4 (submit guard), P4.1 IOC boundary: `submit_order` rejects
/// an order whose `expires_at_auction` is strictly BEFORE its arm round (`!= 0 &&
/// < arm_auction_id`) — it could never fold or fill. Expiry EQUAL to the arm round is
/// now legal (IOC, missing-features §2.3) — covered by `tests/ioc.rs`.
#[test]
fn submit_of_already_expired_order_is_rejected() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    // Roll two full (empty) rounds so current_auction_id >= 2 (genesis is 0, and expiry 0
    // means GTC — so both `expiry == current` AND `expiry < current` need a current id of
    // at least 2 to be distinguishable from a GTC 0).
    for _ in 0..2 {
        ctx.process_chunk(&pdas, 0, 64);
        ctx.finalize_clear(&pdas);
        ctx.start_auction(&pdas);
    }
    let cur = ctx.market(&pdas).current_auction_id;
    assert!(
        cur >= 2,
        "rolled past genesis so nonzero expiries are testable"
    );

    let t = ctx.new_funded_signer();
    // expiry < arm (= current in Collect) ⇒ could never fold ⇒ rejected Custom(46).
    let err = ctx
        .try_submit_order_expiring(&pdas, &t, SIDE_BUY, 40, 5, cur - 1)
        .expect_err("an order expiring before its arm round is rejected at submit");
    assert!(
        format!("{:?}", err.err).contains("Custom(46)"),
        "rejected with OrderAlreadyExpired (46), got {:?}",
        err.err
    );
    // expiry == arm ⇒ IOC ⇒ ACCEPTED (P4.1; one-round life is proven in tests/ioc.rs).
    ctx.try_submit_order_expiring(&pdas, &t, SIDE_BUY, 40, 5, cur)
        .expect("an IOC order (expiry == arm round) is accepted");
    // A future expiry (or GTC = 0) is accepted.
    let ok = ctx.try_submit_order_expiring(&pdas, &t, SIDE_BUY, 40, 5, cur + 1);
    assert!(ok.is_ok(), "a future expiry is accepted");
    assert_eq!(
        ctx.order_slab(&pdas).count,
        2,
        "the IOC and the future-expiry order both rested"
    );
}
