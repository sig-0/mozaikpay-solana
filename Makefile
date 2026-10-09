# Optional local settings, see .env.example
-include .env

# Default RPC endpoints for Solana devnet and mainnet.
# Override by exporting SOLANA_DEVNET_RPC / SOLANA_MAINNET_RPC in the shell or in .env
SOLANA_DEVNET_RPC ?= https://api.devnet.solana.com
SOLANA_MAINNET_RPC ?= https://api.mainnet-beta.solana.com
export SOLANA_DEVNET_RPC
export SOLANA_MAINNET_RPC

PROGRAM_ID := MzkicG4ev5reLESn2RZvPVufghwgwQhRqvhNqVr8oNb
PROGRAM_SO := target/deploy/mozaik_cctp_forwarder.so
DEPLOY_SO := target/verifiable/mozaik_cctp_forwarder.so
REPO_URL := https://github.com/sig-0/mozaikpay-solana

# Build commands

# Every build targets SBPF v3, which mainnet and devnet support, with one platform-tools version
SBPF_ARCH := v3
TOOLS_VERSION := v1.54

# Solana's verifiable-build image for Solana 4.1.2, the one solana-verify 0.5.2 picks. Builds and
# verifications name it, because OtterSec's remote verifier has no image for 4.1.2 of its own
VERIFY_IMAGE := solanafoundation/solana-verifiable-build@sha256:2e0b78f44ee76612e9260c7c988570c5e14de6fbd93e0ab07115ec7054473b4f

# Builds the program and its IDL with the Anchor CLI. The program keypair stays outside the repo, so
# the key check is skipped. The tests pin the program id, and check-deploy checks PROGRAM_KEYPAIR
.PHONY: build
build:
	NO_DNA=1 anchor build --arch $(SBPF_ARCH) --tools-version $(TOOLS_VERSION) --ignore-keys

# Builds the program only. Tests, CI and deployments use this build. cargo build-sbf keeps an
# existing binary that is newer than its output, so the old binary goes first
.PHONY: build-sbf
build-sbf:
	rm -f $(PROGRAM_SO)
	cargo fetch --locked
	cargo build-sbf --arch $(SBPF_ARCH) --tools-version $(TOOLS_VERSION) \
		--manifest-path programs/mozaik-cctp-forwarder/Cargo.toml

# Builds the deploy artifact in Solana's verifiable-build Docker image (linux/amd64), so anyone can
# rebuild the deployed bytes from the source with solana-verify. Keeps a copy in target/verifiable,
# which later builds do not replace. CI runs it on every push and publishes the artifact and its hash
.PHONY: verifiable-build
verifiable-build:
	rm -f $(PROGRAM_SO) $(DEPLOY_SO)
	mkdir -p $(dir $(DEPLOY_SO))
	solana-verify build --library-name mozaik_cctp_forwarder --arch $(SBPF_ARCH) --base-image $(VERIFY_IMAGE) \
		--cargo-build-sbf-args=--tools-version=$(TOOLS_VERSION)
	cp $(PROGRAM_SO) $(DEPLOY_SO)

# Copies the IDL of the last build into idl/, where it is tracked
.PHONY: idl
idl: build
	mkdir -p idl
	cp target/idl/mozaik_cctp_forwarder.json idl/mozaik_cctp_forwarder.json

.PHONY: clean
clean:
	cargo clean

.PHONY: lint
lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings
	cargo clippy --lib -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic \
		-D clippy::indexing_slicing -D clippy::cast_possible_truncation
	cargo fmt --manifest-path e2e/Cargo.toml -- --check
	cargo fmt --manifest-path fuzz/mozaik_cctp_forwarder/Cargo.toml -- --check
	cargo clippy --manifest-path e2e/Cargo.toml --locked --all-targets -- -D warnings

.PHONY: format
format:
	cargo fmt --all
	cargo fmt --manifest-path e2e/Cargo.toml
	cargo fmt --manifest-path fuzz/mozaik_cctp_forwarder/Cargo.toml

# Test commands

# Runs the LiteSVM tests against the devnet and mainnet fixtures
.PHONY: test
test: build-sbf
	cargo test --locked

# Runs the end-to-end tests on local validators that mirror devnet and mainnet
.PHONY: test-e2e
test-e2e: build-sbf
	e2e/run.sh devnet
	e2e/run.sh mainnet

# Mutation testing with cargo-mutants. scripts/cargo-sbf rebuilds the program before every test
# build, so that each mutant runs as the SBF binary that the tests load
.PHONY: mutate
mutate:
	CARGO=$(CURDIR)/scripts/cargo-sbf cargo-mutants mutants --build-timeout 900 --timeout 600

# Fuzzes the program with Crucible (stateful invariant fuzzing) against the mainnet and devnet
# fixtures. crucible run exits 0 even when it finds a violation, so the crashes directory decides
CRUCIBLE_SECONDS ?= 300
CRUCIBLE_CRASHES := fuzz/mozaik_cctp_forwarder/crashes

