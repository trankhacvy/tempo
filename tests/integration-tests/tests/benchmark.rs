//! Clearing benchmark (the headline deliverable).
//!
//! Measures the compute-unit (CU) profile of every hot-path instruction under
//! LiteSVM and derives the orders-per-block ceiling from Solana's per-account
//! write-lock budget (12M CU/account/block) and per-transaction limit
//! (1.4M CU/tx). Writes the result to `docs/bench/cu_report.md`.
//!
//! This measures the Stage A **sharded** design — the OrderSlab is split into
//! `num_slab_shards` shards (`SLAB_CAP` orders each) that submit and fold in parallel;
//! the single AuctionHistogram and Market stay shared. Per-instruction CU matches the
//! pre-shard baseline (`cu_report_pre_shard.md`); the win is parallel intake across shards.
//!
//! Run with:  cargo test -p tempo-integration-tests --test benchmark -- --ignored --nocapture
//!
//! NOTE: LiteSVM CU accounting is a faithful proxy for the on-chain meter but is
//! not guaranteed identical to mainnet-beta. The *relative* profile, the scaling
//! with num_ticks / chunk size, and the derived ceilings are the signal.

use std::fmt::Write as _;
use std::path::PathBuf;

use tempo_integration_tests::*;

/// Per-transaction CU limit (Solana compute budget).
const TX_CU_LIMIT: u64 = 1_400_000;
/// Per-account write-lock CU budget per block (Solana scheduler).
const ACCOUNT_BLOCK_CU: u64 = 12_000_000;
/// Stage A per-shard slab capacity. A shard is created by `init_shard` via a single CPI
/// `CreateAccount`, which the runtime caps at 10_240 bytes — so at the current
/// `ORDER_LEN = 112` (Stage C1) a shard holds at most 90 orders; the plan (§0.3) fixed
/// the cap at 90 to stay within one create through all stages. Throughput scales by
/// running `num_slab_shards` of these in parallel.
const SLAB_CAP: u32 = 90;

/// Measure the CU of submitting the `occupancy+1`-th order into a fresh market
/// (so the find_free_slot + per-trader-count scans run over `occupancy` orders).
fn measure_submit_cu(occupancy: u32) -> u64 {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 256, SLAB_CAP);
    // Fill `occupancy` orders across enough traders (per-trader cap = 8).
    let mut filled = 0u32;
    while filled < occupancy {
        let t = ctx.new_funded_signer();
        let batch = core::cmp::min(8, occupancy - filled);
        for _ in 0..batch {
            // distinct-ish prices within the 256-tick window (price = tick + 1).
            // Plain taker orders — filling the slab is the point, not crossing.
            let price = 1 + (filled as u64 % 200);
            ctx.submit_order(&pdas, &t, SIDE_BUY, price, 5);
            filled += 1;
        }
    }
    // Measure one more submit (fresh trader so the per-trader scan is cheap).
    let t = ctx.new_funded_signer();
    ctx.try_submit_order(&pdas, &t, SIDE_BUY, 7, 5)
        .expect("submit")
        .compute_units_consumed
}

/// Measure the CU of folding `k` orders in a single process_chunk.
fn measure_chunk_cu(k: u32) -> u64 {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 256, SLAB_CAP);
    let mut filled = 0u32;
    while filled < k {
        let t = ctx.new_funded_signer();
        let batch = core::cmp::min(8, k - filled);
        for _ in 0..batch {
            // Uniform taker-buys at one price → all fold into the same region/bucket,
            // isolating the per-order fold cost (clean O(orders)).
            ctx.submit_order(&pdas, &t, SIDE_BUY, 100, 5);
            filled += 1;
        }
    }
    ctx.process_chunk(&pdas, 0, k).compute_units_consumed
}

/// Measure finalize_clear CU at a given num_ticks (the O(ticks) discovery pass).
fn measure_finalize_cu(num_ticks: u32) -> u64 {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, num_ticks, 16);
    let b = ctx.new_funded_signer();
    let s = ctx.new_funded_signer();
    // Cross at a mid-book price (tick num_ticks/2 → price num_ticks/2): taker-buy
    // vs maker-sell (ask auction). Maker liquidity comes from the quote book.
    let price = (num_ticks / 2).max(1) as u64;
    ctx.submit_order(&pdas, &b, SIDE_BUY, price, 10);
    ctx.post_maker_order(&pdas, &s, SIDE_SELL, price, 10);
    ctx.process_chunk(&pdas, 0, 16);
    ctx.process_maker_quote(&pdas, &s.pubkey());
    ctx.finalize_clear(&pdas).compute_units_consumed
}

