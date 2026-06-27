"use client";

import { useEffect, useRef, useState } from "react";

const HERMES_BASE = "https://hermes.pyth.network";
const BENCHMARKS_BASE = "https://benchmarks.pyth.network";

export const SOL_USD_FEED_ID = "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";
export const SOL_USD_TV_SYMBOL = "Crypto.SOL/USD";

interface HermesPrice {
    price: string;
    conf: string;
    expo: number;
    publish_time: number;
}

interface HermesParsedItem {
    id: string;
    price: HermesPrice;
}

interface HermesResponse {
    parsed?: HermesParsedItem[];
}

function isHermesResponse(value: unknown): value is HermesResponse {
    if (typeof value !== "object" || value === null) return false;
    const parsed = (value as { parsed?: unknown }).parsed;
    if (parsed === undefined) return true;
    if (!Array.isArray(parsed)) return false;
    return parsed.every((item) => {
        if (typeof item !== "object" || item === null) return false;
        const price = (item as { price?: unknown }).price;
        if (typeof price !== "object" || price === null) return false;
        const p = price as Record<string, unknown>;
        return typeof p.price === "string" && typeof p.expo === "number";
    });
}

function toUsd(price: HermesPrice): number {
    return Number(price.price) * 10 ** price.expo;
}

function firstUsd(value: unknown): number | null {
    if (!isHermesResponse(value)) return null;
    const item = value.parsed?.[0];
    if (item === undefined) return null;
    const usd = toUsd(item.price);
    return Number.isFinite(usd) && usd > 0 ? usd : null;
}

export interface OhlcBar {
    time: number;
    open: number;
    high: number;
    low: number;
    close: number;
}

interface TvHistory {
    s: string;
    t?: number[];
    o?: number[];
    h?: number[];
    l?: number[];
    c?: number[];
}

function isTvHistory(value: unknown): value is TvHistory {
    if (typeof value !== "object" || value === null) return false;
    return typeof (value as { s?: unknown }).s === "string";
}

export async function fetchSolUsdLatest(signal?: AbortSignal): Promise<number | null> {
    const url = `${HERMES_BASE}/v2/updates/price/latest?ids[]=${SOL_USD_FEED_ID}&parsed=true&encoding=hex`;
    const res = await fetch(url, { signal });
    if (!res.ok) return null;
    const json: unknown = await res.json();
    return firstUsd(json);
}

export async function fetchSolUsdHistory(
    fromUnix: number,
    toUnix: number,
    signal?: AbortSignal,
): Promise<OhlcBar[]> {
    const symbol = encodeURIComponent(SOL_USD_TV_SYMBOL);
    const url = `${BENCHMARKS_BASE}/v1/shims/tradingview/history?symbol=${symbol}&resolution=1&from=${fromUnix}&to=${toUnix}`;
    const res = await fetch(url, { signal });
    if (!res.ok) return [];
    const json: unknown = await res.json();
    if (!isTvHistory(json) || json.s !== "ok") return [];
    const { t, o, h, l, c } = json;
    if (!t || !o || !h || !l || !c) return [];
    const bars: OhlcBar[] = [];
    for (let i = 0; i < t.length; i++) {
        const time = t[i];
        const open = o[i];
        const high = h[i];
        const low = l[i];
        const close = c[i];
        if (
            time === undefined ||
            open === undefined ||
            high === undefined ||
            low === undefined ||
            close === undefined
        ) {
            continue;
        }
        bars.push({ time, open, high, low, close });
    }
    return bars;
}

export function useSolUsdPrice(): { price: number | null } {
    const [price, setPrice] = useState<number | null>(null);
    const sourceRef = useRef<EventSource | null>(null);

    useEffect(() => {
        const controller = new AbortController();
        void fetchSolUsdLatest(controller.signal)
            .then((usd) => {
                if (usd !== null) setPrice((prev) => prev ?? usd);
            })
            .catch(() => undefined);

        const streamUrl = `${HERMES_BASE}/v2/updates/price/stream?ids[]=${SOL_USD_FEED_ID}&parsed=true&encoding=hex&allow_unordered=true`;
        const source = new EventSource(streamUrl);
        sourceRef.current = source;
        source.onmessage = (event: MessageEvent<string>) => {
            try {
                const usd = firstUsd(JSON.parse(event.data));
                if (usd !== null) setPrice(usd);
            } catch {
                // skip malformed frames; the next tick recovers.
            }
        };
        source.onerror = () => {
            // EventSource auto-reconnects; keep the last good price meanwhile.
        };

        return () => {
            controller.abort();
            source.close();
            sourceRef.current = null;
        };
    }, []);

    return { price };
}
