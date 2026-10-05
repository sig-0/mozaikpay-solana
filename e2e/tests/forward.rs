use {
    anchor_lang::{
        AccountDeserialize, InstructionData, ToAccountMetas,
        prelude::{ProgramData, Pubkey},
        solana_program::{
            bpf_loader_upgradeable::{self, UpgradeableLoaderState},
            instruction::Instruction,
            program_option::COption,
            program_pack::Pack,
            system_program,
        },
    },
    anchor_spl::{
        associated_token::{
            self, get_associated_token_address,
            spl_associated_token_account::instruction::create_associated_token_account_idempotent,
        },
        token::spl_token::{
            self,
            instruction::{approve_checked, mint_to, revoke},
            state::{Account as TokenAccount, Mint},
        },
    },
    mozaik_cctp_forwarder::{
        FORWARDER_SEED, ForwarderError, MAX_FEE_BPS, MESSAGE_TRANSMITTER_ID,
        token_messenger_minter_v2::ID as TOKEN_MESSENGER_MINTER_ID,
    },
    solana_commitment_config::CommitmentConfig,
    solana_keypair::{Keypair, read_keypair_file},
    solana_rpc_client::{api::client_error::Result as ClientResult, rpc_client::RpcClient},
    solana_signer::Signer,
    solana_transaction::{InstructionError, Signature, Transaction, TransactionError},
    std::{env, fs},
};

const USDC_DECIMALS: u8 = 6;
const RELAYER_LAMPORTS: u64 = 1_000_000_000;
const SIGNATURE_FEE: u64 = 5_000;

const SOLANA_DOMAIN: u32 = 5;
const BASE_DOMAIN: u32 = 6;
const FAST: u32 = 1000;
const STANDARD: u32 = 2000;

const MESSAGE_SENT_RENT_PAYER: usize = 8;
const MESSAGE_SENT_PREFIX: usize = 8 + 32 + 8 + 4;
const HEADER_SOURCE_DOMAIN: usize = 4;
const HEADER_DESTINATION_DOMAIN: usize = 8;
const HEADER_DESTINATION_CALLER: usize = 108;
const HEADER_MIN_FINALITY: usize = 140;
const HEADER_LEN: usize = 148;
const BODY_BURN_TOKEN: usize = 4;
const BODY_MINT_RECIPIENT: usize = 36;
const BODY_AMOUNT: usize = 68;
const BODY_MESSAGE_SENDER: usize = 100;
const BODY_MAX_FEE: usize = 132;
const BODY_LEN_WITHOUT_HOOK: usize = 228;

struct Env {
    client: RpcClient,
    usdc: Pubkey,
    minter: Keypair,
}

struct Deposit {
    relayer: Keypair,
    owner: Keypair,
    account: [u8; 20],
    source: Pubkey,
}

struct BurnMessage {
    rent_payer: Pubkey,
    source_domain: u32,
    destination_domain: u32,
    destination_caller: [u8; 32],
    min_finality_threshold: u32,
    burn_token: Pubkey,
    mint_recipient: [u8; 32],
    amount: u64,
    message_sender: Pubkey,
    max_fee: u64,
    body_len: usize,
}

fn var(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} is not set, run the tests with e2e/run.sh"))
}

fn setup() -> Env {
    Env {
        client: RpcClient::new_with_commitment(var("RPC_URL"), CommitmentConfig::confirmed()),
        usdc: var("USDC_MINT").parse().unwrap(),
        minter: read_keypair_file(var("MINTER_KEYPAIR")).unwrap(),
    }
}

fn forwarder_of(account: &[u8; 20]) -> Pubkey {
    Pubkey::find_program_address(&[FORWARDER_SEED, account], &mozaik_cctp_forwarder::id()).0
}

fn tmm_pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &TOKEN_MESSENGER_MINTER_ID).0
}

fn new_base_account() -> [u8; 20] {
    Keypair::new().pubkey().to_bytes()[..20].try_into().unwrap()
}

fn padded(account: &[u8; 20]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[12..].copy_from_slice(account);
    out
}

fn funded(env: &Env) -> Keypair {
    let keypair = Keypair::new();
    let signature = env
        .client
        .request_airdrop(&keypair.pubkey(), RELAYER_LAMPORTS)
        .unwrap();
    env.client.poll_for_signature(&signature).unwrap();
    keypair
}

fn send(
    env: &Env,
    instructions: &[Instruction],
    payer: &Keypair,
    signers: &[&Keypair],
) -> ClientResult<Signature> {
    let transaction = Transaction::new_signed_with_payer(
        instructions,
        Some(&payer.pubkey()),
        signers,
        env.client.get_latest_blockhash()?,
    );

    env.client.send_and_confirm_transaction(&transaction)
}

