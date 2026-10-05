use {
    crate::{
        Burn, FAST, Forwarder, Kind, STANDARD, Step,
        message::{self, BurnMessage},
        view::{HOLDERS, TRACKED, View},
    },
    crucible_fuzzer::{fuzz_assert, fuzz_assert_eq},
    solana_account::ReadableAccount,
    solana_signer::Signer,
};

const SOLANA_DOMAIN: u32 = 5;
const BASE_DOMAIN: u32 = 6;
const MAX_FEE_BPS: u128 = 20;
const BODY_LEN_WITHOUT_HOOK: usize = 228;

pub fn check(fixture: &mut Forwarder) {
    let after = fixture.view();
    fuzz_assert_eq!(
        u128::from(after.supply).checked_sub(after.tracked_total()),
        Some(fixture.outside),
        "USDC left the tracked accounts other than by a burn: supply {} tracked {}",
        after.supply,
        after.tracked_total()
    );
    if let Some(step) = fixture.last.take() {
        effects(fixture, &step, &after);
    }
    messages(fixture);
}

fn amounts(view: &View) -> [Option<u64>; TRACKED] {
    view.tracked.map(|token| token.map(|token| token.amount))
}

fn shift(amounts: &mut [Option<u64>; TRACKED], index: usize, delta: i128) {
    if let Some(amount) = amounts[index].as_mut() {
        *amount = u64::try_from(i128::from(*amount) + delta).unwrap_or(u64::MAX);
    }
}

fn effects(fixture: &mut Forwarder, step: &Step, after: &View) {
    let before = &step.before;
    fuzz_assert_eq!(
        after.foreign,
        before.foreign,
        "the foreign-mint account changed on {:?}",
        step.kind
    );

    if let Kind::Forward {
        source,
        base,
        amount,
        max_fee,
        threshold,
        event,
    } = step.kind
    {
        forward(
            fixture,
            step,
            after,
            Burn {
                event,
                base,
                amount,
                max_fee,
                threshold,
            },
            source,
        );
        return;
    }

    if !step.ok {
        fuzz_assert_eq!(
            after,
            before,
            "the failed {:?} changed token state",
            step.kind
        );
        return;
    }

    let mut expected = amounts(before);
    let mut supply = i128::from(before.supply);
    match step.kind {
        Kind::Forward { .. } | Kind::Authority => {}
        Kind::Transfer { from, to, amount } => {
            shift(&mut expected, from, -i128::from(amount));
            shift(&mut expected, to, i128::from(amount));
        }
        Kind::MintTo { to, amount } => {
            shift(&mut expected, to, i128::from(amount));
            supply += i128::from(amount);
        }
        Kind::CreateVault { base } => {
            let vault = HOLDERS + base;
            if before.tracked[vault].is_none() {
                expected[vault] = Some(0);
                fuzz_assert!(
                    after.tracked[vault].is_some_and(|token| token.mint == fixture.usdc
                        && token.owner == fixture.forwarders[base]
                        && token.delegate.is_none()
                        && token.close_authority.is_none()
                        && !token.frozen),
                    "the created vault of base {base} is {:?}",
                    after.tracked[vault]
                );
            }
        }
    }

    fuzz_assert_eq!(
        amounts(after),
        expected,
        "{:?} moved USDC other than its own amount",
        step.kind
    );
    fuzz_assert_eq!(
        i128::from(after.supply),
        supply,
        "{:?} changed the USDC supply",
        step.kind
    );
}

