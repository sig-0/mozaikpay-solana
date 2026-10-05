mod cluster;
mod invariants;
mod message;
mod view;

use {
    anchor_lang::{
        InstructionData, ToAccountMetas,
        prelude::Pubkey,
        solana_program::{instruction::Instruction, program_option::COption, program_pack::Pack},
        system_program,
    },
    anchor_spl::{
        associated_token::{
            self, get_associated_token_address,
            spl_associated_token_account::instruction::create_associated_token_account_idempotent,
        },
        token::spl_token::{
            self,
            instruction::{self as token_instruction, AuthorityType},
            state::{Account as TokenAccount, AccountState, Mint},
        },
    },
    crucible_fuzzer::*,
    mozaik_cctp_forwarder::{
        FORWARDER_SEED, MESSAGE_TRANSMITTER_ID,
        token_messenger_minter_v2::ID as TOKEN_MESSENGER_MINTER_ID,
    },
    solana_account::Account,
    solana_keypair::Keypair,
    solana_signer::Signer,
    std::rc::Rc,
    view::{BASES, HOLDERS, TRACKED, Token, View},
};

const TOKEN_ACCOUNT_RENT: u64 = 2_039_280;
const MINT_RENT: u64 = 1_461_600;
const FAST: u32 = 1000;
const STANDARD: u32 = 2000;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Forward {
        source: usize,
        base: usize,
        amount: u64,
        max_fee: u64,
        threshold: u32,
        event: Pubkey,
    },
    Transfer {
        from: usize,
        to: usize,
        amount: u64,
    },
    MintTo {
        to: usize,
        amount: u64,
    },
    CreateVault {
        base: usize,
    },
    Authority,
}

#[derive(Clone, Debug)]
struct Step {
    kind: Kind,
    ok: bool,
    before: View,
}

#[derive(Clone, Copy, Debug)]
struct Burn {
    event: Pubkey,
    base: usize,
    amount: u64,
    max_fee: u64,
    threshold: u32,
}

#[derive(Clone)]
struct Forwarder {
    ctx: TestContext,
    usdc: Pubkey,
    payer: Rc<Keypair>,
    authority: Rc<Keypair>,
    stranger: Pubkey,
    keys: Rc<[Keypair; HOLDERS]>,
    bases: [[u8; 20]; BASES],
    forwarders: [Pubkey; BASES],
    tracked: [Pubkey; TRACKED],
    foreign: Pubkey,
    outside: u128,
    nonce: u64,
    burns: Vec<Burn>,
    last: Option<Step>,
}

fn tmm_pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &TOKEN_MESSENGER_MINTER_ID).0
}

fn forwarder_of(account: &[u8; 20]) -> Pubkey {
    Pubkey::find_program_address(&[FORWARDER_SEED, account], &mozaik_cctp_forwarder::id()).0
}

