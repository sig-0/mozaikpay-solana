use {
    agave_feature_set::FeatureSet,
    anchor_lang::prelude::Pubkey,
    base64::{Engine, engine::general_purpose::STANDARD},
    crucible_test_context::{EmptyInvocationCallback, litesvm::LiteSVM},
    solana_account::Account,
};

pub struct Cluster {
    pub usdc: Pubkey,
    pub token_messenger_minter: &'static [u8],
    pub message_transmitter: &'static [u8],
    pub accounts: [&'static str; 6],
    pub features: &'static str,
}

macro_rules! fixture {
    ($dir:literal, $file:literal) => {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../programs/mozaik-cctp-forwarder/tests/fixtures/",
            $dir,
            "/",
            $file
        )
    };
}

macro_rules! cluster {
    ($dir:literal, $usdc:literal) => {
        Cluster {
            usdc: anchor_lang::pubkey!($usdc),
            token_messenger_minter: include_bytes!(fixture!($dir, "token_messenger_minter_v2.so")),
            message_transmitter: include_bytes!(fixture!($dir, "message_transmitter_v2.so")),
            accounts: [
                include_str!(fixture!($dir, "token_messenger.json")),
                include_str!(fixture!($dir, "token_minter.json")),
                include_str!(fixture!($dir, "remote_token_messenger_base.json")),
                include_str!(fixture!($dir, "local_token_usdc.json")),
                include_str!(fixture!($dir, "message_transmitter.json")),
                include_str!(fixture!($dir, "usdc_mint.json")),
            ],
            features: include_str!(fixture!($dir, "features.json")),
        }
    };
}

pub fn selected() -> Cluster {
    if cfg!(feature = "devnet") {
        cluster!("devnet", "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU")
    } else {
        cluster!("mainnet", "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")
    }
}

impl Cluster {
    pub fn svm(&self) -> LiteSVM {
        let ids: Vec<String> = serde_json::from_str(self.features).expect("features.json");
        let mut features = FeatureSet::default();
        for id in ids {
            features.activate(&id.parse().expect("feature id"), 0);
        }

        let tracing = std::env::var("CRUCIBLE_FUZZ_DEBUGGABLE").is_ok();
        let mut svm = LiteSVM::new_debuggable(tracing)
            .with_feature_set(features)
            .with_sysvars()
            .with_feature_accounts()
            .with_builtins()
            .with_default_programs()
            .with_transaction_history(0)
            .with_sigverify(false)
            .with_blockhash_check(false);
        if tracing {
            svm.set_invocation_inspect_callback(EmptyInvocationCallback);
        }

        for json in self.accounts {
            let value: serde_json::Value = serde_json::from_str(json).expect("account fixture");
            let account = &value["account"];
            svm.set_account(
                value["pubkey"]
                    .as_str()
                    .and_then(|key| key.parse().ok())
                    .expect("pubkey"),
                Account {
                    lamports: account["lamports"].as_u64().expect("lamports"),
                    data: account["data"][0]
                        .as_str()
                        .and_then(|data| STANDARD.decode(data).ok())
                        .expect("data"),
                    owner: account["owner"]
                        .as_str()
                        .and_then(|owner| owner.parse().ok())
                        .expect("owner"),
                    executable: account["executable"].as_bool().expect("executable"),
                    rent_epoch: 0,
                },
            )
            .expect("set fixture account");
        }

        svm
    }
}
