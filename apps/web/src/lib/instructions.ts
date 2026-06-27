import {
  address,
  type Address,
  type Instruction,
  type TransactionSigner,
} from "@solana/kit";

import {
  COLLATERAL_MINT,
  USER_TOKEN_ACCOUNT,
  VAULT_TOKEN_ACCOUNT,
} from "./config";
import {
  deriveOrderPdas,
  deriveUserCollateralPda,
  deriveVaultAuthorityPda,
  deriveVaultPda,
  getCancelOrderInstructionAsync,
  getDepositInstructionAsync,
  getInitCollateralInstructionAsync,
  getInitPositionInstructionAsync,
  getSettleFillInstructionAsync,
  getSubmitOrderInstructionAsync,
  getWithdrawInstructionAsync,
  TEMPO_PROGRAM,
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

export async function buildInitCollateralIx(
  owner: string,
): Promise<Instruction> {
  const signer = addressSigner(owner);
  return getInitCollateralInstructionAsync({ payer: signer, owner: signer });
}

export interface CollateralAccounts {
  /** Defaults to NEXT_PUBLIC_USER_TOKEN_ACCOUNT, else the provided override. */
  userTokenAccount?: string;
}

export async function buildDepositIx(
  owner: string,
  amount: bigint,
  opts: CollateralAccounts = {},
): Promise<Instruction> {
  const signer = addressSigner(owner);
  const vaultTokenAccount = requireConfigured(
    VAULT_TOKEN_ACCOUNT,
    "NEXT_PUBLIC_VAULT_TOKEN_ACCOUNT",
  );
  const userTokenAccount = requireConfigured(
    opts.userTokenAccount ?? USER_TOKEN_ACCOUNT,
    "NEXT_PUBLIC_USER_TOKEN_ACCOUNT",
  );
  const collateralMint = requireConfigured(
    COLLATERAL_MINT,
    "NEXT_PUBLIC_COLLATERAL_MINT",
  );
  const vault = await deriveVaultPda(collateralMint);
  return getDepositInstructionAsync({
    owner: signer,
    vault,
    vaultTokenAccount,
    userTokenAccount,
    amount,
  });
}

export async function buildWithdrawIx(
  owner: string,
  amount: bigint,
  opts: CollateralAccounts = {},
): Promise<Instruction> {
  const signer = addressSigner(owner);
  const vaultTokenAccount = requireConfigured(
    VAULT_TOKEN_ACCOUNT,
    "NEXT_PUBLIC_VAULT_TOKEN_ACCOUNT",
  );
  const userTokenAccount = requireConfigured(
    opts.userTokenAccount ?? USER_TOKEN_ACCOUNT,
    "NEXT_PUBLIC_USER_TOKEN_ACCOUNT",
  );
  const collateralMint = requireConfigured(
    COLLATERAL_MINT,
    "NEXT_PUBLIC_COLLATERAL_MINT",
  );
  const vault = await deriveVaultPda(collateralMint);
  const vaultAuthority = await deriveVaultAuthorityPda();
  return getWithdrawInstructionAsync({
    owner: signer,
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
  const { orderSlab, eventAuthority } = await deriveOrderPdas(market);
  return getSubmitOrderInstructionAsync({
    trader: signer,
    market,
    orderSlab,
    eventAuthority,
    tempoProgram: TEMPO_PROGRAM,
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
  const { eventAuthority } = await deriveOrderPdas(address(market));
  let userCollateral: Address | undefined;
  let vault: Address | undefined;
  if (COLLATERAL_MINT) {
    userCollateral = await deriveUserCollateralPda(address(owner));
    vault = await deriveVaultPda(address(COLLATERAL_MINT));
  }
  return getSettleFillInstructionAsync({
    cranker: signer,
    market: address(market),
    eventAuthority,
    tempoProgram: TEMPO_PROGRAM,
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
  const { eventAuthority } = await deriveOrderPdas(address(market));
  return getCancelOrderInstructionAsync({
    trader: signer,
    market: address(market),
    eventAuthority,
    tempoProgram: TEMPO_PROGRAM,
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
