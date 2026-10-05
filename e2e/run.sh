#!/usr/bin/env bash
# Runs the end-to-end tests on a local validator that mirrors one cluster: Circle's CCTP V2
# programs and state, the cluster's USDC mint with a throwaway mint authority, and the cluster's
# feature gates. The forwarder goes live through the upgradeable loader, like a real deployment.
#
# Usage: e2e/run.sh devnet|mainnet
# By default the program is in genesis with a throwaway upgrade authority and the deployment is
# an upgrade. Set PROGRAM_KEYPAIR to the program id keypair to do a first deployment instead.
set -euo pipefail

case "${1:-}" in
  devnet)
    usdc=4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU
    ;;
  mainnet)
    usdc=EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v
    ;;
  *)
    echo "usage: $0 devnet|mainnet" >&2
    exit 1
    ;;
esac

program_id=MzkicG4ev5reLESn2RZvPVufghwgwQhRqvhNqVr8oNb
tmm=CCTPV2vPZJS2u2BBsUoscuikbYjnpFmbFsvVuJdgUMQe
mt=CCTPV2Sm4AdWt5296sk4P66VBZ7bEhcARwFaaS9YPbeC
rpc=http://127.0.0.1:8899

root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$root/e2e/Cargo.toml"
fixtures="$root/programs/mozaik-cctp-forwarder/tests/fixtures/$1"
program_so="$root/target/deploy/mozaik_cctp_forwarder.so"

if [ ! -f "$program_so" ]; then
  echo "$program_so is missing, run make build-sbf first" >&2
  exit 1
fi

if [ -n "${PROGRAM_KEYPAIR:-}" ] && [ "$(solana-keygen pubkey "$PROGRAM_KEYPAIR")" != "$program_id" ]; then
  echo "PROGRAM_KEYPAIR is not the keypair of $program_id" >&2
  exit 1
fi

if curl -s "$rpc" >/dev/null; then
  echo "Something already listens on $rpc" >&2
  exit 1
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/mozaik-e2e.XXXXXX")"
ledger="$tmp/ledger"
validator=""

cleanup() {
  if [ -n "$validator" ]; then
    kill "$validator" 2>/dev/null || true
    wait "$validator" 2>/dev/null || true
  fi

  rm -rf "$tmp"
}

trap cleanup EXIT

echo "Building the tests..."
cargo test --manifest-path "$manifest" --locked --test forward --no-run --quiet

for key in deployer minter; do
  solana-keygen new --no-bip39-passphrase --silent --force -o "$tmp/$key.json" >/dev/null
done

deployer="$tmp/deployer.json"
minter="$tmp/minter.json"

cargo run --manifest-path "$manifest" --locked --quiet --bin usdc_mint -- \
  "$fixtures/usdc_mint.json" "$(solana-keygen pubkey "$minter")" > "$tmp/usdc_mint.json"

args=(
  --reset
  --quiet
  --ledger "$ledger"
  --mint "$(solana-keygen pubkey "$deployer")"
  --upgradeable-program "$tmm" "$fixtures/token_messenger_minter_v2.so" none
  --upgradeable-program "$mt" "$fixtures/message_transmitter_v2.so" none
  --account - "$tmp/usdc_mint.json"
)

for account in token_messenger token_minter remote_token_messenger_base local_token_usdc message_transmitter; do
  args+=(--account - "$fixtures/$account.json")
done

for feature in $(jq -r '.[]' "$fixtures/inactive_features.json"); do
  args+=(--deactivate-feature "$feature")
done

if [ -z "${PROGRAM_KEYPAIR:-}" ]; then
  args+=(--upgradeable-program "$program_id" "$program_so" "$deployer")
fi

echo "Starting a validator with the $1 state..."
solana-test-validator "${args[@]}" &
validator=$!

until curl -s "$rpc" -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | grep -q '"ok"'; do
  if ! kill -0 "$validator" 2>/dev/null; then
    echo "The validator exited:" >&2
    tail -n 50 "$ledger/validator.log" >&2
    exit 1
  fi
  sleep 0.5
done

echo "Validator is ready (PID: $validator)"

active="$(solana feature status -u "$rpc" --display-all --output json \
  | jq -c '[.features[] | select(.status == "active") | .id] | sort')"

if [ "$active" != "$(jq -c . "$fixtures/features.json")" ]; then
  echo "The active features differ from $1:" >&2
  diff <(jq -r '.[]' <<<"$active") <(jq -r '.[]' "$fixtures/features.json") >&2 || true
  exit 1
fi

echo "Deploying the forwarder..."
solana program deploy "$program_so" --program-id "${PROGRAM_KEYPAIR:-$program_id}" \
  --upgrade-authority "$deployer" --keypair "$deployer" -u "$rpc"

solana airdrop 10 "$minter" -u "$rpc" >/dev/null

echo ""
echo "=== Running e2e tests against $1 ==="
RPC_URL="$rpc" USDC_MINT="$usdc" MINTER_KEYPAIR="$minter" PROGRAM_SO="$program_so" \
  cargo test --manifest-path "$manifest" --locked --test forward -- --test-threads=1