fn open_deposit(env: &Env) -> Deposit {
    let relayer = funded(env);
    let owner = Keypair::new();
    let account = new_base_account();
    let source = get_associated_token_address(&owner.pubkey(), &env.usdc);

    send(
        env,
        &[
            create_associated_token_account_idempotent(
                &relayer.pubkey(),
                &owner.pubkey(),
                &env.usdc,
                &spl_token::ID,
            ),
            approve_checked(
                &spl_token::ID,
                &source,
                &env.usdc,
                &forwarder_of(&account),
                &owner.pubkey(),
                &[],
                u64::MAX,
                USDC_DECIMALS,
            )
            .unwrap(),
        ],
        &relayer,
        &[&relayer, &owner],
    )
    .expect("setup transaction");

    Deposit {
        relayer,
        owner,
        account,
        source,
    }
}

fn mint(env: &Env, deposit: &Deposit, amount: u64) {
    send(
        env,
        &[mint_to(
            &spl_token::ID,
            &env.usdc,
            &deposit.source,
            &env.minter.pubkey(),
            &[],
            amount,
        )
        .unwrap()],
        &env.minter,
        &[&env.minter],
    )
    .expect("mint");
}

fn forward(
    env: &Env,
    relayer: &Keypair,
    source: Pubkey,
    account: [u8; 20],
    amount: u64,
    max_fee: u64,
    min_finality_threshold: u32,
) -> (ClientResult<Signature>, Keypair) {
    let event = Keypair::new();
    let forwarder = forwarder_of(&account);

    let instruction = Instruction {
        program_id: mozaik_cctp_forwarder::id(),
        accounts: mozaik_cctp_forwarder::accounts::Forward {
            payer: relayer.pubkey(),
            forwarder,
            source,
            vault: get_associated_token_address(&forwarder, &env.usdc),
            mint: env.usdc,
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
            local_token: tmm_pda(&[b"local_token", env.usdc.as_ref()]),
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
            account,
            amount,
            max_fee,
            min_finality_threshold,
        }
        .data(),
    };

    let result = send(env, &[instruction], relayer, &[relayer, &event]);
    (result, event)
}

fn token_account(env: &Env, address: &Pubkey) -> Option<TokenAccount> {
    env.client
        .get_account_with_commitment(address, env.client.commitment())
        .unwrap()
        .value
        .map(|account| TokenAccount::unpack(&account.data).unwrap())
}

fn supply(env: &Env) -> u64 {
    Mint::unpack(&env.client.get_account_data(&env.usdc).unwrap())
        .unwrap()
        .supply
}

fn burn_message(env: &Env, event: &Pubkey) -> BurnMessage {
    let account = env.client.get_account(event).expect("event account");
    assert_eq!(account.owner, MESSAGE_TRANSMITTER_ID);

    let data = account.data.as_slice();
    let header = &data[MESSAGE_SENT_PREFIX..];
    let body = &header[HEADER_LEN..];
    let u32_at = |b: &[u8], at: usize| u32::from_be_bytes(b[at..at + 4].try_into().unwrap());
    let bytes32_at = |b: &[u8], at: usize| -> [u8; 32] { b[at..at + 32].try_into().unwrap() };
    let u256_low = |b: &[u8], at: usize| {
        assert!(b[at..at + 24].iter().all(|byte| *byte == 0));
        u64::from_be_bytes(b[at + 24..at + 32].try_into().unwrap())
    };

    BurnMessage {
        rent_payer: Pubkey::new_from_array(bytes32_at(data, MESSAGE_SENT_RENT_PAYER)),
        source_domain: u32_at(header, HEADER_SOURCE_DOMAIN),
        destination_domain: u32_at(header, HEADER_DESTINATION_DOMAIN),
        destination_caller: bytes32_at(header, HEADER_DESTINATION_CALLER),
        min_finality_threshold: u32_at(header, HEADER_MIN_FINALITY),
        burn_token: Pubkey::new_from_array(bytes32_at(body, BODY_BURN_TOKEN)),
        mint_recipient: bytes32_at(body, BODY_MINT_RECIPIENT),
        amount: u256_low(body, BODY_AMOUNT),
        message_sender: Pubkey::new_from_array(bytes32_at(body, BODY_MESSAGE_SENDER)),
        max_fee: u256_low(body, BODY_MAX_FEE),
        body_len: body.len(),
    }
}

fn assert_not_delegated(result: ClientResult<Signature>) {
    let error = result.expect_err("forward must fail");
    assert_eq!(
        error.get_transaction_error(),
        Some(TransactionError::InstructionError(
            0,
            InstructionError::Custom(ForwarderError::NotDelegated.into()),
        )),
        "{error}",
    );
}

#[test]
fn deployment_is_live() {
    let env = setup();
    let program_id = mozaik_cctp_forwarder::id();

    let program = env.client.get_account(&program_id).unwrap();
    assert!(program.executable);
    assert_eq!(program.owner, bpf_loader_upgradeable::ID);

    let data = env
        .client
        .get_account_data(&bpf_loader_upgradeable::get_program_data_address(
            &program_id,
        ))
        .unwrap();
    let program_data = ProgramData::try_deserialize(&mut data.as_slice()).unwrap();
    assert!(program_data.slot > 0);
    assert!(program_data.upgrade_authority_address.is_some());

    let so = fs::read(var("PROGRAM_SO")).unwrap();
    let (code, padding) =
        data[UpgradeableLoaderState::size_of_programdata_metadata()..].split_at(so.len());
    assert_eq!(code, so);
    assert!(padding.iter().all(|byte| *byte == 0));
}

