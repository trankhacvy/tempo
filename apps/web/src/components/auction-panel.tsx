"use client";

import { Badge } from "@/components/ui/badge";
import { DataRow } from "@/components/data-row";
import { EmptyHint } from "@/components/ui/skeleton";
import { type MarketView } from "@/lib/data";
import { Phase, phaseLabel } from "@/lib/tempo-client";
import { price1e8ToUsd } from "@/lib/tempo-math";
import { explorerAddressUrl } from "@/lib/tx";
import { shortenAddress } from "@/lib/utils";

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

function usd(price1e8: bigint): string {
    const v = price1e8ToUsd(price1e8);
    return v !== null ? `$${v.toFixed(2)}` : "—";
}

export function AuctionPanel({
    view,
    countdown,
    activeOrders,
}: {
    view: MarketView | null;
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
    const countdownLabel = countdown ?? "window closed";

    return (
        <div className="space-y-1 p-4">
            <div className="flex items-center justify-between pb-1">
                <a
                    href={explorerAddressUrl(view.address)}
                    target="_blank"
                    rel="noreferrer"
                    className="font-mono text-xs text-muted-foreground underline-offset-2 hover:underline"
                >
                    {shortenAddress(view.address, 6)}
                </a>
                <Badge variant={phaseVariant(view.phase)}>{phaseLabel(view.phase)}</Badge>
            </div>

            <DataRow label="Auction">#{view.auctionId.toString()}</DataRow>
            <DataRow label={collecting ? "Closes in" : "Clears in"}>{countdownLabel}</DataRow>
            <DataRow label="Last bid fill">
                <span className={view.lastBidFillPrice > 0n ? "text-up" : "text-muted-foreground"}>
                    {usd(view.lastBidFillPrice)}
                </span>
            </DataRow>
            <DataRow label="Last ask fill">
                <span className={view.lastAskFillPrice > 0n ? "text-down" : "text-muted-foreground"}>
                    {usd(view.lastAskFillPrice)}
                </span>
            </DataRow>
            <DataRow label="Tick size">{usd(view.tickSize)}</DataRow>
            <DataRow label="Window floor">{usd(view.windowFloorPrice)}</DataRow>
            <DataRow label="Ticks">{view.numTicks}</DataRow>
            <DataRow label="Active orders">
                {activeOrders !== null
                    ? `${activeOrders} / ${view.ordersPerAuctionCap}`
                    : `— / ${view.ordersPerAuctionCap}`}
            </DataRow>
            <DataRow label="Maint. margin">{(view.maintenanceMarginBps / 100).toFixed(2)}%</DataRow>
            <DataRow label="Oracle">
                <a
                    href={explorerAddressUrl(view.oracle)}
                    target="_blank"
                    rel="noreferrer"
                    className="underline-offset-2 hover:underline"
                >
                    {shortenAddress(view.oracle, 6)}
                </a>
            </DataRow>
        </div>
    );
}
