//! The Phase-2 admin release (plan.md §3.1–§3.3): the hot/staged parameter
//! split, the propose→delay→apply engine (apply is PERMISSIONLESS — the delay
//! is enforced by consensus, not authority honesty), the two-step authority
//! transfer, and the hard-gated oracle repoint.

use solana_sdk::pubkey::Pubkey;
use tempo_integration_tests::*;

const PAUSE_ALL: u8 = 3;

/// Hot params (§3.2) apply immediately and are read at use-time: a min-notional
/// set live rejects the very next dust order. Non-authority callers rejected.
#[test]
fn hot_params_update_applies_immediately() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let trader = ctx.new_funded_signer();

    // Dust passes before the update (min_order_notional starts 0 = disabled).
    ctx.submit_order(&pdas, &trader, SIDE_BUY, 10, 1);

    // A stranger cannot retune the market.
    let stranger = ctx.new_funded_signer();
    assert!(
        ctx.try_update_market_params(&pdas, &stranger, 0, 0, 1_000, 0, 0)
            .is_err(),
        "only the authority may update params"
    );

    // The authority sets min_order_notional = 1_000, effective immediately.
    let authority = ctx.market_authority_keypair(&pdas).expect("authority");
    ctx.try_update_market_params(&pdas, &authority, 0, 0, 1_000, 0, 0)
        .expect("hot update");
    assert!(
        ctx.try_submit_order(&pdas, &trader, SIDE_BUY, 10, 1)
            .is_err(),
        "the very next dust order is rejected — hot params are read at use-time"
    );
    // A non-dust order still passes (the market is otherwise untouched).
    ctx.submit_order(&pdas, &trader, SIDE_BUY, 40, 30);
}

/// The staged risk path (§3.2): out-of-bounds proposals can't even be staged
/// (shared validator with init); early apply → PendingDelayNotElapsed; wrong
/// kind → NoPendingUpdate; post-delay apply writes all four fields; a second
/// apply fails (exactly-once).
#[test]
fn staged_risk_update_delay_kinds_and_exactly_once() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 32, 8);

    // Out-of-bounds: initial < maintenance is rejected AT PROPOSE (same table
    // as initialize_market — one source of truth).
    assert!(
        ctx.try_propose_risk_update(&pdas, 600, 400, 100, 0)
            .is_err(),
        "initial < maintenance cannot even be staged"
    );

    // Nothing staged yet → apply is NoPendingUpdate.
    assert!(
        ctx.try_apply_risk_update(&pdas).is_err(),
        "apply with nothing pending must fail"
    );

    // Stage a valid change, then try to apply too early.
    ctx.try_propose_risk_update(&pdas, 600, 1200, 200, 100)
        .expect("valid proposal stages");
    assert!(
        ctx.try_apply_risk_update(&pdas).is_err(),
        "apply before the delay must fail (PendingDelayNotElapsed)"
    );

    // Warp past the delay: the PERMISSIONLESS apply (payer, not authority)
    // lands and all four fields change.
    let now = ctx.current_slot();
    ctx.warp_slot(now + 3_001);
    ctx.try_apply_risk_update(&pdas).expect("post-delay apply");
    let m = ctx.market(&pdas);
    assert_eq!(m.maintenance_margin_bps, 600, "maintenance updated");

    // Exactly-once: the slot is cleared.
    assert!(
        ctx.try_apply_risk_update(&pdas).is_err(),
        "a staged change applies exactly once"
    );
}

/// The two-step authority transfer (§3.3): a wrong signer cannot accept, the
/// staged key can, and the old key loses its powers at that instant.
#[test]
fn authority_transfer_is_two_step() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let new_authority = ctx.new_funded_signer();
    let imposter = ctx.new_funded_signer();

    ctx.try_propose_authority_transfer(&pdas, &new_authority.pubkey())
        .expect("propose");

    // Only the STAGED key may accept.
    assert!(
        ctx.try_accept_authority_transfer(&pdas, &imposter).is_err(),
        "an imposter cannot accept a transfer staged for someone else"
    );
    ctx.try_accept_authority_transfer(&pdas, &new_authority)
        .expect("the staged key accepts");

    // The old authority is powerless; the new one governs.
    let old = ctx.market_authority_keypair(&pdas).expect("recorded");
    assert!(
        ctx.try_set_pause_signed(&pdas, &old, 1).is_err(),
        "the old authority lost its powers at accept"
    );
    ctx.try_set_pause_signed(&pdas, &new_authority, 1)
        .expect("the new authority governs");
}

/// The oracle repoint (§3.3) behind all four gates: propose requires
/// PAUSE_ROLL; apply requires the delay, full pause + quiescence, and a LIVE
/// staged feed — then commits address + feed id atomically.
#[test]
fn oracle_repoint_gates_then_commits_atomically() {
    let mut ctx = TestContext::new();
    let oracle_a = Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.set_oracle(&oracle_a, 100_000, -8);
    let pdas = ctx.init_market_with_oracle(10, 64, 16, oracle_a);

    let oracle_b = Pubkey::new_unique();
    ctx.set_oracle(&oracle_b, 200_000, -8);

    // Gate 0: proposing on an un-paused market is refused (MarketNotQuiescent).
    assert!(
        ctx.try_propose_set_oracle(&pdas, &oracle_b, &SOL_USD_FEED_ID)
            .is_err(),
        "propose requires the market to already be winding down (PAUSE_ROLL)"
    );

    ctx.set_pause(&pdas, PAUSE_ALL);
    ctx.try_propose_set_oracle(&pdas, &oracle_b, &SOL_USD_FEED_ID)
        .expect("propose while winding down");

    // Gate 1: the delay.
    assert!(
        ctx.try_apply_set_oracle(&pdas, &oracle_b).is_err(),
        "apply before the delay must fail"
    );
    let now = ctx.current_slot();
    ctx.warp_slot(now + 3_001);

    // Gate 2: quiescence — the market is still in Collect (round not drained).
    assert!(
        ctx.try_apply_set_oracle(&pdas, &oracle_b).is_err(),
        "apply on an un-drained market must fail (MarketNotQuiescent)"
    );

    // Drain the (empty) round: fold → finalize → reset every shard. The market
    // parks in Discovered with shards_ready == num_slab_shards (PAUSE_ROLL
    // blocks the roll — exactly the §3.2 wind-down state).
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d.max(now + 3_001));
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.try_reset_shard(&pdas, 0)
        .expect("reset the single shard");

    // Keep the staged feed FRESH at apply time (warping advanced the clock).
    ctx.set_oracle(&oracle_b, 200_000, -8);

    // All gates pass: the repoint commits atomically.
    ctx.try_apply_set_oracle(&pdas, &oracle_b)
        .expect("apply after delay + pause + quiescence + live feed");
    assert_eq!(ctx.market(&pdas).oracle, oracle_b, "oracle repointed");

    // The new feed is authoritative: funding reads it.
    ctx.update_funding(&pdas, &oracle_b);
}
