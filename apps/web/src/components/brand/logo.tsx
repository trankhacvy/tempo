import { cn } from "@/lib/utils";

// Tempo glyph: bars converging to a single uniform clearing line (the batch auction metaphor).
export function TempoMark({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" fill="none" className={cn("h-6 w-6", className)} aria-hidden>
            <path d="M3 6h7" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
            <path d="M3 18h7" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" opacity="0.55" />
            <path d="M10 6 14 12 10 18" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
            <path d="M14 12h7" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
        </svg>
    );
}

export function Wordmark({
    className,
    glyphClassName,
}: {
    className?: string;
    glyphClassName?: string;
}) {
    return (
        <span className={cn("inline-flex items-center gap-2 font-semibold tracking-tight", className)}>
            <span className="grid h-8 w-8 place-items-center bg-primary/15 text-primary ring-1 ring-primary/30">
                <TempoMark className={cn("h-5 w-5", glyphClassName)} />
            </span>
            <span className="text-[1.05rem] font-semibold tracking-[0.02em]">Tempo</span>
        </span>
    );
}
