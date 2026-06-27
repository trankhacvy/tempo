"use client";

import { ArrowDownRight, ArrowUpRight, ExternalLink, Gavel, Zap } from "lucide-react";

import { EmptyHint, SkeletonRows } from "@/components/ui/skeleton";
import { DEFAULT_MARKET } from "@/lib/config";
import { useRecentActivity, type Activity } from "@/lib/events";
import { explorerTxUrl } from "@/lib/tx";
import { cn } from "@/lib/utils";

function relativeTime(blockTime: number | null): string {
    if (blockTime === null) return "";
    const secs = Math.max(0, Math.floor(Date.now() / 1000 - blockTime));
    if (secs < 60) return `${secs}s ago`;
    const mins = Math.floor(secs / 60);
    if (mins < 60) return `${mins}m ago`;
    const hrs = Math.floor(mins / 60);
    return `${hrs}h ago`;
}

function sideLabel(side: number): string {
    return side === 0 ? "Buy" : "Sell";
}

function ActivityRow({ item }: { item: Activity }) {
    const time = relativeTime(item.blockTime);

    let icon: React.ReactNode;
    let title: React.ReactNode;
    let detail: React.ReactNode;

    if (item.kind === "orderSubmitted") {
        const buy = item.side === 0;
        icon = buy ? (
            <ArrowUpRight className="size-3.5 text-up" />
        ) : (
            <ArrowDownRight className="size-3.5 text-down" />
        );
        title = (
            <span>
                <span className={cn("font-medium", buy ? "text-up" : "text-down")}>
                    {sideLabel(item.side)}
                </span>{" "}
                <span className="text-muted-foreground">{item.isMaker === 1 ? "maker" : "taker"}</span>
            </span>
        );
        detail = (
            <span className="tnum">
                {item.quantity.toString()} @ {item.price.toString()}
            </span>
        );
    } else if (item.kind === "clearingFinalized") {
        icon = <Gavel className="size-3.5 text-primary" />;
        title = (
            <span>
                <span className="font-medium text-foreground">Cleared</span>{" "}
                <span className="text-muted-foreground">#{item.auctionId.toString()}</span>
            </span>
        );
        detail = (
            <span className="tnum">
                <span className="text-up">{item.bidClearingPrice.toString()}</span>
                {" / "}
                <span className="text-down">{item.askClearingPrice.toString()}</span>
            </span>
        );
    } else {
        const buy = item.side === 0;
        icon = <Zap className="size-3.5 text-primary" />;
        title = (
            <span>
                <span className="font-medium text-foreground">Filled</span>{" "}
                <span className={cn(buy ? "text-up" : "text-down")}>{sideLabel(item.side)}</span>
            </span>
        );
        detail = <span className="tnum">{item.fill.toString()}</span>;
    }

    return (
        <a
            href={explorerTxUrl(item.signature)}
            target="_blank"
            rel="noreferrer"
            className="group flex items-center justify-between gap-3 py-2 text-sm hover:bg-muted/30"
        >
            <span className="flex min-w-0 items-center gap-2">
                <span className="grid size-6 shrink-0 place-items-center bg-secondary/40">{icon}</span>
                <span className="min-w-0 truncate">{title}</span>
            </span>
            <span className="flex shrink-0 items-center gap-2 font-mono text-xs text-muted-foreground">
                {detail}
                <span className="hidden text-muted-foreground/60 sm:inline">{time}</span>
                <ExternalLink className="size-3 opacity-0 transition-opacity group-hover:opacity-60" />
            </span>
        </a>
    );
}

export function ActivityPanel({ market }: { market: string }) {
    const resolved = market.trim() !== "" ? market : DEFAULT_MARKET;
    const { activity, loading, error } = useRecentActivity(resolved);

    return (
        <div className="px-4 py-2">
            {resolved.trim() === "" ? (
                <EmptyHint title="No market configured">
                    Set <code className="font-mono">NEXT_PUBLIC_TEMPO_MARKET</code> or load a market
                    in the Market tab to stream program activity.
                </EmptyHint>
            ) : loading ? (
                <SkeletonRows rows={5} />
            ) : error ? (
                <p className="text-sm text-destructive">Could not load activity: {error}</p>
            ) : activity.length === 0 ? (
                <EmptyHint title="No recent activity">
                    Submit an order, or run the crank daemon so orders clear and fill.
                </EmptyHint>
            ) : (
                <div className="divide-y divide-border/50">
                    {activity.map((item, i) => (
                        <ActivityRow key={`${item.signature}-${i}`} item={item} />
                    ))}
                </div>
            )}
        </div>
    );
}
