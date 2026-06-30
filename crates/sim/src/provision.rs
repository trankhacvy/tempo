//! The one-shot provisioner. Driven by the master keypair, it stands up the
//! simulated market and (in Phase B) the money path, then funds and initializes the
//! agent accounts and writes the artifact. Every step is idempotent so a re-run
//! after a devnet reset recovers the world from the persisted keys.

use std::path::Path;

use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use solana_system_interface::instruction as system_instruction;

use tempo_common::load_keypair_file;
use tempo_sdk::accounts::UserCollateralView;
use tempo_sdk::ix::{InitVault, InitVaultInstructionArgs, InitializeMarket, InitializeMarketInstructionArgs};
use tempo_sdk::{ix, pda, MarketPdas, SOL_USD_FEED_ID, TEMPO_PROGRAM_ID};

use crate::artifact::{AgentEntry, SimArtifact, TraderEntry};
use crate::config::ProvisionConfig;
use crate::error::SimError;
use crate::persona::Persona;
use crate::spl::{self, SYSTEM_PROGRAM_ID};

const PERSONA_CYCLE: [&str; 4] = ["noise", "momentum", "passive", "reckless"];

/// Stand up the whole simulation and return the artifact.
pub fn provision(cfg: &ProvisionConfig) -> Result<SimArtifact, SimError> {
    let rpc = RpcClient::new_with_commitment(cfg.rpc_url.clone(), commitment(&cfg.commitment));
    let master = load_keypair_file(&cfg.master_keypair).map_err(SimError::Common)?;
    std::fs::create_dir_all(&cfg.keys_dir)?;

    let master_balance = rpc
        .get_balance(&master.pubkey())
        .map_err(|e| SimError::Rpc(e.to_string()))?;
    tracing::info!(master = %master.pubkey(), lamports = master_balance, "provisioner: master loaded");
    if master_balance == 0 {
        return Err(SimError::Provision(
            "master keypair has 0 SOL — fund it on devnet before provisioning".into(),
        ));
    }

    // --- agents ---
    let (keeper_kp, keeper_path) = load_or_create(&cfg.keys_dir, "keeper")?;
    let (liq_kp, liq_path) = load_or_create(&cfg.keys_dir, "liquidator")?;
    let mut mm: Vec<(Keypair, String)> = Vec::new();
    for i in 0..cfg.num_mm {
        mm.push(load_or_create(&cfg.keys_dir, &format!("mm-{i}"))?);
    }
    let mut traders: Vec<(Keypair, String, Persona, u64)> = Vec::new();
    for i in 0..cfg.num_traders {
        let (kp, path) = load_or_create(&cfg.keys_dir, &format!("trader-{i}"))?;
        let persona = Persona::parse(PERSONA_CYCLE[(i as usize) % PERSONA_CYCLE.len()]);
        traders.push((kp, path, persona, (i as u64) + 1));
    }

    // --- fund agents with SOL for rent + fees ---
    fund_if_needed(&rpc, &master, &keeper_kp.pubkey(), cfg.fund_lamports)?;
    fund_if_needed(&rpc, &master, &liq_kp.pubkey(), cfg.fund_lamports)?;
    for (kp, _) in &mm {
        fund_if_needed(&rpc, &master, &kp.pubkey(), cfg.fund_lamports)?;
    }
    for (kp, _, _, _) in &traders {
        fund_if_needed(&rpc, &master, &kp.pubkey(), cfg.fund_lamports)?;
    }

    // --- market ---
    let (market_seed_kp, _) = load_or_create(&cfg.keys_dir, "market-seed")?;
    let oracle: Pubkey = cfg
        .oracle
        .parse()
        .map_err(|_| SimError::Config("invalid TEMPO_SIM_ORACLE pubkey".into()))?;

    let mint = if cfg.is_money_market() {
        Some(ensure_mint(&rpc, &master, cfg)?)
    } else {
        None
    };

    let (market, _) = pda::market(&market_seed_kp.pubkey());
    let pdas = MarketPdas::derive(market);
    ensure_market(&rpc, &master, &market_seed_kp, &pdas, oracle, mint, cfg)?;

    // --- money path (Phase B): vault + per-agent collateral/positions ---
    let vault_token_account = if let Some(mint) = mint {
        let vta = ensure_vault(&rpc, &master, &mint)?;
        // Liquidator needs only its penalty-receiving collateral ledger.
        ensure_collateral(&rpc, &master, &liq_kp, mint)?;
        // Makers + traders get the full money path so they can hold positions.
        for (kp, _) in &mm {
            setup_money_agent(&rpc, &master, kp, &pdas, mint, &vta, cfg)?;
        }
        for (kp, _, _, _) in &traders {
            setup_money_agent(&rpc, &master, kp, &pdas, mint, &vta, cfg)?;
        }
        Some(vta)
    } else {
        None
    };

    let artifact = SimArtifact {
        market: market.to_string(),
        market_seed_pubkey: market_seed_kp.pubkey().to_string(),
        oracle: oracle.to_string(),
        collateral_mint: mint.map(|m| m.to_string()),
        vault_token_account: vault_token_account.map(|v| v.to_string()),
        keeper: AgentEntry {
            keypair_path: keeper_path,
            pubkey: keeper_kp.pubkey().to_string(),
        },
        liquidator: AgentEntry {
            keypair_path: liq_path,
            pubkey: liq_kp.pubkey().to_string(),
        },
        market_makers: mm
            .iter()
            .map(|(kp, path)| AgentEntry {
                keypair_path: path.clone(),
                pubkey: kp.pubkey().to_string(),
            })
            .collect(),
        traders: traders
            .iter()
            .map(|(kp, path, persona, seed)| TraderEntry {
                keypair_path: path.clone(),
                pubkey: kp.pubkey().to_string(),
                persona: persona.as_str().to_string(),
                seed: *seed,
            })
            .collect(),
    };
    artifact.save(&cfg.artifact_path)?;
    tracing::info!(path = %cfg.artifact_path, market = %market, "provisioner: artifact written");
    Ok(artifact)
}