fn set_token_account(ctx: &mut TestContext, address: Pubkey, token: TokenAccount) {
    let mut data = vec![0u8; TokenAccount::LEN];
    TokenAccount::pack(token, &mut data).expect("pack token account");
    ctx.svm
        .set_account(
            address,
            Account {
                lamports: TOKEN_ACCOUNT_RENT,
                data,
                owner: spl_token::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .expect("set token account");
}

fn token_account(
    mint: Pubkey,
    owner: Pubkey,
    amount: u64,
    delegate: Option<(Pubkey, u64)>,
) -> TokenAccount {
    TokenAccount {
        mint,
        owner,
        amount,
        delegate: delegate.map_or(COption::None, |(key, _)| COption::Some(key)),
        state: AccountState::Initialized,
        is_native: COption::None,
        delegated_amount: delegate.map_or(0, |(_, amount)| amount),
        close_authority: COption::None,
    }
}

#[fuzz_fixture]
impl Forwarder {
    pub fn setup() -> Self {
        let cluster = cluster::selected();
        let mut ctx = TestContext::new();
        ctx.svm = cluster.svm();
        let mut ctx = ctx.with_compute_budget(400_000);

        ctx.add_program_from_bytes(
            &mozaik_cctp_forwarder::id(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/deploy/mozaik_cctp_forwarder.so"
            )),
        )
        .expect("load forwarder");
        ctx.add_program_from_bytes(&TOKEN_MESSENGER_MINTER_ID, cluster.token_messenger_minter)
            .expect("load token messenger minter");
        ctx.add_program_from_bytes(&MESSAGE_TRANSMITTER_ID, cluster.message_transmitter)
            .expect("load message transmitter");

        let usdc = cluster.usdc;
        let payer = Keypair::new_from_array([1; 32]);
        let authority = Keypair::new_from_array([2; 32]);
        let keys: [Keypair; HOLDERS] =
            std::array::from_fn(|i| Keypair::new_from_array([16 + i as u8; 32]));
        for key in keys.iter().chain([&payer, &authority]) {
            ctx.svm
                .airdrop(&key.pubkey(), 100_000_000_000)
                .expect("airdrop");
        }

        let mut mint_account = ctx.svm.get_account(&usdc).expect("usdc mint");
        let mut mint = Mint::unpack(&mint_account.data).expect("usdc mint");
        mint.mint_authority = COption::Some(authority.pubkey());
        mint.freeze_authority = COption::Some(authority.pubkey());
        Mint::pack(mint, &mut mint_account.data).expect("pack usdc mint");
        ctx.svm
            .set_account(usdc, mint_account)
            .expect("override usdc authorities");

        let bases: [[u8; 20]; BASES] = std::array::from_fn(|i| [0xb0 + i as u8; 20]);
        let forwarders = bases.map(|base| forwarder_of(&base));
        let holders = keys.each_ref().map(|key| key.pubkey());
        let tracked: [Pubkey; TRACKED] = std::array::from_fn(|i| {
            get_associated_token_address(
                if i < HOLDERS {
                    &holders[i]
                } else {
                    &forwarders[i - HOLDERS]
                },
                &usdc,
            )
        });

        let initial = [
            (25_000_000, Some((forwarders[0], u64::MAX))),
            (25_000_000, Some((forwarders[1], 30_000_000))),
            (25_000_000, None),
            (100_000_000, None),
            (0, None),
        ];
        for (i, (amount, delegate)) in initial.into_iter().enumerate() {
            set_token_account(
                &mut ctx,
                tracked[i],
                token_account(usdc, holders[i], amount, delegate),
            );
        }

        let foreign_mint = Keypair::new_from_array([3; 32]).pubkey();
        let mut data = vec![0u8; Mint::LEN];
        Mint::pack(
            Mint {
                mint_authority: COption::Some(authority.pubkey()),
                supply: 1_000_000,
                decimals: 6,
                is_initialized: true,
                freeze_authority: COption::None,
            },
            &mut data,
        )
        .expect("pack foreign mint");
        ctx.svm
            .set_account(
                foreign_mint,
                Account {
                    lamports: MINT_RENT,
                    data,
                    owner: spl_token::ID,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .expect("foreign mint");
        let foreign = get_associated_token_address(&holders[0], &foreign_mint);
        set_token_account(
            &mut ctx,
            foreign,
            token_account(
                foreign_mint,
                holders[0],
                1_000_000,
                Some((forwarders[0], u64::MAX)),
            ),
        );

        let mut fixture = Self {
            ctx,
            usdc,
            payer: Rc::new(payer),
            authority: Rc::new(authority),
            stranger: Keypair::new_from_array([4; 32]).pubkey(),
            keys: Rc::new(keys),
            bases,
            forwarders,
            tracked,
            foreign,
            outside: 0,
            nonce: 0,
            burns: Vec::new(),
            last: None,
        };
        let view = fixture.view();
        fixture.outside = u128::from(view.supply) - view.tracked_total();
        fixture
    }

    pub fn action_forward(
        &mut self,
        #[range(0..9)] source: usize,
        #[range(0..3)] base: usize,
        #[range(0..60)] mode: u8,
        #[range(0..60_000_000)] amount: u64,
        #[range(0..200_000)] max_fee: u64,
        #[range(0..4_000)] threshold: u32,
    ) -> bool {
        let before = self.view();
        let source_token = before.source(source);
        let amount = match (mode % 4, source_token) {
            (1, Some(token)) => token.amount,
            (2, Some(token)) => token.delegated_amount,
            (3, _) => u64::MAX,
            _ => amount,
        };
        let pull = source_token.map_or(amount, |token| {
            amount.min(token.amount).min(token.delegated_amount)
        });
        let vault_balance = before.tracked[HOLDERS + base].map_or(0, |token| token.amount);
        let cap = u64::try_from((u128::from(pull) + u128::from(vault_balance)) * 20 / 10_000)
            .unwrap_or(u64::MAX);
        let max_fee = match mode / 4 % 5 {
            0 => 0,
            1 => cap,
            2 => cap.saturating_add(1),
            3 => max_fee,
            _ => u64::MAX,
        };
        let threshold = match mode / 20 {
            0 => FAST,
            1 => STANDARD,
            _ => threshold,
        };

        self.nonce += 1;
        let mut seed = [0x5e; 32];
        seed[..8].copy_from_slice(&self.nonce.to_le_bytes());
        let event = Keypair::new_from_array(seed);

        let forwarder = self.forwarders[base];
        let source_key = if source < TRACKED {
            self.tracked[source]
        } else {
            self.foreign
        };
        let instruction = Instruction {
            program_id: mozaik_cctp_forwarder::id(),
            accounts: mozaik_cctp_forwarder::accounts::Forward {
                payer: self.payer.pubkey(),
                forwarder,
                source: source_key,
                vault: self.tracked[HOLDERS + base],
                mint: self.usdc,
                sender_authority: tmm_pda(&[b"sender_authority"]),
                denylist_account: tmm_pda(&[b"denylist_account", forwarder.as_ref()]),
                message_transmitter: Pubkey::find_program_address(
                    &[b"message_transmitter"],
                    &MESSAGE_TRANSMITTER_ID,
                )
                .0,
                token_messenger: tmm_pda(&[b"token_messenger"]),
                remote_token_messenger: tmm_pda(&[b"remote_token_messenger", b"6"]),
                token_minter: tmm_pda(&[b"token_minter"]),
                local_token: tmm_pda(&[b"local_token", self.usdc.as_ref()]),
                message_sent_event_data: event.pubkey(),
                event_authority: tmm_pda(&[b"__event_authority"]),
                message_transmitter_program: MESSAGE_TRANSMITTER_ID,
                token_messenger_minter_program: TOKEN_MESSENGER_MINTER_ID,
                token_program: spl_token::ID,
                associated_token_program: associated_token::ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: mozaik_cctp_forwarder::instruction::Forward {
                account: self.bases[base],
                amount,
                max_fee,
                min_finality_threshold: threshold,
            }
            .data(),
        };
        let payer = Rc::clone(&self.payer);
        let ok = self.send(instruction, &[&payer, &event]);
        self.record(
            Kind::Forward {
                source,
                base,
                amount,
                max_fee,
                threshold,
                event: event.pubkey(),
            },
            ok,
            before,
        )
    }

    pub fn action_approve(
        &mut self,
        #[range(0..5)] holder: usize,
        #[range(0..3)] base: usize,
        #[range(0..3)] amount_mode: u8,
        #[range(0..60_000_000)] amount: u64,
    ) -> bool {
        let delegate = self.forwarders[base];
        let amount = match amount_mode {
            0 => amount,
            1 => u64::MAX,
            _ => self.view().tracked[holder].map_or(amount, |token| token.amount),
        };
        self.owner_call(holder, |source, owner| {
            token_instruction::approve(&spl_token::ID, source, &delegate, owner, &[], amount)
        })
    }

    pub fn action_approve_stranger(
        &mut self,
        #[range(0..5)] holder: usize,
        #[range(0..60_000_000)] amount: u64,
    ) -> bool {
        let stranger = self.stranger;
        self.owner_call(holder, |source, owner| {
            token_instruction::approve(&spl_token::ID, source, &stranger, owner, &[], amount)
        })
    }

    pub fn action_revoke(&mut self, #[range(0..5)] holder: usize) -> bool {
        self.owner_call(holder, |source, owner| {
            token_instruction::revoke(&spl_token::ID, source, owner, &[])
        })
    }

    pub fn action_set_owner(
        &mut self,
        #[range(0..5)] holder: usize,
        #[range(0..8)] new_owner: usize,
    ) -> bool {
        let new_owner = match self.keys.get(new_owner) {
            Some(key) => key.pubkey(),
            None => self.forwarders[new_owner - HOLDERS],
        };
        self.owner_call(holder, |source, owner| {
            token_instruction::set_authority(
                &spl_token::ID,
                source,
                Some(&new_owner),
                AuthorityType::AccountOwner,
                owner,
                &[],
            )
        })
    }

    pub fn action_transfer(
        &mut self,
        #[range(0..5)] from: usize,
        #[range(0..8)] to: usize,
        #[range(0..60_000_000)] amount: u64,
    ) -> bool {
        let before = self.view();
        let destination = self.tracked[to];
        let ok = self.signed_by_owner(&before, from, |source, owner| {
            token_instruction::transfer(&spl_token::ID, source, &destination, owner, &[], amount)
        });
        self.record(Kind::Transfer { from, to, amount }, ok, before)
    }

    pub fn action_mint_to(
        &mut self,
        #[range(0..8)] to: usize,
        #[range(0..50_000_000)] amount: u64,
    ) -> bool {
        let before = self.view();
        let authority = Rc::clone(&self.authority);
        let ok = token_instruction::mint_to(
            &spl_token::ID,
            &self.usdc,
            &self.tracked[to],
            &authority.pubkey(),
            &[],
            amount,
        )
        .is_ok_and(|instruction| self.send(instruction, &[&authority]));
        self.record(Kind::MintTo { to, amount }, ok, before)
    }

    pub fn action_create_vault(&mut self, #[range(0..3)] base: usize) -> bool {
        let before = self.view();
        let funder = Rc::clone(&self.keys);
        let instruction = create_associated_token_account_idempotent(
            &funder[3].pubkey(),
            &self.forwarders[base],
            &self.usdc,
            &spl_token::ID,
        );
        let ok = self.send(instruction, &[&funder[3]]);
        self.record(Kind::CreateVault { base }, ok, before)
    }

    pub fn action_freeze(&mut self, #[range(0..8)] target: usize) -> bool {
        self.freeze_authority_call(target, token_instruction::freeze_account)
    }

    pub fn action_thaw(&mut self, #[range(0..8)] target: usize) -> bool {
        self.freeze_authority_call(target, token_instruction::thaw_account)
    }
}

type FreezeBuilder = fn(
    &Pubkey,
    &Pubkey,
    &Pubkey,
    &Pubkey,
    &[&Pubkey],
) -> Result<Instruction, anchor_lang::prelude::ProgramError>;

impl Forwarder {
    fn view(&self) -> View {
        View::read(&self.ctx, &self.usdc, &self.tracked, &self.foreign)
    }

    fn send(&mut self, instruction: Instruction, signers: &[&Keypair]) -> bool {
        self.ctx
            .raw_call(instruction)
            .signers(signers)
            .send()
            .is_ok_and(|outcome| outcome.is_success())
    }

    fn record(&mut self, kind: Kind, ok: bool, before: View) -> bool {
        self.last = Some(Step { kind, ok, before });
        ok
    }

    fn signed_by_owner(
        &mut self,
        before: &View,
        holder: usize,
        build: impl FnOnce(&Pubkey, &Pubkey) -> Result<Instruction, anchor_lang::prelude::ProgramError>,
    ) -> bool {
        let keys = Rc::clone(&self.keys);
        let Some(owner) = before.tracked[holder]
            .and_then(|token: Token| keys.iter().find(|key| key.pubkey() == token.owner))
        else {
            return false;
        };
        build(&self.tracked[holder], &owner.pubkey())
            .is_ok_and(|instruction| self.send(instruction, &[owner]))
    }

    fn owner_call(
        &mut self,
        holder: usize,
        build: impl FnOnce(&Pubkey, &Pubkey) -> Result<Instruction, anchor_lang::prelude::ProgramError>,
    ) -> bool {
        let before = self.view();
        let ok = self.signed_by_owner(&before, holder, build);
        self.record(Kind::Authority, ok, before)
    }

    fn freeze_authority_call(&mut self, target: usize, build: FreezeBuilder) -> bool {
        let before = self.view();
        let authority = Rc::clone(&self.authority);
        let ok = build(
            &spl_token::ID,
            &self.tracked[target],
            &self.usdc,
            &authority.pubkey(),
            &[],
        )
        .is_ok_and(|instruction| self.send(instruction, &[&authority]));
        self.record(Kind::Authority, ok, before)
    }
}

#[cfg(feature = "mainnet")]
#[invariant_test]
fn mainnet(fixture: &mut Forwarder) {
    invariants::check(fixture);
}

#[cfg(feature = "devnet")]
#[invariant_test]
fn devnet(fixture: &mut Forwarder) {
    invariants::check(fixture);
}
