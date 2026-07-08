//! P5.5/P5.6 (missing-features §3.4): `close_position` (disc 44) and
//! `close_market` (disc 45) — the rent-reclaim half of `init_position` /
//! `initialize_market`. A close must be impossible while anything of value
//! (exposure, collateral, unflushed PnL, live orders, open interest) still
//! lives behind the account.

use tempo_integration_tests::*;

// ---------------------------------------------------------------- positions

/// A position with OPEN EXPOSURE cannot be closed — only trading/liquidation
/// flattens it; deleting the account is never an exit.
#[test]
fn close_position_rejects_non_flat() {
    let mut ctx = TestContext::new();
    let tick = 10u64;
    let pdas = ctx.init_market(tick, 16, 64);

    // One crossed round leaves the taker with size −12 (clearing-only market:
    // collateral 0, realized 0 — size alone must block the close).
    let mb = ctx.new_funded_signer();
    let ts = ctx.new_funded_signer();
    ctx.init_position(&pdas, &mb);
    ctx.post_maker_order(&pdas, &mb, SIDE_BUY, 4 * tick, 12);
    let o = ctx.submit_order(&pdas, &ts, SIDE_SELL, 4 * tick, 20);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &mb.pubkey());
    ctx.finalize_clear(&pdas);
    let (_m, fill) = ctx.settle_fill(&pdas, o);
    assert_eq!(fill, 12);

    let err = ctx
        .try_close_position(&pdas, &ts)
        .expect_err("a non-flat position must not close");
    assert!(
        format!("{:?}", err.err).contains("Custom(17)"),
        "rejected InvalidOrderStatus(17), got {:?}",
        err.err
    );
    assert!(
        ctx.lamports(&ctx.position_pda(&pdas, &ts.pubkey()).0) > 0,
        "the position account survived"
    );
}

/// A cross-margin group member cannot be closed while enrolled — it is part of
/// the group's solvency set. Leaving the group first makes it closable.
#[test]
fn close_position_rejects_cross_member_until_removed() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    let owner = ctx.new_funded_signer();
    ctx.init_position(&pdas, &owner);
    let (pos, _) = ctx.position_pda(&pdas, &owner.pubkey());
    ctx.init_margin_account(&owner);
    ctx.add_position_to_margin(&pdas, &owner, &pos)
        .expect("enroll the flat position");

    // Enrolled (margin_mode == 1) → close rejected even though it is flat.
    let err = ctx
        .try_close_position(&pdas, &owner)
        .expect_err("a cross-group member must not close");
    assert!(
        format!("{:?}", err.err).contains("Custom(17)"),
        "rejected InvalidOrderStatus(17), got {:?}",
        err.err
    );

    // Leave the group → the same close now succeeds.
    ctx.remove_position_from_margin(&owner, &pos)
        .expect("leave the group");
    ctx.try_close_position(&pdas, &owner)
        .expect("closable once isolated again");
    assert_eq!(ctx.lamports(&pos), 0, "the position account is gone");
}

/// Happy path: a flat, drained, isolated position closes and its rent lands on
/// the owner. Only the owner may do it.
#[test]
fn close_position_refunds_rent_to_the_owner() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    let owner = ctx.new_funded_signer();
    ctx.init_position(&pdas, &owner);
    let (pos, _) = ctx.position_pda(&pdas, &owner.pubkey());
    let rent = ctx.lamports(&pos);
    assert!(rent > 0);

    // A stranger signing as "owner" is rejected (the position PDA is derived
    // from the real owner, so the stranger's derived PDA simply doesn't exist;
    // passing the real position under the stranger's key fails the owner check).
    let stranger = ctx.new_funded_signer();
    assert!(
        ctx.try_close_position(&pdas, &stranger).is_err(),
        "a stranger cannot close someone else's position"
    );

    let owner_before = ctx.lamports(&owner.pubkey());
    ctx.try_close_position(&pdas, &owner).expect("close");
    assert_eq!(ctx.lamports(&pos), 0, "position account closed");
    assert!(
        ctx.lamports(&owner.pubkey()) > owner_before,
        "rent (minus the tx fee the payer covers) flowed to the owner"
    );
}

// ------------------------------------------------------------------ markets

