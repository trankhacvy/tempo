"use client";

import { Activity } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { Badge } from "@/components/ui/badge";
import { EmptyHint } from "@/components/ui/skeleton";
import { fetchHistogramView, type HistogramView } from "@/lib/auction";
import { type MarketView } from "@/lib/data";
import { Phase, phaseLabel } from "@/lib/tempo-client";
import { price1e8ToUsd } from "@/lib/tempo-math";
import { explorerAddressUrl } from "@/lib/tx";
import { cn, shortenAddress } from "@/lib/utils";

const POLL_MS = 3000;

function usd(price1e8: bigint): string {
    const v = price1e8ToUsd(price1e8);
    return v !== null ? `$${v.toFixed(2)}` : "—";
}

function phaseVariant(phase: number): "default" | "success" | "muted" | "destructive" {
    switch (phase) {
        case Phase.Discovered:
            return "success";
        case Phase.Settling:
            return "destructive";
        case Phase.Collect:
            return "default";
        default:
            return "muted";
    }
}

function fmtQty(v: bigint): string {
    if (v === 0n) return "";
    if (v >= 1_000_000n) return `${(Number(v) / 1_000_000).toFixed(1)}M`;
    if (v >= 1_000n) return `${(Number(v) / 1_000).toFixed(1)}k`;
    return v.toString();
}

export function AuctionView({
    view,
    market,
    slotsLeft,
    countdown,
    activeOrders,
}: {
    view: MarketView | null;
    market: string;
    slotsLeft: bigint | null;
    countdown: string | null;
    activeOrders: number | null;
}) {
    if (!view) {
        return (
            <div className="p-4">
                <EmptyHint title="Loading market…">
                    Streaming the default market from devnet.
                </EmptyHint>
            </div>
        );
    }

    const collecting = view.phase === Phase.Collect;
    const countdownLabel =
        slotsLeft === null ? "—" : countdown !== null ? countdown : "window closed";

    return (
        <div className="flex flex-col gap-3 p-4">
            {/* Status header */}
            <div className="flex items-center justify-between rounded-md border border-border bg-secondary/20 px-4 py-3">
                <div className="flex items-center gap-3">
                    <Activity className="h-4 w-4 text-primary" />
                    <div>
                        <div className="flex items-center gap-2">
                            <span className="font-mono text-sm font-semibold tnum">
                                Auction #{view.auctionId.toString()}
                            </span>
                            <Badge variant={phaseVariant(view.phase)}>{phaseLabel(view.phase)}</Badge>
                        </div>
                        <div className="mt-0.5 text-[11px] text-muted-foreground">
                            {collecting ? "Closes in" : "Clears in"}{" "}
                            <span className="font-mono tnum text-foreground">{countdownLabel}</span>
                        </div>
                    </div>
                </div>
                <div className="text-right">
                    <div className="text-[10px] uppercase tracking-wide text-muted-foreground">
                        Last bid / ask
                    </div>
                    <div className="mt-0.5 font-mono text-sm tnum">
                        <span className={view.lastBidFillPrice > 0n ? "text-up" : "text-muted-foreground/60"}>
                            {usd(view.lastBidFillPrice)}
                        </span>
                        <span className="text-muted-foreground/50"> / </span>
                        <span className={view.lastAskFillPrice > 0n ? "text-down" : "text-muted-foreground/60"}>
                            {usd(view.lastAskFillPrice)}
                        </span>
                    </div>
                </div>
            </div>

            {/* Depth ladder */}
            <DepthLadder market={market} view={view} />

            {/* Facts */}
            <div className="grid grid-cols-3 gap-2 sm:grid-cols-6">
                <Stat label="Tick" value={usd(view.tickSize)} />
                <Stat label="Window" value={usd(view.windowFloorPrice)} />
                <Stat label="Ticks" value={view.numTicks.toString()} />
                <Stat label="Orders" value={`${activeOrders ?? "—"} / ${view.ordersPerAuctionCap}`} />
                <Stat label="Maint" value={`${(view.maintenanceMarginBps / 100).toFixed(2)}%`} />
                <Stat
                    label="Oracle"
                    value={shortenAddress(view.oracle, 4)}
                    href={explorerAddressUrl(view.oracle)}
                />
            </div>
        </div>
    );
}

