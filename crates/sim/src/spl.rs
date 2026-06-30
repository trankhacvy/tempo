//! Minimal off-chain SPL Token + Associated-Token-Account helpers, hand-rolled over
//! the stable instruction byte layouts so the crate needs no `spl-token` dependency
//! (which would fight the workspace's solana-3 dependency tree). Used only by the
//! one-shot provisioner, over a blocking RPC client.

use solana_client::rpc_client::RpcClient;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature, Signer};
use solana_sdk::transaction::Transaction;
use solana_system_interface::instruction as system_instruction;

use crate::error::SimError;

/// SPL Token program (`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`).
pub const SPL_TOKEN_PROGRAM_ID: Pubkey =
    solana_sdk::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// Associated Token Account program (`ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL`).
pub const ATA_PROGRAM_ID: Pubkey =
    solana_sdk::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/// System program (`11111111111111111111111111111111`).
pub const SYSTEM_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("11111111111111111111111111111111");

/// Byte length of an SPL `Mint` account.
const MINT_LEN: usize = 82;

/// Build, sign (with all `signers`, the first being the fee payer), send, and
/// confirm a transaction.
pub fn send(
    rpc: &RpcClient,
    signers: &[&Keypair],
    ixs: &[Instruction],
) -> Result<Signature, SimError> {
    let payer = signers
        .first()
        .ok_or_else(|| SimError::Provision("send: no signers".into()))?;
    let blockhash = rpc
        .get_latest_blockhash()
        .map_err(|e| SimError::Rpc(e.to_string()))?;
    let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), signers, blockhash);
    rpc.send_and_confirm_transaction(&tx)
        .map_err(|e| SimError::Rpc(e.to_string()))
}

/// The associated token account address for `(wallet, mint)`. `wallet` may be a PDA
/// (e.g. the vault authority).
pub fn associated_token_address(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            wallet.as_ref(),
            SPL_TOKEN_PROGRAM_ID.as_ref(),
            mint.as_ref(),
        ],
        &ATA_PROGRAM_ID,
    )
    .0
}

/// Create an SPL mint at `mint`'s address with `decimals`, mint authority =
/// `master`, no freeze authority. The mint keypair is supplied so re-provisioning is
/// deterministic.
pub fn create_mint(
    rpc: &RpcClient,
    master: &Keypair,
    mint: &Keypair,
    decimals: u8,
) -> Result<(), SimError> {
    let rent = rpc
        .get_minimum_balance_for_rent_exemption(MINT_LEN)
        .map_err(|e| SimError::Rpc(e.to_string()))?;

    let create = system_instruction::create_account(
        &master.pubkey(),
        &mint.pubkey(),
        rent,
        MINT_LEN as u64,
        &SPL_TOKEN_PROGRAM_ID,
    );

    // InitializeMint2 (tag 20): decimals, mint_authority(32), freeze COption::None(1).
    let mut data = Vec::with_capacity(1 + 1 + 32 + 1);
    data.push(20u8);
    data.push(decimals);
    data.extend_from_slice(master.pubkey().as_ref());
    data.push(0u8); // freeze authority = None
    let init = Instruction {
        program_id: SPL_TOKEN_PROGRAM_ID,
        accounts: vec![AccountMeta::new(mint.pubkey(), false)],
        data,
    };

    send(rpc, &[master, mint], &[create, init])?;
    Ok(())
}

/// Create the associated token account for `(owner, mint)` if absent (idempotent).
/// Returns the ATA address.
pub fn create_ata(
    rpc: &RpcClient,
    master: &Keypair,
    owner: &Pubkey,
    mint: &Pubkey,
) -> Result<Pubkey, SimError> {
    let ata = associated_token_address(owner, mint);
    // ATA instruction enum: 1 = CreateIdempotent.
    let ix = Instruction {
        program_id: ATA_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(master.pubkey(), true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*owner, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
        ],
        data: vec![1u8],
    };
    send(rpc, &[master], &[ix])?;
    Ok(ata)
}

/// Mint `amount` base units to `dest` (signed by the mint authority, `master`).
pub fn mint_to(
    rpc: &RpcClient,
    master: &Keypair,
    mint: &Pubkey,
    dest: &Pubkey,
    amount: u64,
) -> Result<(), SimError> {
    // MintTo (tag 7): amount u64 LE.
    let mut data = Vec::with_capacity(1 + 8);
    data.push(7u8);
    data.extend_from_slice(&amount.to_le_bytes());
    let ix = Instruction {
        program_id: SPL_TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*dest, false),
            AccountMeta::new_readonly(master.pubkey(), true),
        ],
        data,
    };
    send(rpc, &[master], &[ix])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ata_derivation_is_deterministic() {
        let wallet = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        assert_eq!(
            associated_token_address(&wallet, &mint),
            associated_token_address(&wallet, &mint)
        );
    }

    #[test]
    fn well_known_program_ids() {
        assert_eq!(
            SPL_TOKEN_PROGRAM_ID.to_string(),
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        );
        assert_eq!(
            ATA_PROGRAM_ID.to_string(),
            "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
        );
    }
}
