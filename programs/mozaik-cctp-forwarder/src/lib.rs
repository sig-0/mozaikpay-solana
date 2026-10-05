use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{self, CloseAccount, Mint, Token, TokenAccount, TransferChecked},
};
use token_messenger_minter_v2::{cpi::accounts::DepositForBurn, types::DepositForBurnParams};

declare_id!("MzkicG4ev5reLESn2RZvPVufghwgwQhRqvhNqVr8oNb");

declare_program!(token_messenger_minter_v2);

// declare_program! reads Circle's IDL without telling cargo, so this makes cargo rebuild the
// program when the IDL changes
const _: &[u8] = include_bytes!("../idls/token_messenger_minter_v2.json");

#[cfg(not(feature = "no-entrypoint"))]
solana_security_txt::security_txt! {
    name: "MozaikPay CCTP Forwarder",
    project_url: "https://github.com/sig-0/mozaikpay-solana",
    contacts: "link:https://github.com/sig-0/mozaikpay-solana/security/advisories/new",
    policy: "https://github.com/sig-0/mozaikpay-solana/blob/main/SECURITY.md",
    preferred_languages: "en",
    source_code: "https://github.com/sig-0/mozaikpay-solana"
}

#[constant]
pub const FORWARDER_SEED: &[u8] = b"forwarder";

#[constant]
pub const MAX_FEE_BPS: u64 = 20;

#[constant]
pub const BASE_DOMAIN: u32 = 6;

#[constant]
pub const FAST_FINALITY_THRESHOLD: u32 = 1000;

#[constant]
pub const STANDARD_FINALITY_THRESHOLD: u32 = 2000;

pub const MESSAGE_TRANSMITTER_ID: Pubkey = pubkey!("CCTPV2Sm4AdWt5296sk4P66VBZ7bEhcARwFaaS9YPbeC");

#[program]
pub mod mozaik_cctp_forwarder {
    use super::*;

    /// Moves up to `amount` USDC from a token account that delegates to the forwarder of
    /// `account`, then burns the whole vault balance through CCTP V2 with `account` on Base as
    /// the mint recipient. Lowers `max_fee` to 20 bps of the burned amount when it is above that.
    /// Anyone can call it.
    pub fn forward(
        ctx: Context<Forward>,
        account: [u8; 20],
        amount: u64,
        max_fee: u64,
        min_finality_threshold: u32,
    ) -> Result<()> {
        require!(
            min_finality_threshold == FAST_FINALITY_THRESHOLD
                || min_finality_threshold == STANDARD_FINALITY_THRESHOLD,
            ForwarderError::UnsupportedFinalityThreshold
        );

        let accounts = ctx.accounts;
        let amount = amount
            .min(accounts.source.amount)
            .min(accounts.source.delegated_amount);
        require!(amount > 0, ForwarderError::NothingToForward);

        let bump = [ctx.bumps.forwarder];
        let signer: &[&[&[u8]]] = &[&[FORWARDER_SEED, &account, &bump]];

        token::transfer_checked(
            CpiContext::new_with_signer(
                Token::id(),
                TransferChecked {
                    from: accounts.source.to_account_info(),
                    mint: accounts.mint.to_account_info(),
                    to: accounts.vault.to_account_info(),
                    authority: accounts.forwarder.to_account_info(),
                },
                signer,
            ),
            amount,
            accounts.mint.decimals,
        )?;

        accounts.vault.reload()?;
        let burn_amount = accounts.vault.amount;
        let fee_cap = u128::from(burn_amount) * u128::from(MAX_FEE_BPS) / 10_000;
        // fee_cap <= burn_amount, so the conversion never saturates
        let max_fee = max_fee.min(u64::try_from(fee_cap).unwrap_or(u64::MAX));

        let mut mint_recipient = [0u8; 32];
        mint_recipient[12..].copy_from_slice(&account);

        token_messenger_minter_v2::cpi::deposit_for_burn(
            CpiContext::new_with_signer(
                token_messenger_minter_v2::ID,
                DepositForBurn {
                    owner: accounts.forwarder.to_account_info(),
                    event_rent_payer: accounts.payer.to_account_info(),
                    sender_authority_pda: accounts.sender_authority.to_account_info(),
                    burn_token_account: accounts.vault.to_account_info(),
                    denylist_account: accounts.denylist_account.to_account_info(),
                    message_transmitter: accounts.message_transmitter.to_account_info(),
                    token_messenger: accounts.token_messenger.to_account_info(),
                    remote_token_messenger: accounts.remote_token_messenger.to_account_info(),
                    token_minter: accounts.token_minter.to_account_info(),
                    local_token: accounts.local_token.to_account_info(),
                    burn_token_mint: accounts.mint.to_account_info(),
                    message_sent_event_data: accounts.message_sent_event_data.to_account_info(),
                    message_transmitter_program: accounts
                        .message_transmitter_program
                        .to_account_info(),
                    token_messenger_minter_program: accounts
                        .token_messenger_minter_program
                        .to_account_info(),
                    token_program: accounts.token_program.to_account_info(),
                    system_program: accounts.system_program.to_account_info(),
                    event_authority: accounts.event_authority.to_account_info(),
                    program: accounts.token_messenger_minter_program.to_account_info(),
                },
                signer,
            ),
            DepositForBurnParams {
                amount: burn_amount,
                destination_domain: BASE_DOMAIN,
                mint_recipient: Pubkey::new_from_array(mint_recipient),
                destination_caller: Pubkey::default(),
                max_fee,
                min_finality_threshold,
            },
        )?;

        token::close_account(CpiContext::new_with_signer(
            Token::id(),
            CloseAccount {
                account: accounts.vault.to_account_info(),
                destination: accounts.payer.to_account_info(),
                authority: accounts.forwarder.to_account_info(),
            },
            signer,
        ))
    }
}