.PHONY: crucible
crucible: build-sbf
	crucible run mozaik_cctp_forwarder mainnet --release --stateful --stop-on-crash --timeout $(CRUCIBLE_SECONDS)
	crucible run mozaik_cctp_forwarder devnet --release --stateful --stop-on-crash --timeout $(CRUCIBLE_SECONDS)
	@if [ -n "$$(find $(CRUCIBLE_CRASHES) -name 'crash_*' 2>/dev/null)" ]; then \
		find $(CRUCIBLE_CRASHES) -name 'crash_*.meta.json'; exit 1; fi

# Refreshes the fixtures from both clusters: Circle's CCTP V2 programs, the accounts
# deposit_for_burn reads, and the active feature gates
.PHONY: fixtures
fixtures:
	scripts/fixtures.sh devnet
	scripts/fixtures.sh mainnet

# Static analysis and supply chain

# Checks the dependencies for advisories, licenses, bans and sources (see deny.toml)
.PHONY: deny
deny:
	cargo deny --locked check

# Fails if the tracked IDL differs from the IDL of a fresh Anchor build
.PHONY: idl-check
idl-check: build
	diff -u idl/mozaik_cctp_forwarder.json target/idl/mozaik_cctp_forwarder.json

# Deployment commands
# Require DEPLOYER_KEYPAIR, PROGRAM_BUILD_SHA (the sha256 of the reviewed verifiable build), PROGRAM_KEYPAIR for
# a deploy and PROGRAM_BUILD_COMMIT (the git commit of the deployed source) for a verification, in the env (see .env.example)

# Extra flags for solana program deploy, for example DEPLOY_FLAGS="--with-compute-unit-price 50000 --use-rpc"
DEPLOY_FLAGS ?=

require-deployer = $(if $(DEPLOYER_KEYPAIR),,$(error DEPLOYER_KEYPAIR is required))
require-program = $(if $(PROGRAM_KEYPAIR),,$(error PROGRAM_KEYPAIR is required))
require-hash = $(if $(PROGRAM_BUILD_SHA),,$(error PROGRAM_BUILD_SHA is required))
require-commit = $(if $(PROGRAM_BUILD_COMMIT),,$(error PROGRAM_BUILD_COMMIT is required))

.PHONY: require-deploy
require-deploy:
	@$(require-deployer)
	@$(require-program)
	@$(require-hash)

# Runs the tests on the deploy artifact in target/verifiable (from make verifiable-build or the
# Build CI job), then checks that it is SBPF v3 and the reviewed one, and that
# PROGRAM_KEYPAIR is the keypair of the program id
.PHONY: check-deploy
check-deploy: require-deploy
	cp $(DEPLOY_SO) $(PROGRAM_SO)
	cargo test --locked
	test "$$(od -An -t u2 -j 18 -N 2 $(DEPLOY_SO) | tr -d ' ')" = 247
	test "$$(od -An -t u4 -j 48 -N 4 $(DEPLOY_SO) | tr -d ' ')" = 3
	test "$$(shasum -a 256 $(DEPLOY_SO) | cut -d ' ' -f 1)" = $(PROGRAM_BUILD_SHA)
	test "$$(solana-keygen pubkey $(PROGRAM_KEYPAIR))" = $(PROGRAM_ID)

.PHONY: deploy-devnet
deploy-devnet: check-deploy
	solana program deploy $(DEPLOY_SO) --program-id $(PROGRAM_KEYPAIR) \
		--keypair $(DEPLOYER_KEYPAIR) --url $(SOLANA_DEVNET_RPC) $(DEPLOY_FLAGS)

.PHONY: deploy-mainnet
deploy-mainnet: check-deploy
	solana program deploy $(DEPLOY_SO) --program-id $(PROGRAM_KEYPAIR) \
		--keypair $(DEPLOYER_KEYPAIR) --url $(SOLANA_MAINNET_RPC) $(DEPLOY_FLAGS)

# Uploads the build parameters of PROGRAM_BUILD_COMMIT for the deployed program, signed by the upgrade authority,
# without a local build. solana-verify loads the Solana CLI config and its keypair even when --keypair is set, so it
# gets a throwaway config that points at the deployer
verify-from-repo = dir="$$(mktemp -d)"; trap 'rm -rf "$$dir"' EXIT; \
	solana config set --config "$$dir/config.yml" --keypair $(DEPLOYER_KEYPAIR) --url $(1) >/dev/null; \
	solana-verify verify-from-repo $(REPO_URL) --program-id $(PROGRAM_ID) --commit-hash $(PROGRAM_BUILD_COMMIT) \
		--library-name mozaik_cctp_forwarder --arch $(SBPF_ARCH) \
		--cargo-build-sbf-args=--tools-version=$(TOOLS_VERSION) --base-image $(VERIFY_IMAGE) --skip-build \
		--config "$$dir/config.yml" --keypair $(DEPLOYER_KEYPAIR) --url $(1)

