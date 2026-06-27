import {
    BaseSignerWalletAdapter,
    WalletReadyState,
    type SupportedTransactionVersions,
    type WalletName,
} from "@solana/wallet-adapter-base";
import { Keypair, Transaction, VersionedTransaction, type TransactionVersion } from "@solana/web3.js";

const BURNER_NAME = "Burner (devnet)" as WalletName<"Burner (devnet)">;

// 1×1 transparent svg data-uri (wallet-adapter requires an icon data URL).
const ICON =
    "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxIiBoZWlnaHQ9IjEiLz4=" as const;

/**
 * Dev-only wallet adapter backed by a local `Keypair`, so transactions can be
 * signed in-page without a browser extension (devnet testing / automation).
 * NEVER enable this against mainnet — the secret lives in the client bundle.
 */
export class BurnerWalletAdapter extends BaseSignerWalletAdapter {
    readonly name = BURNER_NAME;
    readonly url = "https://docs.tempo.dev";
    readonly icon = ICON;
    readonly supportedTransactionVersions: SupportedTransactionVersions = new Set<TransactionVersion>([
        "legacy",
        0,
    ]);
    readonly readyState = WalletReadyState.Installed;

    private readonly keypair: Keypair;
    private connectingFlag = false;
    private connectedKey: Keypair | null = null;

    constructor(secretKey: Uint8Array) {
        super();
        this.keypair = Keypair.fromSecretKey(secretKey);
    }

    get connecting(): boolean {
        return this.connectingFlag;
    }

    get publicKey() {
        return this.connectedKey ? this.connectedKey.publicKey : null;
    }

    async connect(): Promise<void> {
        if (this.connectedKey) return;
        this.connectingFlag = true;
        this.connectedKey = this.keypair;
        this.connectingFlag = false;
        this.emit("connect", this.keypair.publicKey);
    }

    async disconnect(): Promise<void> {
        this.connectedKey = null;
        this.emit("disconnect");
    }

    async signTransaction<T extends Transaction | VersionedTransaction>(transaction: T): Promise<T> {
        if (transaction instanceof VersionedTransaction) {
            transaction.sign([this.keypair]);
        } else {
            transaction.partialSign(this.keypair);
        }
        return transaction;
    }
}

/** Parse a secret key from a JSON byte array (Solana `id.json`) or base58. */
export function parseBurnerSecret(raw: string): Uint8Array {
    const trimmed = raw.trim();
    if (trimmed.startsWith("[")) {
        return Uint8Array.from(JSON.parse(trimmed) as number[]);
    }
    // base58 fallback
    return Keypair.fromSecretKey(bs58Decode(trimmed)).secretKey;
}

// Minimal base58 decode (avoids adding a dep for the dev-only burner path).
function bs58Decode(s: string): Uint8Array {
    const ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    const bytes: number[] = [0];
    for (const ch of s) {
        const value = ALPHABET.indexOf(ch);
        if (value === -1) throw new Error("invalid base58 char");
        let carry = value;
        for (let j = 0; j < bytes.length; j++) {
            carry += bytes[j]! * 58;
            bytes[j] = carry & 0xff;
            carry >>= 8;
        }
        while (carry > 0) {
            bytes.push(carry & 0xff);
            carry >>= 8;
        }
    }
    for (let k = 0; k < s.length && s[k] === "1"; k++) bytes.push(0);
    return Uint8Array.from(bytes.reverse());
}
