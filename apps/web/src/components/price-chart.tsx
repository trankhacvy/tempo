"use client";

import {
    CandlestickSeries,
    ColorType,
    LineStyle,
    createChart,
    type CandlestickData,
    type IChartApi,
    type IPriceLine,
    type ISeriesApi,
    type UTCTimestamp,
} from "lightweight-charts";
import { useEffect, useRef } from "react";

import { type MarketView } from "@/lib/data";
import { fetchSolUsdHistory, useSolUsdPrice, type OhlcBar } from "@/lib/pyth";
import { tickToUsd } from "@/lib/tempo-math";

// lightweight-charts requires actual color strings — keep these as constants.
const C_UP = "#3ecf8e"; // matches --up oklch(0.76 0.17 152)
const C_DOWN = "#e23b3b"; // matches --down oklch(0.625 0.232 22)
const C_GRID = "rgba(255,255,255,0.05)";
const C_BORDER = "rgba(255,255,255,0.1)";
const C_TEXT = "hsl(0, 0%, 61%)"; // muted-foreground

const HISTORY_SECONDS = 2 * 60 * 60;
const BAR_SECONDS = 60;

function toCandle(bar: OhlcBar): CandlestickData<UTCTimestamp> {
    return {
        time: bar.time as UTCTimestamp,
        open: bar.open,
        high: bar.high,
        low: bar.low,
        close: bar.close,
    };
}

function clearingUsd(view: MarketView | null): number | null {
    if (!view) return null;
    const bid = tickToUsd(view.lastBidFillPrice, view.tickSize);
    const ask = tickToUsd(view.lastAskFillPrice, view.tickSize);
    if (bid !== null && ask !== null) return (bid + ask) / 2;
    return bid ?? ask;
}

export function PriceChart({ view }: { view: MarketView | null }) {
    const { price } = useSolUsdPrice();
    const containerRef = useRef<HTMLDivElement | null>(null);
    const chartRef = useRef<IChartApi | null>(null);
    const seriesRef = useRef<ISeriesApi<"Candlestick"> | null>(null);
    const lastBarRef = useRef<CandlestickData<UTCTimestamp> | null>(null);
    const priceLineRef = useRef<IPriceLine | null>(null);

    useEffect(() => {
        const container = containerRef.current;
        if (!container) return;

        const chart = createChart(container, {
            layout: {
                background: { type: ColorType.Solid, color: "transparent" },
                textColor: C_TEXT,
                fontFamily: "var(--font-mono), ui-monospace, monospace",
                attributionLogo: false,
            },
            grid: {
                vertLines: { color: C_GRID },
                horzLines: { color: C_GRID },
            },
            rightPriceScale: { borderColor: C_BORDER },
            timeScale: { borderColor: C_BORDER, timeVisible: true, secondsVisible: false },
            crosshair: {
                horzLine: { labelBackgroundColor: C_DOWN },
                vertLine: { labelBackgroundColor: C_DOWN },
            },
            autoSize: true,
        });

        const series = chart.addSeries(CandlestickSeries, {
            upColor: C_UP,
            downColor: C_DOWN,
            wickUpColor: C_UP,
            wickDownColor: C_DOWN,
            borderVisible: false,
        });

        chartRef.current = chart;
        seriesRef.current = series;

        const controller = new AbortController();
        const now = Math.floor(Date.now() / 1000);
        void fetchSolUsdHistory(now - HISTORY_SECONDS, now, controller.signal)
            .then((bars) => {
                if (controller.signal.aborted || bars.length === 0) return;
                const candles = bars.map(toCandle);
                series.setData(candles);
                lastBarRef.current = candles[candles.length - 1] ?? null;
                chart.timeScale().fitContent();
            })
            .catch(() => undefined);

        return () => {
            controller.abort();
            chart.remove();
            chartRef.current = null;
            seriesRef.current = null;
            lastBarRef.current = null;
            priceLineRef.current = null;
        };
    }, []);

    // Stream live price into the current bar.
    useEffect(() => {
        const series = seriesRef.current;
        if (!series || price === null) return;
        const slot = (Math.floor(Date.now() / 1000 / BAR_SECONDS) * BAR_SECONDS) as UTCTimestamp;
        const last = lastBarRef.current;

        const next: CandlestickData<UTCTimestamp> =
            last && last.time === slot
                ? {
                      time: slot,
                      open: last.open,
                      high: Math.max(last.high, price),
                      low: Math.min(last.low, price),
                      close: price,
                  }
                : { time: slot, open: last?.close ?? price, high: price, low: price, close: price };

        if (last && next.time < last.time) return;
        series.update(next);
        lastBarRef.current = next;
    }, [price]);

    // Overlay the last clearing price as a dashed line.
    useEffect(() => {
        const series = seriesRef.current;
        if (!series) return;
        const usd = clearingUsd(view);
        if (priceLineRef.current) {
            series.removePriceLine(priceLineRef.current);
            priceLineRef.current = null;
        }
        if (usd === null) return;
        priceLineRef.current = series.createPriceLine({
            price: usd,
            color: C_DOWN,
            lineWidth: 1,
            lineStyle: LineStyle.Dashed,
            axisLabelVisible: true,
            title: "clear",
        });
    }, [view]);

    return (
        <div className="flex min-h-0 flex-1 flex-col">
            {/* Chart header */}
            <div className="flex h-9 shrink-0 items-center justify-between border-b border-border px-4">
                <div className="flex items-center gap-2">
                    <span className="text-sm font-semibold">SOL / USD</span>
                    <span className="font-mono text-sm text-muted-foreground tnum">
                        {price !== null ? `$${price.toFixed(2)}` : "—"}
                    </span>
                </div>
                <span className="text-[10px] uppercase tracking-wide text-muted-foreground/70">
                    Pyth · devnet
                </span>
            </div>

            {/* Chart canvas */}
            <div ref={containerRef} className="relative min-h-0 flex-1" />
        </div>
    );
}
