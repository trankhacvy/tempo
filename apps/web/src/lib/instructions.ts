import {
  address,
  getProgramDerivedAddress,
  getUtf8Encoder,
  type Address,
  type Instruction,
  type TransactionSigner,
} from "@solana/kit";
import { findAssociatedTokenPda, TOKEN_PROGRAM_ADDRESS } from "@solana-program/token";

import {
  COLLATERAL_MINT,
  VAULT_TOKEN_ACCOUNT,
} from "./config";
import {
  findEventAuthorityPda,
  findUserCollateralPda,
  findVaultPda,
  getCancelOrderInstructionAsync,
  getDepositInstruction,
  getInitCollateralInstruction,
  getInitPositionInstructionAsync,
  getSettleFillInstructionAsync,
  getSubmitOrderInstructionAsync,
  getWithdrawInstruction,
  TEMPO_PROGRAM_PROGRAM_ADDRESS,
} from "./tempo-client";

// A minimal TransactionSigner that carries only the address. The generated
// instruction builders read `.address` to populate signer account metas; the
// actual signature is produced later by the wallet (see lib/tx.ts), so a
// no-op signer is sufficient at build time.
export function addressSigner(addr: string): TransactionSigner {
  const a = address(addr);
  return {
    address: a,
    signTransactions: async () => {
      throw new Error(
        "addressSigner cannot sign; wallet signs the assembled transaction.",
      );
    },
  };
}

function requireConfigured(value: string, name: string): Address {
  if (!value) {
    throw new Error(
      `Missing ${name}. Set it in apps/web/.env.local to enable this action.`,
    );
  }
  return address(value);
}

// --- PDA helpers (current codama client: find*Pda({seeds}) -> [address, bump]) ---

async function eventAuthorityAddr(): Promise<Address> {
  const [pda] = await findEventAuthorityPda();
  return pda;
}

async function userCollateralAddr(owner: Address, mint: Address): Promise<Address> {
  const [pda] = await findUserCollateralPda({ owner, mint });
  return pda;
}

async function vaultAddr(mint: Address): Promise<Address> {
  const [pda] = await findVaultPda({ collateralMint: mint });
  return pda;
}

/** The connected wallet's associated token account for the collateral mint —
 *  derived per-wallet (the faucet creates it), not a fixed env account. */
async function userAtaAddr(owner: Address, mint: Address): Promise<Address> {
  const [ata] = await findAssociatedTokenPda({
    owner,
    mint,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });
  return ata;
}

// The vault authority is a seed-only PDA with no generated finder.
async function vaultAuthorityAddr(): Promise<Address> {
  const [pda] = await getProgramDerivedAddress({
    programAddress: TEMPO_PROGRAM_PROGRAM_ADDRESS,
    seeds: [getUtf8Encoder().encode("vault_authority")],
  });
  return pda;
}

export async function buildInitCollateralIx(
  owner: string,
): Promise<Instruction> {
  const signer = addressSigner(owner);
  const mint = requireConfigured(COLLATERAL_MINT, "NEXT_PUBLIC_COLLATERAL_MINT");
  // init needs the PDA + bump, so pass the full ProgramDerivedAddress tuple.
  const userCollateral = await findUserCollateralPda({
    owner: address(owner),
    mint,
  });
  const vault = await vaultAddr(mint);
  return getInitCollateralInstruction({
    payer: signer,
    owner: signer,
    userCollateral,
    vault,
  });
}

export async function buildDepositIx(
  owner: string,
  amount: bigint,
): Promise<Instruction> {
  const signer = addressSigner(owner);
  const vaultTokenAccount = requireConfigured(
    VAULT_TOKEN_ACCOUNT,
    "NEXT_PUBLIC_VAULT_TOKEN_ACCOUNT",
  );
  const mint = requireConfigured(COLLATERAL_MINT, "NEXT_PUBLIC_COLLATERAL_MINT");
  const userTokenAccount = await userAtaAddr(address(owner), mint);
  const userCollateral = await userCollateralAddr(address(owner), mint);
  const vault = await vaultAddr(mint);
  return getDepositInstruction({
    owner: signer,
    userCollateral,
    vault,
    vaultTokenAccount,
    userTokenAccount,
    amount,
  });
}

