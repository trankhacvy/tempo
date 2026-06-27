import type { ReactNode } from "react";

export function DataRow({ label, children }: { label: string; children: ReactNode }) {
    return (
        <div className="flex items-center justify-between gap-4 border-b border-border/60 py-2 text-sm last:border-0">
            <span className="text-muted-foreground">{label}</span>
            <span className="text-right font-mono tabular-nums text-foreground">{children}</span>
        </div>
    );
}
