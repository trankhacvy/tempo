import { address, type Address } from "@solana/kit";

import { COLLATERAL_MINT, PROGRAM_ID } from "./config";
import { getRpc } from "./rpc";
import {
    deriveUserCollateralPda,
    findPositionPda,
    readI128le,
    readI64le,
    readU16le,
    readU32le,
    readU64le,
} from "./tempo-client";

// The on-chain accounts are zero-copy `#[repr(C)]` structs behind a 2-byte
// prefix (1 discriminator + 1 version), then little-endian fields. The
// authoritative byte offsets are documented in
// tests/integration-tests/src/lib.rs.
//
// NOTE: the Codama-generated account decoders model only a 1-byte
// discriminator and are therefore off-by-one against the real layout (verified
// against devnet: they produce phase=67, tickSize=256 for a market that is
// actually phase=3, tickSize=1). We decode by raw offset here instead, matching
// the program's documented layout.
const PREFIX = 2;

const MARKET_MIN_LEN = PREFIX + 220; // maintenance_margin_bps ends at post-prefix 220
const USER_COLLATERAL_MIN_LEN = PREFIX + 49; // bump at post-prefix 48
const POSITION_MIN_LEN = PREFIX + 121; // Position::DATA_LEN (bump at post-prefix 120)

export class UntrustedAccountError extends Error {}

interface RawAccount {
    owner: string;
    data: Uint8Array;
}

async function fetchRaw(addr: Address): Promise<RawAccount | null> {
    const info = await getRpc().getAccountInfo(addr, { encoding: "base64" }).send();
    if (info.value === null) return null;
    const [b64] = info.value.data;
    return { owner: info.value.owner, data: Uint8Array.from(Buffer.from(b64, "base64")) };
}

/** Validate ownership + length before trusting on-chain bytes. */
function assertProgramAccount(raw: RawAccount, minLen: number, label: string): void {
    if (raw.owner !== PROGRAM_ID) {
        throw new UntrustedAccountError(
            `${label} is not owned by the Tempo program (owner ${raw.owner}).`,
        );
    }
    if (raw.data.length < minLen) {
        throw new UntrustedAccountError(
            `${label} data is too short (${raw.data.length} < ${minLen} bytes).`,
        );
    }
}

function slice(data: Uint8Array, off: number, len: number): Uint8Array {
    return data.subarray(PREFIX + off, PREFIX + off + len);
}

function readByte(data: Uint8Array, off: number): number {
    return data[PREFIX + off] ?? 0;
}

export interface MarketView {
    address: string;
    phase: number;
    auctionId: bigint;
    phaseDeadlineSlot: bigint;
    tickSize: bigint;
    numTicks: number;
    lastBidFillPrice: bigint;
    lastAskFillPrice: bigint;
    fundingIndex: bigint;
    activeOrderCount: bigint;
    accumulatedOrderCount: bigint;
    ordersPerAuctionCap: number;
    maintenanceMarginBps: number;
    authority: string;
    oracle: string;
}

// Post-prefix offsets (from tests/integration-tests/src/lib.rs):
//   current_auction_id u64 @ 0, phase_deadline_slot u64 @ 8, tick_size u64 @ 16,
//   last_bid_fill u64 @ 24, last_ask_fill u64 @ 32, accumulated_order_count @ 40,
//   active_order_count @ 48, orders_per_auction_cap u32 @ 56, num_ticks u32 @ 60,
//   authority Address @ 64, market_seed Address @ 96, oracle Address @ 128,
//   phase u8 @ 160, bump u8 @ 161, funding_index i128 @ 162, last_funding_ts u64 @ 178,
//   oracle_feed_id [32] @ 186, maintenance_margin_bps u16 @ 218.
function decodeMarket(addr: string, d: Uint8Array): MarketView {
    return {
        address: addr,
        auctionId: readU64le(slice(d, 0, 8)),
        phaseDeadlineSlot: readU64le(slice(d, 8, 8)),
        tickSize: readU64le(slice(d, 16, 8)),
        lastBidFillPrice: readU64le(slice(d, 24, 8)),
        lastAskFillPrice: readU64le(slice(d, 32, 8)),
        accumulatedOrderCount: readU64le(slice(d, 40, 8)),
        activeOrderCount: readU64le(slice(d, 48, 8)),
        ordersPerAuctionCap: readU32le(slice(d, 56, 4)),
        numTicks: readU32le(slice(d, 60, 4)),
        authority: encodeAddress(slice(d, 64, 32)),
        oracle: encodeAddress(slice(d, 128, 32)),
        phase: readByte(d, 160),
        fundingIndex: readI128le(slice(d, 162, 16)),
        maintenanceMarginBps: readU16le(slice(d, 218, 2)),
    };
}