fn forward(fixture: &mut Forwarder, step: &Step, after: &View, burn: Burn, source: usize) {
    let before = &step.before;
    let forwarder = fixture.forwarders[burn.base];
    let vault = HOLDERS + burn.base;
    let token = before.source(source);
    let delegated = source < TRACKED
        && token.is_some_and(|token| {
            token.mint == fixture.usdc && token.delegate == Some(forwarder) && !token.frozen
        });
    let pull = token.filter(|_| delegated).map_or(0, |token| {
        burn.amount.min(token.amount).min(token.delegated_amount)
    });
    let threshold = burn.threshold == FAST || burn.threshold == STANDARD;
    let vault_open = before.tracked[vault].is_none_or(|token| !token.frozen);

    fuzz_assert!(
        !step.ok || (delegated && pull > 0),
        "forward to base {} succeeded from source {source} {token:?}, which does not delegate a positive pull to {forwarder}",
        burn.base
    );
    fuzz_assert!(
        step.ok || !(delegated && pull > 0 && threshold && vault_open),
        "forward of {} from source {source} {token:?} to base {} at threshold {} failed although it pulls {pull}",
        burn.amount,
        burn.base,
        burn.threshold
    );

    if !step.ok {
        fuzz_assert_eq!(after, before, "the failed forward changed token state");
        fuzz_assert!(
            fixture
                .ctx
                .svm
                .get_account(&burn.event)
                .is_none_or(|account| account.lamports == 0),
            "the failed forward left an event account at {}",
            burn.event
        );
        return;
    }
    if !delegated || pull == 0 {
        return;
    }

    let burned = before.tracked[vault]
        .map_or(0, |token| token.amount)
        .saturating_add(pull);
    let cap = u64::try_from(u128::from(burned) * MAX_FEE_BPS / 10_000).unwrap_or(u64::MAX);

    let mut expected = before.tracked;
    expected[vault] = None;
    if let Some(token) = expected[source].as_mut() {
        token.amount -= pull;
        token.delegated_amount -= pull;
        if token.delegated_amount == 0 {
            token.delegate = None;
        }
    }

    fuzz_assert!(
        after.tracked[vault].is_none(),
        "the vault of base {} is open after a forward: {:?}",
        burn.base,
        after.tracked[vault]
    );
    fuzz_assert_eq!(
        after.tracked,
        expected,
        "forward pulling {pull} from source {source} to base {} moved other balances",
        burn.base
    );
    fuzz_assert_eq!(
        before.supply.checked_sub(after.supply),
        Some(burned),
        "forward burned {:?} instead of the vault balance {burned}",
        before.supply.checked_sub(after.supply)
    );

    fixture.burns.push(Burn {
        amount: burned,
        max_fee: burn.max_fee.min(cap),
        ..burn
    });
}

fn messages(fixture: &Forwarder) {
    let sent = fixture
        .ctx
        .svm
        .accounts_db()
        .inner
        .values()
        .filter(|account| {
            account.lamports() > 0 && message::is_message_sent(account.owner(), account.data())
        })
        .count();
    fuzz_assert_eq!(
        sent,
        fixture.burns.len(),
        "{sent} MessageSent accounts for {} forwards",
        fixture.burns.len()
    );

    for burn in &fixture.burns {
        let message = fixture
            .ctx
            .svm
            .get_account(&burn.event)
            .and_then(|account| message::parse(&account.owner, &account.data));
        let Some(message) = message else {
            fuzz_assert!(false, "MessageSent {} is missing or malformed", burn.event);
            continue;
        };
        let mut mint_recipient = [0u8; 32];
        mint_recipient[12..].copy_from_slice(&fixture.bases[burn.base]);
        let expected = BurnMessage {
            rent_payer: fixture.payer.pubkey(),
            source_domain: SOLANA_DOMAIN,
            destination_domain: BASE_DOMAIN,
            destination_caller: [0; 32],
            min_finality_threshold: burn.threshold,
            burn_token: fixture.usdc,
            mint_recipient,
            amount: burn.amount,
            message_sender: fixture.forwarders[burn.base],
            max_fee: burn.max_fee,
            body_len: BODY_LEN_WITHOUT_HOOK,
        };
        fuzz_assert_eq!(
            message,
            expected,
            "MessageSent {} does not match its forward",
            burn.event
        );
        fuzz_assert!(
            u128::from(message.max_fee) * 10_000 <= u128::from(message.amount) * MAX_FEE_BPS,
            "MessageSent {} max fee {} exceeds 20 bps of {}",
            burn.event,
            message.max_fee,
            message.amount
        );
        fuzz_assert!(
            message.min_finality_threshold == FAST || message.min_finality_threshold == STANDARD,
            "MessageSent {} has threshold {}",
            burn.event,
            message.min_finality_threshold
        );
    }
}
