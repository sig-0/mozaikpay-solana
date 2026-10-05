use {
    agave_feature_set::FeatureSet,
    anchor_lang::{
        InstructionData, ToAccountMetas,
        prelude::Pubkey,
        solana_program::{
            instruction::Instruction, program_option::COption, program_pack::Pack, system_program,
        },
    },
    anchor_spl::{
        associated_token::{self, get_associated_token_address},
        token::spl_token::{
            self,
            state::{Account as TokenAccount, AccountState, Mint},
        },
    },
    base64::{Engine, engine::general_purpose::STANDARD},
    litesvm::{
        LiteSVM,
        types::{FailedTransactionMetadata, TransactionMetadata},
    },
    mozaik_cctp_forwarder::{
        FORWARDER_SEED, MESSAGE_TRANSMITTER_ID,
        token_messenger_minter_v2::ID as TOKEN_MESSENGER_MINTER_ID,
    },
    solana_account::Account,
    solana_keypair::Keypair,
    solana_message::Message,
    solana_signer::Signer,
    solana_transaction::Transaction,
};

const PROGRAM_ID: Pubkey = anchor_lang::pubkey!("MzkicG4ev5reLESn2RZvPVufghwgwQhRqvhNqVr8oNb");
const PROGRAM_SO: &[u8] = include_bytes!(concat!(
    env!("CARGO_TARGET_TMPDIR"),
    "/../deploy/mozaik_cctp_forwarder.so"
));
const COMPUTE_BUDGET: Pubkey = anchor_lang::pubkey!("ComputeBudget111111111111111111111111111111");
const TOKEN_ACCOUNT_RENT: u64 = 2_039_280;
const SOURCE_BALANCE: u64 = 25_000_000;
const SIGNATURE_FEE: u64 = 5_000;
const BASE_COMPUTE_UNITS: u64 = 90_000;
const PDA_STEP_COMPUTE_UNITS: u64 = 1_500;

const SOLANA_DOMAIN: u32 = 5;
const BASE_DOMAIN: u32 = 6;
const FAST: u32 = 1000;
const STANDARD_THRESHOLD: u32 = 2000;

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

type Outcome = Result<TransactionMetadata, Box<FailedTransactionMetadata>>;