fn commitment(s: &str) -> CommitmentConfig {
    match s {
        "processed" => CommitmentConfig::processed(),
        "finalized" => CommitmentConfig::finalized(),
        _ => CommitmentConfig::confirmed(),
    }
}

fn load_or_create(dir: &str, name: &str) -> Result<(Keypair, String), SimError> {
    let path = format!("{dir}/{name}.json");
    if Path::new(&path).exists() {
        let kp = load_keypair_file(&path).map_err(SimError::Common)?;
        return Ok((kp, path));
    }
    let kp = Keypair::new();
    let bytes: Vec<u8> = kp.to_bytes().to_vec();
    std::fs::write(&path, serde_json::to_string(&bytes)?)?;
    Ok((kp, path))
}

fn fund_if_needed(
    rpc: &RpcClient,
    master: &Keypair,
    agent: &Pubkey,
    lamports: u64,
) -> Result<(), SimError> {
    let balance = rpc
        .get_balance(agent)
        .map_err(|e| SimError::Rpc(e.to_string()))?;
    if balance >= lamports {
        return Ok(());
    }
    let top_up = lamports - balance;
    let ix = system_instruction::transfer(&master.pubkey(), agent, top_up);
    spl::send(rpc, &[master], &[ix])?;
    Ok(())
}

fn account_exists(rpc: &RpcClient, key: &Pubkey) -> bool {
    rpc.get_account(key).is_ok()
}

fn ensure_mint(rpc: &RpcClient, master: &Keypair, cfg: &ProvisionConfig) -> Result<Pubkey, SimError> {
    let (mint_kp, _) = load_or_create(&cfg.keys_dir, "collateral-mint")?;
    if !account_exists(rpc, &mint_kp.pubkey()) {
        spl::create_mint(rpc, master, &mint_kp, cfg.collateral_decimals)?;
        tracing::info!(mint = %mint_kp.pubkey(), "provisioner: collateral mint created");
    }
    Ok(mint_kp.pubkey())
}