/// Measure finalize_clear CU with `num_shards` shards passed, each holding a folded
/// order, at the max tick count. This isolates the Design-Z (DDR-1) per-shard
/// completeness scan: finalize scans every shard it is passed (`all_active_orders_
/// accumulated`, an O(capacity) loop per shard) on top of the O(ticks) discovery pass.
/// The delta vs the 1-shard finalize is the price of proving completeness across K
/// shards in one tx — the thing that grows with K·SLAB_CAP.
fn measure_finalize_sharded_cu(num_shards: u16) -> u64 {
    let mut ctx = TestContext::new();
    ctx.market_num_slab_shards = num_shards;
    let pdas = ctx.init_market(1, 256, SLAB_CAP);
    let price = 128u64; // mid-window
                        // One taker buy into EVERY shard (so finalize must scan all K), plus one maker sell
                        // to actually cross (ask auction). Each shard is then folded.
    for shard in 0..num_shards {
        let t = ctx.new_funded_signer();
        ctx.submit_order_to_shard(&pdas, &t, SIDE_BUY, price, 5, shard);
    }
    let s = ctx.new_funded_signer();
    ctx.post_maker_order(&pdas, &s, SIDE_SELL, price, 5);
    for shard in 0..num_shards {
        ctx.process_chunk_shard(&pdas, shard, 0, SLAB_CAP);
    }
    ctx.process_maker_quote(&pdas, &s.pubkey());
    ctx.finalize_clear(&pdas).compute_units_consumed
}

/// Measure settle_fill CU (includes the marginal-tick cumulative scan).
fn measure_settle_cu() -> u64 {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 256, 16);
    let b = ctx.new_funded_signer();
    let s = ctx.new_funded_signer();
    // Taker-buy crosses maker-sell (ask auction). We measure settle_fill on the
    // taker order; maker liquidity comes from the quote book.
    let buy = ctx.submit_order(&pdas, &b, SIDE_BUY, 100, 10);
    ctx.post_maker_order(&pdas, &s, SIDE_SELL, 100, 10);
    ctx.process_chunk(&pdas, 0, 16);
    ctx.process_maker_quote(&pdas, &s.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, buy).0.compute_units_consumed
}

/// Measure reset_shard + start_auction CU (the roll tail — P5.1).
fn measure_reset_and_roll_cu() -> (u64, u64) {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 256, 16);
    ctx.process_chunk(&pdas, 0, 16);
    ctx.finalize_clear(&pdas);
    let reset = ctx
        .try_reset_shard(&pdas, 0)
        .expect("reset")
        .compute_units_consumed;
    let roll = ctx
        .try_start_auction(&pdas)
        .expect("roll")
        .compute_units_consumed;
    (reset, roll)
}

