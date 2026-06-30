"use client";

import { Activity } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { ActivityPanel } from "@/components/activity-panel";
import { AuctionHistogram } from "@/components/auction-histogram";
import { AuctionFacts } from "@/components/auction-panel";
import { Badge } from "@/components/ui/badge";
import { MyOrders } from "@/components/my-orders";
import { PositionsPanel } from "@/components/positions-panel";
import { PriceChart } from "@/components/price-chart";
import {
    ResizableHandle,
    ResizablePanel,
    ResizablePanelGroup,
} from "@/components/ui/resizable";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { TradePanel } from "@/components/trade-panel";
import { fetchOrderBook } from "@/lib/auction";
import { DEFAULT_MARKET } from "@/lib/config";
import { fetchMarketView, type MarketView } from "@/lib/data";
import { useSolUsdPrice } from "@/lib/pyth";
import { getRpc } from "@/lib/rpc";
import { Phase, phaseLabel } from "@/lib/tempo-client";
import { price1e8ToUsd } from "@/lib/tempo-math";
import { useInterval } from "@/lib/use-interval";
import { cn, shortenAddress } from "@/lib/utils";

const POLL_MS = 3000;
const SLOT_MS = 400;

export function slotsToCountdown(slotsLeft: bigint | null): string | null {
    if (slotsLeft === null || slotsLeft <= 0n) return null;
    const totalSec = Math.ceil((Number(slotsLeft) * SLOT_MS) / 1000);
    const m = Math.floor(totalSec / 60);
    const s = totalSec % 60;
    return `${m}:${s.toString().padStart(2, "0")}`;
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

function usd(price1e8: bigint): string {
    const v = price1e8ToUsd(price1e8);
    return v !== null ? `$${v.toFixed(2)}` : "—";
}

const TAB_TRIGGER = "h-9 px-4 text-xs font-medium";

export function Dashboard() {
    const [view, setView] = useState<MarketView | null>(null);
    const [slot, setSlot] = useState<bigint | null>(null);
    const [activeOrders, setActiveOrders] = useState<number | null>(null);
    const { price: oracleUsd } = useSolUsdPrice();
    const market = view?.address ?? DEFAULT_MARKET;

    const pollMarket = useCallback(async () => {
        if (!view) return;
        try {
            const next = await fetchMarketView(view.address);
            if (next !== null) setView(next);
        } catch {
            // transient RPC error; keep the last good view.
        }
    }, [view]);

    const pollSlot = useCallback(async () => {
        try {
            setSlot(await getRpc().getSlot().send());
        } catch {
            // transient
        }
    }, []);

    const pollOrders = useCallback(async () => {
        if (!market) return;
        try {
            const book = await fetchOrderBook(market);
            setActiveOrders(book.count);
        } catch {
            // transient
        }
    }, [market]);

    useEffect(() => {
        if (!DEFAULT_MARKET || view) return;
        fetchMarketView(DEFAULT_MARKET)
            .then((v) => { if (v) setView(v); })
            .catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    useInterval(() => void pollMarket(), view ? POLL_MS : null);
    useInterval(() => void pollSlot(), view ? POLL_MS : null);
    useInterval(() => void pollOrders(), view ? POLL_MS : null);

    const slotsLeft = view && slot !== null ? view.phaseDeadlineSlot - slot : null;
    const countdown = slotsToCountdown(slotsLeft);

    return (
        <div className="flex flex-col" style={{ height: "calc(100vh - 2.75rem)" }}>
            <MarketStatsBar view={view} oracleUsd={oracleUsd} activeOrders={activeOrders} />

            <div className="flex min-h-0 flex-1">
                {/* Left column: chart + tabs, vertically resizable */}
                <div className="flex min-w-0 flex-1 flex-col">
                    <ResizablePanelGroup direction="vertical" className="min-h-0 flex-1">
                        <ResizablePanel defaultSize={62} minSize={25}>
                            <div className="h-full min-h-0">
                                <PriceChart view={view} />
                            </div>
                        </ResizablePanel>
                        <ResizableHandle withHandle />
                        <ResizablePanel defaultSize={38} minSize={15}>
                            <BottomTabs
                                view={view}
                                market={market}
                                countdown={countdown}
                                slotsLeft={slotsLeft}
                                activeOrders={activeOrders}
                            />
                        </ResizablePanel>
                    </ResizablePanelGroup>
                </div>

                {/* Right sidebar: trading */}
                <aside className="flex w-[340px] shrink-0 flex-col overflow-y-auto border-l border-border">
                    <TradePanel market={market} view={view} oracleUsd={oracleUsd} countdown={countdown} />
                </aside>
            </div>
        </div>
    );
}

function MarketStatsBar({
    view,
    oracleUsd,
    activeOrders,
}: {
    view: MarketView | null;
    oracleUsd: number | null;
    activeOrders: number | null;
}) {
    return (
        <div className="flex h-auto shrink-0 items-center gap-6 overflow-x-auto border-b border-border px-4 py-2.5">
            <StatCell label="Mark · SOL/USD">
                <span className="font-mono text-base font-semibold tnum">
                    {oracleUsd !== null ? `$${oracleUsd.toFixed(2)}` : "—"}
                </span>
            </StatCell>

            <div className="h-4 w-px shrink-0 bg-border" />

            <StatCell label="Last bid">
                <span className={cn("font-mono text-xs tnum", view && view.lastBidFillPrice > 0n ? "text-up" : "text-muted-foreground")}>
                    {view ? usd(view.lastBidFillPrice) : "—"}
                </span>
            </StatCell>
            <StatCell label="Last ask">
                <span className={cn("font-mono text-xs tnum", view && view.lastAskFillPrice > 0n ? "text-down" : "text-muted-foreground")}>
                    {view ? usd(view.lastAskFillPrice) : "—"}
                </span>
            </StatCell>
            <StatCell label="Active orders">
                <span className="font-mono text-xs tnum">
                    {activeOrders !== null ? activeOrders.toString() : "—"}
                </span>
            </StatCell>

            <StatCell label="Oracle · Pyth" className="ml-auto hidden lg:block">
                {view ? (
                    <span className="font-mono text-xs tnum text-muted-foreground">
                        {shortenAddress(view.oracle, 6)}
                    </span>
                ) : (
                    <span className="animate-ticker font-mono text-xs tnum text-muted-foreground/50">
                        syncing…
                    </span>
                )}
            </StatCell>
        </div>
    );
}

function StatCell({
    label,
    children,
    className,
}: {
    label: string;
    children: React.ReactNode;
    className?: string;
}) {
    return (
        <div className={cn("shrink-0", className)}>
            <div className="text-[10px] font-medium uppercase leading-none tracking-wide text-muted-foreground">
                {label}
            </div>
            <div className="mt-0.5">{children}</div>
        </div>
    );
}

function AuctionStrip({
    view,
    slotsLeft,
    countdown,
}: {
    view: MarketView | null;
    slotsLeft: bigint | null;
    countdown: string | null;
}) {
    const phase = view?.phase ?? Phase.Collect;
    const id = view ? view.auctionId.toString() : "—";
    const countdownLabel =
        slotsLeft === null ? "—" : countdown !== null ? countdown : "window closed";

    return (
        <div className="shrink-0 border-b border-border px-4 py-3">
            <div className="flex items-center justify-between">
                <div className="flex items-center gap-2 text-xs">
                    <Activity className="h-3.5 w-3.5 text-primary" />
                    <span className="font-mono font-semibold tnum">Auction #{id}</span>
                </div>
                <Badge variant={phaseVariant(phase)}>{phaseLabel(phase)}</Badge>
            </div>
            <div className="mt-2 flex items-center justify-between text-[11px] text-muted-foreground">
                <span>
                    {phase === Phase.Collect ? "Closes in" : "Clears in"}:{" "}
                    <span className="font-mono tnum text-foreground">{countdownLabel}</span>
                </span>
                <span className="font-mono tnum">
                    <span className={view && view.lastBidFillPrice > 0n ? "text-up" : "text-muted-foreground/50"}>{view ? usd(view.lastBidFillPrice) : "—"}</span>
                    {" / "}
                    <span className={view && view.lastAskFillPrice > 0n ? "text-down" : "text-muted-foreground/50"}>{view ? usd(view.lastAskFillPrice) : "—"}</span>
                </span>
            </div>
        </div>
    );
}

function BottomTabs({
    view,
    market,
    countdown,
    slotsLeft,
    activeOrders,
}: {
    view: MarketView | null;
    market: string;
    countdown: string | null;
    slotsLeft: bigint | null;
    activeOrders: number | null;
}) {
    return (
        <Tabs
            defaultValue="auction"
            className="flex h-full min-h-0 shrink-0 flex-col border-t border-border [&>[data-slot=tabs-list]]:gap-0"
        >
            <TabsList
                variant="line"
                className="!w-full justify-start rounded-none border-b border-border bg-transparent px-0 !h-9 shrink-0"
            >
                <TabsTrigger value="auction" className={TAB_TRIGGER}>
                    Auction
                </TabsTrigger>
                <TabsTrigger value="position" className={TAB_TRIGGER}>
                    Position
                </TabsTrigger>
                <TabsTrigger value="orders" className={TAB_TRIGGER}>
                    Orders
                </TabsTrigger>
                <TabsTrigger value="activity" className={TAB_TRIGGER}>
                    Activity
                </TabsTrigger>
            </TabsList>

            <TabsContent value="auction" className="min-h-0 flex-1 overflow-y-auto p-0">
                <div className="w-full max-w-md">
                    <AuctionStrip view={view} slotsLeft={slotsLeft} countdown={countdown} />
                    <AuctionHistogram market={market || null} view={view} />
                    <AuctionFacts view={view} activeOrders={activeOrders} />
                </div>
            </TabsContent>
            <TabsContent value="position" className="min-h-0 flex-1 overflow-y-auto p-0">
                <PositionsPanel view={view} />
            </TabsContent>
            <TabsContent value="orders" className="min-h-0 flex-1 overflow-y-auto p-0">
                <MyOrders market={market || null} view={view} />
            </TabsContent>
            <TabsContent value="activity" className="min-h-0 flex-1 overflow-y-auto p-0">
                <ActivityPanel market={market} />
            </TabsContent>
        </Tabs>
    );
}
