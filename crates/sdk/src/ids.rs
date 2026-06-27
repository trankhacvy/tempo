use solana_pubkey::Pubkey;

/// The Tempo program id (`declare_id!` in `program/src/lib.rs`).
pub const TEMPO_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("8gpzMDNnKNz422jW3hs54TRmZK2H5uEwgfEQbjWAwnJD");

/// Pyth receiver program id — the owner the program requires for any
/// `PriceUpdateV2` oracle account.
pub const PYTH_RECEIVER_ID: Pubkey = Pubkey::new_from_array([
    12, 183, 250, 187, 82, 247, 166, 72, 187, 91, 49, 125, 154, 1, 139, 144, 87, 203, 2, 71, 116,
    250, 254, 1, 230, 196, 223, 152, 204, 56, 88, 129,
]);

/// SOL/USD feed id the program expects in the oracle account.
pub const SOL_USD_FEED_ID: [u8; 32] = [
    239, 13, 139, 111, 218, 44, 235, 164, 29, 161, 93, 64, 149, 209, 218, 57, 42, 13, 47, 142, 208,
    198, 199, 188, 15, 76, 250, 200, 194, 128, 181, 109,
];

/// SPL Token program id.
pub const SPL_TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