export interface CollateralView {
    address: string;
    owner: string;
    balance: bigint;
    locked: bigint;
    free: bigint;
}

// Post-prefix offsets: owner Address @ 0, balance u64 @ 32, locked u64 @ 40.
function decodeCollateral(addr: string, d: Uint8Array): CollateralView {
    const balance = readU64le(slice(d, 32, 8));
    const locked = readU64le(slice(d, 40, 8));
    return {
        address: addr,
        owner: encodeAddress(slice(d, 0, 32)),
        balance,
        locked,
        free: balance > locked ? balance - locked : 0n,
    };
}

// Minimal base58 (Solana pubkey) encoder for a 32-byte slice.
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

export async function fetchMarketView(marketAddress: string): Promise<MarketView | null> {
    const raw = await fetchRaw(address(marketAddress));
    if (raw === null) return null;
    assertProgramAccount(raw, MARKET_MIN_LEN, "Market account");
    return decodeMarket(marketAddress, raw.data);
}

/** Returns null when no collateral mint is configured or the ledger has not been
 *  initialized yet. The ledger is mint-scoped (CR-3). */
export async function fetchCollateralView(owner: string): Promise<CollateralView | null> {
    if (!COLLATERAL_MINT) return null;
    const pda = await deriveUserCollateralPda(address(owner), address(COLLATERAL_MINT));
    const raw = await fetchRaw(pda);
    if (raw === null) return null;
    assertProgramAccount(raw, USER_COLLATERAL_MIN_LEN, "UserCollateral account");
    return decodeCollateral(pda, raw.data);
}

export async function userCollateralAddress(owner: string): Promise<string> {
    if (!COLLATERAL_MINT) {
        throw new Error("NEXT_PUBLIC_COLLATERAL_MINT is not set.");
    }
    return deriveUserCollateralPda(address(owner), address(COLLATERAL_MINT));
}

export interface PositionView {
    address: string;
    owner: string;
    market: string;
    size: bigint;
    entryPrice: bigint;
    collateral: bigint;
    realizedPnl: bigint;
    lastFundingIndex: bigint;
    bump: number;
}

// Post-prefix offsets (from program/src/state/position.rs):
//   owner Address @ 0, market Address @ 32, size i64 @ 64, entry_price u64 @ 72,
//   collateral u64 @ 80, realized_pnl i128 @ 88, last_funding_index i128 @ 104,
//   bump u8 @ 120. (With the 2-byte prefix, `size` is at raw byte 66 — matches
//   apps/bots/src/devnet-maker-quote-e2e.ts.)
function decodePosition(addr: string, d: Uint8Array): PositionView {
    return {
        address: addr,
        owner: encodeAddress(slice(d, 0, 32)),
        market: encodeAddress(slice(d, 32, 32)),
        size: readI64le(slice(d, 64, 8)),
        entryPrice: readU64le(slice(d, 72, 8)),
        collateral: readU64le(slice(d, 80, 8)),
        realizedPnl: readI128le(slice(d, 88, 16)),
        lastFundingIndex: readI128le(slice(d, 104, 16)),
        bump: readByte(d, 120),
    };
}

/** Returns null when the wallet has no position in this market yet. */
export async function fetchPositionView(
    owner: string,
    market: string,
): Promise<PositionView | null> {
    const [pda] = await findPositionPda({ market: address(market), owner: address(owner) });
    const raw = await fetchRaw(pda);
    if (raw === null) return null;
    assertProgramAccount(raw, POSITION_MIN_LEN, "Position account");
    return decodePosition(pda, raw.data);
}
