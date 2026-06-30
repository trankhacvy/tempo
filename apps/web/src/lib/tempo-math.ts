// Pure TS mirrors of the on-chain margin/mark math (program/src/margin.rs,
// program/src/mark.rs). All on-chain integer quantities are `bigint`; `number`
// appears only for derived display ratios. No floats touch the chain values.
//
// ⚠️ v1 unit assumption (margin.rs): collateral, realized_pnl, and
// `|size| · price` share one base unit. The web app surfaces raw base units.

/** Unrealized PnL of a signed position marked at `mark` (margin.rs::unrealized_pnl).
 *  Long gains as the mark rises; short gains as it falls. */
export function unrealizedPnl(size: bigint, entryPrice: bigint, markPrice: bigint): bigint {
    return size * (markPrice - entryPrice);
}

/** Account equity = collateral + realized PnL + unrealized PnL (margin.rs::equity). */
export function equity(collateral: bigint, realizedPnl: bigint, unrealized: bigint): bigint {
    return collateral + realizedPnl + unrealized;
}

/** Maintenance margin = `|size| · mark · maintenance_bps / 10_000` (margin.rs::maintenance_margin). */
export function maintenanceMargin(size: bigint, markPrice: bigint, maintenanceBps: number): bigint {
    const notional = abs(size) * markPrice;
    return (notional * BigInt(maintenanceBps)) / 10_000n;
}

/** A position is liquidatable when equity < maintenance margin (margin.rs::is_liquidatable). */
export function isLiquidatable(eq: bigint, maintenance: bigint): boolean {
    return eq < maintenance;
}

/** Margin ratio = equity / maintenance margin, as a display `number`. Returns
 *  `null` for a flat position (no maintenance requirement / divide-by-zero). */
export function marginRatio(eq: bigint, maintenance: bigint): number | null {
    if (maintenance <= 0n) return null;
    return Number(eq) / Number(maintenance);
}

/** Price at which equity hits the maintenance margin (the liquidation price),
 *  derived from margin.rs. Returns `null` for a flat position.
 *
 *  Solve for the mark `p` where equity == maintenance:
 *    collateral + realizedPnl + size·(p − entry) == |size|·p·mmBps/10_000
 *  Let base = collateral + realizedPnl − size·entry, m = mmBps/10_000.
 *    long  (size>0): base + size·p == size·p·m  → p = base / (size·(m − 1))
 *    short (size<0): base + size·p == −size·p·m → p = base / (−size·(m − 1) ... )
 *  Implemented with integer bps arithmetic; rounds to the nearest whole tick.
 *  The on-chain check is the source of truth — this is an estimate for display. */
export function liquidationPrice(
    size: bigint,
    entryPrice: bigint,
    collateral: bigint,
    realizedPnl: bigint,
    maintenanceBps: number,
): bigint | null {
    if (size === 0n) return null;
    const bps = BigInt(maintenanceBps);
    // base = (collateral + realizedPnl − size·entry), scaled by 10_000.
    const base = (collateral + realizedPnl - size * entryPrice) * 10_000n;
    // denom = size·(10_000 − mmBps) for a long; for a short the maintenance term
    // flips sign: size·(10_000 + mmBps). (size carries the long/short sign.)
    const denom = size > 0n ? size * (10_000n - bps) : size * (10_000n + bps);
    if (denom === 0n) return null;
    const p = base / denom;
    return p > 0n ? p : 0n;
}

/** Mark-price proxy from a market's last clearing prices, mirroring the
 *  no-oracle branches of mark.rs::compute_mark_price:
 *    both sides crossed → midpoint; one side → that side; neither → `null`.
 *  The oracle-band clamp is not reproduced here (the web view lacks the oracle
 *  price value); this is the best on-chain-derived mark available client-side. */
export function markPrice(lastBidFillPrice: bigint, lastAskFillPrice: bigint): bigint | null {
    const bid = lastBidFillPrice > 0n;
    const ask = lastAskFillPrice > 0n;
    if (bid && ask) return (lastBidFillPrice + lastAskFillPrice) / 2n;
    if (bid) return lastBidFillPrice;
    if (ask) return lastAskFillPrice;
    return null;
}

function abs(v: bigint): bigint {
    return v < 0n ? -v : v;
}

// Tick ↔ USD conversion. The on-chain window is oracle-anchored (v7): tick 0 is
// `window_floor_price`, so `price_1e8 = window_floor + tick · tickSize`
// (Market::tick_to_price). Fixed 1e8 fixed-point.
const PRICE_SCALE = 100_000_000n;

/** A 1e8 fixed-point price (e.g. a market's `last_bid_fill_price`, already a price
 *  not a tick) → USD. Non-positive maps to `null`. */
export function price1e8ToUsd(price1e8: bigint): number | null {
    if (price1e8 <= 0n) return null;
    return Number(price1e8) / Number(PRICE_SCALE);
}

/** A tick index → USD (number, for display/charting), anchored on the round's
 *  window floor. A non-positive resulting price maps to `null`. */
export function tickToUsd(
    tick: bigint,
    tickSize: bigint,
    windowFloor: bigint = 0n,
): number | null {
    if (tickSize <= 0n) return null;
    const price1e8 = windowFloor + tick * tickSize;
    if (price1e8 <= 0n) return null;
    return Number(price1e8) / Number(PRICE_SCALE);
}

/** A live USD price → the nearest in-window tick index for a market's tick size. */
export function usdToTick(
    usd: number,
    tickSize: bigint,
    windowFloor: bigint = 0n,
): bigint {
    if (usd <= 0 || tickSize <= 0n) return 0n;
    const price1e8 = BigInt(Math.round(usd * Number(PRICE_SCALE)));
    const tick = (price1e8 - windowFloor) / tickSize;
    return tick > 0n ? tick : 0n;
}

/** A base-unit quantity for a USD notional at `usd` price (floor, ≥ 1). */
export function notionalToQty(notionalUsd: number, usd: number): bigint {
    if (notionalUsd <= 0 || usd <= 0) return 0n;
    const qty = Math.floor(notionalUsd / usd);
    return qty >= 1 ? BigInt(qty) : 1n;
}
