"use client";

import { useWallet } from "@solana/wallet-adapter-react";
import { useCallback, useEffect, useState } from "react";

import { DataRow } from "@/components/data-row";
import { EmptyHint, SkeletonRows } from "@/components/ui/skeleton";
import { fetchPositionView, type MarketView, type PositionView } from "@/lib/data";
import {
    equity as computeEquity,
    markPrice as computeMarkPrice,
    price1e8ToUsd,
    unrealizedPnl,
} from "@/lib/tempo-math";
import { useInterval } from "@/lib/use-interval";
import { cn } from "@/lib/utils";

const POLL_MS = 3000;

function signClass(v: bigint): string {
    if (v > 0n) return "text-up";
    if (v < 0n) return "text-down";
    return "text-foreground";
}

function usd(price1e8: bigint | null): string {
    if (price1e8 === null) return "—";
    const v = price1e8ToUsd(price1e8);
    return v !== null ? `$${v.toFixed(2)}` : "—";
}

export function PositionsPanel({ view }: { view: MarketView | null }) {
    const { publicKey, connected } = useWallet();
    const owner = publicKey?.toBase58() ?? null;
    const market = view?.address ?? null;

    const [position, setPosition] = useState<PositionView | null>(null);
    const [exists, setExists] = useState<boolean | null>(null);

    const refresh = useCallback(async () => {
        if (!owner || !market) {
            setPosition(null);
            setExists(null);
            return;
        }
        try {
            const p = await fetchPositionView(owner, market);
            setPosition(p);
            setExists(p !== null);
        } catch {
            setPosition(null);
            setExists(null);
        }
    }, [owner, market]);

    useEffect(() => {
        void refresh();
    }, [refresh]);
    useInterval(() => void refresh(), owner && market ? POLL_MS : null);

    if (!connected || !owner) {
        return (
            <div className="p-4">
                <p className="text-sm text-muted-foreground">Connect a wallet to view your position.</p>
            </div>
        );
    }

    const mark = view ? computeMarkPrice(view.lastBidFillPrice, view.lastAskFillPrice) : null;
    const flat = position !== null && position.size === 0n;

    return (
        <div className="space-y-4 p-4">
            {!market ? (
                <SkeletonRows rows={6} />
            ) : exists === false || flat ? (
                <EmptyHint title="No open position">
                    Submit an order in the Trade panel to open a position in this market.
                </EmptyHint>
            ) : position ? (
                <PositionDetail position={position} mark={mark} />
            ) : (
                <SkeletonRows rows={6} />
            )}
        </div>
    );
}

function PositionDetail({ position, mark }: { position: PositionView; mark: bigint | null }) {
    const { size, entryPrice, collateral, realizedPnl } = position;
    const sideLabel = size > 0n ? "Long" : "Short";
    const upnl = mark !== null ? unrealizedPnl(size, entryPrice, mark) : null;
    const eq = upnl !== null ? computeEquity(collateral, realizedPnl, upnl) : null;

    return (
        <div className="space-y-1">
            <DataRow label="Side">
                <span className={size > 0n ? "text-up" : "text-down"}>{sideLabel}</span>
            </DataRow>
            <DataRow label="Size">{size.toString()}</DataRow>
            <DataRow label="Entry">{usd(entryPrice)}</DataRow>
            <DataRow label="Mark">{usd(mark)}</DataRow>
            <DataRow label="Collateral">{collateral.toString()}</DataRow>
            <DataRow label="Realized PnL">
                <span className={signClass(realizedPnl)}>{realizedPnl.toString()}</span>
            </DataRow>
            <DataRow label="Unrealized PnL">
                {upnl !== null ? <span className={signClass(upnl)}>{upnl.toString()}</span> : "—"}
            </DataRow>
            <DataRow label="Equity">{eq !== null ? eq.toString() : "—"}</DataRow>
        </div>
    );
}