# Runs before finalize
.PHONY: verify-devnet
verify-devnet:
	@$(require-deployer)
	@$(require-commit)
	$(call verify-from-repo,$(SOLANA_DEVNET_RPC))

# The same as verify-devnet, then asks OtterSec's remote verifier to rebuild the program from PROGRAM_BUILD_COMMIT
# and compare it with the program on mainnet
.PHONY: verify-mainnet
verify-mainnet:
	@$(require-deployer)
	@$(require-commit)
	$(call verify-from-repo,$(SOLANA_MAINNET_RPC))
	solana-verify remote submit-job --program-id $(PROGRAM_ID) \
		--uploader "$$(solana-keygen pubkey $(DEPLOYER_KEYPAIR))" --url $(SOLANA_MAINNET_RPC)

# The name, logo and contacts that explorers show for the program. They go into a metadata account of Solana's Program
# Metadata Program, not into the program, whose security.txt format has no logo field. Only the upgrade authority can
# create the account, so it is written before finalize, and finalize refuses to run without it
METADATA_JSON := metadata/security.json
METADATA_LOGO := metadata/logo.png
METADATA_LOGO_URL := https://raw.githubusercontent.com/sig-0/mozaikpay-solana/main/$(METADATA_LOGO)
METADATA_CLI := npx --yes @solana-program/program-metadata@0.10.0

# Writes METADATA_JSON once it links METADATA_LOGO_URL and that URL serves METADATA_LOGO
write-metadata = test -f $(METADATA_LOGO) || { echo "$(METADATA_LOGO) is missing" >&2; exit 1; }; \
	grep -qF $(METADATA_LOGO_URL) $(METADATA_JSON) || { echo "$(METADATA_JSON) does not link $(METADATA_LOGO_URL)" >&2; exit 1; }; \
	curl -fsSL $(METADATA_LOGO_URL) | cmp -s - $(METADATA_LOGO) || { echo "$(METADATA_LOGO_URL) does not serve $(METADATA_LOGO)" >&2; exit 1; }; \
	$(METADATA_CLI) --keypair $(DEPLOYER_KEYPAIR) --rpc $(1) write security $(PROGRAM_ID) $(METADATA_JSON) --format json

require-metadata = $(METADATA_CLI) --rpc $(1) fetch security $(PROGRAM_ID) >/dev/null || \
	{ echo "The program has no metadata on $(1). Run make metadata-devnet or make metadata-mainnet first" >&2; exit 1; }

.PHONY: metadata-devnet
metadata-devnet:
	@$(require-deployer)
	$(call write-metadata,$(SOLANA_DEVNET_RPC))

.PHONY: metadata-mainnet
metadata-mainnet:
	@$(require-deployer)
	$(call write-metadata,$(SOLANA_MAINNET_RPC))

# solana program show needs a signer even though it only reads, so it gets a throwaway one
show-program = key="$$(mktemp)"; trap 'rm -f "$$key"' EXIT; \
	solana-keygen new --no-bip39-passphrase --silent --force -o "$$key" >/dev/null; \
	solana program show $(PROGRAM_ID) --url $(1) --keypair "$$key"

.PHONY: show-devnet
show-devnet:
	@$(call show-program,$(SOLANA_DEVNET_RPC))

.PHONY: show-mainnet
show-mainnet:
	@$(call show-program,$(SOLANA_MAINNET_RPC))

# Removes the upgrade authority, so the deployed program can never change. Irreversible.
# Runs only when the deployed program is the reviewed binary
.PHONY: finalize-devnet
finalize-devnet:
	@$(require-deployer)
	@$(require-hash)
	@$(call require-metadata,$(SOLANA_DEVNET_RPC))
	test "$$(shasum -a 256 $(DEPLOY_SO) | cut -d ' ' -f 1)" = $(PROGRAM_BUILD_SHA)
	scripts/check-deployed.sh $(SOLANA_DEVNET_RPC) $(PROGRAM_ID) $(DEPLOY_SO)
	solana program set-upgrade-authority $(PROGRAM_ID) --final \
		--keypair $(DEPLOYER_KEYPAIR) --url $(SOLANA_DEVNET_RPC)

.PHONY: finalize-mainnet
finalize-mainnet:
	@$(require-deployer)
	@$(require-hash)
	@$(call require-metadata,$(SOLANA_MAINNET_RPC))
	test "$$(shasum -a 256 $(DEPLOY_SO) | cut -d ' ' -f 1)" = $(PROGRAM_BUILD_SHA)
	scripts/check-deployed.sh $(SOLANA_MAINNET_RPC) $(PROGRAM_ID) $(DEPLOY_SO)
	solana program set-upgrade-authority $(PROGRAM_ID) --final \
		--keypair $(DEPLOYER_KEYPAIR) --url $(SOLANA_MAINNET_RPC)
