//! Plan task 2.1 — censorship resistance.
//!
//! A hostile cranker cannot get a round finalized while it deliberately skips
//! (censors) some resting order: `finalize_clear` fails the completeness check
//! while any Resting order remains un-accumulated. A *non-initial* signer can
//! then accumulate exactly the skipped order, after which `finalize_clear`
//! succeeds. The skipped order is in no way disadvantaged.

use tempo_integration_tests::*;

#[test]
fn skipped_order_blocks_finalize_until_a_different_signer_includes_it() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 8);

    // 4 orders in slots 0..=3.
    let t0 = ctx.new_funded_signer();
    let t1 = ctx.new_funded_signer();
    let t2 = ctx.new_funded_signer();
    let victim = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &t0, SIDE_BUY, 50, 20);
    ctx.submit_order(&pdas, &t1, SIDE_SELL, 30, 20);
    ctx.submit_order(&pdas, &t2, SIDE_BUY, 50, 10);
    // The victim order sits in slot 3 and will be censored by the hostile crank.
    let victim_id = ctx.submit_order(&pdas, &victim, SIDE_SELL, 50, 10);

    // --- Hostile cranker folds only slots 0..=2, censoring slot 3. ---
    let hostile = ctx.new_funded_signer();
    ctx.process_chunk_by(&pdas, &hostile, 0, 3);
    // Authoritative counts (PERF-1): folded count is the histogram's accumulated_count;
    // the live-order count is the slab header's count.
    assert_eq!(ctx.histogram(&pdas).accumulated_count, 3);
    assert_eq!(ctx.order_slab(&pdas).count, 4);

    // The censored order is still Resting (not accumulated).
    let victim_order = ctx
        .orders(&pdas)
        .into_iter()
        .find(|o| o.order_id == victim_id)
        .unwrap();
    assert_eq!(
        victim_order.status, STATUS_RESTING,
        "victim order still resting"
    );

    // Completeness check blocks finalize while the Resting order is unaccumulated.
    assert!(
        ctx.try_finalize_clear(&pdas).is_err(),
        "finalize must fail with a censored order"
    );

    // --- A different, non-initial signer accumulates exactly the skipped order. ---
    let rescuer = ctx.new_funded_signer();
    ctx.process_chunk_by(&pdas, &rescuer, 3, 1);
    assert_eq!(ctx.histogram(&pdas).accumulated_count, 4);
    let victim_order = ctx
        .orders(&pdas)
        .into_iter()
        .find(|o| o.order_id == victim_id)
        .unwrap();
    assert_eq!(
        victim_order.status, STATUS_ACCUMULATED,
        "victim order now accumulated"
    );

    // Now finalize succeeds.
    ctx.finalize_clear(&pdas);
    assert_eq!(ctx.market(&pdas).phase, PHASE_DISCOVERED);

    // And the previously-censored order can settle its fill like any other.
    let (_, _fill) = ctx.settle_fill(&pdas, victim_id);
    let victim_order = ctx
        .orders(&pdas)
        .into_iter()
        .find(|o| o.order_id == victim_id)
        .unwrap();
    assert_eq!(victim_order.status, STATUS_CONSUMED, "victim order settled");
}
