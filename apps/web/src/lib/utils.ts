import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export function shortenAddress(address: string, chars = 4): string {
    return `${address.slice(0, chars)}…${address.slice(-chars)}`;
}

export function isValidBase58Address(addr: string): boolean {
    return /^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(addr.trim());
}

export function formatUnits(value: bigint, decimals: number): string {
    const divisor = 10n ** BigInt(decimals);
    const whole = value / divisor;
    const remainder = value % divisor;
    if (remainder === 0n) return whole.toString();
    const fracStr = remainder.toString().padStart(decimals, "0").replace(/0+$/, "");
    return `${whole}.${fracStr}`;
}

export function parseUnits(value: string, decimals: number): bigint {
    const [whole = "0", frac = ""] = value.trim().split(".");
    const fracPadded = frac.padEnd(decimals, "0").slice(0, decimals);
    return BigInt(whole) * 10n ** BigInt(decimals) + BigInt(fracPadded || "0");
}