export async function buildWithdrawIx(
  owner: string,
  amount: bigint,
): Promise<Instruction> {
  const signer = addressSigner(owner);
  const vaultTokenAccount = requireConfigured(
    VAULT_TOKEN_ACCOUNT,
    "NEXT_PUBLIC_VAULT_TOKEN_ACCOUNT",
  );
  const mint = requireConfigured(COLLATERAL_MINT, "NEXT_PUBLIC_COLLATERAL_MINT");
  const userTokenAccount = await userAtaAddr(address(owner), mint);
  const userCollateral = await userCollateralAddr(address(owner), mint);
  const vault = await vaultAddr(mint);
  const vaultAuthority = await vaultAuthorityAddr();
  return getWithdrawInstruction({
    owner: signer,
    userCollateral,
    vault,
    vaultAuthority,
    vaultTokenAccount,
    userTokenAccount,
    amount,
  });
}

export interface OrderParams {
  market: string;
  side: 0 | 1; // 0 = buy, 1 = sell
  price: bigint;
  quantity: bigint;
}

export async function buildSubmitOrderIx(
  trader: string,
  params: OrderParams,
): Promise<Instruction> {
  const signer = addressSigner(trader);
  const market = address(params.market);
  const eventAuthority = await eventAuthorityAddr();
  // orderSlab is auto-derived by the async builder from `market`.
  return getSubmitOrderInstructionAsync({
    trader: signer,
    market,
    eventAuthority,
    tempoProgram: TEMPO_PROGRAM_PROGRAM_ADDRESS,
    side: params.side,
    price: params.price,
    quantity: params.quantity,
    reduceOnly: false,
  });
}

/** Open the connected wallet's Position PDA for a market (payer == owner). */
export async function buildInitPositionIx(
  owner: string,
  market: string,
): Promise<Instruction> {
  const signer = addressSigner(owner);
  // position is auto-derived by the async builder from `market` + `owner`.
  return getInitPositionInstructionAsync({
    payer: signer,
    owner: signer,
    market: address(market),
  });
}

/** Pull the connected wallet's own fill from a published ClearingResult.
 *  `position` is required by the program for any non-zero fill; `userCollateral`
 *  + `vault` are wired when a collateral mint is configured (margin money path). */
export async function buildSettleFillIx(
  owner: string,
  market: string,
  orderId: bigint,
  position: string,
  // Slab slot index from the `OrderSubmitted` event (known-issues §2.7): the O(1)
  // settle hint. Defaults to an out-of-range sentinel that forces the on-chain
  // scan fallback when the caller doesn't track slots. Validated on-chain either way.
  slotHint = 0xffffffff,
): Promise<Instruction> {
  const signer = addressSigner(owner);
  const eventAuthority = await eventAuthorityAddr();
  let userCollateral: Address | undefined;
  let vault: Address | undefined;
  if (COLLATERAL_MINT) {
    const mint = address(COLLATERAL_MINT);
    userCollateral = await userCollateralAddr(address(owner), mint);
    vault = await vaultAddr(mint);
  }
  // orderSlab + clearingResult are auto-derived by the async builder from `market`.
  return getSettleFillInstructionAsync({
    cranker: signer,
    market: address(market),
    eventAuthority,
    tempoProgram: TEMPO_PROGRAM_PROGRAM_ADDRESS,
    orderId,
    slotHint,
    position: address(position),
    userCollateral,
    vault,
  });
}

/** Cancel a resting order the connected wallet owns. */
export async function buildCancelOrderIx(
  trader: string,
  market: string,
  orderId: bigint,
  // O(1) slab slot hint (§2.7); defaults to the scan-fallback sentinel.
  slotHint = 0xffffffff,
): Promise<Instruction> {
  const signer = addressSigner(trader);
  const eventAuthority = await eventAuthorityAddr();
  return getCancelOrderInstructionAsync({
    trader: signer,
    market: address(market),
    eventAuthority,
    tempoProgram: TEMPO_PROGRAM_PROGRAM_ADDRESS,
    orderId,
    slotHint,
  });
}

/** Close (or reduce) a position by submitting an order on the opposing side.
 *  Flips `side` of the supplied params; the batch auction matches it next round. */
export async function buildCloseIx(
  trader: string,
  params: OrderParams,
): Promise<Instruction> {
  const opposing: OrderParams = { ...params, side: params.side === 0 ? 1 : 0 };
  return buildSubmitOrderIx(trader, opposing);
}

/** Whether collateral deposit/withdraw is configured in env. */
export const collateralConfigured = Boolean(VAULT_TOKEN_ACCOUNT);