#[allow(clippy::too_many_arguments)]
fn ensure_market(
    rpc: &RpcClient,
    master: &Keypair,
    market_seed: &Keypair,
    pdas: &MarketPdas,
    oracle: Pubkey,
    mint: Option<Pubkey>,
    cfg: &ProvisionConfig,
) -> Result<(), SimError> {
    if account_exists(rpc, &pdas.market) {
        tracing::info!(market = %pdas.market, "provisioner: market already exists, skipping");
        return Ok(());
    }
    let (_, market_bump) = pda::market(&market_seed.pubkey());
    let (_, histogram_bump) = pda::histogram(&pdas.market);
    let (_, order_slab_bump) = pda::order_slab(&pdas.market);
    let (event_authority, _) = pda::event_authority();

    let ix = InitializeMarket {
        payer: master.pubkey(),
        authority: master.pubkey(),
        market_seed: market_seed.pubkey(),
        market: pdas.market,
        histogram: pdas.histogram,
        order_slab: pdas.order_slab,
        oracle,
        system_program: SYSTEM_PROGRAM_ID,
        event_authority,
        tempo_program: TEMPO_PROGRAM_ID,
    }
    .instruction(InitializeMarketInstructionArgs {
        market_bump,
        histogram_bump,
        order_slab_bump,
        tick_size: cfg.tick_size,
        num_ticks: cfg.num_ticks,
        orders_per_auction_cap: cfg.cap,
        oracle_feed_id: SOL_USD_FEED_ID,
        maintenance_margin_bps: cfg.maint_bps,
        liquidation_penalty_bps: cfg.penalty_bps,
        maker_fee_bps: 0,
        taker_fee_bps: 0,
        integrator_share_bps: 0,
        crank_fee: 0,
        collateral_mint: mint.map(|m| m.to_bytes()).unwrap_or_default(),
        max_price_move_bps_per_slot: cfg.max_price_move_bps_per_slot,
        soft_stale_slots: cfg.soft_stale_slots,
        initial_margin_bps: cfg.initial_bps,
        max_position_notional: 0,
    });
    spl::send(rpc, &[master, market_seed], &[ix])?;
    tracing::info!(market = %pdas.market, money = cfg.is_money_market(), "provisioner: market created");
    Ok(())
}

fn ensure_vault(rpc: &RpcClient, master: &Keypair, mint: &Pubkey) -> Result<Pubkey, SimError> {
    let (vault, vault_bump) = pda::vault(mint);
    let (vault_authority, authority_bump) = pda::vault_authority();
    let vault_token_account = spl::create_ata(rpc, master, &vault_authority, mint)?;
    if account_exists(rpc, &vault) {
        return Ok(vault_token_account);
    }
    let ix = InitVault {
        payer: master.pubkey(),
        admin: master.pubkey(),
        vault,
        vault_token_account,
        collateral_mint: *mint,
        system_program: SYSTEM_PROGRAM_ID,
    }
    .instruction(InitVaultInstructionArgs {
        vault_bump,
        authority_bump,
    });
    spl::send(rpc, &[master], &[ix])?;
    tracing::info!(%vault, "provisioner: vault initialized");
    Ok(vault_token_account)
}

fn ensure_collateral(
    rpc: &RpcClient,
    master: &Keypair,
    owner: &Keypair,
    mint: Pubkey,
) -> Result<(), SimError> {
    let (uc, _) = pda::user_collateral(&owner.pubkey(), &mint);
    if account_exists(rpc, &uc) {
        return Ok(());
    }
    let init = ix::init_collateral(master.pubkey(), owner.pubkey(), mint);
    send_benign(rpc, &[master, owner], &[init])
}

fn setup_money_agent(
    rpc: &RpcClient,
    master: &Keypair,
    owner: &Keypair,
    pdas: &MarketPdas,
    mint: Pubkey,
    vault_token_account: &Pubkey,
    cfg: &ProvisionConfig,
) -> Result<(), SimError> {
    ensure_collateral(rpc, master, owner, mint)?;

    // Fund the ledger once: skip if it already holds free collateral.
    let (uc, _) = pda::user_collateral(&owner.pubkey(), &mint);
    let already_funded = rpc
        .get_account(&uc)
        .ok()
        .and_then(|a| UserCollateralView::decode(&a.data).ok())
        .map(|v| v.free() > 0)
        .unwrap_or(false);
    if !already_funded {
        let user_ata = spl::create_ata(rpc, master, &owner.pubkey(), &mint)?;
        spl::mint_to(rpc, master, &mint, &user_ata, cfg.deposit_amount)?;
        let dep = ix::deposit(
            owner.pubkey(),
            mint,
            *vault_token_account,
            user_ata,
            spl::SPL_TOKEN_PROGRAM_ID,
            cfg.deposit_amount,
        );
        spl::send(rpc, &[owner], &[dep])?;
    }

    let (position, _) = pda::position(&pdas.market, &owner.pubkey());
    if !account_exists(rpc, &position) {
        let init = ix::init_position(pdas, master.pubkey(), owner.pubkey());
        send_benign(rpc, &[master, owner], &[init])?;
    }
    Ok(())
}

fn send_benign(rpc: &RpcClient, signers: &[&Keypair], ixs: &[Instruction]) -> Result<(), SimError> {
    match spl::send(rpc, signers, ixs) {
        Ok(_) => Ok(()),
        Err(SimError::Rpc(s)) if is_benign(&s) => Ok(()),
        Err(e) => Err(e),
    }
}

fn is_benign(s: &str) -> bool {
    let s = s.to_lowercase();
    s.contains("already") || s.contains("custom program error")
}
