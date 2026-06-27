import { cn } from "@/lib/utils"

function Skeleton({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="skeleton"
      className={cn("animate-pulse rounded-md bg-muted", className)}
      {...props}
    />
  )
}

function SkeletonRows({ rows = 4 }: { rows?: number }) {
    return (
        <div className="space-y-2" aria-hidden>
            {Array.from({ length: rows }).map((_, i) => (
                <div key={i} className="flex items-center justify-between gap-4 py-2">
                    <Skeleton className="h-3 w-20" />
                    <Skeleton className="h-3 w-24" />
                </div>
            ))}
        </div>
    );
}

function EmptyHint({
    title,
    children,
}: {
    title: string;
    children?: React.ReactNode;
}) {
    return (
        <div className="border border-dashed border-border/70 bg-secondary/10 p-4 text-sm text-muted-foreground">
            <p className="font-medium text-foreground">{title}</p>
            {children && <div className="mt-1 text-muted-foreground">{children}</div>}
        </div>
    );
}

export { Skeleton, SkeletonRows, EmptyHint }