/// Every quiescence gate rejects with `MarketNotQuiescent` (49); only the
/// authority may even try.
#[test]
fn close_market_gates_reject_by_code() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);
    let authority = ctx.market_authority_keypair(&pdas).expect("authority");

    // A resting order arms for the NEXT round, then an empty round is driven to
    // Discovered (this also creates the ClearingResult, which a fresh market
    // does not have yet — without it the close fails on account shape, not gates).
    let t = ctx.new_funded_signer();
    let o = ctx.submit_order(&pdas, &t, SIDE_BUY, 30, 5);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    // Zero fill → the GTC order re-arms RESTING (it will squat its shard).
    ctx.settle_fill(&pdas, o);

    // A stranger is rejected on authority, not quiescence.
    let stranger = ctx.new_funded_signer();
    assert!(
        ctx.try_close_market(&pdas, &stranger).is_err(),
        "non-authority cannot close a market"
    );

    // UNPAUSED (though post-clearing) → MarketNotQuiescent.
    let err = ctx
        .try_close_market(&pdas, &authority)
        .expect_err("an unpaused market must not close");
    assert!(
        format!("{:?}", err.err).contains("Custom(49)"),
        "rejected MarketNotQuiescent(49), got {:?}",
        err.err
    );

    // Paused but shards not reset yet (shards_ready 0) → still 49.
    ctx.set_pause(&pdas, 3); // PAUSE_ALL
    let err = ctx
        .try_close_market(&pdas, &authority)
        .expect_err("un-reset shards must block the close");
    assert!(
        format!("{:?}", err.err).contains("Custom(49)"),
        "rejected MarketNotQuiescent(49), got {:?}",
        err.err
    );

    // Reset every shard (the resting order SURVIVES a reset) → the market-level
    // gates now pass, but the non-empty shard blocks with the same loud code.
    for shard_id in 0..pdas.num_slab_shards {
        ctx.reset_shard(&pdas, shard_id);
    }
    let err = ctx
        .try_close_market(&pdas, &authority)
        .expect_err("a shard holding a live order must block the close");
    assert!(
        format!("{:?}", err.err).contains("Custom(49)"),
        "rejected MarketNotQuiescent(49) at the shard scan, got {:?}",
        err.err
    );
    assert!(
        ctx.lamports(&pdas.market) > 0,
        "the market survived every rejected attempt"
    );
}

/// Happy path: a fully drained, paused, post-clearing market closes — every
/// PDA (shards, histogram, clearing result, market) is reclaimed and the rent
/// lands on the authority.
#[test]
fn close_market_reclaims_every_account() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);
    let authority = ctx.market_authority_keypair(&pdas).expect("authority");

    // Drive one EMPTY round to a quiescent end-state: fold nothing, discover,
    // then reset every shard (shards_ready == num) WITHOUT rolling.
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    for shard_id in 0..pdas.num_slab_shards {
        ctx.reset_shard(&pdas, shard_id);
    }
    ctx.set_pause(&pdas, 3); // PAUSE_ALL: no intake, no roll

    let rent_total: u64 = std::iter::once(ctx.lamports(&pdas.market))
        .chain(std::iter::once(ctx.lamports(&pdas.histogram)))
        .chain(std::iter::once(ctx.lamports(&pdas.clearing)))
        .chain((0..pdas.num_slab_shards).map(|s| ctx.lamports(&pdas.slab_shard(s).0)))
        .sum();
    assert!(rent_total > 0);
    let authority_before = ctx.lamports(&authority.pubkey());

    ctx.try_close_market(&pdas, &authority).expect("close");

    assert_eq!(ctx.lamports(&pdas.market), 0, "market closed");
    assert_eq!(ctx.lamports(&pdas.histogram), 0, "histogram closed");
    assert_eq!(ctx.lamports(&pdas.clearing), 0, "clearing result closed");
    for shard_id in 0..pdas.num_slab_shards {
        assert_eq!(
            ctx.lamports(&pdas.slab_shard(shard_id).0),
            0,
            "shard {shard_id} closed"
        );
    }
    assert_eq!(
        ctx.lamports(&authority.pubkey()),
        authority_before + rent_total,
        "every account's rent landed on the authority"
    );
}