#[test]
fn forwards_deposit_to_base_account() {
    let env = setup();
    let deposit = open_deposit(&env);
    let forwarder = forwarder_of(&deposit.account);

    let source = token_account(&env, &deposit.source).unwrap();
    assert_eq!(source.mint, env.usdc);
    assert_eq!(source.owner, deposit.owner.pubkey());
    assert_eq!(source.amount, 0);
    assert_eq!(source.delegate, COption::Some(forwarder));
    assert_eq!(source.delegated_amount, u64::MAX);

    let amount = 10_000_000;
    mint(&env, &deposit, amount);
    assert_eq!(token_account(&env, &deposit.source).unwrap().amount, amount);

    let relayer = funded(&env);
    let relayer_before = env.client.get_balance(&relayer.pubkey()).unwrap();
    let supply_before = supply(&env);

    let (result, event) = forward(
        &env,
        &relayer,
        deposit.source,
        deposit.account,
        amount,
        0,
        STANDARD,
    );
    result.expect("forward");

    let message = burn_message(&env, &event.pubkey());
    assert_eq!(message.rent_payer, relayer.pubkey());
    assert_eq!(message.source_domain, SOLANA_DOMAIN);
    assert_eq!(message.destination_domain, BASE_DOMAIN);
    assert_eq!(message.destination_caller, [0u8; 32]);
    assert_eq!(message.min_finality_threshold, STANDARD);
    assert_eq!(message.burn_token, env.usdc);
    assert_eq!(message.mint_recipient, padded(&deposit.account));
    assert_eq!(message.amount, amount);
    assert_eq!(message.message_sender, forwarder);
    assert_eq!(message.max_fee, 0);
    assert_eq!(message.body_len, BODY_LEN_WITHOUT_HOOK);

    assert_eq!(supply(&env), supply_before - amount);

    let source = token_account(&env, &deposit.source).unwrap();
    assert_eq!(source.amount, 0);
    assert_eq!(source.delegate, COption::Some(forwarder));
    assert_eq!(source.delegated_amount, u64::MAX - amount);

    let vault = get_associated_token_address(&forwarder, &env.usdc);
    assert!(token_account(&env, &vault).is_none());

    let event_account = env.client.get_account(&event.pubkey()).unwrap();
    assert_eq!(
        event_account.lamports,
        env.client
            .get_minimum_balance_for_rent_exemption(event_account.data.len())
            .unwrap()
    );

    let relayer_after = env.client.get_balance(&relayer.pubkey()).unwrap();
    assert_eq!(
        relayer_before - relayer_after,
        2 * SIGNATURE_FEE + event_account.lamports
    );
}

#[test]
fn forwards_later_deposits() {
    let env = setup();
    let deposit = open_deposit(&env);
    let first = 4_000_000;
    let second = 6_000_000;

    mint(&env, &deposit, first);
    let (result, _) = forward(
        &env,
        &deposit.relayer,
        deposit.source,
        deposit.account,
        first,
        0,
        STANDARD,
    );
    result.expect("first forward");

    mint(&env, &deposit, second);
    let max_fee = second * MAX_FEE_BPS / 10_000;
    let (result, event) = forward(
        &env,
        &deposit.relayer,
        deposit.source,
        deposit.account,
        second,
        max_fee,
        FAST,
    );
    result.expect("second forward");

    let message = burn_message(&env, &event.pubkey());
    assert_eq!(message.mint_recipient, padded(&deposit.account));
    assert_eq!(message.amount, second);
    assert_eq!(message.max_fee, max_fee);
    assert_eq!(message.min_finality_threshold, FAST);

    let source = token_account(&env, &deposit.source).unwrap();
    assert_eq!(source.amount, 0);
    assert_eq!(source.delegated_amount, u64::MAX - first - second);
}

#[test]
fn rejects_other_base_account() {
    let env = setup();
    let deposit = open_deposit(&env);
    mint(&env, &deposit, 1_000_000);

    let (result, _) = forward(
        &env,
        &deposit.relayer,
        deposit.source,
        new_base_account(),
        1_000_000,
        0,
        STANDARD,
    );

    assert_not_delegated(result);
    assert_eq!(
        token_account(&env, &deposit.source).unwrap().amount,
        1_000_000
    );
}

#[test]
fn rejects_revoked_delegate() {
    let env = setup();
    let deposit = open_deposit(&env);
    mint(&env, &deposit, 1_000_000);

    send(
        &env,
        &[revoke(
            &spl_token::ID,
            &deposit.source,
            &deposit.owner.pubkey(),
            &[],
        )
        .unwrap()],
        &deposit.relayer,
        &[&deposit.relayer, &deposit.owner],
    )
    .expect("revoke");

    let (result, _) = forward(
        &env,
        &deposit.relayer,
        deposit.source,
        deposit.account,
        1_000_000,
        0,
        STANDARD,
    );

    assert_not_delegated(result);
    assert_eq!(
        token_account(&env, &deposit.source).unwrap().amount,
        1_000_000
    );
}
