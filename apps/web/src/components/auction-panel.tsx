"use client";

import { type MarketView } from "@/lib/data";
import { price1e8ToUsd } from "@/lib/tempo-math";
import { explorerAddressUrl } from "@/lib/tx";
import { shortenAddress } from "@/lib/utils";

function usd(price1e8: bigint): string {
    const v = price1e8ToUsd(price1e8);
    return v !== null ? `$${v.toFixed(2)}` : "—";
}

/** Compact, non-table footer of the round's static parameters — sits under the
 *  AuctionStrip + AuctionHistogram in the Auction tab. */
export function AuctionFacts({
    view,
    activeOrders,
}: {
    view: MarketView | null;
    activeOrders: number | null;
}) {
    if (!view) return null;
    const facts: [string, string][] = [
        ["Tick", usd(view.tickSize)],
        ["Window", usd(view.windowFloorPrice)],
        ["Ticks", view.numTicks.toString()],
        ["Orders", `${activeOrders ?? "—"} / ${view.ordersPerAuctionCap}`],
        ["Maint", `${(view.maintenanceMarginBps / 100).toFixed(2)}%`],
    ];
    return (
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-t border-border px-4 py-3 text-[11px]">
            {facts.map(([label, value]) => (
                <span key={label} className="flex items-center gap-1.5">
                    <span className="uppercase tracking-wide text-muted-foreground">{label}</span>
                    <span className="font-mono tnum text-foreground">{value}</span>
                </span>
            ))}
            <a
                href={explorerAddressUrl(view.oracle)}
                target="_blank"
                rel="noreferrer"
                className="flex items-center gap-1.5 underline-offset-2 hover:underline"
            >
                <span className="uppercase tracking-wide text-muted-foreground">Oracle</span>
                <span className="font-mono tnum text-muted-foreground">{shortenAddress(view.oracle, 4)}</span>
            </a>
        </div>
    );
}