#[derive(Accounts)]
#[instruction(account: [u8; 20])]
pub struct Forward<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: Holds no data. The seeds bind it to `account`.
    #[account(seeds = [FORWARDER_SEED, account.as_ref()], bump)]
    pub forwarder: UncheckedAccount<'info>,

    #[account(
        mut,
        token::mint = mint,
        constraint = source.delegate.contains(&forwarder.key()) @ ForwarderError::NotDelegated,
    )]
    pub source: Box<Account<'info, TokenAccount>>,

    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = mint,
        associated_token::authority = forwarder,
    )]
    pub vault: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub mint: Box<Account<'info, Mint>>,

    /// CHECK: Validated by the token messenger minter.
    pub sender_authority: UncheckedAccount<'info>,

    /// CHECK: Validated by the token messenger minter.
    pub denylist_account: UncheckedAccount<'info>,

    /// CHECK: Validated by the token messenger minter.
    #[account(mut)]
    pub message_transmitter: UncheckedAccount<'info>,

    /// CHECK: Validated by the token messenger minter.
    pub token_messenger: UncheckedAccount<'info>,

    /// CHECK: Validated by the token messenger minter.
    pub remote_token_messenger: UncheckedAccount<'info>,

    /// CHECK: Validated by the token messenger minter.
    pub token_minter: UncheckedAccount<'info>,

    /// CHECK: Validated by the token messenger minter.
    #[account(mut)]
    pub local_token: UncheckedAccount<'info>,

    #[account(mut)]
    pub message_sent_event_data: Signer<'info>,

    /// CHECK: Validated by the token messenger minter.
    pub event_authority: UncheckedAccount<'info>,

    /// CHECK: Fixed to the message transmitter program.
    #[account(address = MESSAGE_TRANSMITTER_ID)]
    pub message_transmitter_program: UncheckedAccount<'info>,

    /// CHECK: Fixed to the token messenger minter program.
    #[account(address = token_messenger_minter_v2::ID)]
    pub token_messenger_minter_program: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,

    pub associated_token_program: Program<'info, AssociatedToken>,

    pub system_program: Program<'info, System>,
}

#[error_code]
pub enum ForwarderError {
    #[msg("The amount, the source balance or the delegated amount is zero")]
    NothingToForward,
    #[msg("The source token account does not delegate to the forwarder")]
    NotDelegated,
    #[msg("The minimum finality threshold is not 1000 or 2000")]
    UnsupportedFinalityThreshold,
}