struct Cluster {
    usdc: Pubkey,
    token_messenger_minter: &'static [u8],
    message_transmitter: &'static [u8],
    accounts: [&'static str; 6],
    features: &'static str,
}

macro_rules! cluster {
    ($dir:literal, $usdc:literal) => {
        Cluster {
            usdc: anchor_lang::pubkey!($usdc),
            token_messenger_minter: include_bytes!(concat!(
                "fixtures/",
                $dir,
                "/token_messenger_minter_v2.so"
            )),
            message_transmitter: include_bytes!(concat!(
                "fixtures/",
                $dir,
                "/message_transmitter_v2.so"
            )),
            accounts: [
                include_str!(concat!("fixtures/", $dir, "/token_messenger.json")),
                include_str!(concat!("fixtures/", $dir, "/token_minter.json")),
                include_str!(concat!(
                    "fixtures/",
                    $dir,
                    "/remote_token_messenger_base.json"
                )),
                include_str!(concat!("fixtures/", $dir, "/local_token_usdc.json")),
                include_str!(concat!("fixtures/", $dir, "/message_transmitter.json")),
                include_str!(concat!("fixtures/", $dir, "/usdc_mint.json")),
            ],
            features: include_str!(concat!("fixtures/", $dir, "/features.json")),
        }
    };
}

fn devnet() -> Cluster {
    cluster!("devnet", "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU")
}

fn mainnet() -> Cluster {
    cluster!("mainnet", "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")
}

macro_rules! cluster_tests {
    ($($name:ident),* $(,)?) => {
        mod devnet {
            $(#[test]
            fn $name() {
                super::$name(&super::devnet());
            })*
        }

        mod mainnet {
            $(#[test]
            fn $name() {
                super::$name(&super::mainnet());
            })*
        }
    };
}

cluster_tests!(
    burns_to_base_account,
    burns_whole_vault_balance,
    caps_fee_on_vault_balance,
    burns_at_fast_threshold,
    burns_at_fast_threshold_without_fee,
    accepts_fee_at_cap,
    lowers_fee_above_cap,
    lowers_maximum_fee,
    caps_amount_at_balance,
    caps_amount_at_delegated_amount,
    forwards_rest_after_earlier_forward,
    rejects_zero_amount,
    rejects_empty_source,
    rejects_exhausted_allowance,
    rejects_unknown_finality_threshold,
    rejects_frozen_source,
    rejects_source_without_delegate,
    rejects_source_of_other_mint,
    rejects_vault_as_source,
    rejects_other_base_account,
    rejects_unsigned_payer,
    rejects_reused_event_account,
    rejects_substituted_token_messenger_program,
    rejects_substituted_message_transmitter_program,
    rejects_denylisted_forwarder,
    rejects_fake_event_authority,
    rejects_other_domain_remote_token_messenger,
);

struct Env {
    svm: LiteSVM,
    usdc: Pubkey,
    payer: Keypair,
    account: [u8; 20],
    forwarder: Pubkey,
    source: Pubkey,
}

#[test]
fn embeds_sbpf_v3_program() {
    let machine = u16::from_le_bytes(PROGRAM_SO[18..20].try_into().unwrap());
    let flags = u32::from_le_bytes(PROGRAM_SO[48..52].try_into().unwrap());

    assert_eq!((machine, flags), (247, 3));
}

#[test]
fn derives_golden_forwarder() {
    let golden: [u8; 20] = [
        0x28, 0x7b, 0x02, 0xe0, 0x9a, 0x22, 0x0f, 0x91, 0x1f, 0x5b, 0xba, 0xaa, 0x3f, 0x1c, 0x9d,
        0x17, 0x0c, 0x1b, 0x9a, 0x08,
    ];

    assert_eq!(
        Pubkey::find_program_address(&[FORWARDER_SEED, &golden], &PROGRAM_ID),
        (
            anchor_lang::pubkey!("D1MkPZmqZFNRcCqJjqssMQ2MvPKMDwohKWjhRrFf2aDM"),
            255
        )
    );
}

fn forwarder_of(account: &[u8; 20]) -> Pubkey {
    Pubkey::find_program_address(&[FORWARDER_SEED, account], &PROGRAM_ID).0
}

fn pda_steps(seeds: &[&[u8]], program_id: &Pubkey) -> u64 {
    255 - u64::from(Pubkey::find_program_address(seeds, program_id).1)
}

fn compute_bound(env: &Env) -> u64 {
    let forwarder = pda_steps(&[FORWARDER_SEED, &env.account], &PROGRAM_ID);
    let vault = pda_steps(
        &[
            env.forwarder.as_ref(),
            spl_token::ID.as_ref(),
            env.usdc.as_ref(),
        ],
        &associated_token::ID,
    );
    let denylist = pda_steps(
        &[b"denylist_account", env.forwarder.as_ref()],
        &TOKEN_MESSENGER_MINTER_ID,
    );

    BASE_COMPUTE_UNITS + PDA_STEP_COMPUTE_UNITS * (forwarder + 3 * vault + denylist)
}

fn tmm_pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &TOKEN_MESSENGER_MINTER_ID).0
}

fn feature_set(json: &str) -> FeatureSet {
    let ids: Vec<String> = serde_json::from_str(json).unwrap();
    let mut features = FeatureSet::default();

    for id in ids {
        features.activate(&id.parse().unwrap(), 0);
    }

    features
}

fn load_account(svm: &mut LiteSVM, json: &str) {
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let account = &value["account"];

    svm.set_account(
        value["pubkey"].as_str().unwrap().parse().unwrap(),
        Account {
            lamports: account["lamports"].as_u64().unwrap(),
            data: STANDARD
                .decode(account["data"][0].as_str().unwrap())
                .unwrap(),
            owner: account["owner"].as_str().unwrap().parse().unwrap(),
            executable: account["executable"].as_bool().unwrap(),
            rent_epoch: 0,
        },
    )
    .unwrap();
}

