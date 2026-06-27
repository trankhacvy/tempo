import { address, type Address } from "@solana/kit";

import { PROGRAM_ID } from "./config";
import { getRpc } from "./rpc";
import { readU32le, readU64le, findAuctionHistogramHeaderPda, findOrderSlabHeaderPda } from "./tempo-client";
import { tickToUsd } from "./tempo-math";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface TickData {
    tick: number;           // tick index (0-based)
    demand: bigint;         // BidDemand + AskDemand at this tick
    supply: bigint;         // BidSupply + AskSupply at this tick
    priceUsd: number | null;
}

export interface HistogramView {
    numTicks: number;
    nonZeroTicks: TickData[];           // only ticks with demand > 0 or supply > 0
    estimatedClearingTick: number | null;
    estimatedClearingUsd: number | null;
    totalDemand: bigint;
    totalSupply: bigint;
}

export interface OrderView {
    slotIndex: number;
    orderId: bigint;
    side: 0 | 1;           // 0 = buy, 1 = sell
    price: bigint;
    quantity: bigint;
    remaining: bigint;
    status: 0 | 1 | 2 | 3; // Empty=0, Resting=1, Accumulated=2, Consumed=3
    priceUsd: number | null;
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

async function fetchRaw(addr: Address): Promise<{ owner: string; data: Uint8Array } | null> {
    const info = await getRpc().getAccountInfo(addr, { encoding: "base64" }).send();
    if (info.value === null) return null;
    const [b64] = info.value.data;
    return {
        owner: info.value.owner,
        data: Uint8Array.from(Buffer.from(b64, "base64")),
    };
}

// Minimal base58 encoder for a 32-byte address slice (matches data.ts).
const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
function encodeAddress(bytes: Uint8Array): string {
    let zeros = 0;
    while (zeros < bytes.length && bytes[zeros] === 0) zeros++;
    let value = 0n;
    for (const b of bytes) value = value * 256n + BigInt(b);
    let out = "";
    while (value > 0n) {
        const rem = Number(value % 58n);
        value /= 58n;
        out = B58[rem] + out;
    }
    return "1".repeat(zeros) + out;
}

// ---------------------------------------------------------------------------
// AuctionHistogram byte layout (absolute offsets):
//   [0]      disc = 2
//   [1]      version = 1
//   [2..10]  auction_id          u64 le
//   [10..18] accumulated_count   u64 le
//   [18..22] num_ticks           u32 le
//   [22..54] market              Address (32 bytes)
//   [54]     bump                u8
//   [55..]   4 regions × num_ticks × 8 bytes (u64 le):
//              Region 0 BidDemand:  55 ..          55 + T*8
//              Region 1 BidSupply:  55 + T*8    .. 55 + 2*T*8
//              Region 2 AskDemand:  55 + 2*T*8  .. 55 + 3*T*8
//              Region 3 AskSupply:  55 + 3*T*8  .. 55 + 4*T*8
// ---------------------------------------------------------------------------

const HISTOGRAM_DISC = 2;
const HISTOGRAM_HEADER = 55;

export async function fetchHistogramView(
    market: string,
    tickSize: bigint,
): Promise<HistogramView | null> {
    try {
        const [histPda] = await findAuctionHistogramHeaderPda({ market: address(market) });
        const raw = await fetchRaw(histPda);
        if (!raw) return null;
        if (raw.owner !== PROGRAM_ID) return null;

        const d = raw.data;
        if (d.length < HISTOGRAM_HEADER) return null;
        if (d[0] !== HISTOGRAM_DISC) return null;

        const numTicks = readU32le(d.subarray(18, 22));
        if (numTicks === 0) return null;

        const minLen = HISTOGRAM_HEADER + 4 * numTicks * 8;
        if (d.length < minLen) return null;

        const T = numTicks;
        const demand: bigint[] = [];
        const supply: bigint[] = [];
        let totalDemand = 0n;
        let totalSupply = 0n;

        for (let t = 0; t < T; t++) {
            const bidDemand = readU64le(d.subarray(HISTOGRAM_HEADER + t * 8, HISTOGRAM_HEADER + t * 8 + 8));
            const bidSupply = readU64le(d.subarray(HISTOGRAM_HEADER + T * 8 + t * 8, HISTOGRAM_HEADER + T * 8 + t * 8 + 8));
            const askDemand = readU64le(d.subarray(HISTOGRAM_HEADER + 2 * T * 8 + t * 8, HISTOGRAM_HEADER + 2 * T * 8 + t * 8 + 8));
            const askSupply = readU64le(d.subarray(HISTOGRAM_HEADER + 3 * T * 8 + t * 8, HISTOGRAM_HEADER + 3 * T * 8 + t * 8 + 8));
            const dm = bidDemand + askDemand;
            const sp = bidSupply + askSupply;
            demand.push(dm);
            supply.push(sp);
            totalDemand += dm;
            totalSupply += sp;
        }

        // Cumulative demand from high ticks down (buyers at price >= t)
        const cumDemand = new Array<bigint>(T).fill(0n);
        cumDemand[T - 1] = demand[T - 1]!;
        for (let t = T - 2; t >= 0; t--) {
            cumDemand[t] = cumDemand[t + 1]! + demand[t]!;
        }

        // Cumulative supply from low ticks up (sellers at price <= t)
        const cumSupply = new Array<bigint>(T).fill(0n);
        cumSupply[0] = supply[0]!;
        for (let t = 1; t < T; t++) {
            cumSupply[t] = cumSupply[t - 1]! + supply[t]!;
        }

        // Estimated clearing tick = highest t where cumDemand[t] >= cumSupply[t]
        // (and at least one side has non-zero volume)
        let estimatedClearingTick: number | null = null;
        for (let t = T - 1; t >= 0; t--) {
            const cd = cumDemand[t] ?? 0n;
            const cs = cumSupply[t] ?? 0n;
            if (cd > 0n && cs > 0n && cd >= cs) {
                estimatedClearingTick = t;
                break;
            }
        }

        const nonZeroTicks: TickData[] = [];
        for (let t = 0; t < T; t++) {
            const dm = demand[t] ?? 0n;
            const sp = supply[t] ?? 0n;
            if (dm > 0n || sp > 0n) {
                nonZeroTicks.push({
                    tick: t,
                    demand: dm,
                    supply: sp,
                    priceUsd: tickToUsd(BigInt(t), tickSize),
                });
            }
        }

        const estimatedClearingUsd =
            estimatedClearingTick !== null
                ? tickToUsd(BigInt(estimatedClearingTick), tickSize)
                : null;

        return {
            numTicks: T,
            nonZeroTicks,
            estimatedClearingTick,
            estimatedClearingUsd,
            totalDemand,
            totalSupply,
        };
    } catch {
        return null;
    }
}

// ---------------------------------------------------------------------------
// OrderSlab byte layout (absolute offsets):
//   [0]      disc = 4
//   [1]      version = 3
//   [2..10]  auction_id          u64 le
//   [10..18] next_order_id       u64 le
//   [18..22] capacity            u32 le
//   [22..26] count               u32 le
//   [26..58] market              Address (32 bytes)
//   [58]     bump                u8
//   [59..63] next_free_hint      u32 le
//   [63..]   Order slots, 88 bytes each:
//              +0..8   price         u64 le
//              +8..16  quantity      u64 le
//              +16..24 remaining     u64 le
//              +24..32 order_id      u64 le
//              +32..64 trader        Address (32 bytes)
//              +64     side          u8 (0=buy, 1=sell)
//              +65     is_maker      u8
//              +66     status        u8 (0=Empty, 1=Resting, 2=Accumulated, 3=Consumed)
//              +67..72 _padding      5 bytes
//              +72..80 cum_before    u64 le
//              +80..88 reserved_margin u64 le
// ---------------------------------------------------------------------------

const SLAB_DISC = 4;
const SLAB_HEADER = 63;
const SLOT_SIZE = 88;

export async function fetchMyOrders(
    market: string,
    walletAddress: string,
    tickSize: bigint,
): Promise<OrderView[]> {
    try {
        const [slabPda] = await findOrderSlabHeaderPda({ market: address(market) });
        const raw = await fetchRaw(slabPda);
        if (!raw) return [];
        if (raw.owner !== PROGRAM_ID) return [];

        const d = raw.data;
        if (d.length < SLAB_HEADER) return [];
        if (d[0] !== SLAB_DISC) return [];

        const capacity = readU32le(d.subarray(18, 22));
        if (capacity === 0) return [];

        const orders: OrderView[] = [];

        for (let i = 0; i < capacity; i++) {
            const base = SLAB_HEADER + i * SLOT_SIZE;
            if (base + SLOT_SIZE > d.length) break;

            const status = d[base + 66] as 0 | 1 | 2 | 3;
            // Skip Empty or Consumed
            if (status === 0 || status === 3) continue;

            const traderBytes = d.subarray(base + 32, base + 64);
            const traderStr = encodeAddress(traderBytes);
            if (traderStr !== walletAddress) continue;

            const price = readU64le(d.subarray(base + 0, base + 8));
            const quantity = readU64le(d.subarray(base + 8, base + 16));
            const remaining = readU64le(d.subarray(base + 16, base + 24));
            const orderId = readU64le(d.subarray(base + 24, base + 32));
            const side = (d[base + 64] ?? 0) as 0 | 1;

            orders.push({
                slotIndex: i,
                orderId,
                side,
                price,
                quantity,
                remaining,
                status,
                priceUsd: tickToUsd(price, tickSize),
            });
        }

        return orders;
    } catch {
        return [];
    }
}
