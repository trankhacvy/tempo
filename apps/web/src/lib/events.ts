"use client";

import { getBase58Encoder, type Signature } from "@solana/kit";
import { useCallback, useEffect, useState } from "react";

import { PROGRAM_ID } from "./config";
import { getRpc } from "./rpc";
import {
    getClearingFinalizedEventDecoder,
    getFillSettledEventDecoder,
    getOrderSubmittedEventDecoder,
    TEMPO_PROGRAM,
} from "./tempo-client";

// The program emits events through a self-CPI to `EmitEvent` (disc 228): the
// inner instruction's data is an 8-byte Anchor-style tag, a 1-byte event
// discriminator, then the Codama-encoded payload (see apps/bots/src/indexer).
// We read recent program signatures, pull each transaction's inner
// instructions, and decode the ones addressed to the Tempo program.
const EVENT_TAG_LEN = 8;
const EVENT_DISCRIMINATOR_LEN = 1;
const EVENT_HEADER_LEN = EVENT_TAG_LEN + EVENT_DISCRIMINATOR_LEN;

const DISC_ORDER_SUBMITTED = 1;
const DISC_CLEARING_FINALIZED = 4;
const DISC_FILL_SETTLED = 5;

const DEFAULT_LIMIT = 12;
const MAX_TX_LOOKUPS = 14;
const POLL_MS = 6000;

export type ActivityKind = "orderSubmitted" | "clearingFinalized" | "fillSettled";

interface BaseActivity {
    signature: string;
    slot: bigint;
    blockTime: number | null;
}

export interface OrderSubmittedActivity extends BaseActivity {
    kind: "orderSubmitted";
    trader: string;
    orderId: bigint;
    auctionId: bigint;
    price: bigint;
    quantity: bigint;
    side: number;
    isMaker: number;
}

export interface ClearingFinalizedActivity extends BaseActivity {
    kind: "clearingFinalized";
    auctionId: bigint;
    bidClearingPrice: bigint;
    bidMatchedVolume: bigint;
    askClearingPrice: bigint;
    askMatchedVolume: bigint;
}

export interface FillSettledActivity extends BaseActivity {
    kind: "fillSettled";
    trader: string;
    orderId: bigint;
    auctionId: bigint;
    fill: bigint;
    side: number;
    isMaker: number;
}

export type Activity = OrderSubmittedActivity | ClearingFinalizedActivity | FillSettledActivity;

// Narrow structural views of the JSON `getTransaction` response — only the
// fields we read. @solana/kit's full RPC return type is deeply conditional on
// the request config; a typed boundary keeps the call site strict without `any`.
interface RawInnerInstruction {
    programIdIndex: number;
    accounts: readonly number[];
    data: string;
}

interface RawTransaction {
    slot: bigint;
    blockTime: bigint | null;
    transaction: { message: { accountKeys: readonly string[] } };
    meta: {
        innerInstructions?: readonly { instructions: readonly RawInnerInstruction[] }[] | null;
    } | null;
}

interface RawSignatureInfo {
    signature: string;
    slot: bigint;
    blockTime: bigint | null;
    err: unknown;
}

function decodeEvent(
    data: Uint8Array,
    base: BaseActivity,
): Activity | null {
    if (data.length < EVENT_HEADER_LEN) return null;
    const discriminator = data[EVENT_TAG_LEN] ?? 0;
    const payload = data.subarray(EVENT_HEADER_LEN);
    try {
        if (discriminator === DISC_ORDER_SUBMITTED) {
            const e = getOrderSubmittedEventDecoder().decode(payload);
            return {
                ...base,
                kind: "orderSubmitted",
                trader: e.trader,
                orderId: e.orderId,
                auctionId: e.auctionId,
                price: e.price,
                quantity: e.quantity,
                side: e.side,
                isMaker: e.isMaker,
            };
        }
        if (discriminator === DISC_CLEARING_FINALIZED) {
            const e = getClearingFinalizedEventDecoder().decode(payload);
            return {
                ...base,
                kind: "clearingFinalized",
                auctionId: e.auctionId,
                bidClearingPrice: e.bidClearingPrice,
                bidMatchedVolume: e.bidMatchedVolume,
                askClearingPrice: e.askClearingPrice,
                askMatchedVolume: e.askMatchedVolume,
            };
        }
        if (discriminator === DISC_FILL_SETTLED) {
            const e = getFillSettledEventDecoder().decode(payload);
            return {
                ...base,
                kind: "fillSettled",
                trader: e.trader,
                orderId: e.orderId,
                auctionId: e.auctionId,
                fill: e.fill,
                side: e.side,
                isMaker: e.isMaker,
            };
        }
    } catch {
        return null;
    }
    return null;
}

function activitiesFromTx(tx: RawTransaction, signature: string): Activity[] {
    const keys = tx.transaction.message.accountKeys;
    const base: BaseActivity = {
        signature,
        slot: tx.slot,
        blockTime: tx.blockTime !== null ? Number(tx.blockTime) : null,
    };
    const encoder = getBase58Encoder();
    const out: Activity[] = [];
    for (const group of tx.meta?.innerInstructions ?? []) {
        for (const ix of group.instructions) {
            if (keys[ix.programIdIndex] !== PROGRAM_ID) continue;
            const data = new Uint8Array(encoder.encode(ix.data));
            const decoded = decodeEvent(data, base);
            if (decoded !== null) out.push(decoded);
        }
    }
    return out;
}

export async function fetchRecentActivity(limit = DEFAULT_LIMIT): Promise<Activity[]> {
    const rpc = getRpc();
    const sigInfos = (await rpc
        .getSignaturesForAddress(TEMPO_PROGRAM, { limit })
        .send()) as readonly RawSignatureInfo[];

    const landed = sigInfos.filter((s) => s.err === null).slice(0, MAX_TX_LOOKUPS);

    const settled = await Promise.allSettled(
        landed.map(async (info) => {
            const tx = (await rpc
                .getTransaction(info.signature as Signature, {
                    maxSupportedTransactionVersion: 0,
                    encoding: "json",
                })
                .send()) as RawTransaction | null;
            return tx === null ? [] : activitiesFromTx(tx, info.signature);
        }),
    );

    const events: Activity[] = [];
    for (const r of settled) {
        if (r.status === "fulfilled") events.push(...r.value);
    }
    events.sort((a, b) => {
        if (a.slot !== b.slot) return a.slot > b.slot ? -1 : 1;
        return 0;
    });
    return events;
}

export interface RecentActivityState {
    activity: Activity[];
    loading: boolean;
    error: string | null;
}

export function useRecentActivity(market: string): RecentActivityState {
    const [activity, setActivity] = useState<Activity[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [loadedOnce, setLoadedOnce] = useState(false);

    const enabled = market.trim() !== "";

    const refresh = useCallback(async () => {
        if (!enabled) {
            setActivity([]);
            setError(null);
            return;
        }
        setLoading(true);
        try {
            const next = await fetchRecentActivity();
            setActivity(next);
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setLoading(false);
            setLoadedOnce(true);
        }
    }, [enabled]);

    useEffect(() => {
        if (!enabled) return;
        void refresh();
        const id = setInterval(() => void refresh(), POLL_MS);
        return () => clearInterval(id);
    }, [enabled, refresh]);

    return { activity, loading: loading && !loadedOnce, error };
}