/// P5.1 — the ROUND-TAIL model (docs/bench/round_latency.md): per-phase tx
/// counts and Σ CU for one full round at the dev-default 16 shards × 90 orders,
/// from measured per-instruction CU. This is the deterministic half of the C2
/// (double-buffered histogram) decision; the wall-clock half comes from the
/// devnet orchestrator run recorded in the same report.
#[test]
#[ignore = "benchmark; run with --ignored --nocapture"]
fn benchmark_round_tail_model() {
    const SHARDS: u64 = 16;
    const ORDERS: u64 = SHARDS * SLAB_CAP as u64; // 1,440

    let submit_cu = measure_submit_cu(SLAB_CAP - 1);
    let chunk_cu = measure_chunk_cu(SLAB_CAP);
    let finalize_cu = measure_finalize_sharded_cu(SHARDS as u16);
    let settle_cu = measure_settle_cu();
    let (reset_cu, roll_cu) = measure_reset_and_roll_cu();

    let phases: [(&str, u64, u64); 6] = [
        ("intake (submit_order)", ORDERS, submit_cu),
        ("accumulate (process_chunk ×shards)", SHARDS, chunk_cu),
        ("discover (finalize_clear)", 1, finalize_cu),
        ("settle (settle_fill ×orders)", ORDERS, settle_cu),
        ("reset (reset_shard ×shards)", SHARDS, reset_cu),
        ("roll (start_auction)", 1, roll_cu),
    ];

    println!(
        "\n# Round-tail model — 16 shards × {} orders/shard",
        SLAB_CAP
    );
    println!("| phase | txs | CU/tx | Σ CU |");
    println!("|---|---|---|---|");
    for (name, txs, cu) in &phases {
        println!("| {} | {} | {} | {} |", name, txs, cu, txs * cu);
    }
    let settle_total = ORDERS * settle_cu;
    let reset_total = SHARDS * reset_cu;
    let tail = settle_total + reset_total + roll_cu;
    println!(
        "\nserial tail (settle+reset+roll): {} txs, Σ {} CU ≈ {:.1} Market-write blocks",
        ORDERS + SHARDS + 1,
        tail,
        tail as f64 / ACCOUNT_BLOCK_CU as f64
    );
    println!(
        "collect window: {} slots — the tail C2 would overlap is ~{:.1} blocks of Market-locked work",
        2, // COLLECT_WINDOW_SLOTS
        tail as f64 / ACCOUNT_BLOCK_CU as f64
    );

    assert!(reset_cu < TX_CU_LIMIT && roll_cu < TX_CU_LIMIT);
    assert!(settle_cu > 0 && reset_cu > 0 && roll_cu > 0);
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

#[test]
#[ignore = "benchmark; run with --ignored to regenerate cu_report.md"]
fn benchmark_cu_profile() {
    // --- submit_order: CU vs slab occupancy (shows the O(n) free-slot scan) ---
    let submit: Vec<(u32, u64)> = [1u32, 30, 60, 89]
        .iter()
        .map(|&occ| (occ, measure_submit_cu(occ)))
        .collect();

    // --- process_chunk: fold cost at the clean endpoints (1 order vs a full
    // shard, SLAB_CAP orders). The per-order fold cost is small relative to the
    // fixed base, so we report the endpoints and the incremental cost rather than a
    // noisy multi-point series. ---
    let chunk: Vec<(u32, u64)> = [1u32, SLAB_CAP]
        .iter()
        .map(|&k| (k, measure_chunk_cu(k)))
        .collect();
    let (k_lo, cu_lo) = chunk[0];
    let (k_hi, cu_hi) = chunk[1];
    let per_order = (cu_hi.saturating_sub(cu_lo)) / (k_hi - k_lo) as u64;
    let base_chunk = cu_lo.saturating_sub(per_order * k_lo as u64);
    // Max orders in a single chunk tx under the 1.4M CU/tx limit.
    let max_orders_per_chunk_tx = (TX_CU_LIMIT - base_chunk) / per_order.max(1);

    // --- finalize_clear: CU vs num_ticks (O(ticks) pass over 4 regions) ---
    let finalize: Vec<(u32, u64)> = [64u32, 128, 256]
        .iter()
        .map(|&t| (t, measure_finalize_cu(t)))
        .collect();
    let (max_ticks, max_finalize_cu) = *finalize.last().unwrap();

    // --- finalize_clear: CU vs shard count (the Design-Z per-shard completeness scan
    // at max ticks). Isolates the O(K·capacity) shard scan on top of the O(ticks) pass. ---
    let finalize_sharded: Vec<(u16, u64)> = [1u16, 8, 16]
        .iter()
        .map(|&k| (k, measure_finalize_sharded_cu(k)))
        .collect();
    let (max_shards, max_shards_finalize_cu) = *finalize_sharded.last().unwrap();

    // --- settle_fill ---
    let settle_cu = measure_settle_cu();

    // --- derived: per-shard write-lock load of one full shard (SLAB_CAP orders) ---
    // Settle write-locks Market for the OI update, so a shard's settle CU competes for
    // Market's 12M/block budget. Submission now write-locks only its own shard (Market is
    // read-only on submit since PERF-1), so K shards submit in parallel; this models one
    // shard's end-to-end load, and aggregate throughput is K× the parallel part.
    const FULL_AUCTION_ORDERS: u64 = SLAB_CAP as u64;
    let submit_cu_worst = submit.last().unwrap().1;
    let submit_total = FULL_AUCTION_ORDERS * submit_cu_worst;
    let settle_total = FULL_AUCTION_ORDERS * settle_cu;
    let market_load = submit_total + cu_hi + max_finalize_cu + settle_total;
    let blocks_for_full_auction = market_load.div_ceil(ACCOUNT_BLOCK_CU);

    // --- build the report ---
    let mut r = String::new();
    let _ = writeln!(r, "# Tempo Clearing Benchmark (CU profile)\n");
    let _ = writeln!(
        r,
        "Measured under LiteSVM 0.13 (in-process SVM). CU accounting is a faithful \
proxy for the on-chain meter; treat the *relative profile*, the *scaling*, and the \
*derived ceilings* as the signal, not the absolute digits. This is the **Stage A sharded** \
design: the OrderSlab is split into `num_slab_shards` shards, each holding up to \
`SLAB_CAP` orders, that submit and fold in parallel (submit is read-only on Market since \
PERF-1); the single AuctionHistogram and Market remain shared. Per-instruction CU is \
unchanged from the pre-shard baseline (`cu_report_pre_shard.md`) — sharding adds no \
per-tx overhead; it multiplies intake throughput by the shard count.\n"
    );
    let _ = writeln!(
        r,
        "Solana limits used: **{} CU/tx**, **{} CU/account/block** (write-lock).\n",
        TX_CU_LIMIT, ACCOUNT_BLOCK_CU
    );

    let _ = writeln!(r, "## submit_order — CU vs slab occupancy\n");
    let _ = writeln!(r, "Writes Market + OrderSlab. The cost grows with occupancy because `find_free_slot` and the per-trader-cap count scan the slab (O(n)).\n");
    let _ = writeln!(r, "| orders already resting | CU |");
    let _ = writeln!(r, "|---|---|");
    for (occ, cu) in &submit {
        let _ = writeln!(r, "| {} | {} |", occ, cu);
    }

    let _ = writeln!(r, "\n## process_chunk — fold cost\n");
    let _ = writeln!(r, "Writes the shard + Market + AuctionHistogram. Folding is O(orders) but the per-order cost is small next to the fixed base (event CPI + account I/O), so we report the clean endpoints: one order vs a full shard (SLAB_CAP orders) in a single chunk. Shards fold in parallel into the one histogram (commutative addition).\n");
    let _ = writeln!(r, "| orders folded | CU |");
    let _ = writeln!(r, "|---|---|");
    for (k, cu) in &chunk {
        let _ = writeln!(r, "| {} | {} |", k, cu);
    }
    let _ = writeln!(
        r,
        "\nIncremental: ~**{} CU base** + ~**{} CU/order**. A single chunk tx could fold \
~**{}** orders under the {} CU/tx limit — far more than a slab can hold — so **folding \
compute is not the constraint**; the slab's single-account size cap is.\n",
        base_chunk, per_order, max_orders_per_chunk_tx, TX_CU_LIMIT
    );

    let _ = writeln!(r, "## finalize_clear — CU vs num_ticks\n");
    let _ = writeln!(
        r,
        "One transaction; a single O(ticks) pass over the 4 histogram regions (both crosses).\n"
    );
    let _ = writeln!(r, "| num_ticks | CU |");
    let _ = writeln!(r, "|---|---|");
    for (t, cu) in &finalize {
        let _ = writeln!(r, "| {} | {} |", t, cu);
    }
    let _ = writeln!(
        r,
        "\nAt the max supported {} ticks finalize uses ~{} CU — {:.1}% of the {} CU/tx \
limit, so the discovery pass fits comfortably in one tx across the whole tick range.\n",
        max_ticks,
        max_finalize_cu,
        100.0 * max_finalize_cu as f64 / TX_CU_LIMIT as f64,
        TX_CU_LIMIT
    );

    let _ = writeln!(
        r,
        "## finalize_clear — CU vs shard count (Design-Z completeness scan)\n"
    );
    let _ = writeln!(
        r,
        "At 256 ticks, passing K shards (each folded). On top of the O(ticks) discovery \
pass, finalize scans every shard it is passed (`all_active_orders_accumulated`, an \
O(capacity) loop per shard) to prove completeness in one tx (DDR-1). This is the cost \
that grows with K·SLAB_CAP.\n"
    );
    let _ = writeln!(r, "| shards passed | CU |");
    let _ = writeln!(r, "|---|---|");
    for (k, cu) in &finalize_sharded {
        let _ = writeln!(r, "| {} | {} |", k, cu);
    }
    let _ = writeln!(
        r,
        "\nAt the dev target of {} shards × {} cap, finalize uses ~{} CU — {:.1}% of the \
{} CU/tx limit. The per-shard scan adds ~{} CU/shard on top of the ~{} CU tick pass, so \
finalize stays a single tx well under the limit at the dev target; K·SLAB_CAP is the \
cost to watch as shard count grows (DDR-1 re-review trigger: chunked finalize past \
~40 shards).\n",
        max_shards,
        SLAB_CAP,
        max_shards_finalize_cu,
        100.0 * max_shards_finalize_cu as f64 / TX_CU_LIMIT as f64,
        TX_CU_LIMIT,
        (max_shards_finalize_cu.saturating_sub(finalize_sharded[0].1))
            / (max_shards.saturating_sub(1)).max(1) as u64,
        finalize_sharded[0].1,
    );

    let _ = writeln!(r, "## settle_fill — CU\n");
    let _ = writeln!(
        r,
        "One order per tx (writes Market + OrderSlab, + Position when filled). Includes \
the marginal-tick cumulative scan. Measured: **{} CU**.\n",
        settle_cu
    );

    let _ = writeln!(r, "## The former hard ceiling — now sharded away\n");
    let _ = writeln!(
        r,
        "Solana caps a CPI-created/grown account at **10_240 bytes** per instruction \
(`MAX_PERMITTED_DATA_INCREASE`). At `ORDER_LEN = 112` (Stage C1) one OrderSlab account tops out near \
**90 orders**, which is why the pre-shard design was capped at 128 and could not reach \
\"thousands of orders\". **Stage A removes this by sharding**: the slab is split into \
`num_slab_shards` independent shard accounts (`init_shard`), each sized at `SLAB_CAP` \
(90 orders, kept within one CPI `CreateAccount` through every stage). N shards ⇒ N·SLAB_CAP \
orders/round with no per-tx overhead, and — because submit is read-only on Market and each \
shard is its own account — submissions and settlements to different shards run in parallel. \
The single histogram is still O(ticks) and untouched. Completeness stays a hard gate: \
`finalize_clear` refuses until every shard it is passed scans as fully folded (Design Z), an O(K) \
check backed by a per-shard confirming scan.\n"
    );

    let _ = writeln!(r, "## Throughput of one full shard (SLAB_CAP orders)\n");
    let _ = writeln!(r, "Settle write-locks Market (OI), so a shard's settle CU competes for Market's 12M-CU/block budget; submit is read-only on Market and hits only its own shard, so shards submit in parallel. Modelling one full shard end-to-end (aggregate = this × num_slab_shards for the parallel parts):\n");
    let _ = writeln!(r, "| phase | txs | CU each | CU total |");
    let _ = writeln!(r, "|---|---|---|---|");
    let _ = writeln!(
        r,
        "| submit (own shard, parallel) | {} | ~{} (occ 89) | ~{} |",
        FULL_AUCTION_ORDERS, submit_cu_worst, submit_total
    );
    let _ = writeln!(r, "| accumulate | 1 | ~{} | ~{} |", cu_hi, cu_hi);
    let _ = writeln!(
        r,
        "| finalize | 1 | ~{} | ~{} |",
        max_finalize_cu, max_finalize_cu
    );
    let _ = writeln!(
        r,
        "| settle | {} | ~{} | ~{} |",
        FULL_AUCTION_ORDERS, settle_cu, settle_total
    );
    let _ = writeln!(
        r,
        "| **total Market write-lock** | | | **~{} CU** |",
        market_load
    );
    let _ = writeln!(
        r,
        "\nA full shard puts ~{} CU on the Market write-lock — about {:.1}% of one block's \
12M budget — so one shard's settle-side load clears in ~{} block(s). Submission is now \
parallel across shards (Market read-only), so intake scales with `num_slab_shards`; the \
remaining shared serialization is the Market OI write on settle (a candidate for OI-sharding \
if the benchmark shows it is the wall).\n",
        market_load,
        100.0 * market_load as f64 / ACCOUNT_BLOCK_CU as f64,
        blocks_for_full_auction
    );

    let _ = writeln!(r, "\n## Reading the result\n");
    let _ = writeln!(
        r,
        "- The clearing math (`finalize_clear`) is **not** the bottleneck — it is O(ticks), \
one tx, and fits the per-tx limit comfortably across the whole supported tick range.\n\
- The ceiling is **write-lock contention on the shared accounts** (Market / OrderSlab / \
Histogram). Submission, accumulation and settlement each serialize per market within a block.\n\
- `submit_order` and `settle_fill` cost grows with slab occupancy (the O(n) scans) — \
the slab-scan/free-list optimization and de-hot-pathing Market would lift the submit/settle \
ceiling; sharding the slab + histogram would lift the accumulation ceiling. Whether that \
work is warranted is exactly what these numbers are meant to decide.\n"
    );

    let path = workspace_root().join("docs/bench/cu_report.md");
    std::fs::write(&path, r).expect("write cu_report.md");
    eprintln!("wrote {}", path.display());

    // Sanity guards (also assert nothing regressed catastrophically):
    assert!(
        max_finalize_cu < TX_CU_LIMIT,
        "finalize at max ticks must fit one tx"
    );
    assert!(
        max_shards_finalize_cu < TX_CU_LIMIT,
        "finalize at max shards must fit one tx"
    );
    assert!(settle_cu < TX_CU_LIMIT, "settle must fit one tx");
    assert!(per_order > 0, "fold cost must be measurable");
}
