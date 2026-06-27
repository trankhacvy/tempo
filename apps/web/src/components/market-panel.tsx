"use client";

import { RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataRow } from "@/components/data-row";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { EmptyHint, SkeletonRows } from "@/components/ui/skeleton";
import { DEFAULT_MARKET } from "@/lib/config";
import { fetchMarketView, type MarketView } from "@/lib/data";
import { phaseLabel, Phase } from "@/lib/tempo-client";
import { explorerAddressUrl } from "@/lib/tx";
import { isValidBase58Address, shortenAddress } from "@/lib/utils";

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

export function MarketPanel({ onMarketLoaded }: { onMarketLoaded?: (view: MarketView) => void }) {
    const [input, setInput] = useState(DEFAULT_MARKET);
    const [market, setMarket] = useState<MarketView | null>(null);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const load = useCallback(
        async (addr: string) => {
            if (!isValidBase58Address(addr)) {
                setError("Enter a valid market address.");
                return;
            }
            setLoading(true);
            setError(null);
            try {
                const view = await fetchMarketView(addr.trim());
                if (view === null) {
                    setMarket(null);
                    setError("No account found at that address on devnet.");
                    return;
                }
                setMarket(view);
                onMarketLoaded?.(view);
            } catch (e) {
                setMarket(null);
                setError(e instanceof Error ? e.message : String(e));
            } finally {
                setLoading(false);
            }
        },
        [onMarketLoaded],
    );

    useEffect(() => {
        if (DEFAULT_MARKET && isValidBase58Address(DEFAULT_MARKET)) void load(DEFAULT_MARKET);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    return (
        <div className="space-y-4 p-4">
            <div className="space-y-2">
                <Label htmlFor="market-address">Market address</Label>
                <div className="flex gap-2">
                    <Input
                        id="market-address"
                        placeholder="Market PDA address"
                        value={input}
                        onChange={(e) => setInput(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter") void load(input);
                        }}
                        disabled={loading}
                        className="font-mono"
                    />
                    <Button onClick={() => void load(input)} disabled={loading || input.trim() === ""}>
                        {loading ? <RefreshCw className="size-4 animate-spin" /> : "Load"}
                    </Button>
                </div>
            </div>

            {error && <p className="text-sm text-destructive">{error}</p>}

            {!DEFAULT_MARKET && !market && !loading && !error && (
                <EmptyHint title="No market configured">
                    Set <code className="font-mono">NEXT_PUBLIC_TEMPO_MARKET</code> in{" "}
                    <code className="font-mono">.env.local</code>, or paste a devnet Market PDA
                    above. Run <code className="font-mono">just keeper</code> to create one on
                    devnet.
                </EmptyHint>
            )}

            {loading && !market && <SkeletonRows rows={6} />}

            {market && (
                <div className="space-y-1">
                    <div className="flex items-center justify-between pb-1">
                        <a
                            href={explorerAddressUrl(market.address)}
                            target="_blank"
                            rel="noreferrer"
                            className="font-mono text-xs text-muted-foreground underline-offset-2 hover:underline"
                        >
                            {shortenAddress(market.address, 6)}
                        </a>
                        <Badge variant={phaseVariant(market.phase)}>{phaseLabel(market.phase)}</Badge>
                    </div>
                    <DataRow label="Auction ID">{market.auctionId.toString()}</DataRow>
                    <DataRow label="Tick size">{market.tickSize.toString()}</DataRow>
                    <DataRow label="Num ticks">{market.numTicks}</DataRow>
                    <DataRow label="Last bid fill">{market.lastBidFillPrice.toString()}</DataRow>
                    <DataRow label="Last ask fill">{market.lastAskFillPrice.toString()}</DataRow>
                    <DataRow label="Funding index">{market.fundingIndex.toString()}</DataRow>
                    <DataRow label="Active orders">{market.activeOrderCount.toString()}</DataRow>
                    <DataRow label="Orders / auction cap">{market.ordersPerAuctionCap}</DataRow>
                    <DataRow label="Oracle">{shortenAddress(market.oracle, 6)}</DataRow>
                    <DataRow label="Authority">{shortenAddress(market.authority, 6)}</DataRow>
                </div>
            )}
        </div>
    );
}
