"use client";

import { useEffect, useState } from "react";

import { EmptyHint, Skeleton } from "@/components/ui/skeleton";
import { fetchHistogramView, type HistogramView, type TickData } from "@/lib/auction";
import { type MarketView } from "@/lib/data";
import { useInterval } from "@/lib/use-interval";
import { cn } from "@/lib/utils";

const POLL_MS = 3000;
const BAR_MAX_PX = 80;
const SKELETON_ROWS = 6;

interface AuctionHistogramProps {
    market: string | null;
    view: MarketView | null;
}

function formatQty(v: bigint): string {
    if (v === 0n) return "";
    if (v >= 1_000_000n) return `${(Number(v) / 1_000_000).toFixed(1)}M`;
    if (v >= 1_000n) return `${(Number(v) / 1_000).toFixed(1)}k`;
    return v.toString();
}

function DepthRow({
    td,
    maxVal,
    isClearing,
}: {
    td: TickData;
    maxVal: number;
    isClearing: boolean;
}) {
    const demandPx = maxVal > 0 ? Math.round((Number(td.demand) / maxVal) * BAR_MAX_PX) : 0;
    const supplyPx = maxVal > 0 ? Math.round((Number(td.supply) / maxVal) * BAR_MAX_PX) : 0;
    const priceLabel =
        td.priceUsd !== null
            ? `$${td.priceUsd.toFixed(2)}`
            : `#${td.tick}`;

    return (
        <div
            className={cn(
                "flex items-center gap-1 px-2 py-0.5 text-xs",
                isClearing && "bg-accent/20",
            )}
        >
            {/* Demand qty label */}
            <span className="w-12 text-right font-mono tnum text-muted-foreground">
                {td.demand > 0n ? formatQty(td.demand) : ""}
            </span>

            {/* Demand bar — right-aligned (grows toward center) */}
            <div
                className="flex justify-end"
                style={{ width: `${BAR_MAX_PX}px` }}
            >
                <div
                    className="h-3 rounded-sm bg-up/60"
                    style={{ width: `${demandPx}px` }}
                />
            </div>

            {/* Price label */}
            <span
                className={cn(
                    "w-20 text-center font-mono tnum",
                    isClearing
                        ? "font-medium text-foreground"
                        : "text-muted-foreground",
                )}
            >
                {priceLabel}
                {isClearing && " ●"}
            </span>

            {/* Supply bar — left-aligned (grows away from center) */}
            <div style={{ width: `${BAR_MAX_PX}px` }}>
                <div
                    className="h-3 rounded-sm bg-down/60"
                    style={{ width: `${supplyPx}px` }}
                />
            </div>

            {/* Supply qty label */}
            <span className="w-12 font-mono tnum text-muted-foreground">
                {td.supply > 0n ? formatQty(td.supply) : ""}
            </span>
        </div>
    );
}

export function AuctionHistogram({ market, view }: AuctionHistogramProps) {
    const [histogram, setHistogram] = useState<HistogramView | null>(null);
    const [loading, setLoading] = useState(false);

    const tickSize = view?.tickSize ?? 0n;
    const windowFloor = view?.windowFloorPrice ?? 0n;
    const active = !!market && tickSize > 0n;

    async function load() {
        if (!active) return;
        try {
            const h = await fetchHistogramView(market!, tickSize, windowFloor);
            setHistogram(h);
        } catch {
            // silent — keep last known state
        } finally {
            setLoading(false);
        }
    }

    useEffect(() => {
        if (!active) return;
        setLoading(true);
        void load();
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [market, tickSize.toString(), windowFloor.toString()]);

    useInterval(() => void load(), active ? POLL_MS : null);

    const ticks = histogram?.nonZeroTicks ?? [];
    const maxVal =
        ticks.length > 0
            ? Math.max(
                  ...ticks.map((t) => Math.max(Number(t.demand), Number(t.supply))),
              )
            : 0;

    return (
        <div className="shrink-0 border-b border-border">
            {/* Header */}
            <div className="flex h-9 items-center justify-between border-b border-border px-3">
                <span className="text-xs font-medium text-foreground">Depth</span>
                <div className="flex items-center gap-3 text-[11px] text-muted-foreground">
                    <span className="flex items-center gap-1">
                        <span className="inline-block h-2 w-2 rounded-full bg-up/70" />
                        BUY
                    </span>
                    <span className="flex items-center gap-1">
                        <span className="inline-block h-2 w-2 rounded-full bg-down/70" />
                        SELL
                    </span>
                </div>
            </div>

            {/* Body */}
            {loading ? (
                <div className="space-y-1 p-2" aria-hidden>
                    {Array.from({ length: SKELETON_ROWS }).map((_, i) => (
                        <Skeleton key={i} className="h-4 w-full" />
                    ))}
                </div>
            ) : !active || ticks.length === 0 ? (
                <div className="p-3">
                    <EmptyHint title="No orders yet">
                        Submit orders to see the depth chart.
                    </EmptyHint>
                </div>
            ) : (
                <div className="max-h-[200px] overflow-y-auto">
                    {/* Column headers */}
                    <div className="flex items-center gap-1 px-2 py-0.5 text-[10px] text-muted-foreground/60">
                        <span className="w-12 text-right">qty</span>
                        <div style={{ width: `${BAR_MAX_PX}px` }} />
                        <span className="w-20 text-center">price</span>
                        <div style={{ width: `${BAR_MAX_PX}px` }} />
                        <span className="w-12">qty</span>
                    </div>
                    {ticks.map((td) => (
                        <DepthRow
                            key={td.tick}
                            td={td}
                            maxVal={maxVal}
                            isClearing={td.tick === histogram?.estimatedClearingTick}
                        />
                    ))}
                    {histogram?.estimatedClearingUsd != null && (
                        <div className="px-2 py-1 text-[10px] text-muted-foreground/70">
                            Est. clearing:{" "}
                            <span className="font-mono text-foreground">
                                ${histogram.estimatedClearingUsd.toFixed(2)}
                            </span>
                        </div>
                    )}
                </div>
            )}
        </div>
    );
}
