"use client";

import { useEffect, useRef } from "react";

/** Run `callback` every `delayMs`. A `null` delay pauses the interval. The
 *  latest callback is always used without resetting the timer (Dan Abramov's
 *  pattern). No external dependency. */
export function useInterval(callback: () => void, delayMs: number | null): void {
    const saved = useRef(callback);

    useEffect(() => {
        saved.current = callback;
    }, [callback]);

    useEffect(() => {
        if (delayMs === null) return;
        const id = setInterval(() => saved.current(), delayMs);
        return () => clearInterval(id);
    }, [delayMs]);
}
