use {
    anchor_lang::{
        prelude::Pubkey,
        solana_program::{program_option::COption, program_pack::Pack},
    },
    anchor_spl::token::spl_token::{
        self,
        state::{Account as TokenAccount, AccountState, Mint},
    },
    crucible_fuzzer::TestContext,
};

pub const HOLDERS: usize = 5;
pub const BASES: usize = 3;
pub const TRACKED: usize = HOLDERS + BASES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
    pub delegate: Option<Pubkey>,
    pub delegated_amount: u64,
    pub frozen: bool,
    pub close_authority: Option<Pubkey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub supply: u64,
    pub tracked: [Option<Token>; TRACKED],
    pub foreign: Option<Token>,
}

fn option(value: COption<Pubkey>) -> Option<Pubkey> {
    match value {
        COption::Some(key) => Some(key),
        COption::None => None,
    }
}

fn token(ctx: &TestContext, address: &Pubkey) -> Option<Token> {
    let account = ctx.svm.get_account(address)?;
    if account.owner != spl_token::ID || account.lamports == 0 || account.data.is_empty() {
        return None;
    }
    let state = TokenAccount::unpack(&account.data).ok()?;
    Some(Token {
        mint: state.mint,
        owner: state.owner,
        amount: state.amount,
        delegate: option(state.delegate),
        delegated_amount: state.delegated_amount,
        frozen: state.state == AccountState::Frozen,
        close_authority: option(state.close_authority),
    })
}

impl View {
    pub fn read(
        ctx: &TestContext,
        usdc: &Pubkey,
        tracked: &[Pubkey; TRACKED],
        foreign: &Pubkey,
    ) -> Self {
        let supply = ctx
            .svm
            .get_account(usdc)
            .and_then(|account| Mint::unpack(&account.data).ok())
            .map_or(0, |mint| mint.supply);
        Self {
            supply,
            tracked: tracked.each_ref().map(|address| token(ctx, address)),
            foreign: token(ctx, foreign),
        }
    }

    pub fn source(&self, index: usize) -> Option<Token> {
        self.tracked.get(index).copied().unwrap_or(self.foreign)
    }

    pub fn tracked_total(&self) -> u128 {
        self.tracked
            .iter()
            .flatten()
            .map(|token| u128::from(token.amount))
            .sum()
    }
}