fn set_token_account(
    svm: &mut LiteSVM,
    address: Pubkey,
    mint: Pubkey,
    owner: Pubkey,
    amount: u64,
    delegate: COption<Pubkey>,
) {
    let mut data = vec![0u8; TokenAccount::LEN];
    TokenAccount::pack(
        TokenAccount {
            mint,
            owner,
            amount,
            delegate,
            state: AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: if delegate.is_some() { u64::MAX } else { 0 },
            close_authority: COption::None,
        },
        &mut data,
    )
    .unwrap();

    svm.set_account(
        address,
        Account {
            lamports: TOKEN_ACCOUNT_RENT,
            data,
            owner: spl_token::ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

fn update_token_account(
    svm: &mut LiteSVM,
    address: Pubkey,
    update: impl FnOnce(&mut TokenAccount),
) {
    let mut account = svm.get_account(&address).unwrap();
    let mut state = TokenAccount::unpack(&account.data).unwrap();
    update(&mut state);
    TokenAccount::pack(state, &mut account.data).unwrap();
    svm.set_account(address, account).unwrap();
}

fn token_account(svm: &LiteSVM, address: &Pubkey) -> Option<TokenAccount> {
    svm.get_account(address)
        .filter(|a| a.owner == spl_token::ID && !a.data.is_empty())
        .map(|a| TokenAccount::unpack(&a.data).unwrap())
}

fn supply(svm: &LiteSVM, mint: &Pubkey) -> u64 {
    Mint::unpack(&svm.get_account(mint).unwrap().data)
        .unwrap()
        .supply
}

fn setup(cluster: &Cluster) -> Env {
    let mut svm = LiteSVM::default()
        .with_feature_set(feature_set(cluster.features))
        .with_builtins()
        .with_lamports(1_000_000_000_000_000)
        .with_sysvars()
        .with_default_programs()
        .with_sigverify(true)
        .with_blockhash_check(true);

    svm.add_program(PROGRAM_ID, PROGRAM_SO).unwrap();
    svm.add_program(TOKEN_MESSENGER_MINTER_ID, cluster.token_messenger_minter)
        .unwrap();
    svm.add_program(MESSAGE_TRANSMITTER_ID, cluster.message_transmitter)
        .unwrap();

    for account in cluster.accounts {
        load_account(&mut svm, account);
    }

    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();

    let account: [u8; 20] = Keypair::new().pubkey().to_bytes()[..20].try_into().unwrap();
    let forwarder = forwarder_of(&account);
    let owner = Keypair::new().pubkey();
    let source = get_associated_token_address(&owner, &cluster.usdc);

    set_token_account(
        &mut svm,
        source,
        cluster.usdc,
        owner,
        SOURCE_BALANCE,
        COption::Some(forwarder),
    );

    Env {
        svm,
        usdc: cluster.usdc,
        payer,
        account,
        forwarder,
        source,
    }
}

fn vault_of(env: &Env) -> Pubkey {
    get_associated_token_address(&env.forwarder, &env.usdc)
}

fn forward_instruction(
    env: &Env,
    account: [u8; 20],
    amount: u64,
    max_fee: u64,
    min_finality_threshold: u32,
    event: &Keypair,
) -> Instruction {
    let forwarder = forwarder_of(&account);

    Instruction {
        program_id: PROGRAM_ID,
        accounts: mozaik_cctp_forwarder::accounts::Forward {
            payer: env.payer.pubkey(),
            forwarder,
            source: env.source,
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
    }
}

fn substitute(instruction: &mut Instruction, from: Pubkey, to: Pubkey) {
    instruction
        .accounts
        .iter_mut()
        .find(|meta| meta.pubkey == from)
        .unwrap()
        .pubkey = to;
}

fn send(env: &mut Env, instruction: Instruction, event: &Keypair) -> Outcome {
    let mut limit = vec![2u8];
    limit.extend_from_slice(&400_000u32.to_le_bytes());
    let compute = Instruction::new_with_bytes(COMPUTE_BUDGET, &limit, vec![]);

    let message = Message::new(&[compute, instruction], Some(&env.payer.pubkey()));
    let tx = Transaction::new(&[&env.payer, event], message, env.svm.latest_blockhash());

    env.svm.send_transaction(tx).map_err(Box::new)
}

fn forward(
    env: &mut Env,
    amount: u64,
    max_fee: u64,
    min_finality_threshold: u32,
) -> (Outcome, Keypair) {
    let event = Keypair::new();
    let instruction = forward_instruction(
        env,
        env.account,
        amount,
        max_fee,
        min_finality_threshold,
        &event,
    );

    (send(env, instruction, &event), event)
}

fn failure_logs(result: Outcome) -> Vec<String> {
    result.expect_err("transaction must fail").meta.logs
}

fn circle_invoked(logs: &[String]) -> bool {
    let invoke = format!("Program {TOKEN_MESSENGER_MINTER_ID} invoke");
    logs.iter().any(|line| line.starts_with(&invoke))
}

fn assert_logged(logs: &[String], needle: &str) {
    assert!(
        logs.iter().any(|line| line.contains(needle)),
        "missing {needle} in logs: {logs:#?}",
    );
}

fn assert_forwarder_error(result: Outcome, code: &str) {
    let logs = failure_logs(result);
    assert_logged(&logs, &format!("Error Code: {code}."));
    assert!(
        !circle_invoked(&logs),
        "Circle ran before {code}: {logs:#?}"
    );
}

fn assert_circle_error(result: Outcome, code: &str) {
    let logs = failure_logs(result);
    assert_logged(&logs, &format!("Error Code: {code}."));
    assert!(circle_invoked(&logs), "Circle did not run: {logs:#?}");
}

struct BurnMessage {
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

fn burn_message(svm: &LiteSVM, event: &Pubkey) -> BurnMessage {
    let account = svm.get_account(event).expect("event account");
    assert_eq!(account.owner, MESSAGE_TRANSMITTER_ID);

    let header = &account.data[MESSAGE_SENT_PREFIX..];
    let body = &header[HEADER_LEN..];
    let u32_at = |b: &[u8], at: usize| u32::from_be_bytes(b[at..at + 4].try_into().unwrap());
    let bytes32_at = |b: &[u8], at: usize| -> [u8; 32] { b[at..at + 32].try_into().unwrap() };
    let u256_low = |b: &[u8], at: usize| {
        assert!(b[at..at + 24].iter().all(|byte| *byte == 0));
        u64::from_be_bytes(b[at + 24..at + 32].try_into().unwrap())
    };

    BurnMessage {
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

fn padded(account: &[u8; 20]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[12..].copy_from_slice(account);
    out
}

fn burns_to_base_account(cluster: &Cluster) {
    let mut env = setup(cluster);
    let payer_before = env.svm.get_balance(&env.payer.pubkey()).unwrap();
    let supply_before = supply(&env.svm, &env.usdc);
    let amount = 10_000_000;

    let (result, event) = forward(&mut env, amount, 0, STANDARD_THRESHOLD);
    let meta = result.expect("forward");
    assert!(meta.compute_units_consumed < compute_bound(&env));

    let message = burn_message(&env.svm, &event.pubkey());
    assert_eq!(message.source_domain, SOLANA_DOMAIN);
    assert_eq!(message.destination_domain, BASE_DOMAIN);
    assert_eq!(message.destination_caller, [0u8; 32]);
    assert_eq!(message.min_finality_threshold, STANDARD_THRESHOLD);
    assert_eq!(message.burn_token, env.usdc);
    assert_eq!(message.mint_recipient, padded(&env.account));
    assert_eq!(message.amount, amount);
    assert_eq!(message.message_sender, env.forwarder);
    assert_eq!(message.max_fee, 0);
    assert_eq!(message.body_len, BODY_LEN_WITHOUT_HOOK);

    assert_eq!(supply(&env.svm, &env.usdc), supply_before - amount);

    let source = token_account(&env.svm, &env.source).unwrap();
    assert_eq!(source.amount, SOURCE_BALANCE - amount);
    assert_eq!(source.delegate, COption::Some(env.forwarder));
    assert_eq!(source.delegated_amount, u64::MAX - amount);

    assert!(token_account(&env.svm, &vault_of(&env)).is_none());

    let event_rent = env.svm.get_balance(&event.pubkey()).unwrap();
    let payer_after = env.svm.get_balance(&env.payer.pubkey()).unwrap();
    assert_eq!(payer_before - payer_after, 2 * SIGNATURE_FEE + event_rent);
}

fn burns_whole_vault_balance(cluster: &Cluster) {
    let mut env = setup(cluster);
    let stray = 3_000_000;
    let vault = vault_of(&env);
    set_token_account(
        &mut env.svm,
        vault,
        env.usdc,
        env.forwarder,
        stray,
        COption::None,
    );

    let (result, event) = forward(&mut env, 1_000_000, 0, STANDARD_THRESHOLD);
    result.expect("forward");

    assert_eq!(
        burn_message(&env.svm, &event.pubkey()).amount,
        1_000_000 + stray
    );
    assert!(token_account(&env.svm, &vault).is_none());
}

fn caps_fee_on_vault_balance(cluster: &Cluster) {
    let mut env = setup(cluster);
    let vault = vault_of(&env);
    set_token_account(
        &mut env.svm,
        vault,
        env.usdc,
        env.forwarder,
        3_000_000,
        COption::None,
    );

    let (result, event) = forward(&mut env, 1_000_000, 8_000, STANDARD_THRESHOLD);
    result.expect("forward");

    let message = burn_message(&env.svm, &event.pubkey());
    assert_eq!(message.amount, 4_000_000);
    assert_eq!(message.max_fee, 8_000);
}

fn burns_at_fast_threshold(cluster: &Cluster) {
    let mut env = setup(cluster);
    let amount = 5_000_000;
    let max_fee = amount * 20 / 10_000;

    let (result, event) = forward(&mut env, amount, max_fee, FAST);
    result.expect("forward");

    let message = burn_message(&env.svm, &event.pubkey());
    assert_eq!(message.min_finality_threshold, FAST);
    assert_eq!(message.max_fee, max_fee);
}

fn burns_at_fast_threshold_without_fee(cluster: &Cluster) {
    let mut env = setup(cluster);

    let (result, event) = forward(&mut env, 5_000_000, 0, FAST);
    result.expect("forward");

    let message = burn_message(&env.svm, &event.pubkey());
    assert_eq!(message.min_finality_threshold, FAST);
    assert_eq!(message.max_fee, 0);
}

fn accepts_fee_at_cap(cluster: &Cluster) {
    let mut env = setup(cluster);
    let amount = 1_000_000;
    let cap = amount * 20 / 10_000;

    let (result, event) = forward(&mut env, amount, cap, STANDARD_THRESHOLD);
    result.expect("forward");

    assert_eq!(burn_message(&env.svm, &event.pubkey()).max_fee, cap);
}

fn lowers_fee_above_cap(cluster: &Cluster) {
    let mut env = setup(cluster);
    let amount = 1_000_000;
    let cap = amount * 20 / 10_000;

    let (result, event) = forward(&mut env, amount, cap + 1, STANDARD_THRESHOLD);
    result.expect("forward");

    assert_eq!(burn_message(&env.svm, &event.pubkey()).max_fee, cap);
}

fn lowers_maximum_fee(cluster: &Cluster) {
    let mut env = setup(cluster);

    let (result, event) = forward(&mut env, 1_234_567, u64::MAX, STANDARD_THRESHOLD);
    result.expect("forward");

    assert_eq!(burn_message(&env.svm, &event.pubkey()).max_fee, 2_469);
}

fn rejects_zero_amount(cluster: &Cluster) {
    let mut env = setup(cluster);

    let (result, _) = forward(&mut env, 0, 0, STANDARD_THRESHOLD);

    assert_forwarder_error(result, "NothingToForward");
}

fn rejects_unknown_finality_threshold(cluster: &Cluster) {
    let mut env = setup(cluster);

    let (result, _) = forward(&mut env, 1_000_000, 0, 1_500);

    assert_forwarder_error(result, "UnsupportedFinalityThreshold");
}

fn caps_amount_at_balance(cluster: &Cluster) {
    let mut env = setup(cluster);

    let (result, event) = forward(&mut env, u64::MAX, 0, STANDARD_THRESHOLD);
    result.expect("forward");

    assert_eq!(
        burn_message(&env.svm, &event.pubkey()).amount,
        SOURCE_BALANCE
    );
    let source = token_account(&env.svm, &env.source).unwrap();
    assert_eq!(source.amount, 0);
    assert_eq!(source.delegated_amount, u64::MAX - SOURCE_BALANCE);
}

fn caps_amount_at_delegated_amount(cluster: &Cluster) {
    let mut env = setup(cluster);
    let source = env.source;
    update_token_account(&mut env.svm, source, |state| {
        state.delegated_amount = 999_999;
    });

    let (result, event) = forward(&mut env, 1_000_000, 0, STANDARD_THRESHOLD);
    result.expect("forward");

    assert_eq!(burn_message(&env.svm, &event.pubkey()).amount, 999_999);
    let source = token_account(&env.svm, &env.source).unwrap();
    assert_eq!(source.amount, SOURCE_BALANCE - 999_999);
    assert_eq!(source.delegate, COption::None);
    assert_eq!(source.delegated_amount, 0);
}

fn forwards_rest_after_earlier_forward(cluster: &Cluster) {
    let mut env = setup(cluster);
    let amount = 20_000_000;

    let (first, _) = forward(&mut env, amount, 0, STANDARD_THRESHOLD);
    first.expect("first forward");
    let (second, event) = forward(&mut env, amount, 0, STANDARD_THRESHOLD);
    second.expect("second forward");

    assert_eq!(
        burn_message(&env.svm, &event.pubkey()).amount,
        SOURCE_BALANCE - amount
    );
    assert_eq!(token_account(&env.svm, &env.source).unwrap().amount, 0);
}

fn rejects_empty_source(cluster: &Cluster) {
    let mut env = setup(cluster);
    let source = env.source;
    update_token_account(&mut env.svm, source, |state| {
        state.amount = 0;
    });

    let (result, _) = forward(&mut env, 1_000_000, 0, STANDARD_THRESHOLD);

    assert_forwarder_error(result, "NothingToForward");
}

fn rejects_exhausted_allowance(cluster: &Cluster) {
    let mut env = setup(cluster);
    let source = env.source;
    update_token_account(&mut env.svm, source, |state| {
        state.delegated_amount = 0;
    });

    let (result, _) = forward(&mut env, 1_000_000, 0, STANDARD_THRESHOLD);

    assert_forwarder_error(result, "NothingToForward");
    assert_eq!(
        token_account(&env.svm, &env.source).unwrap().amount,
        SOURCE_BALANCE
    );
}

fn rejects_frozen_source(cluster: &Cluster) {
    let mut env = setup(cluster);
    let source = env.source;
    update_token_account(&mut env.svm, source, |state| {
        state.state = AccountState::Frozen;
    });

    let (result, _) = forward(&mut env, 1_000_000, 0, STANDARD_THRESHOLD);

    let logs = failure_logs(result);
    assert_logged(&logs, "Account is frozen");
    assert!(!circle_invoked(&logs));
    assert_eq!(
        token_account(&env.svm, &env.source).unwrap().amount,
        SOURCE_BALANCE
    );
}

fn rejects_source_without_delegate(cluster: &Cluster) {
    let mut env = setup(cluster);
    let source = env.source;
    update_token_account(&mut env.svm, source, |state| {
        state.delegate = COption::None;
        state.delegated_amount = 0;
    });

    let (result, _) = forward(&mut env, 1_000_000, 0, STANDARD_THRESHOLD);

    assert_forwarder_error(result, "NotDelegated");
}

fn rejects_source_of_other_mint(cluster: &Cluster) {
    let mut env = setup(cluster);
    let other_mint = Keypair::new().pubkey();
    let owner = Keypair::new().pubkey();
    let source = get_associated_token_address(&owner, &other_mint);
    set_token_account(
        &mut env.svm,
        source,
        other_mint,
        owner,
        SOURCE_BALANCE,
        COption::Some(env.forwarder),
    );
    env.source = source;

    let (result, _) = forward(&mut env, 1_000_000, 0, STANDARD_THRESHOLD);

    assert_forwarder_error(result, "ConstraintTokenMint");
}

fn rejects_vault_as_source(cluster: &Cluster) {
    let mut env = setup(cluster);
    let vault = vault_of(&env);
    set_token_account(
        &mut env.svm,
        vault,
        env.usdc,
        env.forwarder,
        3_000_000,
        COption::Some(env.forwarder),
    );
    env.source = vault;

    let (result, _) = forward(&mut env, 1_000_000, 0, STANDARD_THRESHOLD);

    assert_forwarder_error(result, "ConstraintDuplicateMutableAccount");
}

fn rejects_other_base_account(cluster: &Cluster) {
    let mut env = setup(cluster);
    let other: [u8; 20] = Keypair::new().pubkey().to_bytes()[..20].try_into().unwrap();
    let event = Keypair::new();
    let instruction = forward_instruction(&env, other, 1_000_000, 0, STANDARD_THRESHOLD, &event);

    let result = send(&mut env, instruction, &event);

    assert_forwarder_error(result, "NotDelegated");
}

fn rejects_unsigned_payer(cluster: &Cluster) {
    let mut env = setup(cluster);
    let event = Keypair::new();
    let mut instruction =
        forward_instruction(&env, env.account, 1_000_000, 0, STANDARD_THRESHOLD, &event);
    let other = Keypair::new().pubkey();
    env.svm.airdrop(&other, 1_000_000_000).unwrap();
    let payer = instruction
        .accounts
        .iter_mut()
        .find(|meta| meta.pubkey == env.payer.pubkey())
        .unwrap();
    payer.pubkey = other;
    payer.is_signer = false;

    let result = send(&mut env, instruction, &event);

    assert_forwarder_error(result, "AccountNotSigner");
}

fn rejects_reused_event_account(cluster: &Cluster) {
    let mut env = setup(cluster);
    let (result, event) = forward(&mut env, 1_000_000, 0, STANDARD_THRESHOLD);
    result.expect("first forward");
    env.svm.expire_blockhash();

    let instruction =
        forward_instruction(&env, env.account, 1_000_000, 0, STANDARD_THRESHOLD, &event);
    let result = send(&mut env, instruction, &event);

    let logs = failure_logs(result);
    assert_logged(&logs, "already in use");
    assert_eq!(
        token_account(&env.svm, &env.source).unwrap().amount,
        SOURCE_BALANCE - 1_000_000
    );
}

fn rejects_substituted_token_messenger_program(cluster: &Cluster) {
    let mut env = setup(cluster);
    let event = Keypair::new();
    let mut instruction =
        forward_instruction(&env, env.account, 1_000_000, 0, STANDARD_THRESHOLD, &event);
    substitute(
        &mut instruction,
        TOKEN_MESSENGER_MINTER_ID,
        Keypair::new().pubkey(),
    );

    let result = send(&mut env, instruction, &event);

    assert_forwarder_error(result, "ConstraintAddress");
}

fn rejects_substituted_message_transmitter_program(cluster: &Cluster) {
    let mut env = setup(cluster);
    let event = Keypair::new();
    let mut instruction =
        forward_instruction(&env, env.account, 1_000_000, 0, STANDARD_THRESHOLD, &event);
    substitute(
        &mut instruction,
        MESSAGE_TRANSMITTER_ID,
        Keypair::new().pubkey(),
    );

    let result = send(&mut env, instruction, &event);

    assert_forwarder_error(result, "ConstraintAddress");
}

fn rejects_denylisted_forwarder(cluster: &Cluster) {
    let mut env = setup(cluster);
    env.svm
        .set_account(
            tmm_pda(&[b"denylist_account", env.forwarder.as_ref()]),
            Account {
                lamports: 1_000_000,
                data: vec![0u8; 8],
                owner: TOKEN_MESSENGER_MINTER_ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();

    let (result, _) = forward(&mut env, 1_000_000, 0, STANDARD_THRESHOLD);

    assert_circle_error(result, "DenylistedAccount");
    assert_eq!(
        token_account(&env.svm, &env.source).unwrap().amount,
        SOURCE_BALANCE
    );
}

fn rejects_fake_event_authority(cluster: &Cluster) {
    let mut env = setup(cluster);
    let event = Keypair::new();
    let mut instruction =
        forward_instruction(&env, env.account, 1_000_000, 0, STANDARD_THRESHOLD, &event);
    substitute(
        &mut instruction,
        tmm_pda(&[b"__event_authority"]),
        Keypair::new().pubkey(),
    );

    let result = send(&mut env, instruction, &event);

    assert_circle_error(result, "ConstraintSeeds");
}

fn rejects_other_domain_remote_token_messenger(cluster: &Cluster) {
    let mut env = setup(cluster);
    let base_messenger = tmm_pda(&[b"remote_token_messenger", b"6"]);
    let mut ethereum_messenger = env.svm.get_account(&base_messenger).unwrap();
    ethereum_messenger.data[8..12].copy_from_slice(&0u32.to_le_bytes());
    let ethereum_address = tmm_pda(&[b"remote_token_messenger", b"0"]);
    env.svm
        .set_account(ethereum_address, ethereum_messenger)
        .unwrap();

    let event = Keypair::new();
    let mut instruction =
        forward_instruction(&env, env.account, 1_000_000, 0, STANDARD_THRESHOLD, &event);
    substitute(&mut instruction, base_messenger, ethereum_address);

    let result = send(&mut env, instruction, &event);

    assert_circle_error(result, "InvalidDestinationDomain");
}