function Stat({ label, value, href }: { label: string; value: string; href?: string }) {
    const body = (
        <div className="rounded-md border border-border/60 bg-secondary/10 px-3 py-2">
            <div className="text-[10px] uppercase tracking-wide text-muted-foreground">{label}</div>
            <div className="mt-0.5 truncate font-mono text-xs tnum text-foreground">{value}</div>
        </div>
    );
    return href ? (
        <a href={href} target="_blank" rel="noreferrer" className="block transition-colors hover:border-primary/40">
            {body}
        </a>
    ) : (
        body
    );
}

function DepthLadder({ market, view }: { market: string; view: MarketView }) {
    const [hist, setHist] = useState<HistogramView | null>(null);
    const tickSize = view.tickSize;
    const windowFloor = view.windowFloorPrice;

    const load = useCallback(async () => {
        if (!market || tickSize <= 0n) return;
        try {
            const h = await fetchHistogramView(market, tickSize, windowFloor);
            setHist(h);
        } catch {
            // keep last good
        }
    }, [market, tickSize, windowFloor]);

    useEffect(() => {
        void load();
        const id = setInterval(() => void load(), POLL_MS);
        return () => clearInterval(id);
    }, [load]);

    // High price at top, low at bottom (classic book orientation).
    const ticks = (hist?.nonZeroTicks ?? []).slice().reverse();
    const maxVal =
        ticks.length > 0
            ? Math.max(...ticks.map((t) => Math.max(Number(t.demand), Number(t.supply))))
            : 0;

    return (
        <div className="rounded-md border border-border">
            <div className="flex h-8 items-center justify-between border-b border-border px-3">
                <span className="text-xs font-medium text-foreground">Depth</span>
                <div className="flex items-center gap-4 text-[11px] text-muted-foreground">
                    {hist?.estimatedClearingUsd != null && (
                        <span>
                            Clearing{" "}
                            <span className="font-mono tnum text-foreground">
                                ${hist.estimatedClearingUsd.toFixed(2)}
                            </span>
                        </span>
                    )}
                    <span className="flex items-center gap-1">
                        <span className="inline-block h-2 w-2 rounded-full bg-up/70" /> BID
                    </span>
                    <span className="flex items-center gap-1">
                        <span className="inline-block h-2 w-2 rounded-full bg-down/70" /> ASK
                    </span>
                </div>
            </div>

            {ticks.length === 0 ? (
                <div className="p-4">
                    <EmptyHint title="No resting orders">
                        Depth appears as orders fold into the histogram each round.
                    </EmptyHint>
                </div>
            ) : (
                <div className="divide-y divide-border/30">
                    {ticks.map((t) => {
                        const demandPct = maxVal > 0 ? (Number(t.demand) / maxVal) * 100 : 0;
                        const supplyPct = maxVal > 0 ? (Number(t.supply) / maxVal) * 100 : 0;
                        const isClearing = t.tick === hist?.estimatedClearingTick;
                        return (
                            <div
                                key={t.tick}
                                className={cn(
                                    "grid grid-cols-[1fr_auto_1fr] items-center gap-2 px-3 py-1",
                                    isClearing && "bg-primary/10",
                                )}
                            >
                                {/* Bid (demand) — grows leftward from center */}
                                <div className="flex items-center gap-2">
                                    <span className="w-10 shrink-0 text-right font-mono text-[11px] tnum text-muted-foreground">
                                        {fmtQty(t.demand)}
                                    </span>
                                    <div className="relative flex-1">
                                        <div
                                            className="ml-auto h-4 rounded-sm bg-up/45"
                                            style={{ width: `${demandPct}%` }}
                                        />
                                    </div>
                                </div>

                                {/* Price */}
                                <div
                                    className={cn(
                                        "w-20 text-center font-mono text-xs tnum",
                                        isClearing ? "font-semibold text-foreground" : "text-muted-foreground",
                                    )}
                                >
                                    {t.priceUsd !== null ? `$${t.priceUsd.toFixed(2)}` : `#${t.tick}`}
                                    {isClearing && <span className="text-primary"> ●</span>}
                                </div>

                                {/* Ask (supply) — grows rightward from center */}
                                <div className="flex items-center gap-2">
                                    <div className="relative flex-1">
                                        <div
                                            className="h-4 rounded-sm bg-down/45"
                                            style={{ width: `${supplyPct}%` }}
                                        />
                                    </div>
                                    <span className="w-10 shrink-0 font-mono text-[11px] tnum text-muted-foreground">
                                        {fmtQty(t.supply)}
                                    </span>
                                </div>
                            </div>
                        );
                    })}
                </div>
            )}
        </div>
    );
}
